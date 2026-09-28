//! From a click on the Nexus website to a mod the game loads.
//!
//! One function, five steps, because the interesting part is the order and
//! what happens when a step fails rather than any individual call:
//!
//! ```text
//! nxm://…?key=…  ->  download_link.json  ->  archives/  ->  staging/  ->  MODS
//!   the site's       the CDN url, which      the file      extracted     hardlinks
//!   proof of a       that proof buys for     as sent       once           to the
//!   real click       a free account                                       staged copy
//! ```
//!
//! Each step lands somewhere durable before the next begins. A download that
//! fails leaves nothing; an extraction that fails leaves the archive, so
//! retrying costs no bandwidth; a deploy that fails leaves the staged mod,
//! which the user can deploy later without downloading again.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::model::Conflict;
use super::model::Mod;
use super::{
    archive, decompile, decompile::Decompiler, deploy, discovery, download, erase, loadout,
    merge, nexus, nxm,
};
use super::vanilla::VanillaSource;

/// Where the three durable layers live.
#[derive(Debug, Clone)]
pub struct Places {
    /// archives exactly as downloaded
    pub archives: PathBuf,
    /// one folder per mod, extracted, never modified
    pub staging: PathBuf,
    /// a mended, cleaned or merged build of a staged mod, rebuilt at will
    ///
    /// Deliberately *not* moved when the user picks their own staging folder.
    /// It has to be on the game's volume for the deployed names to be links
    /// rather than copies, and it is this program's working output rather than
    /// the user's own library, so it does not belong in their folder.
    pub derived: PathBuf,
    /// the game's own mods folder
    pub mods_dir: PathBuf,
}

impl Places {
    /// Lay the folders out beside the game, because a hardlink cannot cross
    /// volumes and the deployed names have to be links to the staged files.
    pub fn beside(game_root: &Path, mods_dir: &Path) -> Places {
        let base = deploy::staging_for(game_root);
        Places {
            archives: base.join("archives"),
            staging: base.join("staging"),
            derived: base.join("derived"),
            mods_dir: mods_dir.to_path_buf(),
        }
    }
}

/// What merging one conflict did.
#[derive(Debug, Clone, Serialize)]
pub struct Merged {
    /// the merge's own mod folder
    pub owner: String,
    /// the file written
    pub written: String,
    /// the mods it now stands in for
    pub replaces: Vec<String>,
    /// what reconcile changed in the game as a result
    pub changes: loadout::Changes,
}

/// Build the merge for one conflict, stage it, and let the loadout deploy it.
///
/// # Why a merge goes through here at all
///
/// It used to be written straight into the game as a bare folder, which left it
/// outside everything this program knows how to do: it could not be deactivated
/// or deleted, and -- the part that actually bit -- nothing held its *inputs*
/// out of the game. All three loaded together, and the merge won only because
/// its folder name sorted last. That is the exact failure `Entry::replaces` was
/// written to end.
///
/// So a merge is staged like any other build of ours, into `derived/`, with an
/// entry naming the mods it stands in for. [`loadout::reconcile`] then takes
/// those out of the game and puts the merge in, and switching the merge off or
/// deleting it hands them straight back.
pub fn record_merge(
    conflict: &Conflict,
    mods: &[Mod],
    places: &Places,
    loadout_path: &Path,
    decompiler: &mut Decompiler,
    source: &mut VanillaSource,
) -> Result<Merged, String> {
    // One folder per merged asset, stable across re-merges of the same asset,
    // laid out `derived/<owner>/<owner>/…` because that is the shape `deploy`
    // mirrors into the game.
    let owner = merge::folder_for(&conflict.target);
    let build = places.derived.join(&owner);
    let inner = build.join(&owner);

    // Re-merging the same asset replaces its own build rather than adding to
    // it: otherwise we would be compiling beside files from a previous run
    // whose inputs may no longer be the ones just read.
    if inner.exists() {
        std::fs::remove_dir_all(&inner)
            .map_err(|err| format!("could not clear {}: {err}", inner.display()))?;
    }

    let mut book = loadout::Loadout::read(loadout_path);
    let previous = book.get(&owner);
    // Kept when rebuilding a merge that is already in the game, so reconcile
    // knows what to take away before it puts the new build in.
    let deployed = previous.map(|e| e.deployed.clone()).unwrap_or_default();
    let parents = stands_in_for(previous, &owner, &conflict.mods);

    // Build from the parents, never from a previous merge of them.
    //
    // Absorbing a newcomer *could* just merge the existing build with it --
    // the old merge is a whole-file asset and would host the graft perfectly
    // well. But a merge built that way can never be taken apart again: its
    // inputs' edits are already flattened into one document, so there is no
    // answer to "drop this mod" or "take this mod's new version" except to
    // throw the whole thing away. Going back to the parents every time keeps
    // every rebuild a first merge, and keeps the list of what it stands in
    // for true of what is actually inside it.
    let (mut parent_mods, missing) = read_parents(&book, &parents, mods);
    if !missing.is_empty() {
        return Err(format!(
            "cannot rebuild this merge: {} is no longer staged, so its edits              could not be read back. Delete the merge to get the others back.",
            missing.join(", ")
        ));
    }

    // Assessed again on the real inputs. The conflict that was handed in was
    // measured against whatever is *in the game*, which for a rebuild is the
    // old merge and the newcomer -- not the same question as whether all the
    // parents can live together.
    // A parent read back from staging is a bare scan: a compiled `.MBIN` has
    // a hash and nothing else until it is decompiled, and `merge::run` reads
    // a copy with no properties as one it cannot judge, which would refuse
    // every rebuild whose parents are not already in the game.
    decompile::enrich(&mut parent_mods, decompiler);

    let mut rebuilt = conflict.clone();
    rebuilt.mods = parents.clone();
    merge::run(std::slice::from_mut(&mut rebuilt), &parent_mods, decompiler, source);
    if rebuilt.mergeable != Some(true) {
        return Err(format!(
            "these mods cannot all be combined: {} would each have to win the              same {} propert(y/ies)",
            parents.join(", "),
            rebuilt.overlap.len()
        ));
    }

    let written = merge::install(&rebuilt, &parent_mods, &inner, decompiler, source)?;
    let replaces = parents;
    book.put(loadout::Entry {
        owner: owner.clone(),
        source: build.display().to_string(),
        // A merge has no author's build behind it -- it *is* the build -- so
        // origin is itself. That also makes deleting it delete exactly one
        // thing, and never one of the mods it was made from.
        origin: Some(build.display().to_string()),
        archive: None,
        variant: loadout::Variant::Merged,
        replaces: replaces.clone(),
        deployed,
        built_from: None,
        enabled: true,
        edited: false,
    });

    let changes = loadout::reconcile(&mut book, &places.mods_dir, false);
    book.write(loadout_path)?;

    Ok(Merged {
        owner,
        written: written.to_string_lossy().into_owned(),
        replaces,
        changes,
    })
}

/// Dissolve every merge built out of `owner`, and take its files with it.
///
/// Splitting this out because forgetting the entry is only half of it: an
/// entry the loadout no longer lists is one [`loadout::reconcile`] can no
/// longer undeploy, so the build would sit in the game forever, standing in
/// for a version of a mod that is no longer installed.
fn retire_merges_of(
    book: &mut loadout::Loadout,
    owner: &str,
    mods_dir: &Path,
) -> Vec<String> {
    let gone = book.dissolve_merges_of(owner);
    for entry in &gone {
        let _ = deploy::undeploy(mods_dir, &entry.deployed);
        let _ = std::fs::remove_dir_all(PathBuf::from(&entry.source));
    }
    gone.into_iter().map(|entry| entry.owner).collect()
}

/// Add a line about what the previous version left, if anything did.
///
/// Said rather than done silently: the user chose to keep downloads when they
/// delete a mod, and an update quietly removing one would look like that
/// setting had been ignored.
fn with_reclaimed(mut notes: Vec<String>, freed: &[String]) -> Vec<String> {
    if !freed.is_empty() {
        notes.push(format!(
            "Removed {} the previous version left behind.",
            if freed.len() == 1 {
                "one thing".to_string()
            } else {
                format!("{} things", freed.len())
            }
        ));
    }
    notes
}

/// What the previous install of a mod left behind.
///
/// Captured *before* the loadout record is overwritten, because afterwards
/// nothing names either of these again.
#[derive(Debug, Default)]
struct Previous {
    /// a cleaned or mended build of the old version
    build: Option<PathBuf>,
    /// the archive this install supersedes
    archive: Option<PathBuf>,
}

impl Previous {
    fn of(had: &loadout::Entry) -> Previous {
        Previous {
            // Only ever a build of OURS. An `Original` entry's source is the
            // staged copy, which `archive::install` has already replaced in
            // place -- deleting that would delete what was just installed.
            build: (had.variant != loadout::Variant::Original)
                .then(|| PathBuf::from(&had.source)),
            archive: had.archive.as_deref().map(PathBuf::from),
        }
    }
}

/// Delete what the previous version left, now that nothing points at it.
///
/// `dissolve_merges_of` only retires entries whose variant is `Merged`, so a
/// **cleaned or mended build of the mod being updated** used to be orphaned:
/// the record was overwritten to point at the fresh staged copy and the old
/// build stayed in `derived/` with nothing left that knew it was there.
/// Measured on a real 66-mod library: 16 of 30 derived folders were orphans.
///
/// The superseded archive goes too. That is a deliberate exception to the rule
/// `erase` follows -- there, keeping the download is the default, because
/// reinstalling the *same* mod later is then free. An archive an update has
/// replaced is a different thing: it is a version the user has moved off, and
/// keeping every one of those forever is unbounded growth nothing reports.
///
/// **Nothing outside our own folders is ever deleted**, on the same containment
/// rule `erase` uses. An archive the user installed from their own Downloads
/// folder is theirs, and `install_from_file` must not eat it.
fn reclaim_previous(places: &Places, previous: Previous, now: &Path) -> Vec<String> {
    let roots = erase::Roots::of(
        &places.staging,
        &places.derived,
        &places.archives,
        &places.mods_dir,
        None,
    );
    let mut freed = Vec::new();
    if let Some(build) = previous.build {
        if roots.covers(&build) && std::fs::remove_dir_all(&build).is_ok() {
            freed.push(build.display().to_string());
        }
    }
    if let Some(old) = previous.archive {
        // The same file means this install re-downloaded what was already
        // there, and deleting it would delete what the loadout now points at.
        // Both failing to canonicalise compares equal, which errs towards
        // keeping the file -- the safe direction.
        let same = std::fs::canonicalize(&old).ok() == std::fs::canonicalize(now).ok();
        if !same && roots.covers(&old) && std::fs::remove_file(&old).is_ok() {
            freed.push(old.display().to_string());
        }
    }
    freed
}

/// Every mod a merge is made of, read from wherever it lives now.
///
/// A merge's inputs are, by design, *not* in the game: it stands in for them,
/// so the mods folder holds the merge and nothing of theirs. Rebuilding from
/// them therefore cannot use the scan the rest of the program works from, and
/// has to go back to the staged copies the loadout points at -- `staging/
/// <owner>/<owner>/…`, the same shape `deploy` mirrors into the game.
///
/// A parent that *is* in the game is taken from the live scan instead, which
/// is the ordinary case for a first merge, where nothing stands in for
/// anything yet.
///
/// Returns the mods it could read and the names it could not, because a merge
/// missing one of its inputs must not be rebuilt as though that input never
/// existed -- that would silently drop somebody's edits.
fn read_parents(
    book: &loadout::Loadout,
    parents: &[String],
    live: &[Mod],
) -> (Vec<Mod>, Vec<String>) {
    let mut out = Vec::new();
    let mut missing = Vec::new();
    for name in parents {
        if let Some(found) = live.iter().find(|m| &m.name == name) {
            out.push(found.clone());
            continue;
        }
        let staged = book
            .get(name)
            .map(|entry| PathBuf::from(&entry.source).join(name))
            .filter(|dir| dir.is_dir());
        match staged {
            Some(dir) => out.push(discovery::scan_mod(&dir, name, true)),
            None => missing.push(name.clone()),
        }
    }
    (out, missing)
}

/// Everything a rebuilt merge now stands in for.
///
/// The inputs alone are not the answer. Merging A and B takes both out of the
/// game and leaves the merge holding the asset, so when C turns up editing the
/// same asset the conflict is between *the merge* and C -- those are the two
/// inputs, and A and B appear nowhere in them. Writing just the inputs would
/// drop A and B out of the loadout's shadow and put them straight back in the
/// game, to fight over the asset the merge exists to settle, while the merge
/// itself went missing for naming itself.
///
/// So a rebuild inherits what the build it replaces stood in for. That is
/// sound because the content is inherited the same way: the new merge is built
/// from the old one, which already carries A's and B's edits.
fn stands_in_for(
    previous: Option<&loadout::Entry>,
    owner: &str,
    inputs: &[String],
) -> Vec<String> {
    let mut out: Vec<String> = previous
        .map(|entry| entry.replaces.clone())
        .unwrap_or_default();
    for name in inputs {
        // `put` drops a self-reference too; doing it here as well keeps the
        // value this function returns honest, since it is also what the UI is
        // told the merge replaced.
        if name != owner && !out.iter().any(|held| held == name) {
            out.push(name.clone());
        }
    }
    out
}

/// What one install did.
#[derive(Debug, Clone, Serialize)]
pub struct Installed {
    pub owner: String,
    pub mod_id: u64,
    pub file_id: u64,
    /// where the archive was kept
    pub archive: String,
    pub files: usize,
    pub linked: usize,
    pub copied: usize,
    /// merges that were built from an older version of this mod, and so had
    /// to go: their asset still holds the edits this install just replaced
    #[serde(default)]
    pub dissolved: Vec<String>,
    /// the names this put in the mods folder, which is what removing it takes
    pub top_level: Vec<String>,
    /// anything the user should know, rather than an error
    pub notes: Vec<String>,
}

/// Which step is running, so a UI can say something true while it waits.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum Stage {
    Resolving,
    Downloading { bytes: u64, total: Option<u64> },
    Extracting,
    Deploying,
    Done,
}

/// Do the whole thing, from the website's link to a deployed mod.
pub fn install_from_link(
    api: &nexus::Api,
    link: &nxm::Link,
    places: &Places,
    loadout_path: &Path,
    mut on_stage: impl FnMut(Stage),
) -> Result<Installed, String> {
    if link.expired_now() {
        return Err(
            "that download link has expired. Press the download button on the mod page again."
                .into(),
        );
    }

    on_stage(Stage::Resolving);
    let mirrors = api.download_link(
        link.mod_id,
        link.file_id,
        Some((link.key.as_str(), link.expires)),
    )?;
    let chosen = &mirrors[0];

    // What the file is *called* matters as much as what is in it.
    //
    // A mod's Nexus id and version survive only in its archive's name — that is
    // how every mod adopted from Vortex is identified, and `archive_index` reads
    // it straight back off the path recorded below. The CDN mirror does not
    // reliably carry it: `Unpredictable Shelters 1.4` arrived from a URL ending
    // in `8430ad00-1808-493e-bb04-d6ce529ef1ac`, with no extension, so the
    // download was anonymous, the staging folder was named after the folder
    // *inside* the archive — which has no id and no timestamp and parses to
    // nothing — and the mod lost its page, its version and its update checking
    // the moment it was installed here rather than by Vortex.
    //
    // One extra request, on an install that is already making two. A failure
    // falls back to the URL's name rather than stopping the install: an
    // unidentifiable mod is worth much more than no mod.
    let named = api
        .files(link.mod_id)
        .ok()
        .and_then(|(body, _)| body.files.into_iter().find(|f| f.file_id == link.file_id))
        .map(|f| f.file_name);

    let archive_path = download::fetch(&chosen.uri, &places.archives, named.as_deref(), |p| {
        on_stage(Stage::Downloading {
            bytes: p.bytes,
            total: p.total,
        })
    })?;

    on_stage(Stage::Extracting);
    // Extract into staging, not into the game. The game only ever sees links.
    let plan = archive::install(&archive_path, &places.staging, true)?;

    on_stage(Stage::Deploying);
    // Take out whatever is deployed under this name first: reinstalling a mod
    // over itself is the common case, and deploy refuses to merge into a
    // folder that already has files.
    let staged = places.staging.join(&plan.owner);
    // A reinstall over an existing entry is the common case; take out exactly
    // what the loadout recorded rather than guessing from the name.
    let mut previous = Previous::default();
    if let Some(had) = loadout::Loadout::read(loadout_path).get(&plan.owner) {
        deploy::undeploy(&places.mods_dir, &had.deployed)?;
        previous = Previous::of(had);
    }
    let placed = deploy::deploy(&staged, &places.mods_dir)?;

    // Record it, so the loadout knows this mod is ours to manage.
    let mut book = loadout::Loadout::read(loadout_path);
    let replaces = book
        .get(&plan.owner)
        .map(|e| e.replaces.clone())
        .unwrap_or_default();
    book.put(loadout::Entry {
        owner: plan.owner.clone(),
        source: staged.display().to_string(),
        origin: Some(staged.display().to_string()),
        archive: Some(archive_path.display().to_string()),
        variant: loadout::Variant::Original,
        replaces,
        deployed: placed.top_level.clone(),
        built_from: Some(staged.display().to_string()),
        enabled: true,
        edited: false,
    });
    // Installing over a mod that some merge was built from invalidates that
    // merge: it holds the *old* version's edits and suppresses this one, so
    // the update would never reach the game. Take it out and let the next scan
    // offer the merge again, now against what is actually installed.
    let stale = retire_merges_of(&mut book, &plan.owner, &places.mods_dir);
    book.write(loadout_path)?;
    // Only once the new record is safely on disk: a crash before this leaves
    // the old files in place, which is recoverable, where the other order
    // leaves a loadout pointing at something that has been deleted.
    let freed = reclaim_previous(places, previous, &archive_path);

    on_stage(Stage::Done);
    Ok(Installed {
        owner: plan.owner,
        mod_id: link.mod_id,
        file_id: link.file_id,
        archive: archive_path.display().to_string(),
        files: plan.files,
        linked: placed.linked,
        copied: placed.copied,
        dissolved: stale,
        top_level: placed.top_level,
        notes: with_reclaimed(plan.notes, &freed),
    })
}

/// Install an archive the user already has, skipping the first two steps.
pub fn install_from_file(
    archive_path: &Path,
    places: &Places,
    loadout_path: &Path,
    overwrite: bool,
) -> Result<Installed, String> {
    // Who the archive is, before a byte of it is written anywhere.
    //
    // The refusal below used to sit *after* the extract, which made "no, do
    // not replace it" replace something anyway: `archive::install` clears
    // `staging/<owner>` and writes the new version into it, so declining left
    // the staged copy already updated, the loadout still describing the old
    // one, and the game holding a build nothing pointed at. A refusal has to
    // cost nothing, so it is decided first.
    let named = archive::preview(archive_path, &places.staging)?;

    let book = loadout::Loadout::read(loadout_path);
    // Cloned so the refusal can be decided, and the old entry still read,
    // without holding a borrow of `book` across the write below.
    let known = book.get(&named.owner).cloned();
    if known.is_some() && !overwrite {
        // Not "already in the mods folder": this is a fact about the *loadout*,
        // and the two come apart -- a mod recorded here whose folder another
        // manager has since purged is installed as far as this program is
        // concerned and absent as far as the game is. Saying the wrong one of
        // those sends the user looking in the mods folder for something that
        // is not there.
        return Err(format!(
            "{} is already installed. Choose to replace it, or remove it first.",
            named.owner
        ));
    }

    let plan = archive::install(archive_path, &places.staging, true)?;
    let staged = places.staging.join(&plan.owner);

    let mut previous = Previous::default();
    if let Some(had) = &known {
        deploy::undeploy(&places.mods_dir, &had.deployed)?;
        previous = Previous::of(had);
    }
    let placed = deploy::deploy(&staged, &places.mods_dir)?;

    let mut book = book;
    let replaces = book
        .get(&plan.owner)
        .map(|e| e.replaces.clone())
        .unwrap_or_default();
    book.put(loadout::Entry {
        owner: plan.owner.clone(),
        source: staged.display().to_string(),
        origin: Some(staged.display().to_string()),
        archive: Some(archive_path.display().to_string()),
        variant: loadout::Variant::Original,
        replaces,
        deployed: placed.top_level.clone(),
        built_from: Some(staged.display().to_string()),
        enabled: true,
        edited: false,
    });
    // Installing over a mod that some merge was built from invalidates that
    // merge: it holds the *old* version's edits and suppresses this one, so
    // the update would never reach the game. Take it out and let the next scan
    // offer the merge again, now against what is actually installed.
    let stale = retire_merges_of(&mut book, &plan.owner, &places.mods_dir);
    book.write(loadout_path)?;
    // Only once the new record is safely on disk: a crash before this leaves
    // the old files in place, which is recoverable, where the other order
    // leaves a loadout pointing at something that has been deleted.
    let freed = reclaim_previous(places, previous, &archive_path);

    Ok(Installed {
        owner: plan.owner,
        mod_id: 0,
        file_id: 0,
        archive: archive_path.display().to_string(),
        files: plan.files,
        linked: placed.linked,
        copied: placed.copied,
        dissolved: stale,
        top_level: placed.top_level,
        notes: with_reclaimed(plan.notes, &freed),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn merged_entry(owner: &str, replaces: &[&str]) -> loadout::Entry {
        loadout::Entry {
            owner: owner.into(),
            source: format!("/d/{owner}"),
            origin: None,
            archive: None,
            variant: loadout::Variant::Merged,
            replaces: replaces.iter().map(|s| s.to_string()).collect(),
            deployed: Vec::new(),
            built_from: None,
            enabled: true,
            edited: false,
        }
    }

    #[test]
    fn parents_are_read_from_the_game_or_from_staging_and_missing_ones_are_named() {
        // The three cases a rebuild meets. A parent still in the game (the
        // first merge of it), a parent only staged because a merge already
        // stands in for it, and one whose files are simply not there -- which
        // must be reported, never quietly left out of the rebuild.
        let dir = Dir::new("parents");
        dir.file("staging/Staged Mod/Staged Mod/ASSET.EXML", "<Data/>");

        let mut book = loadout::Loadout::default();
        for name in ["Staged Mod", "Vanished Mod"] {
            book.put(loadout::Entry {
                owner: name.into(),
                source: dir.0.join("staging").join(name).display().to_string(),
                origin: None,
                archive: None,
                variant: loadout::Variant::Original,
                replaces: Vec::new(),
                deployed: Vec::new(),
                built_from: None,
                enabled: true,
                edited: false,
            });
        }

        let live = vec![Mod {
            name: "Live Mod".into(),
            ..Default::default()
        }];
        let parents = vec![
            "Live Mod".to_string(),
            "Staged Mod".to_string(),
            "Vanished Mod".to_string(),
        ];

        let (found, missing) = read_parents(&book, &parents, &live);
        assert_eq!(
            found.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            vec!["Live Mod", "Staged Mod"]
        );
        assert_eq!(missing, vec!["Vanished Mod".to_string()]);
    }

    #[test]
    fn a_first_merge_stands_in_for_exactly_its_inputs() {
        let inputs = vec!["Mod A".to_string(), "Mod B".to_string()];
        assert_eq!(stands_in_for(None, "zzz_merge", &inputs), inputs);
    }

    #[test]
    fn absorbing_a_newcomer_keeps_holding_the_originals_back() {
        // The conflict this time is between the merge and C: A and B are not
        // in the game, so they are not in the inputs. Only the entry being
        // overwritten remembers them, and if the rebuild drops them they go
        // straight back into the game against the asset the merge settles.
        let previous = merged_entry("zzz_merge", &["Mod A", "Mod B"]);
        let inputs = vec!["zzz_merge".to_string(), "Mod C".to_string()];
        assert_eq!(
            stands_in_for(Some(&previous), "zzz_merge", &inputs),
            vec!["Mod A", "Mod B", "Mod C"]
        );
    }

    #[test]
    fn a_merge_never_stands_in_for_itself() {
        let previous = merged_entry("zzz_merge", &["Mod A"]);
        let inputs = vec!["zzz_merge".to_string()];
        let out = stands_in_for(Some(&previous), "zzz_merge", &inputs);
        assert!(!out.contains(&"zzz_merge".to_string()));
    }

    #[test]
    fn re_merging_the_same_inputs_does_not_list_them_twice() {
        // Rebuilding after one input was updated runs with the same names.
        let previous = merged_entry("zzz_merge", &["Mod A", "Mod B"]);
        let inputs = vec!["Mod A".to_string(), "Mod B".to_string()];
        assert_eq!(
            stands_in_for(Some(&previous), "zzz_merge", &inputs),
            vec!["Mod A", "Mod B"]
        );
    }

    /// Updating a cleaned mod must not leave its old build in `derived/`.
    ///
    /// This is the leak the variant filter caused: `dissolve_merges_of` only
    /// retires entries whose variant is `Merged`, so a *cleaned* or *mended*
    /// build was orphaned by the update -- the record moved to the fresh staged
    /// copy and nothing was left that knew the old build existed. Measured on a
    /// real 66-mod library before the fix: 16 of 30 derived folders were orphans.
    #[test]
    fn updating_a_cleaned_mod_takes_its_old_build_with_it() {
        let dir = Dir::new("reclaim_build");
        let places = dir.places();
        std::fs::create_dir_all(&places.derived).unwrap();

        let build = places.derived.join("Cool Mod");
        std::fs::create_dir_all(&build).unwrap();
        std::fs::write(build.join("CLEANED.EXML"), "<Data/>").unwrap();

        let had = loadout::Entry {
            owner: "Cool Mod".into(),
            source: build.display().to_string(),
            origin: Some(places.staging.join("Cool Mod").display().to_string()),
            archive: None,
            variant: loadout::Variant::Cleaned,
            replaces: Vec::new(),
            deployed: Vec::new(),
            built_from: None,
            enabled: true,
            edited: false,
        };

        let freed = reclaim_previous(&places, Previous::of(&had), Path::new("new.zip"));
        assert!(!build.exists(), "the old cleaned build is still in derived/");
        assert_eq!(freed.len(), 1);
    }

    /// The staged copy of an ordinary mod is NOT a build of ours.
    ///
    /// `archive::install` has already replaced it in place by this point, so
    /// deleting what `source` names would delete what was just installed.
    #[test]
    fn updating_an_ordinary_mod_does_not_delete_what_was_just_staged() {
        let dir = Dir::new("reclaim_original");
        let places = dir.places();
        let staged = places.staging.join("Cool Mod");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("A.EXML"), "<Data/>").unwrap();

        let had = loadout::Entry {
            owner: "Cool Mod".into(),
            source: staged.display().to_string(),
            origin: Some(staged.display().to_string()),
            archive: None,
            variant: loadout::Variant::Original,
            replaces: Vec::new(),
            deployed: Vec::new(),
            built_from: None,
            enabled: true,
            edited: false,
        };

        let freed = reclaim_previous(&places, Previous::of(&had), Path::new("new.zip"));
        assert!(staged.exists(), "the freshly staged copy was deleted");
        assert!(freed.is_empty());
    }

    /// The superseded archive goes, but only ours, and never the new one.
    #[test]
    fn the_old_archive_goes_and_the_users_own_does_not() {
        let dir = Dir::new("reclaim_archive");
        let places = dir.places();
        std::fs::create_dir_all(&places.archives).unwrap();

        let old = places.archives.join("Cool Mod 1.0.zip");
        let new = places.archives.join("Cool Mod 2.0.zip");
        std::fs::write(&old, b"old").unwrap();
        std::fs::write(&new, b"new").unwrap();

        // Somewhere that is not ours: the user's own download folder.
        let theirs = dir.0.join("Downloads");
        std::fs::create_dir_all(&theirs).unwrap();
        let mine = theirs.join("Cool Mod 1.0.zip");
        std::fs::write(&mine, b"theirs").unwrap();

        let entry = |archive: &Path| loadout::Entry {
            owner: "Cool Mod".into(),
            source: places.staging.join("Cool Mod").display().to_string(),
            origin: None,
            archive: Some(archive.display().to_string()),
            variant: loadout::Variant::Original,
            replaces: Vec::new(),
            deployed: Vec::new(),
            built_from: None,
            enabled: true,
            edited: false,
        };

        reclaim_previous(&places, Previous::of(&entry(&old)), &new);
        assert!(!old.exists(), "the superseded archive is still there");
        assert!(new.exists(), "the archive just downloaded was deleted");

        // An archive the user installed from their own folder is theirs.
        reclaim_previous(&places, Previous::of(&entry(&mine)), &new);
        assert!(mine.exists(), "deleted a file outside the managed folders");

        // Re-downloading the same file must not delete what the record now
        // points at. Both paths are the same file, so nothing should go.
        reclaim_previous(&places, Previous::of(&entry(&new)), &new);
        assert!(new.exists(), "deleted the archive the loadout now names");
    }

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let path = std::env::current_dir()
                .unwrap_or_else(|_| std::env::temp_dir())
                .join(format!("target/nmscheck_pipeline_{tag}"));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Dir(path)
        }

        fn file(&self, rel: &str, body: &str) {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }

        fn places(&self) -> Places {
            Places {
                archives: self.0.join("archives"),
                staging: self.0.join("staging"),
                derived: self.0.join("derived"),
                mods_dir: self.0.join("MODS"),
            }
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Build a real .zip with 7-Zip, or skip when it is not installed.
    fn make_zip(dir: &Path, name: &str, entries: &[(&str, &str)]) -> Option<PathBuf> {
        let seven = archive::find_7z()?;
        let source = dir.join("_build");
        for (rel, body) in entries {
            let path = source.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).ok()?;
            std::fs::write(path, body).ok()?;
        }
        let zip = dir.join(name);
        let out = std::process::Command::new(seven)
            .arg("a")
            .arg("-tzip")
            .arg(&zip)
            .arg(source.join("*"))
            .args(["-bso0", "-bse0"])
            .output()
            .ok()?;
        let _ = std::fs::remove_dir_all(&source);
        out.status.success().then_some(zip)
    }

    #[test]
    fn an_archive_becomes_a_staged_mod_and_a_deployed_link() {
        let dir = Dir::new("install");
        let places = dir.places();
        std::fs::create_dir_all(&places.mods_dir).unwrap();
        let book = dir.0.join("loadout.json");

        let Some(zip) = make_zip(
            &dir.0,
            "Cool Mod 1.0.zip",
            &[
                ("Cool Mod/MODELS/A.EXML", "<asset/>"),
                ("Cool Mod/readme.txt", "hello"),
            ],
        ) else {
            eprintln!("7-Zip not installed; skipping");
            return;
        };

        let done = install_from_file(&zip, &places, &book, false).unwrap();
        assert_eq!(done.owner, "Cool Mod");
        assert_eq!(done.files, 2);

        // Staged, deployed, and recorded -- all three.
        assert!(places.staging.join("Cool Mod/Cool Mod/MODELS/A.EXML").exists());
        assert!(places.mods_dir.join("Cool Mod/MODELS/A.EXML").exists());
        assert!(loadout::Loadout::read(&book).get("Cool Mod").is_some());
    }

    #[test]
    fn reinstalling_over_an_existing_mod_needs_saying_so() {
        let dir = Dir::new("replace");
        let places = dir.places();
        std::fs::create_dir_all(&places.mods_dir).unwrap();
        let book = dir.0.join("loadout.json");

        let Some(zip) = make_zip(&dir.0, "Cool Mod.zip", &[("Cool Mod/A.EXML", "<v1/>")]) else {
            eprintln!("7-Zip not installed; skipping");
            return;
        };
        install_from_file(&zip, &places, &book, false).unwrap();

        assert!(install_from_file(&zip, &places, &book, false).is_err());
        assert!(install_from_file(&zip, &places, &book, true).is_ok());
    }

    #[test]
    fn a_refused_reinstall_leaves_the_staged_copy_alone() {
        // Saying no has to cost nothing.
        //
        // The refusal used to happen *after* the extract, so declining to
        // replace a mod replaced the staged copy of it anyway: staging held
        // the new version, the loadout still described the old one, and the
        // only way to notice was to read the folder.
        let dir = Dir::new("refused");
        let places = dir.places();
        std::fs::create_dir_all(&places.mods_dir).unwrap();
        let book = dir.0.join("loadout.json");

        let Some(first) = make_zip(&dir.0, "Cool Mod.zip", &[("Cool Mod/A.EXML", "<v1/>")]) else {
            eprintln!("7-Zip not installed; skipping");
            return;
        };
        install_from_file(&first, &places, &book, false).unwrap();

        // A second version of the same mod, refused.
        let second = make_zip(&dir.0, "Cool Mod v2.zip", &[("Cool Mod/A.EXML", "<v2/>")]).unwrap();
        assert!(install_from_file(&second, &places, &book, false).is_err());

        let staged = places.staging.join("Cool Mod/Cool Mod/A.EXML");
        assert_eq!(
            std::fs::read_to_string(&staged).unwrap(),
            "<v1/>",
            "refusing to install must not have staged the new version"
        );
        assert_eq!(
            std::fs::read_to_string(places.mods_dir.join("Cool Mod/A.EXML")).unwrap(),
            "<v1/>",
            "and the game must still be reading the one that is recorded"
        );

        // Saying yes replaces both, and the wording of the refusal does not
        // claim the mods folder, which is not what it was decided from.
        let said = install_from_file(&second, &places, &book, false).unwrap_err();
        assert!(said.contains("already installed"), "{said}");
        assert!(!said.contains("mods folder"), "{said}");

        install_from_file(&second, &places, &book, true).unwrap();
        assert_eq!(std::fs::read_to_string(&staged).unwrap(), "<v2/>");
        assert_eq!(
            std::fs::read_to_string(places.mods_dir.join("Cool Mod/A.EXML")).unwrap(),
            "<v2/>"
        );
    }

    #[test]
    fn a_mod_whose_folder_was_purged_is_still_known_to_be_installed() {
        // The state a Vortex purge leaves: the loadout records the mod and the
        // game folder is empty. The refusal is decided on the loadout, so this
        // must refuse -- and, because it does, the screen has to be able to
        // learn that beforehand rather than offering a plain "Install it" and
        // running into a refusal the user was never asked about.
        let dir = Dir::new("purged");
        let places = dir.places();
        std::fs::create_dir_all(&places.mods_dir).unwrap();
        let book = dir.0.join("loadout.json");

        let Some(zip) = make_zip(&dir.0, "Cool Mod.zip", &[("Cool Mod/A.EXML", "<v1/>")]) else {
            eprintln!("7-Zip not installed; skipping");
            return;
        };
        install_from_file(&zip, &places, &book, false).unwrap();

        // Something else takes the files away, leaving the record behind.
        std::fs::remove_dir_all(places.mods_dir.join("Cool Mod")).unwrap();

        // `archive::preview` alone cannot see this -- there is no folder in the
        // way any more -- which is exactly why `install_preview` asks the
        // loadout as well.
        let blind = archive::preview(&zip, &places.mods_dir).unwrap();
        assert!(!blind.collides, "the folder really is gone");
        assert!(
            loadout::Loadout::read(&book).get(&blind.owner).is_some(),
            "but the loadout still records it, and that is what install refuses on"
        );

        assert!(install_from_file(&zip, &places, &book, false).is_err());
        install_from_file(&zip, &places, &book, true).unwrap();
        assert!(places.mods_dir.join("Cool Mod/A.EXML").exists());
    }

    #[test]
    fn the_layers_live_beside_the_game_not_in_the_user_profile() {
        // A hardlink cannot cross volumes, so staging on C: for a game on D:
        // would silently degrade every deploy into a full copy.
        let places = Places::beside(
            Path::new(r"D:\SteamLibrary\steamapps\common\No Man's Sky"),
            Path::new(r"D:\SteamLibrary\steamapps\common\No Man's Sky\GAMEDATA\MODS"),
        );
        assert!(places.staging.display().to_string().starts_with("D:"));
        assert!(places.archives.display().to_string().starts_with("D:"));
    }

    #[test]
    fn a_mod_with_a_script_beside_it_deploys_to_the_right_places() {
        // The commonest real shape: 36 of the 62 mods in the measured library
        // put a folder, a `.lua` and a readme side by side in one archive.
        // They must land in the mods folder as siblings, exactly as they sit
        // in the archive -- not nested inside a folder named after the zip.
        let dir = Dir::new("amumss");
        let places = dir.places();
        std::fs::create_dir_all(&places.mods_dir).unwrap();
        let book = dir.0.join("loadout.json");

        let Some(zip) = make_zip(
            &dir.0,
            "Unpredictable Shelters 1.3-2308-1-3-1738789143.zip",
            &[
                ("Unpredictable Shelters 1.4/MODELS/A.EXML", "<asset/>"),
                ("Unpredictable Shelters 1.4.lua", "the script"),
                ("Installation Notes for Unpredictable Shelters.txt", "readme"),
            ],
        ) else {
            eprintln!("7-Zip not installed; skipping");
            return;
        };

        install_from_file(&zip, &places, &book, false).unwrap();

        let mods = &places.mods_dir;
        assert!(
            mods.join("Unpredictable Shelters 1.4/MODELS/A.EXML").exists(),
            "the mod folder is not where the game looks for it"
        );

        // The script and the readme must not have been buried inside a folder
        // named after the zip -- that is what this test is really about -- but
        // they belong in staging, not in the game folder, which reads neither.
        let staged = places.staging.join("Unpredictable Shelters 1.4");
        assert!(
            staged.join("Unpredictable Shelters 1.4.lua").is_file(),
            "the script must sit beside the folder, not inside a wrapper"
        );
        assert!(staged
            .join("Installation Notes for Unpredictable Shelters.txt")
            .is_file());
        assert!(!mods.join("Unpredictable Shelters 1.4.lua").exists());
        assert!(!mods
            .join("Installation Notes for Unpredictable Shelters.txt")
            .exists());
    }

}
