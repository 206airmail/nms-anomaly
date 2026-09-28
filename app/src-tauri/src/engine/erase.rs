//! Deleting a mod for real, rather than only taking it out of the game.
//!
//! Uninstalling is deliberately non-destructive: it removes the game's
//! hardlinks and leaves the staged files, so putting the mod back costs a
//! relink. That is the right default, but it means an unwanted mod still
//! occupies its full size on disk, plus its download, forever. This is the
//! other half.
//!
//! # Three things, not one
//!
//! A mod that has been through the whole pipeline exists in up to four places:
//! the downloaded archive, the staged extraction, a derived build if it was
//! mended or cleaned, and the game's hardlinks. Deleting "the mod" has to mean
//! all of them, or the space is not actually recovered.
//!
//! The **download is a separate decision**, because it is the only part that
//! costs bandwidth rather than just disk. Deleting a mod but keeping its
//! archive means reinstalling it later is free and offline.
//!
//! # Nothing outside our own folders is ever deleted
//!
//! This is the one operation here that cannot be undone, and the path it
//! deletes comes from `loadout.json` -- a plain text file the user can edit,
//! that a bad merge could corrupt, and that this program has already written
//! wrong once. So a path is deleted only if it is provably *inside* one of the
//! folders this program owns, checked after resolving the real location on
//! disk so that `..` and links cannot walk out. Anything else is refused by
//! name and left alone, and [`plan`] reports that refusal rather than
//! pretending the mod was removed.
//!
//! # The size reported is the size actually recovered
//!
//! Deployed files are hardlinks to the staged ones, so the same bytes have two
//! names and deleting only the game's copy frees nothing. [`plan`] counts each
//! file once, at its source, which is what the disk will actually give back.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::deploy;
use super::loadout::{Entry, Loadout};

/// The folders this program is allowed to delete inside.
#[derive(Debug, Clone)]
pub struct Roots {
    pub allowed: Vec<PathBuf>,
    pub mods_dir: PathBuf,
    /// where every build this program makes for a mod is written, as
    /// `derived/<owner>`. Kept apart from `allowed` because [`plan`] has to
    /// *look* there, not merely permit it.
    pub derived: PathBuf,
}

impl Roots {
    /// Every folder a mod's files can legitimately live in.
    ///
    /// Both the configured staging folder *and* the default one, because a
    /// library assembled over time has mods in both -- changing the setting
    /// does not move what is already staged.
    pub fn of(staging: &Path, derived: &Path, archives: &Path, mods_dir: &Path, game_root: Option<&Path>) -> Roots {
        let mut allowed = vec![
            staging.to_path_buf(),
            derived.to_path_buf(),
            archives.to_path_buf(),
        ];
        if let Some(root) = game_root {
            allowed.push(deploy::staging_for(root));
        }
        Roots {
            allowed,
            mods_dir: mods_dir.to_path_buf(),
            derived: derived.to_path_buf(),
        }
    }

    /// True when `path` really is inside one of the allowed folders.
    ///
    /// Resolved through the filesystem first, so a `source` of
    /// `D:\Staging\..\..\Windows` is judged on where it actually lands.
    pub fn covers(&self, path: &Path) -> bool {
        let Ok(real) = std::fs::canonicalize(path) else {
            return false; // not there, so nothing to delete and nothing to allow
        };
        self.allowed.iter().any(|root| {
            std::fs::canonicalize(root)
                .map(|root| real.starts_with(&root) && real != root)
                .unwrap_or(false)
        })
    }
}

/// What kind of thing is being deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum What {
    /// the mod as extracted, which is the real copy the game's links point at
    Staged,
    /// a mended, cleaned or merged build
    Derived,
    /// the downloaded archive
    Archive,
}

/// One thing that would be deleted.
#[derive(Debug, Clone, Serialize)]
pub struct Item {
    pub path: String,
    pub what: What,
    pub files: usize,
    pub bytes: u64,
}

/// Everything deleting one mod would do, before any of it happens.
#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub owner: String,
    /// names to take out of the game folder
    pub deployed: Vec<String>,
    pub items: Vec<Item>,
    /// bytes the disk will actually give back
    pub bytes: u64,
    /// paths that are not inside this program's folders, so will not be
    /// touched. Named, because silently skipping them would make "deleted"
    /// untrue.
    pub refused: Vec<String>,
    /// things the user should weigh before saying yes
    pub warnings: Vec<String>,
}

fn measure(path: &Path) -> (usize, u64) {
    if path.is_file() {
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        return (1, size);
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return (0, 0);
    };
    let mut files = 0;
    let mut bytes = 0;
    for entry in entries.flatten() {
        let (f, b) = measure(&entry.path());
        files += f;
        bytes += b;
    }
    (files, bytes)
}

/// Work out what deleting this mod would remove. Touches nothing.
///
/// `with_archive` is the user's separate answer about the download.
pub fn plan(entry: &Entry, roots: &Roots, with_archive: bool) -> Plan {
    let mut items = Vec::new();
    let mut refused = Vec::new();
    let mut warnings = Vec::new();

    // The staged original, and the current build if it is a different folder.
    let origin = entry.origin_path();
    let current = PathBuf::from(&entry.source);

    let mut consider = vec![(origin.clone(), What::Staged)];
    if current != origin {
        consider.push((current.clone(), What::Derived));
    }
    // And the build folder itself, even when nothing points at it any more.
    //
    // Switching a mod back to the build its author shipped deliberately leaves
    // the cleaned or mended build on disk — that is what makes the undo free,
    // and what makes switching back to it free too. But it means `source` no
    // longer names it, so a delete that only looked at `source` walked straight
    // past it: undo, then delete, and the build stayed in `derived/` for good,
    // with nothing left in the loadout that knew it was there. Measured: one
    // orphan of 37 files from a mod the user had deleted with "keep the
    // download" turned off, expecting nothing of it to remain.
    let built = roots.derived.join(&entry.owner);
    if built != origin && built != current {
        consider.push((built, What::Derived));
    }
    if with_archive {
        if let Some(archive) = entry.archive.as_deref() {
            consider.push((PathBuf::from(archive), What::Archive));
        }
    }

    for (path, what) in consider {
        if !path.exists() {
            continue; // already gone
        }
        if !roots.covers(&path) {
            refused.push(path.display().to_string());
            continue;
        }
        let (files, bytes) = measure(&path);
        items.push(Item {
            path: path.display().to_string(),
            what,
            files,
            bytes,
        });
    }

    if entry.archive.is_none() {
        warnings.push(
            "There is no record of a downloaded archive for this mod, so reinstalling it \
             means downloading it again."
                .into(),
        );
    } else if !with_archive {
        warnings.push("The download is being kept, so reinstalling costs no bandwidth.".into());
    }

    if !refused.is_empty() {
        warnings.push(
            "Some of this mod's files are outside the folders this program manages and will \
             be left exactly where they are."
                .into(),
        );
    }

    let bytes = items.iter().map(|i| i.bytes).sum();
    Plan {
        owner: entry.owner.clone(),
        deployed: entry.deployed.clone(),
        items,
        bytes,
        refused,
        warnings,
    }
}

/// What a deletion actually did.
#[derive(Debug, Clone, Serialize)]
pub struct Erased {
    pub owner: String,
    /// names removed from the game folder
    pub undeployed: usize,
    pub removed: Vec<String>,
    pub bytes: u64,
    /// merges that were built out of this mod, and so went with it
    #[serde(default)]
    pub dissolved: Vec<String>,
    /// things that could not be removed, with the reason
    pub problems: Vec<String>,
}

/// Carry out a plan: take the mod out of the game, delete its files, forget it.
///
/// The game's names go first. Deleting a staged file while the game still has
/// a name for it would leave that name pointing at content this program no
/// longer manages -- the file survives, because a hardlink keeps the data
/// alive until the last name goes, so the "deleted" mod would still load.
pub fn erase(plan: &Plan, roots: &Roots, loadout: &mut Loadout) -> Result<Erased, String> {
    let mut problems = Vec::new();

    let undeployed = match deploy::undeploy(&roots.mods_dir, &plan.deployed) {
        Ok(n) => n,
        Err(e) => {
            problems.push(e);
            0
        }
    };

    let mut removed = Vec::new();
    let mut bytes = 0;
    for item in &plan.items {
        let path = PathBuf::from(&item.path);
        // Checked again here, not just in the plan: a plan can be held while
        // the loadout is edited, and this is the irreversible step.
        if !roots.covers(&path) {
            problems.push(format!(
                "{} is not inside a folder this program manages, so it was left alone",
                item.path
            ));
            continue;
        }
        let gone = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match gone {
            Ok(()) => {
                removed.push(item.path.clone());
                bytes += item.bytes;
            }
            Err(e) => problems.push(format!("could not delete {}: {e}", item.path)),
        }
    }

    // Takes any merge built out of this mod with it: that build has this
    // mod's edits baked into it, so leaving it in the game would keep running
    // the mod the user just deleted. The survivors come back at the next
    // reconcile and the conflict, if there still is one, is offered afresh.
    let dissolved = loadout.forget(&plan.owner);
    // Their files go too. `reconcile` takes away what an entry deployed, and
    // these entries no longer exist -- so if this does not do it, nothing
    // will, and the merge stays in the game standing for a mod that is gone.
    for entry in &dissolved {
        if let Err(problem) = deploy::undeploy(&roots.mods_dir, &entry.deployed) {
            problems.push(problem);
        }
        // The build itself is ours, not the user's, and is worth nothing now.
        let build = PathBuf::from(&entry.source);
        if roots.covers(&build) {
            let _ = std::fs::remove_dir_all(&build);
        }
    }
    let dissolved: Vec<String> = dissolved.into_iter().map(|e| e.owner).collect();

    Ok(Erased {
        owner: plan.owner.clone(),
        undeployed,
        removed,
        bytes,
        dissolved,
        problems,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::loadout::Variant;

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let path = std::env::current_dir()
                .unwrap_or_else(|_| std::env::temp_dir())
                .join(format!("target/nmscheck_erase_{tag}"));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Dir(path)
        }
        fn file(&self, rel: &str, body: &str) -> PathBuf {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, body).unwrap();
            path
        }
        fn roots(&self) -> Roots {
            Roots {
                allowed: vec![
                    self.0.join("staging"),
                    self.0.join("derived"),
                    self.0.join("archives"),
                ],
                mods_dir: self.0.join("MODS"),
                derived: self.0.join("derived"),
            }
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn entry(dir: &Dir, source: &str, variant: Variant) -> Entry {
        Entry {
            owner: "M".into(),
            source: dir.0.join(source).display().to_string(),
            origin: Some(dir.0.join("staging/M").display().to_string()),
            archive: Some(dir.0.join("archives/M.zip").display().to_string()),
            variant,
            replaces: Vec::new(),
            deployed: vec!["M".into()],
            built_from: None,
            enabled: true,
            edited: false,
        }
    }

    /// A merge, its two inputs, and all three staged as the pipeline stages them.
    fn merged_library(dir: &Dir) -> Loadout {
        dir.file("staging/A/A/ASSET.EXML", "a");
        dir.file("staging/B/B/ASSET.EXML", "b");
        dir.file("derived/MERGE/MERGE/ASSET.EXML", "a+b");
        std::fs::create_dir_all(dir.0.join("MODS")).unwrap();

        let mut book = Loadout::default();
        for name in ["A", "B"] {
            book.put(Entry {
                owner: name.into(),
                source: dir.0.join(format!("staging/{name}")).display().to_string(),
                origin: Some(dir.0.join(format!("staging/{name}")).display().to_string()),
                archive: None,
                variant: Variant::Original,
                replaces: Vec::new(),
                deployed: Vec::new(),
                built_from: None,
                enabled: true,
                edited: false,
            });
        }
        book.put(Entry {
            owner: "MERGE".into(),
            source: dir.0.join("derived/MERGE").display().to_string(),
            origin: Some(dir.0.join("derived/MERGE").display().to_string()),
            archive: None,
            variant: Variant::Merged,
            replaces: vec!["A".into(), "B".into()],
            deployed: Vec::new(),
            built_from: None,
            enabled: true,
            edited: false,
        });
        book
    }

    #[test]
    fn deleting_a_merge_takes_only_the_merge_and_gives_the_inputs_back() {
        // The whole question a merge raises: its inputs are mods in their own
        // right, installed deliberately, and they are only out of the game
        // because the merge stands in for them. Deleting the merge must take
        // exactly the build we made and hand them straight back -- never delete
        // somebody's mod because it happened to be an ingredient.
        let dir = Dir::new("mergedelete");
        let mut book = merged_library(&dir);

        let changes = super::super::loadout::reconcile(&mut book, &dir.0.join("MODS"), false);
        assert!(changes.problems.is_empty(), "{:?}", changes.problems);
        assert!(dir.0.join("MODS/MERGE/ASSET.EXML").exists(), "merge not in the game");
        assert!(!dir.0.join("MODS/A").exists(), "input A should be held out");
        assert!(!dir.0.join("MODS/B").exists(), "input B should be held out");

        let plan = plan(book.get("MERGE").unwrap(), &dir.roots(), true);
        assert_eq!(plan.items.len(), 1, "only the merge's own build: {:?}", plan.items);
        erase(&plan, &dir.roots(), &mut book).unwrap();

        assert!(book.get("MERGE").is_none(), "merge still listed");
        assert!(book.get("A").is_some(), "input A was deleted with the merge");
        assert!(book.get("B").is_some(), "input B was deleted with the merge");
        assert!(dir.0.join("staging/A/A/ASSET.EXML").exists(), "A's files were deleted");
        assert!(dir.0.join("staging/B/B/ASSET.EXML").exists(), "B's files were deleted");

        // And the reconcile that `delete_mods` runs afterwards puts them back.
        let after = super::super::loadout::reconcile(&mut book, &dir.0.join("MODS"), false);
        assert!(after.problems.is_empty(), "{:?}", after.problems);
        assert!(dir.0.join("MODS/A/ASSET.EXML").exists(), "A did not come back");
        assert!(dir.0.join("MODS/B/ASSET.EXML").exists(), "B did not come back");
        assert!(!dir.0.join("MODS/MERGE").exists(), "merge still in the game");
    }

    #[test]
    fn deactivating_a_merge_also_gives_the_inputs_back() {
        // The reversible half of the same idea, and the reason `superseded`
        // counts only enabled entries.
        let dir = Dir::new("mergeoff");
        let mut book = merged_library(&dir);
        super::super::loadout::reconcile(&mut book, &dir.0.join("MODS"), false);
        assert!(dir.0.join("MODS/MERGE").exists());

        book.entries
            .iter_mut()
            .find(|e| e.owner == "MERGE")
            .unwrap()
            .enabled = false;
        let after = super::super::loadout::reconcile(&mut book, &dir.0.join("MODS"), false);
        assert!(after.problems.is_empty(), "{:?}", after.problems);

        assert!(!dir.0.join("MODS/MERGE").exists(), "merge still in the game");
        assert!(dir.0.join("MODS/A/ASSET.EXML").exists(), "A did not come back");
        assert!(dir.0.join("MODS/B/ASSET.EXML").exists(), "B did not come back");
    }

    #[test]
    fn deleting_an_input_dissolves_the_merge_and_gives_the_rest_back() {
        // Deleting an ingredient is allowed -- it is the user's mod. What must
        // not happen is the merge carrying on without it: the merge's asset
        // has A's edits already baked in, so leaving it deployed would go on
        // running the mod the user just deleted, while B stayed suppressed on
        // its behalf. Taking the merge out puts the library back in a state
        // that can be described, and the conflict is offered again.
        let dir = Dir::new("mergeinput");
        let mut book = merged_library(&dir);
        super::super::loadout::reconcile(&mut book, &dir.0.join("MODS"), false);
        assert!(dir.0.join("MODS/MERGE/ASSET.EXML").exists(), "merge not in the game");

        let plan = plan(book.get("A").unwrap(), &dir.roots(), true);
        let erased = erase(&plan, &dir.roots(), &mut book).unwrap();

        assert!(book.get("A").is_none(), "A is still listed");
        assert!(book.get("MERGE").is_none(), "the merge outlived its input");
        assert_eq!(erased.dissolved, vec!["MERGE".to_string()], "and said so");
        assert!(book.get("B").is_some(), "B was deleted along with the merge");

        let changes = super::super::loadout::reconcile(&mut book, &dir.0.join("MODS"), false);
        assert!(changes.problems.is_empty(), "{:?}", changes.problems);
        assert!(dir.0.join("MODS/B/ASSET.EXML").exists(), "B did not come back");
        assert!(!dir.0.join("MODS/MERGE").exists(), "merge still in the game");
    }

    #[test]
    fn deleting_takes_the_staged_files_the_game_copy_and_forgets_the_mod() {
        let dir = Dir::new("whole");
        dir.file("staging/M/M/A.EXML", "the asset");
        dir.file("archives/M.zip", "the download");
        std::fs::create_dir_all(dir.0.join("MODS")).unwrap();
        deploy::deploy(&dir.0.join("staging/M"), &dir.0.join("MODS")).unwrap();
        assert!(dir.0.join("MODS/M/A.EXML").exists());

        let mut book = Loadout::default();
        book.put(entry(&dir, "staging/M", Variant::Original));

        let plan = plan(book.get("M").unwrap(), &dir.roots(), true);
        assert_eq!(plan.items.len(), 2, "staged files and the download");
        assert!(plan.bytes > 0);

        let done = erase(&plan, &dir.roots(), &mut book).unwrap();
        assert!(done.problems.is_empty(), "{:?}", done.problems);
        assert!(!dir.0.join("MODS/M").exists(), "still in the game");
        assert!(!dir.0.join("staging/M").exists(), "staged copy survived");
        assert!(!dir.0.join("archives/M.zip").exists(), "download survived");
        assert!(book.get("M").is_none(), "still in the mod list");
    }

    #[test]
    fn keeping_the_download_leaves_the_archive_and_nothing_else() {
        let dir = Dir::new("keepzip");
        dir.file("staging/M/M/A.EXML", "the asset");
        dir.file("archives/M.zip", "the download");
        let mut book = Loadout::default();
        book.put(entry(&dir, "staging/M", Variant::Original));

        let plan = plan(book.get("M").unwrap(), &dir.roots(), false);
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].what, What::Staged);

        erase(&plan, &dir.roots(), &mut book).unwrap();
        assert!(!dir.0.join("staging/M").exists());
        assert!(
            dir.0.join("archives/M.zip").exists(),
            "reinstalling has to stay free"
        );
    }

    #[test]
    fn a_build_left_behind_by_an_undo_is_still_deleted() {
        // Switching a mod back to the author's build leaves the cleaned one on
        // disk on purpose -- that is what makes both directions of the switch
        // free. But `source` then stops naming it, and a delete that only read
        // `source` walked past it: undo, then delete, and the build stayed in
        // `derived/` for good with nothing left that knew it was there.
        let dir = Dir::new("orphan_build");
        dir.file("staging/M/M/A.MBIN", "shipped");
        dir.file("derived/M/M/A.EXML", "cleaned");

        let mut book = Loadout::default();
        // Back on the author's build: `source` is staging, and the cleaned
        // build in `derived/M` is named by nothing at all.
        book.put(entry(&dir, "staging/M", Variant::Original));

        let plan = plan(book.get("M").unwrap(), &dir.roots(), false);
        let kinds: Vec<What> = plan.items.iter().map(|i| i.what).collect();
        assert!(kinds.contains(&What::Derived), "{kinds:?}");

        erase(&plan, &dir.roots(), &mut book).unwrap();
        assert!(!dir.0.join("derived/M").exists(), "the orphan survived");
    }

    #[test]
    fn a_mended_mod_loses_both_its_builds() {
        // Delete has to reach the original *and* the derived build, or the
        // space is not recovered and a stale build is left behind.
        let dir = Dir::new("bothbuilds");
        dir.file("staging/M/M/A.EXML", "as shipped");
        dir.file("derived/M/M/A.EXML", "mended");
        let mut book = Loadout::default();
        book.put(entry(&dir, "derived/M", Variant::Cleaned));

        let plan = plan(book.get("M").unwrap(), &dir.roots(), false);
        let kinds: Vec<What> = plan.items.iter().map(|i| i.what).collect();
        assert!(kinds.contains(&What::Staged) && kinds.contains(&What::Derived), "{kinds:?}");

        erase(&plan, &dir.roots(), &mut book).unwrap();
        assert!(!dir.0.join("staging/M").exists());
        assert!(!dir.0.join("derived/M").exists());
    }

    #[test]
    fn a_source_pointing_outside_our_folders_is_refused_not_deleted() {
        // The loadout is a text file the user can edit and this program has
        // already written wrong once. A bad `source` must never delete
        // someone's documents.
        let dir = Dir::new("escape");
        let precious = dir.file("not_ours/Important.txt", "do not delete me");
        let mut book = Loadout::default();
        let mut bad = entry(&dir, "not_ours", Variant::Original);
        bad.origin = Some(dir.0.join("not_ours").display().to_string());
        bad.archive = None;
        book.put(bad);

        let plan = plan(book.get("M").unwrap(), &dir.roots(), true);
        assert!(plan.items.is_empty(), "offered to delete something outside");
        assert_eq!(plan.refused.len(), 1);
        assert_eq!(plan.bytes, 0);

        erase(&plan, &dir.roots(), &mut book).unwrap();
        assert!(precious.exists(), "deleted a file outside our folders");
    }

    #[test]
    fn a_source_that_climbs_out_with_dot_dot_is_refused() {
        let dir = Dir::new("climb");
        let precious = dir.file("not_ours/Important.txt", "do not delete me");
        let mut book = Loadout::default();
        let mut bad = entry(&dir, "staging/../not_ours", Variant::Original);
        bad.origin = Some(dir.0.join("staging/../not_ours").display().to_string());
        bad.archive = None;
        book.put(bad);

        let plan = plan(book.get("M").unwrap(), &dir.roots(), true);
        assert!(plan.items.is_empty(), "a path walked out of staging");
        assert!(precious.exists());
    }

    #[test]
    fn the_staging_root_itself_is_never_deletable() {
        // An entry whose source is the whole staging folder would take every
        // other mod with it.
        let dir = Dir::new("root");
        dir.file("staging/Other Mod/A.EXML", "someone else's");
        let mut book = Loadout::default();
        let mut bad = entry(&dir, "staging", Variant::Original);
        bad.origin = Some(dir.0.join("staging").display().to_string());
        bad.archive = None;
        book.put(bad);

        let plan = plan(book.get("M").unwrap(), &dir.roots(), true);
        assert!(plan.items.is_empty(), "offered to delete the whole library");
        erase(&plan, &dir.roots(), &mut book).unwrap();
        assert!(dir.0.join("staging/Other Mod/A.EXML").exists());
    }

    #[test]
    fn the_size_reported_is_counted_once_not_once_per_hardlink() {
        // Deployed files are hardlinks to the staged ones. Counting both would
        // promise twice the space the disk will actually give back.
        let dir = Dir::new("bytes");
        dir.file("staging/M/M/A.EXML", "0123456789");
        std::fs::create_dir_all(dir.0.join("MODS")).unwrap();
        deploy::deploy(&dir.0.join("staging/M"), &dir.0.join("MODS")).unwrap();

        let mut book = Loadout::default();
        let mut e = entry(&dir, "staging/M", Variant::Original);
        e.archive = None;
        book.put(e);

        let plan = plan(book.get("M").unwrap(), &dir.roots(), false);
        assert_eq!(plan.bytes, 10, "the game's link was counted as well");
    }

    #[test]
    fn deleting_a_mod_that_is_already_gone_is_not_an_error() {
        let dir = Dir::new("absent");
        std::fs::create_dir_all(dir.0.join("MODS")).unwrap();
        let mut book = Loadout::default();
        let mut e = entry(&dir, "staging/M", Variant::Original);
        e.archive = None;
        book.put(e);

        let plan = plan(book.get("M").unwrap(), &dir.roots(), true);
        assert!(plan.items.is_empty());
        let done = erase(&plan, &dir.roots(), &mut book).unwrap();
        assert!(done.problems.is_empty());
        assert!(book.get("M").is_none(), "the record has to go regardless");
    }
}
