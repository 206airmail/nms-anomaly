//! What the game should be reading, and how to make that true.
//!
//! Three layers, and only the last one is the game's business:
//!
//! ```text
//! archives    the .zip/.rar exactly as downloaded          never modified
//! staging     each mod extracted, one folder per mod       never modified
//! derived     a cleaned or merged build of staged mods     rebuilt at will
//! GAMEDATA\MODS   hardlinks to whichever variant is chosen  what the game loads
//! ```
//!
//! Keeping those apart is what makes the risky operations safe. Cleaning a mod
//! does not edit the mod: it writes a *second* build of it under `derived` and
//! changes which one is linked. Undoing is not a restore from backup, it is
//! pointing at the original again -- the original was never touched, so there
//! is nothing that can fail to come back.
//!
//! It also settles a question the old design got wrong. Merging two mods used
//! to write a third mod folder and leave the user to disable the first two by
//! hand; forget, and the game loads the merge *and* both inputs, and the
//! merge loses. Here a merged entry names the mods it [`Entry::replaces`], and
//! [`Loadout::wanted`] simply never deploys those. The bad state is not
//! something to remember to avoid, it is unrepresentable.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::deploy;

/// Which build of a mod the game should see.
///
/// `Cleaned` and `Mended` are deliberately separate even though both are "a
/// build of one mod sitting beside its staged copy". They are different
/// operations with different undos, and the library list shows this word to the
/// user: one label for both meant a mod whose broken XML had been repaired
/// reported itself as "cleaned", which is a different thing that had not
/// happened to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Variant {
    /// the mod exactly as its author shipped it
    Original,
    /// the same mod with whole-file overrides reduced to the edits they make
    Cleaned,
    /// the same mod with a file the game could not read put right
    Mended,
    /// a build combining several mods, which stands in for all of them
    Merged,
}

/// One mod, and what should happen to it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// the folder name the game will see
    pub owner: String,
    /// the folder holding this variant's real files
    pub source: String,
    /// the staged mod as its author shipped it, whatever `source` points at
    /// now.
    ///
    /// Switching to a cleaned, mended or merged build moves `source` to the
    /// derived folder. Without this, that is a one-way door: the path back to
    /// the original is gone, so "undo" has nothing to point at and a second
    /// repair would try to mend the already-mended build.
    ///
    /// `None` for entries recorded before this was tracked; those fall back to
    /// `source`, which is correct for anything still on its original build.
    #[serde(default)]
    pub origin: Option<String>,
    /// the downloaded archive this came from, when it was downloaded here.
    ///
    /// Kept so that deleting a mod can offer to remove the download too --
    /// the one piece that costs bandwidth rather than only disk, which is why
    /// it is a separate decision from deleting the mod.
    #[serde(default)]
    pub archive: Option<String>,
    pub variant: Variant,
    /// mods this one stands in for. They are never deployed while it is.
    #[serde(default)]
    pub replaces: Vec<String>,
    /// the top-level names this put in the mods folder.
    ///
    /// Recorded rather than derived, because a mod is often several things --
    /// a folder, a `.lua`, a readme -- and removing it has to take exactly
    /// those and nothing a neighbour owns.
    #[serde(default)]
    pub deployed: Vec<String>,
    /// the `source` those names were actually deployed from.
    ///
    /// Without this, a rebuilt variant that ships the *same file names* looks
    /// identical to what is already installed and never gets deployed. That is
    /// not hypothetical: mending a malformed `.EXML` changes one file's
    /// contents and nothing else, so comparing names alone reported "already
    /// correct" and left the broken file in the game.
    ///
    /// `None` means "deployed before this was recorded", and falls back to
    /// comparing names, so an older loadout does not relink the whole library.
    #[serde(default)]
    pub built_from: Option<String>,
    /// false when the user has switched it off without uninstalling it
    #[serde(default = "yes")]
    pub enabled: bool,
    /// true when this build carries values the user set by hand.
    ///
    /// Deliberately a flag beside `variant` rather than a fifth variant.
    /// `variant` answers "which *shape* of build is this" — the author's copy,
    /// a patch reduced from an override, a repaired file, a combination of
    /// several mods — and editing is orthogonal to all four: a cleaned mod can
    /// carry hand-set values and still be a cleaned mod. Made a variant, the
    /// pair "cleaned and edited" would have had to pick one word and drop the
    /// other, which is the exact mistake the note on `Variant` records.
    ///
    /// The values themselves are not here. They live in `edits.json`
    /// ([`super::edit::Book`]), keyed by this entry's owner, because a build is
    /// derived from them and not the other way round: the loadout says which
    /// build the game reads, and that stays one fact per mod however many
    /// properties were changed to get it.
    #[serde(default)]
    pub edited: bool,
}

fn yes() -> bool {
    true
}

impl Entry {
    /// The mod as its author shipped it: what a repair reads from and what an
    /// undo goes back to. Falls back to `source` for entries that predate the
    /// field, which are on their original build by definition.
    pub fn origin_path(&self) -> PathBuf {
        PathBuf::from(self.origin.as_deref().unwrap_or(&self.source))
    }
}

/// Everything this program knows it is responsible for.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Loadout {
    #[serde(default)]
    pub entries: Vec<Entry>,
}

impl Loadout {
    pub fn read(path: &Path) -> Loadout {
        super::read_json(path).unwrap_or_default()
    }

    pub fn write(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| format!("could not save the mod list: {e}"))
    }

    pub fn get(&self, owner: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.owner == owner)
    }

    /// Add or replace one entry.
    ///
    /// Changing what a mod *is* -- swapping it to its cleaned build, say --
    /// must not forget what is currently in the game under its name, or the
    /// next reconcile would deploy the new files over the old ones instead of
    /// replacing them, and a leftover `.MBIN` override beats the `.EXML` patch
    /// that replaced it. So an entry that does not say what it deployed keeps
    /// the record it is replacing.
    pub fn put(&mut self, mut entry: Entry) {
        // Nothing stands in for itself. A merge rebuilt to absorb a new mod is
        // handed its own folder among the inputs -- it is the thing in the
        // game, so it is what the newcomer conflicts with -- and an entry that
        // supersedes itself is one [`Self::wanted`] refuses to deploy. Held
        // here rather than at the caller because it is a property of the
        // record, not of merging.
        let itself = entry.owner.clone();
        entry.replaces.retain(|name| name != &itself);

        match self.entries.iter_mut().find(|e| e.owner == entry.owner) {
            Some(slot) => {
                if entry.deployed.is_empty() {
                    // What is in the game, and which build it came from, are
                    // one record: carrying the names forward without the
                    // build they came from would make a rebuilt variant look
                    // like it was already deployed.
                    entry.deployed = std::mem::take(&mut slot.deployed);
                    entry.built_from = slot.built_from.take();
                }
                // The original never changes when a variant does, so it is
                // kept unless the caller is deliberately replacing it -- which
                // is what reinstalling the mod does.
                if entry.origin.is_none() {
                    entry.origin = slot.origin.take();
                }
                *slot = entry;
            }
            None => self.entries.push(entry),
        }
    }

    /// Drop entries whose files are gone from disk, and dissolve what stood on
    /// them. Returns the owners forgotten, newest record first.
    ///
    /// # Why the loadout has to be able to heal itself
    ///
    /// The book is the record of what this program is responsible for, and the
    /// Library builds its list from the book rather than from a scan — it has
    /// to, because a switched-off mod is staged and not deployed and would
    /// otherwise vanish instead of offering its switch. So an entry outlives
    /// its files: delete a mod by hand out of the game folder and out of
    /// staging, as a person reasonably might, and it goes on being listed
    /// forever, with nothing behind it. Clicking it shows an empty pane,
    /// because every *other* surface is built from the scan and agrees it is
    /// not there.
    ///
    /// # What counts as gone, and why it is this strict
    ///
    /// Both of its builds must be missing — `origin`, the copy its author
    /// shipped, and `source`, whichever build is deployed — and nothing it
    /// claims to have deployed may still be in the game folder. A mod with
    /// files anywhere is a mod to reconcile, not to forget.
    ///
    /// `staging` and `derived` guard the whole operation: if either cannot be
    /// read, nothing is pruned. Every `source` lives under one of the two, so a
    /// drive that is unplugged, renamed or not yet mounted would otherwise look
    /// exactly like a whole library deleted by hand. A missing root means
    /// "cannot tell", never "all gone".
    ///
    /// Both, not just staging: a **merge** is the case that needs `derived`. It
    /// has no author's copy behind it — it *is* the build — so `merge::build`
    /// sets its `origin` to the derived folder rather than to anything in
    /// staging. Guarding on staging alone would leave every merge in the
    /// library to be forgotten, and dissolved, because a folder they do not
    /// live in happened to be readable.
    pub fn forget_missing(&mut self, staging: &Path, derived: &Path, mods_dir: &Path) -> Vec<String> {
        if !staging.is_dir() || !derived.is_dir() {
            return Vec::new();
        }
        let dead: Vec<String> = self
            .entries
            .iter()
            .filter(|e| {
                !e.origin_path().exists()
                    && !Path::new(&e.source).exists()
                    && !e.deployed.iter().any(|name| mods_dir.join(name).exists())
            })
            .map(|e| e.owner.clone())
            .collect();
        for owner in &dead {
            // Through `forget`, so a merge built out of a mod that has been
            // deleted by hand goes the same way it would have if the deletion
            // had gone through this program.
            self.forget(owner);
        }
        dead
    }

    /// Forget a mod, and dissolve any merge that was built out of it.
    ///
    /// Returns the merges that went, whole, because the caller has to take
    /// their files out of the game: an entry that is no longer listed is one
    /// [`reconcile`] can no longer undeploy, so dropping it here without
    /// saying what it had deployed would strand its files in the game folder
    /// for good.
    pub fn forget(&mut self, owner: &str) -> Vec<Entry> {
        self.entries.retain(|e| e.owner != owner);
        self.dissolve_merges_of(owner)
    }

    /// Take out every merge that stands in for `owner`, and every merge that
    /// stood in for one of those.
    ///
    /// Call this whenever a mod's *content* changes or goes away: installed
    /// over, updated, uninstalled, deleted.
    ///
    /// Merely striking the name out of the merge's `replaces` is not enough,
    /// and was the wrong answer here for a while. A merge is a single compiled
    /// asset with its inputs' edits already baked into it -- so a mod dropped
    /// from the list goes on running from inside the merge, which is precisely
    /// what the user asked to stop, while every other input stays suppressed.
    /// An updated one is worse: the new version is held out of the game and
    /// the old version keeps playing.
    ///
    /// A merge is only meaningful as a stand-in for exactly the mods it was
    /// built from. Lose one, or change one, and it is no longer that, so it
    /// goes and its surviving inputs come straight back. The next scan sees
    /// the conflict again and offers a fresh merge -- which is the honest
    /// state of affairs, and one click away.
    pub fn dissolve_merges_of(&mut self, owner: &str) -> Vec<Entry> {
        let mut gone: Vec<Entry> = Vec::new();
        // A merge can itself be something another merge stands in for, so
        // removing one can orphan the next. Each round only ever removes
        // entries, so this reaches the end.
        let mut pending: Vec<String> = vec![owner.to_string()];
        while let Some(trigger) = pending.pop() {
            let doomed: Vec<String> = self
                .entries
                .iter()
                .filter(|e| e.variant == Variant::Merged)
                .filter(|e| e.replaces.iter().any(|name| name == &trigger))
                .map(|e| e.owner.clone())
                .collect();
            if doomed.is_empty() {
                continue;
            }
            gone.extend(
                self.entries
                    .iter()
                    .filter(|e| doomed.contains(&e.owner))
                    .cloned(),
            );
            self.entries.retain(|e| !doomed.contains(&e.owner));
            pending.extend(doomed);
        }
        gone
    }

    /// Every mod that some *enabled* entry stands in for.
    ///
    /// A disabled merge suppresses nothing: switching the merge off is how you
    /// get the originals back, so it has to give them up when it goes.
    pub fn superseded(&self) -> BTreeSet<String> {
        self.entries
            .iter()
            .filter(|e| e.enabled)
            .flat_map(|e| e.replaces.iter().cloned())
            .collect()
    }

    /// Mod folder -> the directory its files should be linked from.
    ///
    /// This is the whole decision: enabled, not replaced by something else.
    pub fn wanted(&self) -> BTreeMap<String, PathBuf> {
        let gone = self.superseded();
        self.entries
            .iter()
            .filter(|e| e.enabled && !gone.contains(&e.owner))
            .map(|e| (e.owner.clone(), PathBuf::from(&e.source)))
            .collect()
    }
}

/// What reconciling did, or would do.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Changes {
    pub deployed: Vec<String>,
    pub removed: Vec<String>,
    /// mods left alone because they were already right
    pub unchanged: Vec<String>,
    /// things that went wrong, named, without stopping the rest
    pub problems: Vec<String>,
    /// names more than one mod wanted, and who got them
    #[serde(default)]
    pub clashes: Vec<Clash>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.deployed.is_empty() && self.removed.is_empty() && self.problems.is_empty()
    }
}

/// A file in the mods folder that more than one enabled mod wants to own.
///
/// Only game content can get here. Readmes and build scripts never reach the
/// mods folder at all ([`deploy::is_game_content`]), so the case that used to
/// dominate -- two mods shipping `README.txt` -- is not a clash any more.
#[derive(Debug, Clone, Serialize)]
pub struct Clash {
    /// the path, relative to the mods folder
    pub rel: String,
    /// the mod that gets it
    pub keeper: String,
    /// the mods that give it up
    pub losers: Vec<String>,
}

fn relative_files(root: &Path, prefix: &Path, out: &mut BTreeSet<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = prefix.join(entry.file_name());
        if path.is_dir() {
            relative_files(&path, &rel, out);
        } else {
            out.insert(rel);
        }
    }
}

/// Work out, before anything is written, which mods want the same file.
///
/// Deploying is all-or-nothing per mod: the first file that already exists
/// aborts the whole mod and rolls it back. Without this, one contested file
/// would cost the user everything else that mod ships. Here the file goes to
/// one mod and the others simply do not deploy it, so they still install.
///
/// The keeper is the first owner in name order -- arbitrary, but *stable*,
/// which is what matters: reconciling twice must not swap who holds the file
/// and churn the game folder. Stable is not the same as correct, though. Which
/// mod should win a contested asset is a load-order question, and these are
/// reported so the conflict report can say so rather than having it settled
/// quietly here.
pub fn clashes(wanted: &BTreeMap<String, PathBuf>) -> Vec<Clash> {
    let mut claims: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (owner, source) in wanted {
        let mut files = BTreeSet::new();
        relative_files(source, Path::new(""), &mut files);
        for rel in files {
            if !deploy::is_game_content(&rel) {
                continue; // never deployed, so never contested
            }
            claims
                .entry(rel.display().to_string())
                .or_default()
                .push(owner.clone());
        }
    }

    claims
        .into_iter()
        .filter(|(_, owners)| owners.len() > 1)
        .map(|(rel, mut owners)| {
            owners.sort();
            let keeper = owners.remove(0);
            Clash {
                rel,
                keeper,
                losers: owners,
            }
        })
        .collect()
}

/// Make the mods folder match the loadout.
///
/// Only touches mod folders the loadout knows about. Anything else in
/// `GAMEDATA\MODS` -- mods installed by another manager, or by hand -- is left
/// exactly where it is, because it is not ours to remove.
pub fn reconcile(loadout: &mut Loadout, mods_dir: &Path, dry_run: bool) -> Changes {
    let wanted = loadout.wanted();
    let mut changes = Changes::default();
    changes.clashes = clashes(&wanted);
    // Settled before a single file moves, so that a mod which loses a
    // contested file still installs everything else it ships.
    let mut skips: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for clash in &changes.clashes {
        for loser in &clash.losers {
            skips
                .entry(loser.clone())
                .or_default()
                .insert(clash.rel.clone());
        }
    }
    let nothing = BTreeSet::new();
    // What each mod ends up with in the game, and what it came from, written
    // back at the end: the loadout is the only record of either, and the next
    // reconcile needs both to know what to take away and what to rebuild.
    let mut placed: BTreeMap<String, (Vec<String>, Option<String>)> = BTreeMap::new();

    // Take away what should no longer be there, before putting anything in:
    // a merge and the mods it replaces can share asset paths.
    for entry in &loadout.entries {
        if wanted.contains_key(&entry.owner) || entry.deployed.is_empty() {
            continue;
        }
        if !entry.deployed.iter().any(|n| mods_dir.join(n).exists()) {
            continue; // already not in the game
        }
        if dry_run {
            changes.removed.push(entry.owner.clone());
            continue;
        }
        match deploy::undeploy(mods_dir, &entry.deployed) {
            Ok(_) => {
                placed.insert(entry.owner.clone(), (Vec::new(), None));
                changes.removed.push(entry.owner.clone());
            }
            Err(e) => changes.problems.push(e),
        }
    }

    for (owner, source) in &wanted {
        let entry = loadout.get(owner);
        let already = entry.map(|e| e.deployed.as_slice()).unwrap_or(&[]);
        let skip = skips.get(owner).unwrap_or(&nothing);
        // "Already correct" needs both: the right names *and* the right build
        // behind them. A rebuilt variant often ships identical names -- a
        // mended file differs only in its contents -- so names alone would
        // leave the old build in the game and report success.
        let same_build = match entry.and_then(|e| e.built_from.as_deref()) {
            Some(was) => was == source.display().to_string(),
            None => true, // recorded before we tracked this; trust the names
        };
        if !already.is_empty() && same_build && deployed_from(mods_dir, already, source, skip) {
            changes.unchanged.push(owner.clone());
            continue;
        }
        if dry_run {
            changes.deployed.push(owner.clone());
            continue;
        }
        // Deployed from somewhere else: take it out before relinking, or the
        // deploy will refuse on the first file that already exists.
        if !already.is_empty() {
            if let Err(e) = deploy::undeploy(mods_dir, already) {
                changes.problems.push(e);
                continue;
            }
        }
        match deploy::deploy_skipping(source, mods_dir, skip) {
            Ok(done) => {
                placed.insert(
                    owner.clone(),
                    (done.top_level, Some(source.display().to_string())),
                );
                changes.deployed.push(owner.clone());
            }
            Err(e) => changes.problems.push(e),
        }
    }

    for entry in &mut loadout.entries {
        if let Some((now, from)) = placed.remove(&entry.owner) {
            entry.deployed = now;
            entry.built_from = from;
        }
    }

    changes.deployed.sort();
    changes.removed.sort();
    changes.unchanged.sort();
    changes
}

/// One file, as cheaply as two copies of it can be told apart: where it sits,
/// how big it is, and when it was last written.
///
/// Deploying makes a hardlink, and a hardlink *is* the file -- same size, same
/// modified time, necessarily. The copy `deploy::link_or_copy` falls back to
/// when a link is refused carries both across as well, because Windows copies
/// a file's timestamps with its bytes. So a correctly deployed name matches
/// its source on all three without hashing anything, while a build rewritten
/// under the same name does not.
type Stamp = (PathBuf, u64, Option<std::time::SystemTime>);

fn stamped_files(root: &Path, prefix: &Path, out: &mut BTreeSet<Stamp>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = prefix.join(entry.file_name());
        if path.is_dir() {
            stamped_files(&path, &rel, out);
        } else {
            let (len, at) = match entry.metadata() {
                Ok(data) => (data.len(), data.modified().ok()),
                Err(_) => (0, None),
            };
            out.insert((rel, len, at));
        }
    }
}

/// True when what is deployed came from `source` *as it stands now*.
///
/// ---------------------------------------------------------------------------
/// Why the names are not enough
/// ---------------------------------------------------------------------------
///
/// This compared the set of relative paths. That catches the case it was
/// written for -- the user switched a mod between its original and cleaned
/// builds, which differ by whole files -- and misses one that costs just as
/// much: a build **rewritten in place**.
///
/// Changing a value is exactly that. The edited copy is rebuilt into the same
/// `derived/<owner>__edited` folder, under the same file names, every time a
/// value changes. So the folder path was unchanged, which satisfied the
/// `built_from` check; the names were unchanged, which satisfied this one; and
/// reconcile reported `unchanged` and relinked nothing. The *first* edit to a
/// mod reached the game and every edit after it was dropped in silence --
/// which is the one failure this program exists to prevent, the screen saying
/// a value is set while the game goes on loading the old one.
///
/// Size and modified time settle it, and they cost nothing: the walk is
/// already reading each directory entry, and both come off the entry's own
/// metadata.
///
/// It still compares like with like: the readmes and build scripts under
/// `source` are never deployed, and neither is anything in `skip`, so
/// expecting them in the mods folder would make every mod look wrong and
/// relink the whole library on every reconcile.
fn deployed_from(
    mods_dir: &Path,
    deployed: &[String],
    source: &Path,
    skip: &BTreeSet<String>,
) -> bool {
    if !source.is_dir() {
        return false;
    }
    // Both sides are rooted at the *deployed* folder name -- a staged mod's
    // top level holds the folders it puts in the game -- so the two sets are
    // directly comparable.
    let mut want = BTreeSet::new();
    stamped_files(source, Path::new(""), &mut want);
    want.retain(|(rel, _, _)| {
        deploy::is_game_content(rel) && !skip.contains(&rel.display().to_string())
    });

    let mut have = BTreeSet::new();
    for name in deployed {
        let at = mods_dir.join(name);
        if at.is_dir() {
            stamped_files(&at, Path::new(name), &mut have);
        } else if at.is_file() {
            let (len, when) = match std::fs::metadata(&at) {
                Ok(data) => (data.len(), data.modified().ok()),
                Err(_) => (0, None),
            };
            have.insert((PathBuf::from(name), len, when));
        }
    }
    want == have
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let path = std::env::current_dir()
                .unwrap_or_else(|_| std::env::temp_dir())
                .join(format!("target/nmscheck_loadout_{tag}"));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Dir(path)
        }

        fn file(&self, rel: &str, body: &str) {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn entry(owner: &str, source: &str, variant: Variant) -> Entry {
        Entry {
            owner: owner.into(),
            source: source.into(),
            origin: None,
            archive: None,
            variant,
            replaces: Vec::new(),
            deployed: Vec::new(),
            built_from: None,
            enabled: true,
            edited: false,
        }
    }

    #[test]
    fn a_mod_deleted_by_hand_is_forgotten_and_one_still_there_is_not() {
        let dir = Dir::new("forget_missing");
        let staging = dir.0.join("staging");
        let mods = dir.0.join("MODS");
        std::fs::create_dir_all(staging.join("Still Here")).unwrap();
        std::fs::create_dir_all(&mods).unwrap();

        let mut book = Loadout::default();
        book.put(entry(
            "Still Here",
            staging.join("Still Here").to_str().unwrap(),
            Variant::Original,
        ));
        book.put(entry(
            "Deleted By Hand",
            staging.join("Deleted By Hand").to_str().unwrap(),
            Variant::Original,
        ));

        let derived = dir.0.join("derived");
        std::fs::create_dir_all(&derived).unwrap();
        let gone = book.forget_missing(&staging, &derived, &mods);
        assert_eq!(gone, vec!["Deleted By Hand"]);
        assert!(book.get("Still Here").is_some());
        assert!(book.get("Deleted By Hand").is_none());
    }

    #[test]
    fn an_entry_whose_files_are_still_in_the_game_is_kept() {
        // Staging gone but the folder is still deployed: that is a mod to
        // reconcile, not one to forget. Dropping it would strand its files in
        // the game folder with nothing recorded as owning them.
        let dir = Dir::new("forget_deployed");
        let staging = dir.0.join("staging");
        let mods = dir.0.join("MODS");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(mods.join("MOD.Something")).unwrap();

        let mut book = Loadout::default();
        let mut e = entry(
            "MOD.Something",
            staging.join("gone").to_str().unwrap(),
            Variant::Original,
        );
        e.deployed = vec!["MOD.Something".into()];
        book.put(e);

        let derived = dir.0.join("derived");
        std::fs::create_dir_all(&derived).unwrap();
        assert!(book.forget_missing(&staging, &derived, &mods).is_empty());
        assert!(book.get("MOD.Something").is_some());
    }

    #[test]
    fn an_unreadable_staging_folder_forgets_nothing() {
        // The guard that matters: every `source` lives under staging, so an
        // unplugged or renamed drive looks exactly like a whole library deleted
        // by hand. A missing root means "cannot tell", never "all gone".
        let dir = Dir::new("forget_no_staging");
        let mods = dir.0.join("MODS");
        std::fs::create_dir_all(&mods).unwrap();

        let mut book = Loadout::default();
        book.put(entry("Mod A", "E:/detached/Mod A", Variant::Original));
        book.put(entry("Mod B", "E:/detached/Mod B", Variant::Original));

        let derived = dir.0.join("derived");
        std::fs::create_dir_all(&derived).unwrap();
        assert!(book
            .forget_missing(&dir.0.join("not mounted"), &derived, &mods)
            .is_empty());
        assert_eq!(book.entries.len(), 2);

        // And the other way round: staging fine, derived away. A merge lives
        // only in derived, so this is the guard that keeps it.
        let staging = dir.0.join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        assert!(book
            .forget_missing(&staging, &dir.0.join("no derived"), &mods)
            .is_empty());
        assert_eq!(book.entries.len(), 2);
    }

    #[test]
    fn forgetting_a_deleted_mod_dissolves_the_merge_built_from_it() {
        let dir = Dir::new("forget_dissolves");
        let staging = dir.0.join("staging");
        let mods = dir.0.join("MODS");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&mods).unwrap();

        let mut book = Loadout::default();
        book.put(entry("Mod A", staging.join("Mod A").to_str().unwrap(), Variant::Original));
        let mut merged = entry("zzz_merge", staging.join("zzz").to_str().unwrap(), Variant::Merged);
        merged.replaces = vec!["Mod A".into()];
        book.put(merged);

        let derived = dir.0.join("derived");
        std::fs::create_dir_all(&derived).unwrap();
        book.forget_missing(&staging, &derived, &mods);
        assert!(book.get("Mod A").is_none());
        assert!(book.get("zzz_merge").is_none(), "the merge held Mod A's edits");
    }

    #[test]
    fn a_merge_keeps_the_mods_it_replaces_out_of_the_game() {
        // The old design's failure mode: the merge and both inputs all loaded,
        // and the merge lost. Here the inputs simply are not wanted.
        let mut loadout = Loadout::default();
        loadout.put(entry("Mod A", "/s/Mod A", Variant::Original));
        loadout.put(entry("Mod B", "/s/Mod B", Variant::Original));
        let mut merged = entry("A+B Merged", "/d/A+B Merged", Variant::Merged);
        merged.replaces = vec!["Mod A".into(), "Mod B".into()];
        loadout.put(merged);

        let wanted = loadout.wanted();
        assert_eq!(wanted.keys().collect::<Vec<_>>(), vec!["A+B Merged"]);
    }

    #[test]
    fn a_merge_handed_its_own_name_is_still_deployed() {
        // Rebuilding a merge to absorb a newcomer hands it its own folder
        // among the inputs -- the merge is what is in the game, so it is what
        // the newcomer conflicts with. Left in, it would put the merge in its
        // own shadow and `wanted` would refuse to deploy the one thing that
        // settles the asset.
        let mut loadout = Loadout::default();
        loadout.put(entry("Mod C", "/s/Mod C", Variant::Original));
        let mut merged = entry("zzz_merge", "/d/zzz_merge", Variant::Merged);
        merged.replaces = vec!["zzz_merge".into(), "Mod C".into()];
        loadout.put(merged);

        assert_eq!(loadout.get("zzz_merge").unwrap().replaces, vec!["Mod C"]);
        assert_eq!(loadout.wanted().keys().collect::<Vec<_>>(), vec!["zzz_merge"]);
    }

    #[test]
    fn absorbing_a_new_mod_keeps_the_first_pair_out_of_the_game() {
        // A+B are merged, then C arrives editing the same asset. What the
        // rebuild stands in for is worked out by `pipeline::stands_in_for`;
        // this is the shape it produces, and what the loadout must do with it.
        let mut loadout = Loadout::default();
        loadout.put(entry("Mod A", "/s/Mod A", Variant::Original));
        loadout.put(entry("Mod B", "/s/Mod B", Variant::Original));
        let mut merged = entry("zzz_merge", "/d/zzz_merge", Variant::Merged);
        merged.replaces = vec!["Mod A".into(), "Mod B".into()];
        loadout.put(merged);

        loadout.put(entry("Mod C", "/s/Mod C", Variant::Original));
        let mut again = entry("zzz_merge", "/d/zzz_merge", Variant::Merged);
        again.replaces = vec!["Mod A".into(), "Mod B".into(), "Mod C".into()];
        loadout.put(again);

        assert_eq!(loadout.wanted().keys().collect::<Vec<_>>(), vec!["zzz_merge"]);
    }

    #[test]
    fn switching_a_merge_off_gives_the_originals_back() {
        let mut loadout = Loadout::default();
        loadout.put(entry("Mod A", "/s/Mod A", Variant::Original));
        loadout.put(entry("Mod B", "/s/Mod B", Variant::Original));
        let mut merged = entry("A+B Merged", "/d/A+B Merged", Variant::Merged);
        merged.replaces = vec!["Mod A".into(), "Mod B".into()];
        merged.enabled = false;
        loadout.put(merged);

        let wanted = loadout.wanted();
        assert_eq!(wanted.keys().collect::<Vec<_>>(), vec!["Mod A", "Mod B"]);
    }

    #[test]
    fn uninstalling_an_input_dissolves_the_merge_made_out_of_it() {
        // This used only to strike the name out of the merge's `replaces`,
        // which left the merge in the game with that mod's edits still inside
        // it: the uninstalled mod went on running, and B stayed suppressed on
        // its behalf. A merge stands in for exactly what it was built from.
        let mut loadout = Loadout::default();
        let mut merged = entry("A+B Merged", "/d/A+B Merged", Variant::Merged);
        merged.replaces = vec!["Mod A".into(), "Mod B".into()];
        loadout.put(merged);
        loadout.put(entry("Mod A", "/s/Mod A", Variant::Original));
        loadout.put(entry("Mod B", "/s/Mod B", Variant::Original));

        let gone: Vec<String> = loadout.forget("Mod A").into_iter().map(|e| e.owner).collect();
        assert_eq!(gone, vec!["A+B Merged"]);
        assert!(loadout.get("Mod A").is_none());
        assert!(loadout.get("A+B Merged").is_none(), "merge outlived its input");
        assert_eq!(loadout.wanted().keys().collect::<Vec<_>>(), vec!["Mod B"]);
    }

    #[test]
    fn changing_a_mod_dissolves_the_merge_without_uninstalling_it() {
        // Updating an input is the same problem wearing different clothes: the
        // merge holds the *old* version's edits and suppresses the new one, so
        // the update would never reach the game.
        let mut loadout = Loadout::default();
        loadout.put(entry("Mod A", "/s/Mod A", Variant::Original));
        loadout.put(entry("Mod B", "/s/Mod B", Variant::Original));
        let mut merged = entry("A+B Merged", "/d/A+B Merged", Variant::Merged);
        merged.replaces = vec!["Mod A".into(), "Mod B".into()];
        loadout.put(merged);

        let gone: Vec<String> = loadout
            .dissolve_merges_of("Mod A")
            .into_iter()
            .map(|e| e.owner)
            .collect();
        assert_eq!(gone, vec!["A+B Merged"]);
        assert!(loadout.get("Mod A").is_some(), "the mod itself stays");
        assert_eq!(
            loadout.wanted().keys().collect::<Vec<_>>(),
            vec!["Mod A", "Mod B"]
        );
    }

    #[test]
    fn dissolving_follows_a_merge_that_another_merge_was_built_from() {
        // Not reachable today, since a rebuild always goes back to the
        // parents -- but a loadout written before that did chain, and one
        // orphaned merge must not leave another standing on top of it.
        let mut loadout = Loadout::default();
        loadout.put(entry("Mod A", "/s/Mod A", Variant::Original));
        let mut first = entry("Merge One", "/d/Merge One", Variant::Merged);
        first.replaces = vec!["Mod A".into()];
        loadout.put(first);
        let mut second = entry("Merge Two", "/d/Merge Two", Variant::Merged);
        second.replaces = vec!["Merge One".into()];
        loadout.put(second);

        let gone: Vec<String> = loadout
            .dissolve_merges_of("Mod A")
            .into_iter()
            .map(|e| e.owner)
            .collect();
        assert!(gone.contains(&"Merge One".to_string()));
        assert!(gone.contains(&"Merge Two".to_string()));
        assert_eq!(loadout.wanted().keys().collect::<Vec<_>>(), vec!["Mod A"]);
    }

    #[test]
    fn a_build_rewritten_in_place_reaches_the_game() {
        // The shape of an edit: the edited copy is rebuilt into the *same*
        // folder, under the *same* file names, and only its contents change.
        //
        // That made both of reconcile's "already correct" tests agree -- the
        // folder path had not moved, so `built_from` matched, and no file had
        // been added or removed, so the names matched -- and it reported
        // `unchanged`. The first edit to a mod reached the game; every one
        // after it was dropped without a word, while the screen went on saying
        // the value was set.
        let dir = Dir::new("rewritten");
        dir.file("derived/Cool Mod__edited/Cool Mod/SCENE.MBIN", "TransY 0.0");
        let mods = dir.0.join("MODS");
        std::fs::create_dir_all(&mods).unwrap();
        let built = dir.0.join("derived/Cool Mod__edited");

        let mut loadout = Loadout::default();
        loadout.put(entry("Cool Mod", &built.display().to_string(), Variant::Original));
        assert_eq!(reconcile(&mut loadout, &mods, false).deployed, vec!["Cool Mod"]);
        assert_eq!(
            std::fs::read_to_string(mods.join("Cool Mod/SCENE.MBIN")).unwrap(),
            "TransY 0.0"
        );

        // Edit again. Same folder, same name, new contents -- and the file is
        // replaced rather than written through, exactly as a rebuild does it,
        // because writing through a hardlink would rewrite the deployed copy
        // too and hide the very thing being tested.
        std::thread::sleep(std::time::Duration::from_millis(1100));
        let asset = built.join("Cool Mod/SCENE.MBIN");
        std::fs::remove_file(&asset).unwrap();
        std::fs::write(&asset, "TransY 24.280840").unwrap();

        let after = reconcile(&mut loadout, &mods, false);
        assert_eq!(after.deployed, vec!["Cool Mod"], "the rebuild has to be relinked");
        assert_eq!(
            std::fs::read_to_string(mods.join("Cool Mod/SCENE.MBIN")).unwrap(),
            "TransY 24.280840",
            "the game was left reading the build before the edit"
        );

        // And it settles: a third reconcile with nothing changed must not
        // churn the mods folder.
        let settled = reconcile(&mut loadout, &mods, false);
        assert_eq!(settled.unchanged, vec!["Cool Mod"]);
        assert!(settled.deployed.is_empty());
    }

    #[test]
    fn choosing_the_cleaned_build_swaps_which_files_the_game_sees() {
        let dir = Dir::new("swap");
        // Original ships a whole-file override; the cleaned build ships the
        // sparse patch instead. Different file names, same mod.
        dir.file("staging/Cool Mod/Cool Mod/TABLE.MBIN", "the whole file");
        dir.file("derived/Cool Mod/Cool Mod/TABLE.EXML", "<just the edits/>");
        let mods = dir.0.join("MODS");
        std::fs::create_dir_all(&mods).unwrap();

        let mut loadout = Loadout::default();
        loadout.put(entry(
            "Cool Mod",
            &dir.0.join("staging/Cool Mod").display().to_string(),
            Variant::Original,
        ));
        let done = reconcile(&mut loadout, &mods, false);
        assert_eq!(done.deployed, vec!["Cool Mod"]);
        assert!(mods.join("Cool Mod/TABLE.MBIN").exists());

        // Nothing changed: reconciling again must not churn the files.
        let again = reconcile(&mut loadout, &mods, false);
        assert_eq!(again.unchanged, vec!["Cool Mod"]);
        assert!(again.deployed.is_empty());

        loadout.put(entry(
            "Cool Mod",
            &dir.0.join("derived/Cool Mod").display().to_string(),
            Variant::Cleaned,
        ));
        let swapped = reconcile(&mut loadout, &mods, false);
        assert_eq!(swapped.deployed, vec!["Cool Mod"]);
        assert!(mods.join("Cool Mod/TABLE.EXML").exists());
        assert!(
            !mods.join("Cool Mod/TABLE.MBIN").exists(),
            "the override has to go, or it wins over the patch"
        );
        // The original was never touched, which is what makes undo free.
        assert!(dir.0.join("staging/Cool Mod/Cool Mod/TABLE.MBIN").exists());
    }

    #[test]
    fn mods_this_program_did_not_install_are_left_alone() {
        let dir = Dir::new("foreign");
        dir.file("MODS/Someone Elses Mod/A.EXML", "not ours");
        dir.file("staging/Ours/Ours/B.EXML", "ours");
        let mods = dir.0.join("MODS");

        let mut loadout = Loadout::default();
        loadout.put(entry(
            "Ours",
            &dir.0.join("staging/Ours").display().to_string(),
            Variant::Original,
        ));
        let done = reconcile(&mut loadout, &mods, false);

        assert_eq!(done.deployed, vec!["Ours"]);
        assert!(done.removed.is_empty());
        assert!(mods.join("Someone Elses Mod/A.EXML").exists());
    }

    #[test]
    fn a_dry_run_reports_without_touching_anything() {
        let dir = Dir::new("dry");
        dir.file("staging/Ours/Ours/B.EXML", "ours");
        let mods = dir.0.join("MODS");
        std::fs::create_dir_all(&mods).unwrap();

        let mut loadout = Loadout::default();
        loadout.put(entry(
            "Ours",
            &dir.0.join("staging/Ours").display().to_string(),
            Variant::Original,
        ));
        let done = reconcile(&mut loadout, &mods, true);
        assert_eq!(done.deployed, vec!["Ours"]);
        assert!(!mods.join("Ours").exists());
    }

    #[test]
    fn the_loadout_survives_a_round_trip_through_disk() {
        let dir = Dir::new("persist");
        let path = dir.0.join("loadout.json");
        let mut loadout = Loadout::default();
        let mut merged = entry("A+B", "/d/A+B", Variant::Merged);
        merged.replaces = vec!["Mod A".into()];
        loadout.put(merged);
        loadout.write(&path).unwrap();

        let back = Loadout::read(&path);
        assert_eq!(back.entries.len(), 1);
        assert_eq!(back.entries[0].variant, Variant::Merged);
        assert_eq!(back.entries[0].replaces, vec!["Mod A"]);
        assert!(back.entries[0].enabled, "enabled defaults to true");
    }

    #[test]
    fn notes_and_build_scripts_stay_out_of_the_game_folder() {
        let dir = Dir::new("inert");
        dir.file("staging/Cool Mod/Cool Mod/GLOBALS/A.EXML", "the asset");
        dir.file("staging/Cool Mod/Cool Mod.lua", "the AMUMSS script");
        dir.file("staging/Cool Mod/Installation Notes.txt", "for the user");
        let mods = dir.0.join("MODS");
        std::fs::create_dir_all(&mods).unwrap();

        let mut loadout = Loadout::default();
        loadout.put(entry(
            "Cool Mod",
            &dir.0.join("staging/Cool Mod").display().to_string(),
            Variant::Original,
        ));
        let done = reconcile(&mut loadout, &mods, false);

        assert_eq!(done.deployed, vec!["Cool Mod"]);
        assert!(mods.join("Cool Mod/GLOBALS/A.EXML").exists());
        assert!(!mods.join("Cool Mod.lua").exists());
        assert!(!mods.join("Installation Notes.txt").exists());
        // They are still in staging, which is where they were useful anyway.
        assert!(dir.0.join("staging/Cool Mod/Cool Mod.lua").exists());

        // And the comparison has to agree, or this would relink every time.
        let again = reconcile(&mut loadout, &mods, false);
        assert_eq!(again.unchanged, vec!["Cool Mod"]);
        assert!(again.deployed.is_empty());
    }

    #[test]
    fn two_mods_wanting_one_asset_both_still_install() {
        let dir = Dir::new("clash");
        dir.file("staging/A Mod/Shared/GLOBALS/T.EXML", "from A");
        dir.file("staging/A Mod/A Mod/GLOBALS/A.EXML", "A only");
        dir.file("staging/B Mod/Shared/GLOBALS/T.EXML", "from B");
        dir.file("staging/B Mod/B Mod/GLOBALS/B.EXML", "B only");
        let mods = dir.0.join("MODS");
        std::fs::create_dir_all(&mods).unwrap();

        let mut loadout = Loadout::default();
        for name in ["A Mod", "B Mod"] {
            loadout.put(entry(
                name,
                &dir.0.join(format!("staging/{name}")).display().to_string(),
                Variant::Original,
            ));
        }
        let done = reconcile(&mut loadout, &mods, false);

        assert_eq!(done.deployed, vec!["A Mod", "B Mod"]);
        assert!(done.problems.is_empty(), "{:?}", done.problems);
        assert_eq!(done.clashes.len(), 1);
        assert_eq!(done.clashes[0].keeper, "A Mod");
        assert_eq!(done.clashes[0].losers, vec!["B Mod"]);
        // The contested file has exactly one owner, and both mods' own
        // content arrived regardless.
        assert_eq!(
            std::fs::read_to_string(mods.join("Shared/GLOBALS/T.EXML")).unwrap(),
            "from A"
        );
        assert!(mods.join("A Mod/GLOBALS/A.EXML").exists());
        assert!(mods.join("B Mod/GLOBALS/B.EXML").exists());

        let again = reconcile(&mut loadout, &mods, false);
        assert!(again.deployed.is_empty(), "the keeper must not change");
    }

    #[test]
    fn a_contested_readme_is_not_a_clash_at_all() {
        let dir = Dir::new("readme");
        dir.file("staging/A Mod/A Mod/GLOBALS/A.EXML", "a");
        dir.file("staging/A Mod/README.txt", "A's notes");
        dir.file("staging/B Mod/B Mod/GLOBALS/B.EXML", "b");
        dir.file("staging/B Mod/README.txt", "B's notes");

        let wanted = BTreeMap::from([
            ("A Mod".to_string(), dir.0.join("staging/A Mod")),
            ("B Mod".to_string(), dir.0.join("staging/B Mod")),
        ]);
        assert!(clashes(&wanted).is_empty());
    }

    #[test]
    fn a_rebuilt_variant_reaches_the_game_even_when_the_file_names_match() {
        // The measured failure: mending a malformed `.EXML` changes one file's
        // contents and nothing else. Comparing names alone said "already
        // correct" and left the broken file in the game, while reporting that
        // the repair had worked.
        let dir = Dir::new("rebuilt");
        dir.file("staging/M/M/A.EXML", "<Data><Property name=\"A\"></Data>");
        dir.file("derived/M/M/A.EXML", "<Data><Property name=\"A\"></Property></Data>");
        let mods = dir.0.join("MODS");
        std::fs::create_dir_all(&mods).unwrap();

        let mut loadout = Loadout::default();
        let staged = dir.0.join("staging/M").display().to_string();
        loadout.put(entry("M", &staged, Variant::Original));
        reconcile(&mut loadout, &mods, false);
        assert!(!std::fs::read_to_string(mods.join("M/A.EXML"))
            .unwrap()
            .contains("</Property>"));

        // Switch to the mended build. Same file name, different contents.
        let mended = dir.0.join("derived/M").display().to_string();
        loadout.put(entry("M", &mended, Variant::Cleaned));
        let done = reconcile(&mut loadout, &mods, false);

        assert_eq!(done.deployed, vec!["M"], "the repair never reached the game");
        assert!(
            std::fs::read_to_string(mods.join("M/A.EXML"))
                .unwrap()
                .contains("</Property>"),
            "the game is still reading the broken build"
        );
        // And it settles: reconciling again must not churn the file.
        assert_eq!(reconcile(&mut loadout, &mods, false).unchanged, vec!["M"]);
    }

    #[test]
    fn switching_a_variant_does_not_lose_the_way_back_to_the_original() {
        // Moving `source` to a derived build must not be a one-way door: undo
        // needs somewhere to point, and a second repair must read the mod as
        // shipped rather than the build it already produced.
        let mut loadout = Loadout::default();
        let mut first = entry("M", "/staging/M", Variant::Original);
        first.origin = Some("/staging/M".into());
        loadout.put(first);

        loadout.put(entry("M", "/derived/M", Variant::Cleaned));
        let now = loadout.get("M").unwrap();
        assert_eq!(now.source, "/derived/M");
        assert_eq!(
            now.origin_path(),
            PathBuf::from("/staging/M"),
            "the original was forgotten"
        );

        // Undo goes back to it.
        let back = now.origin_path().display().to_string();
        loadout.put(entry("M", &back, Variant::Original));
        assert_eq!(loadout.get("M").unwrap().source, "/staging/M");
    }

    #[test]
    fn an_entry_recorded_before_origins_were_tracked_still_has_one() {
        // Older loadouts have no origin. They are on their original build by
        // definition, so `source` is the right answer rather than a failure.
        let older = entry("M", "/staging/M", Variant::Original);
        assert_eq!(older.origin, None);
        assert_eq!(older.origin_path(), PathBuf::from("/staging/M"));
    }

    #[test]
    fn switching_presets_does_not_undo_a_repair() {
        // A preset only says which mods are on. Turning one off and back on
        // has to bring back the build the user chose -- the mended one --
        // not send them round the "fix it" loop again every time.
        let dir = Dir::new("presetfix");
        dir.file("staging/M/M/A.EXML", "<Data><Property name=\"A\"></Data>");
        dir.file("derived/M/M/A.EXML", "<Data><Property name=\"A\"></Property></Data>");
        let mods = dir.0.join("MODS");
        std::fs::create_dir_all(&mods).unwrap();

        let mut loadout = Loadout::default();
        loadout.put(entry(
            "M",
            &dir.0.join("derived/M").display().to_string(),
            Variant::Cleaned,
        ));
        reconcile(&mut loadout, &mods, false);

        // A preset that leaves it out.
        loadout.entries[0].enabled = false;
        assert_eq!(reconcile(&mut loadout, &mods, false).removed, vec!["M"]);
        assert!(!mods.join("M").exists());

        // And one that includes it again.
        loadout.entries[0].enabled = true;
        assert_eq!(reconcile(&mut loadout, &mods, false).deployed, vec!["M"]);
        assert!(
            std::fs::read_to_string(mods.join("M/A.EXML"))
                .unwrap()
                .contains("</Property>"),
            "the fix was lost by switching presets"
        );
        assert_eq!(loadout.get("M").unwrap().variant, Variant::Cleaned);
    }

    #[test]
    fn a_loadout_saved_by_a_windows_tool_still_loads() {
        // Notepad and PowerShell both write UTF-8 with a byte-order mark, and
        // serde_json refuses the file outright. Because reading falls back to
        // "nothing recorded", that silently emptied a 62-mod loadout during
        // the cutover and made the whole library look unmanaged.
        let dir = Dir::new("bom");
        let path = dir.0.join("loadout.json");
        let mut saved = Loadout::default();
        saved.put(entry("Cool Mod", "/s/Cool Mod", Variant::Original));
        let json = serde_json::to_string(&saved).unwrap();
        std::fs::write(&path, format!("\u{feff}{json}")).unwrap();

        let back = Loadout::read(&path);
        assert_eq!(back.entries.len(), 1, "a BOM must not empty the mod list");
        assert_eq!(back.entries[0].owner, "Cool Mod");
    }

    #[test]
    fn a_missing_loadout_file_is_an_empty_one_not_a_crash() {
        let dir = Dir::new("missing");
        assert!(Loadout::read(&dir.0.join("nope.json")).entries.is_empty());
    }
}
