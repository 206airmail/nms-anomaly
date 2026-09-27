//! Reduce a whole-file replacement to the edits it actually makes.
//!
//! A mod that ships the entire `REWARDTABLE.MBIN` to change two percentages is
//! carrying 153,414 properties it never meant to touch. Two things follow, and
//! both are bad:
//!
//! * it fights every other mod that edits the same asset, and wins outright,
//!   silently reverting their work even though it disagrees with none of it;
//! * it was built against some older game build, so every property Hello Games
//!   changed since is reverted too — invisibly, because nothing errors.
//!
//! Rewriting it as a sparse patch holding only the differences fixes both. The
//! mod keeps doing exactly what it did, and stops doing the things nobody asked
//! for.
//!
//! **The baseline is the game that is installed right now.** [`super::vanilla`]
//! extracts it from `PCBANKS` and caches it per game build, so a property the
//! mod "changes" only because the game moved on is dropped rather than
//! preserved. Re-run after a game update and the output shrinks or grows to
//! match the new baseline.
//!
//! The output shape follows a patch that was hand-built and confirmed in game:
//! the ancestor chain down to each changed leaf, with rows carrying `_id` so
//! the game matches the right one.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use super::decompile::Decompiler;
use super::exmltree::{self, Tree};
use super::model::{FileKind, Mod};
use super::vanilla::VanillaSource;

/// What pruning one asset produced.
#[derive(Debug, Clone)]
pub struct Pruned {
    pub target: String,
    /// the sparse patch, as EXML text
    pub xml: String,
    /// properties the mod genuinely changes, which the patch keeps
    pub edits: usize,
    /// properties in the original that merely matched vanilla
    pub dropped: usize,
    /// the original's property count, for a before/after
    pub original: usize,
}

impl Pruned {
    /// True when nothing was worth keeping: the mod changes nothing at all.
    pub fn empty(&self) -> bool {
        self.edits == 0
    }

    /// How much smaller the patch is, as a percentage of the original.
    pub fn reduction(&self) -> f64 {
        if self.original == 0 {
            return 0.0;
        }
        100.0 * (self.dropped as f64) / (self.original as f64)
    }
}

/// Which leaf properties in `tree` hold a value the game's own copy does not.
///
/// This is the answer to "what does this mod actually change", and it is asked
/// by two features, not one: cleaning keeps these paths and drops everything
/// else, and [`super::edit`] offers exactly these paths as the ones a user may
/// change. They must be the same set or the editor would offer a property that
/// cleaning then throws away, and the edit would vanish the next time the mod
/// was cleaned.
///
/// `props` must be `tree.flatten()`; it is passed in because every caller has
/// already paid for it and flattening twice is the expensive half of pruning.
///
/// Containers are excluded. A container differs whenever anything under it
/// does, so counting one as a change in its own right would drag in its whole
/// subtree — and, for the editor, would offer a "value" that no input can hold.
pub fn changed_leaves(
    vanilla: &IndexMap<String, Option<String>>,
    tree: &Tree,
    props: &IndexMap<String, Option<String>>,
) -> Vec<String> {
    let index = tree.index();
    let has_children: HashSet<&String> = index
        .keys()
        .filter_map(|p| p.rfind('/').map(|at| &p[..at]))
        .filter_map(|parent| index.get_key_value(parent).map(|(k, _)| k))
        .collect();

    let mut out = Vec::new();
    for (path, value) in props {
        if has_children.contains(path) {
            continue;
        }
        let changed = match vanilla.get(path) {
            Some(base) => !super::exml::values_equal(base.as_deref(), value.as_deref()),
            // Absent from the game's copy: the mod introduces it, which is a
            // change too.
            None => true,
        };
        if changed {
            out.push(path.clone());
        }
    }
    out
}

/// Every ancestor path of `path`, including `path` itself.
fn with_ancestors(path: &str, out: &mut HashSet<String>) {
    out.insert(path.to_string());
    let mut rest = path;
    while let Some(at) = rest.rfind('/') {
        rest = &rest[..at];
        out.insert(rest.to_string());
    }
}

/// Rewrite a copy of a game asset as a patch holding only its differences.
///
/// `vanilla_xml` must be the copy from the *installed* game, not the build the
/// mod was made for; that is the entire point of the exercise.
///
/// `whole_file` says whether the input replaces the asset outright, and it
/// decides what *silence* means -- the same distinction [`super::merge`] draws,
/// for the same reason. A compiled `.MBIN` is the whole table, so a property it
/// omits is one the mod deleted. A `.EXML` is already a patch, so a property it
/// omits is simply one it never mentioned. Get this the wrong way round on a
/// sparse input and every unmentioned property in the game reads as a deletion:
/// the mod is refused as one that "works by removing things", and the one shape
/// this function most wants to fix is the shape it declines to look at.
///
/// # Pruning something that is already a patch
///
/// Worth doing, and not a contradiction. An `.EXML` is sparse in *form* without
/// being sparse in *content*: `BetterRewardsCombined` names 6,183 properties in
/// `REWARDTABLE` and only 2,790 of them are changes, so 3,393 vanilla values
/// travel along inside it. Those are written to the asset at load, so any mod
/// that edits one of them and loses load order is reverted by a mod that never
/// meant to touch it -- the same accident an `.MBIN` causes, wearing a different
/// extension. The tool used to be unable to see it, because it only ever looked
/// at `.MBIN`.
pub fn prune(
    target: &str,
    vanilla_xml: &str,
    mod_xml: &str,
    whole_file: bool,
) -> Result<Pruned, String> {
    let vanilla = exmltree::parse_str(vanilla_xml)?.flatten();
    let full: Tree = exmltree::parse_str(mod_xml)?;
    let props = full.flatten();

    // Some mods work by *removing* nodes: "No Laser Flare" ships a muzzle
    // scene with the flare geometry deleted, and every property it keeps
    // matches vanilla exactly. Its edit count is zero and its effect is total.
    // A sparse patch adds and overrides; it cannot say "delete this", so
    // pruning such a mod would produce an empty file that does nothing.
    //
    // Only a whole-file copy can delete by omission. Asking this of a `.EXML`
    // would refuse every sparse patch in the library, since a patch omits
    // almost everything by definition.
    if whole_file {
        let removed = vanilla
            .keys()
            .filter(|path| !props.contains_key(*path))
            .count();
        if removed > 0 {
            return Err(format!(
                "this mod works by removing {removed} propert(y/ies) the game has, which a \
                 sparse patch cannot express. Leave it as a whole-file replacement."
            ));
        }
    }

    // Leaves only, and the same set the editor offers. See `changed_leaves`.
    let edits = changed_leaves(&vanilla, &full, &props);

    let mut keep: HashSet<String> = HashSet::new();
    for path in &edits {
        with_ancestors(path, &mut keep);
    }

    let pruned = full.retain(&keep);
    let kept_props = pruned.flatten();

    // Every edit must survive, at its own path, with the mod's value. If the
    // retained tree cannot reproduce one, the patch would quietly do less than
    // the mod did, which is the failure this whole feature exists to prevent.
    for path in &edits {
        match kept_props.get(path) {
            Some(value) if value == &props[path] => {}
            _ => {
                return Err(format!(
                    "pruning lost {path}; refusing to write a patch that does \
                     less than the mod it replaces"
                ))
            }
        }
    }

    Ok(Pruned {
        target: target.to_string(),
        xml: exmltree::to_string(&pruned),
        edits: edits.len(),
        dropped: props.len().saturating_sub(kept_props.len()),
        original: props.len(),
    })
}

/// What cleaning one whole-file override would do, or why it cannot.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Plan {
    /// the mod folder the game loads this from, which is how the UI addresses
    /// it and how the loadout entry holding it is found
    pub owner: String,
    pub target: String,
    /// where the file sits inside that mod folder, which is the same place it
    /// sits inside the staged copy the cleaned build is made from
    pub rel: String,
    /// every property in the file, containers as well as leaves
    pub original: usize,
    /// properties the mod genuinely changes. **Leaves only**, so this is not
    /// `original` minus the dead weight -- a nested table has containers in
    /// `original` that can never be an edit. See [`dropped`](Plan::dropped).
    pub edits: usize,
    /// properties cleaning would actually remove: `original` minus what the
    /// pruned tree keeps.
    ///
    /// This, and never `original > edits`, is the test for "does this file
    /// carry values it does not change". The two counts are of different
    /// things: `LaunchThrustersReworked-Comfort` is already a perfectly sparse
    /// patch -- every leaf in it is an edit -- and still reads 70 against 26,
    /// because 44 of those 70 are the `<Property name="Table">` containers the
    /// edits hang off. Cleaning it rewrote the same bytes, reported success,
    /// and left it on the list to be cleaned again.
    pub dropped: usize,
    /// true when this copy replaces the asset outright -- a compiled `.MBIN`.
    ///
    /// Two different questions turn on this and the UI needs them apart. Only a
    /// whole-file copy can *host* a merge, so this is what says whether a merge
    /// is possible at all. And only a whole-file copy reverts the game's own
    /// changes wholesale, so it is also what separates "this mod overrides
    /// everything" from "this patch carries a few values it should not".
    pub whole_file: bool,
    /// why this one cannot be cleaned; `None` when it can
    pub refused: Option<String>,
    /// true when a cleaned build of this file is what the game is loading
    pub cleaned: bool,
    /// the patch itself, withheld from the UI: it can be 40kB of XML
    #[serde(skip)]
    pub xml: Option<String>,
}

impl Plan {
    pub fn can_clean(&self) -> bool {
        self.refused.is_none() && !self.cleaned && self.edits > 0
    }

    /// Does writing this patch change anything at all?
    ///
    /// A whole-file copy always does: even with nothing to drop, the clean
    /// turns an `.MBIN` that wins outright into an `.EXML` the game merges.
    /// A patch with nothing to drop is already what cleaning would make it.
    pub fn worth_cleaning(&self) -> bool {
        self.whole_file || self.dropped > 0
    }
}

/// Swap `.MBIN` for `.EXML` on a path, leaving everything else alone.
fn patch_path(mbin: &Path) -> PathBuf {
    mbin.with_extension("EXML")
}

// ---------------------------------------------------------------------------
// Remembering what pruning produced
//
// [`plan`] runs on every start-up, and on a library nobody has touched it does
// the identical arithmetic over the identical bytes: against 62 mods that is
// ~180 MB of decompiled XML read and parsed to reach conclusions it reached
// last time. Nothing in [`prune`] depends on anything but its two inputs, and
// both of those are already content-addressed by [`Decompiler`] -- so the pair
// of hashes is the key, and the answer is kept under it.
//
// The patch text is kept too, not just the counts. `clean_apply` calls `plan`
// again to get the XML it writes, so a cache that dropped it would move the
// cost rather than remove it.
//
// Nothing expires and nothing needs to: edit a mod and its hash changes, patch
// the game and the vanilla extract's hash changes, and either way the key is
// one that has not been seen before. The old entry is simply never asked for.
// ---------------------------------------------------------------------------

/// Bumped whenever [`prune`] would answer differently for the same input.
///
/// The hashes cover the two files; they cannot cover the rules applied to
/// them. Without this, changing what counts as an edit -- or what gets refused
/// -- would leave every existing entry serving the old answer forever, and the
/// bug would show up as a patch nobody can explain.
/// 2: `.EXML` copies are planned too, and `whole_file` decides what an omitted
/// property means. Every existing entry was computed under the old rules.
/// 3: only assets the game reads a loose `.EXML` for are planned at all — see
/// [`loose_exml_is_read`], which four silently-disabled mods paid for.
const RULES_VERSION: u32 = 3;

fn cache_dir() -> PathBuf {
    super::tools::cache_root().join("prune-cache")
}

fn cache_key(target: &str, mod_sha1: &str, vanilla_sha1: &str, whole_file: bool) -> String {
    let mut hasher = sha1_smol::Sha1::new();
    // `whole_file` is part of the question, not of the files: the same bytes
    // answer differently depending on whether omission means deletion.
    hasher.update(
        format!("{RULES_VERSION}|{target}|{mod_sha1}|{vanilla_sha1}|{whole_file}").as_bytes(),
    );
    hasher.digest().to_string()
}

/// One remembered [`prune`] outcome, success or refusal.
#[derive(serde::Serialize, serde::Deserialize)]
struct Cached {
    /// why no patch could be produced; `None` when one was
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refused: Option<String>,
    /// the patch itself, so cleaning does not have to work it out again
    #[serde(default, skip_serializing_if = "Option::is_none")]
    xml: Option<String>,
    #[serde(default)]
    edits: usize,
    #[serde(default)]
    dropped: usize,
    #[serde(default)]
    original: usize,
}

impl Cached {
    fn of(result: &Result<Pruned, String>) -> Self {
        match result {
            Ok(pruned) => Self {
                refused: None,
                xml: Some(pruned.xml.clone()),
                edits: pruned.edits,
                dropped: pruned.dropped,
                original: pruned.original,
            },
            Err(err) => Self {
                refused: Some(err.clone()),
                xml: None,
                edits: 0,
                dropped: 0,
                original: 0,
            },
        }
    }

    fn into_result(self, target: &str) -> Result<Pruned, String> {
        match self.refused {
            Some(err) => Err(err),
            None => Ok(Pruned {
                target: target.to_string(),
                xml: self.xml.unwrap_or_default(),
                edits: self.edits,
                dropped: self.dropped,
                original: self.original,
            }),
        }
    }
}

/// What was remembered for this key, if anything usable was.
///
/// An entry that is neither a refusal nor a patch is treated as absent rather
/// than as a success with nothing in it: that shape can only come from a
/// truncated or hand-edited file, and believing it would have `clean_apply`
/// write an empty patch over a working mod.
fn recall(dir: &Path, key: &str) -> Option<Cached> {
    let text = std::fs::read_to_string(dir.join(format!("{key}.json"))).ok()?;
    let entry: Cached = serde_json::from_str(&text).ok()?;
    if entry.refused.is_none() && entry.xml.is_none() {
        return None;
    }
    Some(entry)
}

/// Keep an outcome, or quietly do not: a cache that cannot be written is a
/// slow run, and a run that fails because its cache would not write is worse.
fn remember(dir: &Path, key: &str, entry: &Cached) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let Ok(text) = serde_json::to_string(entry) else {
        return;
    };
    // Write under another name and rename over: an interruption then leaves
    // the previous answer or none, never half of one for the next run to read.
    let scratch = dir.join(format!("{key}.writing"));
    if std::fs::write(&scratch, text).is_ok() {
        let _ = std::fs::rename(&scratch, dir.join(format!("{key}.json")));
    } else {
        let _ = std::fs::remove_file(&scratch);
    }
}

/// Every file under `dir`, as paths relative to it.
fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if entry.path().is_dir() {
            walk(&entry.path(), &rel, out);
        } else {
            out.push(rel);
        }
    }
}

/// Overrides that have already been cleaned, read off the two builds.
///
/// They cannot be found from the mods folder: cleaning swaps the `.MBIN` for a
/// patch, so the next scan sees only the patch and the mod simply disappears
/// from the list -- taking the undo with it. The record is the pair of builds
/// themselves. Every `.MBIN` the author shipped that the cleaned build replaces
/// with an `.EXML` at the same place is one file this tool cleaned, which is
/// exactly what happened and needs nothing written down to stay true.
pub fn already_cleaned(origin: &Path, built: &Path, owner: &str) -> Vec<Plan> {
    // Only inside the folder the game loads under this name. A staged mod can
    // hold several of those, and each is a mod in its own right as far as the
    // scan and the UI are concerned.
    let inner = origin.join(owner);
    let mut rels = Vec::new();
    walk(&inner, "", &mut rels);
    rels.into_iter()
        .filter_map(|rel| {
            let upper = rel.to_uppercase();
            // `rel` is relative to the mod folder, the same way a fresh plan's
            // is; the build mirrors `origin`, so the folder goes back on here.
            let at = PathBuf::from(owner).join(rel.split('/').collect::<PathBuf>());
            let was_cleaned = if upper.ends_with(".MBIN") {
                // A compiled override the build replaced with a patch.
                !built.join(&at).exists() && built.join(patch_path(&at)).exists()
            } else if upper.ends_with(".EXML") {
                // A patch pruned in place: same path in both builds, different
                // contents. Compared by length rather than by hash because a
                // prune always changes the size and this runs over every staged
                // file of every cleaned mod.
                let shipped = std::fs::metadata(origin.join(&at)).map(|m| m.len());
                let now = std::fs::metadata(built.join(&at)).map(|m| m.len());
                matches!((shipped, now), (Ok(a), Ok(b)) if a != b)
            } else {
                false
            };
            was_cleaned.then(|| Plan {
                owner: owner.to_string(),
                target: rel.clone(),
                rel,
                original: 0,
                edits: 0,
                dropped: 0,
                whole_file: upper.ends_with(".MBIN"),
                refused: None,
                cleaned: true,
                xml: None,
            })
        })
        .collect()
}

/// Write a cleaned build of a staged mod, leaving the staged copy untouched.
///
/// # Why this does not simply edit the mod
///
/// It used to, and that put cleaning outside everything else this program
/// knows how to do. The patch was written straight into `GAMEDATA\MODS` and the
/// original was kept in app data -- so the loadout, which is the record of what
/// the game is reading, knew nothing about it. [`super::loadout::reconcile`]
/// compares what is deployed against the build it came from, found a folder
/// that no longer matched, and helpfully put the author's `.MBIN` back. Every
/// activate, every preset switch and every merge undid the clean, silently,
/// while the saved original stayed behind and made a second attempt refuse.
///
/// So a clean is now a *build*, exactly like a mend: the whole mod is copied
/// into `derived/`, the overrides are replaced there, and the loadout is
/// pointed at it. Undoing is pointing back at the staged copy, which was never
/// touched, so there is nothing that can fail to come back.
///
/// `plans` are the ones to apply; each is written at `<owner>/<rel>` inside the
/// build, which is where the staged mod keeps it and where `deploy` mirrors it
/// into the game from.
pub fn clean_into(origin: &Path, dest: &Path, plans: &[&Plan]) -> Result<Vec<String>, String> {
    // A build cannot be cleaned into itself. The next two lines clear `dest`
    // and then copy `origin` into it, so one path for both would delete the mod
    // and then fail trying to read what it had just deleted.
    //
    // Only one thing in this program is its own origin, and it is a **merge**:
    // it has no author's copy behind it, so `merge::build` records its origin as
    // the derived folder it lives in -- which is exactly the folder a clean of
    // it would build into, because both are `derived/<owner>`. Held here rather
    // than only at the caller, because the cost of getting it wrong is the
    // user's merge.
    if origin == dest {
        return Err(
            "this is a build this program made, not a copy an author shipped, so there is \
             nothing behind it to clean it from"
                .into(),
        );
    }

    let usable: Vec<&&Plan> = plans.iter().filter(|p| p.can_clean()).collect();
    if usable.is_empty() {
        return Err("nothing here could be cleaned".into());
    }

    if dest.exists() {
        std::fs::remove_dir_all(dest)
            .map_err(|e| format!("could not clear {}: {e}", dest.display()))?;
    }
    copy_tree(origin, dest)?;

    let mut done = Vec::new();
    for plan in usable {
        let Some(xml) = &plan.xml else { continue };
        let mut at = PathBuf::from(&plan.owner);
        for part in plan.rel.split(['/', '\\']).filter(|p| !p.is_empty()) {
            at.push(part);
        }
        let shipped = dest.join(&at);
        let patch = dest.join(patch_path(&at));
        if !shipped.is_file() {
            return Err(format!(
                "{} is not in the copy this mod was staged from, so the cleaned build would \
                 not be the mod you have installed",
                at.display()
            ));
        }
        std::fs::write(&patch, xml)
            .map_err(|e| format!("could not write {}: {e}", patch.display()))?;
        // A compiled override has to go, or the game loads it in preference to
        // the patch that replaced it. Safe here in a way it never was in the
        // mods folder: this is our own copy, not a second name for the staged
        // file.
        //
        // A `.EXML` input is already at the patch's own path, so the write above
        // *was* the replacement and there is nothing left to take away. Removing
        // it here would delete the patch that had just been written.
        if shipped != patch {
            std::fs::remove_file(&shipped)
                .map_err(|e| format!("could not remove {}: {e}", shipped.display()))?;
        }
        done.push(format!(
            "{}: {} properties -> {}",
            plan.target.rsplit('/').next().unwrap_or(&plan.target),
            plan.original,
            plan.original.saturating_sub(plan.dropped)
        ));
    }

    if done.is_empty() {
        return Err("nothing here could be cleaned".into());
    }
    Ok(done)
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("could not make {}: {e}", to.display()))?;
    let entries = std::fs::read_dir(from)
        .map_err(|e| format!("could not read {}: {e}", from.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let into = to.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, &into)?;
        } else {
            std::fs::copy(&path, &into)
                .map_err(|e| format!("could not copy {}: {e}", path.display()))?;
        }
    }
    Ok(())
}

/// True when the game reads a loose `.EXML` at this path in place of the
/// compiled asset it sits beside.
///
/// # This is the whole premise of cleaning, and it is not universal
///
/// Cleaning turns an `.MBIN` into an `.EXML` patch. That is only an improvement
/// where the game actually *reads* the `.EXML`. Measured on a 63-mod library
/// with a session logger attached (game 7.03, exe 179666):
///
/// * used — `GLOBALS\GCGAMEPLAYGLOBALS.GLOBAL.EXML` and five others like it,
///   several contested by up to six mods at once, all applied; and
///   `METADATA\SIMULATION\SOLARSYSTEM\VOXELGENERATORSETTINGS.EXML`, which the
///   logger recorded the game using *in preference to* an `.MXML` beside it.
/// * ignored — every `.EXML` under `MODELS\`. Four mods were cleaned into
///   nothing but those, and the game opened **not one file** from any of them:
///   Convenient Corvette Teleporters (4 `.SCENE.EXML`), Freighter Salvage
///   Terminals (3), No Speed Halo (1 `.MATERIAL.EXML`), Open These Freighter
///   Doors (1 `.ANIM.EXML`). The clincher is `Savegame By Hotkey`, whose
///   `MODELS\COMMON\PLAYER\PLAYERCHARACTER\PLAYERCHARACTER.SCENE.EXML` went
///   unopened in a session that unquestionably loaded the player character.
///
/// Freighter Salvage Terminals is the one that was noticed from in game: its
/// terminals were in the hangar the day before it was cleaned and gone after.
///
/// So this is an **allowlist**, not a denylist. The cost of being wrong in one
/// direction is a mod that could have been cleaned and was not; in the other it
/// is a mod that silently stops working, which is the failure this whole
/// program exists to catch.
fn loose_exml_is_read(target: &str) -> bool {
    let upper = target.to_uppercase().replace('/', "\\");
    upper.starts_with("GLOBALS\\") || upper.starts_with("METADATA\\")
}

/// How many files this mod ships for one game asset.
///
/// More than one and it cannot be cleaned: see the refusal in [`plan`].
fn copies_of(owner: &Mod, target: &str) -> usize {
    owner
        .files
        .iter()
        .filter(|f| f.target.as_deref() == Some(target))
        .count()
}

/// Work out what cleaning would do to every whole-file override in `mods`.
///
/// Reports refusals as entries rather than dropping them: "this mod cannot be
/// cleaned, and here is why" is the more useful answer, and it stops the same
/// mod being offered again next scan.
pub fn plan(
    mods: &[Mod],
    decompiler: &mut Decompiler,
    source: &mut VanillaSource,
) -> Vec<Plan> {
    // Which assets more than one mod ships. Used to decide whether a `.EXML` is
    // worth examining; see below.
    let mut providers: std::collections::HashMap<&str, HashSet<&str>> =
        std::collections::HashMap::new();
    for owner in mods {
        for file in &owner.files {
            if let Some(target) = file.target.as_deref() {
                providers.entry(target).or_default().insert(owner.name.as_str());
            }
        }
    }

    let mut subjects: Vec<(&Mod, &super::model::ModFile, String)> = Vec::new();
    // Refusals decided before any file is opened. Kept as entries rather than
    // dropped, for the reason in the doc above, but they cost nothing: the
    // answer does not depend on reading either copy.
    let mut upfront: Vec<Plan> = Vec::new();
    for owner in mods {
        for file in &owner.files {
            let Some(target) = file.target.as_deref() else {
                continue;
            };
            // The one thing a clean cannot survive. See [`loose_exml_is_read`]:
            // turning an `.MBIN` the game reads into an `.EXML` it ignores does
            // not make the mod sparse, it switches the mod off.
            if file.kind == Some(FileKind::Mbin) && !loose_exml_is_read(target) {
                upfront.push(Plan {
                    owner: owner.name.clone(),
                    target: target.to_string(),
                    rel: file.rel_path.clone(),
                    original: 0,
                    edits: 0,
                    dropped: 0,
                    whole_file: true,
                    refused: Some(
                        "the game does not read a loose .EXML at this path, only the \
                         compiled file — cleaning it would replace a mod that works \
                         with one the game ignores"
                            .into(),
                    ),
                    cleaned: false,
                    xml: None,
                });
                continue;
            }
            let worth_it = match file.kind {
                // Always: a whole-file copy reverts every change the game has
                // made since the mod was built, whether or not anyone contests
                // it.
                Some(FileKind::Mbin) => true,
                // Only when contested. A `.EXML` carrying vanilla values only
                // reverts somebody when there *is* somebody -- it writes what it
                // names and nothing else, so on an asset no one else touches
                // there is nothing to take back. That matters for cost as much
                // as for honesty: this runs after every scan, each target is a
                // separate extraction from PCBANKS, and planning every patch in
                // the library would put ~100 more of them on the start-up path
                // to report findings nobody can act on.
                Some(FileKind::Exml) => {
                    providers.get(target).is_some_and(|who| who.len() > 1)
                }
                _ => false,
            };
            if worth_it {
                subjects.push((owner, file, target.to_string()));
            }
        }
    }

    // One extraction for the whole library; each one shells out to hgpaktool.
    let targets: Vec<String> = subjects.iter().map(|(_, _, t)| t.clone()).collect();
    let extraction = source.fetch(&targets);
    let cache = cache_dir();

    let mut out = upfront;
    for (owner, file, target) in subjects {
        let shipped = PathBuf::from(&file.abs_path);
        let whole_file = file.kind == Some(FileKind::Mbin);
        let mut entry = Plan {
            owner: owner.name.clone(),
            target: target.clone(),
            rel: file.rel_path.clone(),
            original: 0,
            edits: 0,
            dropped: 0,
            whole_file,
            refused: None,
            // A mod whose override is still here has not been cleaned; the ones
            // that have are added by the caller from the two builds.
            cleaned: false,
            xml: None,
        };

        // A mod that ships the same asset twice -- as both `.MBIN` and `.EXML`
        // -- cannot be cleaned by replacing one of them: the other still carries
        // the whole table, so the patch would be written and nothing about what
        // the game loads would change.
        //
        // This restores a check the in-place version made at the point of
        // writing ("this mod ships both forms of that asset and cleaning it
        // would overwrite one"). Building into `derived/` removed the overwrite
        // hazard, which is what made it easy to drop -- but the *no-op* hazard
        // is still there, and a clean that confidently does nothing is worse
        // than one that says why it cannot.
        //
        // Only `paths::ASSET_EXTS` counts here, so a loose `.MXML` is not a
        // second copy: that extension is the AMUMSS localisation table
        // (`FileKind::Mxml`), and every use of it in the measured library is
        // literally `LocTable.MXML`. One mod ships a decompiled asset table
        // under it by accident; the engine does not read it as an asset and
        // neither, on this model, does the game.
        //
        // No mod in the measured 62 trips this. It is here because the shape is
        // cheap to detect and expensive to debug.
        let copies = copies_of(owner, &target);
        if copies > 1 {
            entry.refused = Some(format!(
                "{} ships {copies} copies of this asset, so replacing one of them would \
                 leave the others carrying the whole file. Remove the spare copies from the \
                 mod, or combine it with the mods it contests instead.",
                owner.name
            ));
            out.push(entry);
            continue;
        }

        // Hash the vanilla extract before converting it rather than letting
        // `decompile_file` do it out of sight: the hash is half the cache key
        // below, and it costs nothing here because the conversion needs it
        // anyway.
        let vanilla = extraction
            .found
            .get(&target)
            .and_then(|p| Decompiler::content_hash(p).map(|sha1| (p.clone(), sha1)))
            .and_then(|(p, sha1)| decompiler.decompile(&p, &sha1).map(|mxml| (mxml, sha1)));
        let Some((vanilla_mxml, vanilla_sha1)) = vanilla else {
            entry.refused =
                Some("the game ships no copy of this asset to compare against".into());
            out.push(entry);
            continue;
        };
        // A compiled copy has to go through MBINCompiler to be read at all; a
        // `.EXML` already is the XML, so it is read where it lies.
        let mod_xml_path = if whole_file {
            match decompiler.decompile(&shipped, &file.sha1) {
                Some(path) => path,
                None => {
                    entry.refused = Some("this file could not be decompiled".into());
                    out.push(entry);
                    continue;
                }
            }
        } else {
            shipped.clone()
        };

        let key = cache_key(&target, &file.sha1, &vanilla_sha1, whole_file);
        let result = match recall(&cache, &key) {
            // The whole point of the cache: on a hit neither of the two files
            // is opened, and they are the large ones.
            Some(hit) => hit.into_result(&target),
            None => {
                let Some(vanilla_xml) = std::fs::read_to_string(&vanilla_mxml).ok() else {
                    entry.refused =
                        Some("the game ships no copy of this asset to compare against".into());
                    out.push(entry);
                    continue;
                };
                let Some(mod_xml) = std::fs::read_to_string(&mod_xml_path).ok() else {
                    entry.refused = Some("this file could not be read".into());
                    out.push(entry);
                    continue;
                };
                let result = prune(&target, &vanilla_xml, &mod_xml, whole_file);
                remember(&cache, &key, &Cached::of(&result));
                result
            }
        };

        match result {
            Ok(pruned) => {
                entry.original = pruned.original;
                entry.edits = pruned.edits;
                entry.dropped = pruned.dropped;
                if pruned.empty() {
                    entry.refused = Some(
                        "every value in this file already matches the game; it changes \
                         nothing and can simply be removed"
                            .into(),
                    );
                } else if !entry.worth_cleaning() {
                    // Already sparse in content as well as in form: the pruned
                    // tree *is* the file. Writing it would report a clean, copy
                    // the same bytes into `derived/`, and leave the mod on the
                    // list -- which is exactly what it did until this existed.
                    entry.refused = Some(
                        "this patch already changes every value it names, so there is \
                         nothing in it to take away"
                            .into(),
                    );
                } else {
                    entry.xml = Some(pruned.xml);
                }
            }
            Err(err) => entry.refused = Some(err),
        }
        out.push(entry);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(rows: &[(&str, &str)]) -> String {
        let body: String = rows
            .iter()
            .enumerate()
            .map(|(i, (id, cost))| {
                format!(
                    "<Property name=\"Row\" value=\"E\" _index=\"{i}\">\
                     <Property name=\"Id\" value=\"{id}\" />\
                     <Property name=\"Cost\" value=\"{cost}\" />\
                     </Property>"
                )
            })
            .collect();
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
             <!--File created using MBINCompiler version (7.03.2.2)-->\
             <Data template=\"cGcTable\">{body}</Data>"
        )
    }

    #[test]
    fn only_the_changed_property_survives() {
        let vanilla = table(&[("ALPHA", "1"), ("BETA", "2"), ("GAMMA", "3")]);
        let modded = table(&[("ALPHA", "1"), ("BETA", "99"), ("GAMMA", "3")]);
        let out = prune("T.MBIN", &vanilla, &modded, true).unwrap();

        assert_eq!(out.edits, 1);
        assert!(out.xml.contains("99"));
        assert!(!out.xml.contains("ALPHA"), "untouched rows are dropped");
        assert!(!out.xml.contains("GAMMA"));
        // The surviving row still says which row it is.
        assert!(out.xml.contains("_id=\"BETA\""));
    }

    #[test]
    fn the_patch_reparses_to_the_same_edit_at_the_same_path() {
        // The whole point: the patch must address the row the full file did.
        let vanilla = table(&[("ALPHA", "1"), ("BETA", "2")]);
        let modded = table(&[("ALPHA", "1"), ("BETA", "99")]);
        let out = prune("T.MBIN", &vanilla, &modded, true).unwrap();

        let patch = exmltree::parse_str(&out.xml).unwrap().flatten();
        let full = exmltree::parse_str(&modded).unwrap().flatten();
        assert_eq!(patch["Row[BETA]/Cost"], full["Row[BETA]/Cost"]);
    }

    /// A patch whose every leaf is a change, nested the way a real table is.
    fn nested(bonus: &str, charge: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
             <Data template=\"cGcTechnologyTable\">\
             <Property name=\"Table\">\
             <Property name=\"Table\" value=\"GcTechnology\" _id=\"LAUNCHER\">\
             <Property name=\"StatBonuses\">\
             <Property name=\"StatBonuses\" value=\"GcStatsBonus\" _index=\"1\">\
             <Property name=\"Bonus\" value=\"{bonus}\" />\
             </Property></Property>\
             <Property name=\"ChargeMultiplier\" value=\"{charge}\" />\
             </Property></Property></Data>"
        )
    }

    /// The bug `dropped` exists to answer.
    ///
    /// `LaunchThrustersReworked-Comfort` ships four patches in which every leaf
    /// is a change. There is nothing in them to take away -- and `original`
    /// still reads far above `edits`, because only leaves can ever be edits and
    /// `original` counts the containers they hang off as well. Anything reading
    /// "is there dead weight here" off that difference offers a clean that
    /// rewrites the same bytes, reports success, and comes back next preview.
    #[test]
    fn an_already_sparse_patch_has_nothing_to_drop() {
        let out = prune(
            "NMS_REALITY_GCTECHNOLOGYTABLE.EXML",
            &nested("4.000000", "1.000000"),
            &nested("12.000000", "0.750000"),
            false,
        )
        .unwrap();

        assert_eq!(out.edits, 2, "both leaves are changes");
        assert_eq!(out.dropped, 0, "so cleaning would remove nothing");
        assert!(
            out.original > out.edits,
            "and the two counts still differ, which is the trap: {} vs {}",
            out.original,
            out.edits
        );
    }

    #[test]
    fn nothing_to_drop_is_not_worth_cleaning_unless_the_file_is_whole() {
        let patch = Plan {
            owner: "DemoMod".into(),
            target: "T.EXML".into(),
            rel: "T.EXML".into(),
            original: 70,
            edits: 26,
            dropped: 0,
            whole_file: false,
            refused: None,
            cleaned: false,
            xml: None,
        };
        assert!(!patch.worth_cleaning());
        // The same numbers on a compiled copy still are: cleaning turns an
        // `.MBIN` that wins outright into an `.EXML` the game merges, which is
        // worth doing even when it drops nothing.
        assert!(Plan { whole_file: true, ..patch }.worth_cleaning());
    }

    /// A scratch cache folder that takes itself away again.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("nmscheck-prunecache-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_remembered_patch_comes_back_whole() {
        // The counts are cheap to recompute; the patch is not, and
        // `clean_apply` is the caller that needs it. Losing it here would
        // move the cost rather than remove it.
        let scratch = Scratch::new("whole");
        let vanilla = table(&[("ALPHA", "1"), ("BETA", "2")]);
        let modded = table(&[("ALPHA", "1"), ("BETA", "99")]);
        let fresh = prune("T.MBIN", &vanilla, &modded, true);

        remember(&scratch.0, "k", &Cached::of(&fresh));
        let hit = recall(&scratch.0, "k").expect("just written").into_result("T.MBIN");

        assert_eq!(hit.unwrap().xml, fresh.unwrap().xml);
    }

    #[test]
    fn a_refusal_is_remembered_as_a_refusal() {
        // Re-deriving a refusal costs exactly as much as deriving the patch
        // would have, so the ones that cannot be cleaned have to be kept too.
        let scratch = Scratch::new("refusal");
        let refused: Result<Pruned, String> = Err("nope".into());
        remember(&scratch.0, "k", &Cached::of(&refused));

        let hit = recall(&scratch.0, "k").unwrap().into_result("T.MBIN");
        assert_eq!(hit.unwrap_err(), "nope");
    }

    #[test]
    fn an_entry_that_is_neither_is_ignored_rather_than_believed() {
        // Only a truncated or hand-edited file looks like this. Reading it as
        // "succeeded, with no patch" would have `clean_apply` write an empty
        // EXML over a working mod, which is the one outcome worth guarding.
        let scratch = Scratch::new("empty");
        std::fs::write(scratch.0.join("k.json"), "{}").unwrap();
        assert!(recall(&scratch.0, "k").is_none());

        std::fs::write(scratch.0.join("k.json"), "not json at all").unwrap();
        assert!(recall(&scratch.0, "k").is_none());
    }

    #[test]
    fn a_missing_entry_is_a_miss_not_a_failure() {
        let scratch = Scratch::new("missing");
        assert!(recall(&scratch.0, "never-written").is_none());
    }

    #[test]
    fn every_input_is_part_of_the_key() {
        // Two files and the rules applied to them. Miss any one and a stale
        // answer gets served for an input that no longer matches it.
        let base = cache_key("T.MBIN", "modsha", "vansha", true);
        assert_ne!(base, cache_key("OTHER.MBIN", "modsha", "vansha", true));
        assert_ne!(base, cache_key("T.MBIN", "changed", "vansha", true));
        assert_ne!(base, cache_key("T.MBIN", "modsha", "changed", true));
        assert_eq!(base, cache_key("T.MBIN", "modsha", "vansha", true), "and stable");
    }

    #[test]
    fn a_mod_that_matches_vanilla_prunes_to_nothing() {
        let vanilla = table(&[("ALPHA", "1")]);
        let out = prune("T.MBIN", &vanilla, &vanilla, true).unwrap();
        assert!(out.empty());
        assert_eq!(out.edits, 0);
    }

    #[test]
    fn a_row_the_mod_adds_is_kept_whole() {
        let vanilla = table(&[("ALPHA", "1")]);
        let modded = table(&[("ALPHA", "1"), ("NEW", "7")]);
        let out = prune("T.MBIN", &vanilla, &modded, true).unwrap();
        let patch = exmltree::parse_str(&out.xml).unwrap().flatten();
        assert_eq!(patch.get("Row[NEW]/Cost"), Some(&Some("7".to_string())));
        assert!(!out.xml.contains("ALPHA"));
    }

    #[test]
    fn a_mod_that_deletes_nodes_is_refused_not_emptied() {
        // "No Laser Flare" ships the muzzle scene with the flare removed:
        // 161 vanilla properties down to 137, and every survivor matches
        // vanilla. Pruning it on edit count alone yields an empty patch that
        // silently undoes the mod.
        let vanilla = table(&[("KEEP", "1"), ("FLARE", "1")]);
        let deletes = table(&[("KEEP", "1")]);
        let err = prune("T.MBIN", &vanilla, &deletes, true).unwrap_err();
        assert!(err.contains("removing"), "got: {err}");
    }

    #[test]
    fn float_rendering_noise_is_not_an_edit() {
        // Otherwise a decompile round trip alone would keep the entire file.
        let vanilla = table(&[("A", "65.861860")]);
        let modded = table(&[("A", "65.8618546")]);
        assert!(prune("T.MBIN", &vanilla, &modded, true).unwrap().empty());
    }

    /// A staged mod and a place to build a cleaned copy of it.
    struct Sandbox {
        dir: PathBuf,
    }

    impl Sandbox {
        /// Staged the way `archive::install` lays a mod out: the staged folder
        /// holds exactly what appears in the mods folder, so the mod's own
        /// folder sits one level in and its readme sits beside it.
        fn new(tag: &str) -> Sandbox {
            let dir = std::env::temp_dir().join(format!("nmscheck-prune-{tag}"));
            std::fs::remove_dir_all(&dir).ok();
            std::fs::create_dir_all(dir.join("staging/DemoMod/TABLES")).unwrap();
            std::fs::write(dir.join("staging/DemoMod/TABLES/T.MBIN"), b"original bytes").unwrap();
            std::fs::write(dir.join("staging/DemoMod.lua"), b"the build script").unwrap();
            Sandbox { dir }
        }

        fn origin(&self) -> PathBuf {
            self.dir.join("staging")
        }

        fn built(&self) -> PathBuf {
            self.dir.join("derived")
        }

        fn plan_for(&self, xml: &str) -> Plan {
            Plan {
                owner: "DemoMod".into(),
                target: "TABLES/T.MBIN".into(),
                rel: "TABLES/T.MBIN".into(),
                original: 100,
                edits: 1,
                dropped: 99,
                whole_file: true,
                refused: None,
                cleaned: false,
                xml: Some(xml.to_string()),
            }
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    #[test]
    fn cleaning_builds_a_second_copy_and_never_touches_the_first() {
        // The property the whole design rests on: undoing a clean is pointing
        // the loadout back at the staged mod, so the staged mod has to come
        // through completely untouched -- every byte, and every sibling file
        // the game or AMUMSS might want.
        let box_ = Sandbox::new("build");
        let entry = box_.plan_for("<Data template=\"cGcTable\" />");

        let done = clean_into(&box_.origin(), &box_.built(), &[&entry]).unwrap();
        assert_eq!(done.len(), 1);

        let built = box_.built();
        assert!(
            !built.join("DemoMod/TABLES/T.MBIN").exists(),
            "the override must be gone from the build, or it still wins over the patch"
        );
        assert!(built.join("DemoMod/TABLES/T.EXML").is_file());
        assert!(
            built.join("DemoMod.lua").is_file(),
            "the build has to be a whole mod, not just the file that changed"
        );

        assert_eq!(
            std::fs::read(box_.origin().join("DemoMod/TABLES/T.MBIN")).unwrap(),
            b"original bytes",
            "the staged mod was edited; there is now no way back"
        );
    }

    #[test]
    fn the_two_builds_are_the_record_that_a_clean_happened() {
        // Nothing is written down anywhere: the pair of folders says it, which
        // is what keeps "is this cleaned" true after a reinstall, a preset
        // switch or a hand-edited settings file.
        let box_ = Sandbox::new("record");
        let entry = box_.plan_for("<Data template=\"cGcTable\" />");
        assert!(already_cleaned(&box_.origin(), &box_.built(), "DemoMod").is_empty());

        clean_into(&box_.origin(), &box_.built(), &[&entry]).unwrap();
        let found = already_cleaned(&box_.origin(), &box_.built(), "DemoMod");
        assert_eq!(found.len(), 1);
        assert!(found[0].cleaned);
        // Relative to the mod folder, exactly as a fresh plan's `rel` is: the
        // two kinds of plan land in one list and the UI reads them alike.
        assert_eq!(found[0].rel, "TABLES/T.MBIN");
    }

    #[test]
    fn a_build_cannot_be_cleaned_into_itself() {
        // A merge has no author's copy behind it, so its recorded origin is the
        // derived folder it lives in -- and a clean of it would build into
        // `derived/<owner>`, the same path. Without this guard `clean_into`
        // clears the destination and then fails reading the origin it has just
        // deleted, which destroys the merge.
        let box_ = Sandbox::new("self_clean");
        std::fs::create_dir_all(box_.origin().join("DemoMod")).unwrap();
        let plan = box_.plan_for("<Data />");
        let err = clean_into(&box_.origin(), &box_.origin(), &[&plan]).unwrap_err();
        assert!(err.contains("build this program made"), "got: {err}");
        assert!(box_.origin().join("DemoMod").is_dir(), "it deleted the build");
    }

    #[test]
    fn cleaning_twice_rebuilds_rather_than_refusing() {
        // The old in-place version had to refuse, because a second run would
        // have backed up the patch over the real original. There is no backup
        // now -- the staged mod is the original -- so a rebuild is just a
        // rebuild, which is what makes re-cleaning after a game update work.
        let box_ = Sandbox::new("twice");
        let entry = box_.plan_for("<Data template=\"cGcTable\" />");
        clean_into(&box_.origin(), &box_.built(), &[&entry]).unwrap();

        let again = box_.plan_for("<Data template=\"cGcTable\"><changed/></Data>");
        clean_into(&box_.origin(), &box_.built(), &[&again]).unwrap();
        let text =
            std::fs::read_to_string(box_.built().join("DemoMod/TABLES/T.EXML")).unwrap();
        assert!(text.contains("changed"), "the rebuild kept the old patch");
    }

    #[test]
    fn a_patch_that_carries_vanilla_values_is_pruned_of_them() {
        // The measured shape: `BetterRewardsCombined` names 6,183 properties in
        // REWARDTABLE and changes 2,790 of them, so 3,393 vanilla values travel
        // inside a 504 KB `.EXML` and are written to the asset at load. Sparse
        // in form, not in content.
        let vanilla = table(&[("ALPHA", "1"), ("BETA", "2"), ("GAMMA", "3")]);
        // A patch that edits BETA but also restates ALPHA at the game's value.
        let carrying = table(&[("ALPHA", "1"), ("BETA", "99")]);

        let out = prune("T.MBIN", &vanilla, &carrying, false).unwrap();
        assert_eq!(out.edits, 1, "only BETA is a change");
        assert!(out.xml.contains("99"));
        assert!(
            !out.xml.contains("ALPHA"),
            "the carried vanilla value survived, so it would still revert somebody"
        );
        // GAMMA was never mentioned and must not appear: a patch adds and
        // overrides, and inventing rows would change what the mod does.
        assert!(!out.xml.contains("GAMMA"));
    }

    #[test]
    fn a_patch_is_not_accused_of_deleting_what_it_never_mentioned() {
        // The trap in treating a `.EXML` as a whole file: everything it omits
        // reads as a deletion, so the mod is refused as one that "works by
        // removing things" -- and the shape this most wants to fix is the shape
        // it would decline to look at.
        let vanilla = table(&[("ALPHA", "1"), ("BETA", "2"), ("GAMMA", "3")]);
        let patch = table(&[("BETA", "99")]);

        let as_patch = prune("T.MBIN", &vanilla, &patch, false);
        assert!(as_patch.is_ok(), "refused a sparse patch: {:?}", as_patch.err());
        assert_eq!(as_patch.unwrap().edits, 1);

        // The same bytes read as a whole file really are a deletion of two rows,
        // and that refusal has to stay.
        let err = prune("T.MBIN", &vanilla, &patch, true).unwrap_err();
        assert!(err.contains("removing"), "got: {err}");
    }

    #[test]
    fn a_patch_that_is_all_edits_already_is_left_alone() {
        // Most patches in the measured library are honest: 609 of 610
        // properties are changes. Pruning must find nothing to drop rather than
        // rewrite the file for no reason.
        let vanilla = table(&[("ALPHA", "1"), ("BETA", "2")]);
        let honest = table(&[("ALPHA", "7"), ("BETA", "8")]);
        let out = prune("T.MBIN", &vanilla, &honest, false).unwrap();

        assert_eq!(out.edits, 2);
        // Every row it changes is still there, addressed the same way. What it
        // loses is each row's `Id` *child* property, which restates the `_id`
        // the row is already keyed by -- redundant, and dropped from whole-file
        // copies for the same reason.
        for (id, value) in [("ALPHA", "7"), ("BETA", "8")] {
            assert!(out.xml.contains(&format!("_id=\"{id}\"")), "lost row {id}");
            assert!(out.xml.contains(&format!("value=\"{value}\"")), "lost {id}'s edit");
        }
    }

    #[test]
    fn the_same_bytes_are_cached_apart_for_the_two_readings() {
        // `whole_file` is part of the question, not of the files, so it has to
        // be part of the key or a patch would be served an answer computed for
        // a whole file.
        assert_ne!(
            cache_key("T.MBIN", "modsha", "vansha", true),
            cache_key("T.MBIN", "modsha", "vansha", false),
        );
    }

    #[test]
    fn only_assets_the_game_reads_a_loose_exml_for_are_planned() {
        // The measured evidence, as a rule. Cleaning a `MODELS\` override
        // produces a file the game does not read, which switches the mod off --
        // four mods in the measured library were disabled this way, one of them
        // noticed in game when its freighter hangar terminals disappeared.
        assert!(loose_exml_is_read("GLOBALS/GCGAMEPLAYGLOBALS.GLOBAL.MBIN"));
        assert!(loose_exml_is_read(
            "METADATA/SIMULATION/SOLARSYSTEM/VOXELGENERATORSETTINGS.MBIN"
        ));
        assert!(loose_exml_is_read(
            "metadata/reality/tables/rewardtable.mbin"
        ));
        assert!(!loose_exml_is_read(
            "MODELS/COMMON/SPACECRAFT/COMMONPARTS/HANGARINTERIORPARTS/HANGAR.SCENE.MBIN"
        ));
        assert!(!loose_exml_is_read("MODELS/EFFECTS/WARP/X.MATERIAL.MBIN"));
        assert!(!loose_exml_is_read("TEXTURES/UI/HUD/X.MBIN"));
    }

    #[test]
    fn a_mod_shipping_one_asset_twice_is_refused_rather_than_no_opped() {
        // Both forms of one asset: swapping the `.MBIN` for a patch would leave
        // the `.EXML` carrying the whole table, so the clean would do nothing
        // and say it had worked. A loose `.MXML` is *not* a second copy -- that
        // extension is the AMUMSS localisation table and carries no target.
        use crate::engine::model::ModFile;
        let shipping = |kinds: &[(FileKind, &str)]| Mod {
            name: "Doubled".into(),
            files: kinds
                .iter()
                .map(|(kind, rel)| ModFile {
                    kind: Some(*kind),
                    target: Some("A/T.MBIN".into()),
                    rel_path: (*rel).into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };

        let doubled = shipping(&[(FileKind::Mbin, "A/T.MBIN"), (FileKind::Exml, "A/T.EXML")]);
        assert_eq!(copies_of(&doubled, "A/T.MBIN"), 2, "the spare copy was not seen");

        let single = shipping(&[(FileKind::Mbin, "A/T.MBIN")]);
        assert_eq!(copies_of(&single, "A/T.MBIN"), 1);
        assert_eq!(copies_of(&single, "SOMETHING/ELSE.MBIN"), 0);

        // A localisation table sits beside a mod's assets and is not one of
        // them, so it never carries a target and never counts as a copy.
        let with_loc = Mod {
            files: vec![
                ModFile {
                    kind: Some(FileKind::Mbin),
                    target: Some("A/T.MBIN".into()),
                    ..Default::default()
                },
                ModFile {
                    kind: Some(FileKind::Mxml),
                    target: None,
                    rel_path: "LocTable.MXML".into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert_eq!(copies_of(&with_loc, "A/T.MBIN"), 1, "a LocTable was counted as a copy");
    }

    #[test]
    fn a_refused_plan_writes_nothing() {
        let box_ = Sandbox::new("refused");
        let mut entry = box_.plan_for("");
        entry.xml = None;
        entry.refused = Some("works by removing things".into());

        assert!(clean_into(&box_.origin(), &box_.built(), &[&entry]).is_err());
        assert!(!box_.built().exists(), "a refusal must not leave half a build");
    }

    #[test]
    fn a_stale_baseline_is_dropped_not_preserved() {
        // The mod was built when BETA cost 2; the game now says 5. The mod
        // never meant to touch BETA, so shipping its old value would revert a
        // game change. Against current vanilla it simply is not an edit.
        let current_vanilla = table(&[("ALPHA", "1"), ("BETA", "5")]);
        let old_mod = table(&[("ALPHA", "9"), ("BETA", "2")]);
        let out = prune("T.MBIN", &current_vanilla, &old_mod, true).unwrap();
        // Both differ from today's vanilla, so both are kept -- pruning cannot
        // tell intent apart. What it *can* do is shrink the blast radius and
        // make the stale value visible instead of hiding it in 153,000 others.
        assert_eq!(out.edits, 2);
        assert!(out.xml.contains("_id=\"BETA\""));
    }
}
