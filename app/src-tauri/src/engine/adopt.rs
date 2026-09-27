//! Taking over a mods folder another program deployed, without moving a byte.
//!
//! Mod managers deploy by **hardlink**: the real files live in a staging folder
//! and the game gets second names for them. That is the same arrangement this
//! program uses, which makes the cutover a bookkeeping exercise rather than a
//! migration. Nothing is copied, nothing is moved, and nothing is deleted --
//! we simply write down which staged folder each deployed mod came from, and
//! from then on we are the one maintaining those links.
//!
//! # How we work out where a mod came from
//!
//! Two ways, and the order matters.
//!
//! **The disk itself, first.** A hardlink and its original are the same file,
//! and the file system says so: they share a `(volume, index)` pair that no
//! copy of the same bytes shares. So indexing the staging folder by
//! [`fileid`](super::fileid) and looking up what is in the game answers "which
//! staged folder is this" as a fact rather than an inference. It needs no
//! manifest, no manager to be installed, and no naming convention -- and it
//! cannot be stale, because it is reading the very links the game is loading.
//!
//! **A Vortex manifest, only as a shortcut.** This used to be the *only* way,
//! which was wrong twice over: it assumed everyone arrives from Vortex, and it
//! stopped working the moment Vortex was uninstalled and took its manifest
//! with it -- leaving a mods folder full of perfectly good hardlinks that
//! nothing could read. It is still consulted first when present, because it is
//! one file to read instead of a walk of two trees, but nothing depends on it.
//!
//! **When the files were copied rather than linked** there is no shared
//! identity to find -- some managers copy, and this program's own `deploy`
//! falls back to copying when staging and the game are on different drives.
//! Those are matched on content instead (name and size, decided in aggregate),
//! which is a weaker signal, so a mod matched that way says so.
//!
//! # What this refuses to assume
//!
//! Every mod is checked before it is adopted: the staged folder must still
//! exist, and the files deployed under the mod's names must be the same set
//! the staged folder holds. A mod that fails either test is reported and left
//! alone rather than adopted on a guess, because the loadout's whole job is to
//! know what is in the game, and an entry that is wrong about that is worse
//! than no entry at all.
//!
//! # This does not stop the other manager
//!
//! Vortex still believes it owns these files and will redeploy or purge them
//! on its next deploy. Adopting is one half of the cutover; telling Vortex to
//! stop managing the game is the other, and only the user can do that.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::deploy;
use super::fileid::{self, FileId};
use super::hostenv::VORTEX_MANIFEST;
use super::loadout::{Entry, Loadout, Variant};

/// One mod, as the other manager left it.
#[derive(Debug, Clone, Serialize)]
pub struct Candidate {
    /// what the library calls it: the mod folder the game loads
    pub owner: String,
    /// the staging folder holding the real files
    pub source: String,
    /// the names it put in the mods folder
    pub deployed: Vec<String>,
    pub files: usize,
    /// why it cannot be adopted, when it cannot
    pub refused: Option<String>,
    /// harmless differences worth mentioning
    pub notes: Vec<String>,
}

impl Candidate {
    pub fn adoptable(&self) -> bool {
        self.refused.is_none()
    }
}

/// What a cutover would do.
#[derive(Debug, Clone, Serialize)]
pub struct Survey {
    pub manager: Option<String>,
    pub staging: Option<String>,
    pub candidates: Vec<Candidate>,
    /// mod folders in the game that nothing in staging accounts for
    pub unmanaged: Vec<String>,
    /// Mods running a build **this program** made: cleaned, mended or merged.
    ///
    /// Their deployed names link into `derived/`, not into staging, so tracing
    /// them lands on our own output rather than on any mod's source. They are
    /// reported separately rather than left among `unmanaged`, where they would
    /// read as "installed by hand", and rather than adopted, because adopting
    /// one means guessing both which staged folder it was built from and which
    /// of the three builds it is -- and a loadout entry that is wrong about
    /// what is in the game is worse than no entry at all.
    ///
    /// A mod is only ever in this state because there *was* a loadout. The
    /// answer for it is that loadout, not adoption.
    pub ours: Vec<String>,
}

impl Survey {
    pub fn ready(&self) -> usize {
        self.candidates.iter().filter(|c| c.adoptable()).count()
    }

    pub fn refused(&self) -> usize {
        self.candidates.len() - self.ready()
    }
}

fn top(rel: &str) -> String {
    rel.replace('/', "\\")
        .split('\\')
        .next()
        .unwrap_or("")
        .to_string()
}

/// Every file under `root`, relative to it.
fn files_under(root: &Path, prefix: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = prefix.join(entry.file_name());
        if path.is_dir() {
            files_under(&path, &rel, out);
        } else {
            out.push(rel);
        }
    }
}

/// Names a manager leaves behind that are its own bookkeeping, not the mod.
fn is_managers_own(name: &str) -> bool {
    name.eq_ignore_ascii_case("__folder_managed_by_vortex")
}

/// Read a Vortex manifest, if there is one to read.
///
/// `None` for every way it can be absent or useless, because every one of them
/// means the same thing to the caller: work it out from the disk instead.
fn from_manifest(mods_dir: &Path) -> Option<Sources> {
    let text = std::fs::read_to_string(mods_dir.join(VORTEX_MANIFEST)).ok()?;
    let data: serde_json::Value = serde_json::from_str(&text).ok()?;
    let files = data.get("files")?.as_array()?;

    let mut by_source: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in files {
        let (Some(rel), Some(source)) = (
            entry.get("relPath").and_then(|v| v.as_str()),
            entry.get("source").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let head = top(rel);
        if head.is_empty() {
            continue;
        }
        let names = by_source.entry(source.to_string()).or_default();
        if !names.contains(&head) {
            names.push(head);
        }
    }
    if by_source.is_empty() {
        // A manifest that claims nothing is not a shortcut, it is an empty
        // file. Fall through and read the disk.
        return None;
    }

    Some(Sources {
        manager: Some("Vortex".to_string()),
        staging_root: data
            .get("stagingPath")
            .and_then(|v| v.as_str())
            .map(PathBuf::from),
        by_source,
        guessed: HashSet::new(),
        ours: Vec::new(),
    })
}

/// Every file under `root`, keyed by which file on disk it actually is.
///
/// Folders whose files cannot be identified simply contribute nothing; see
/// [`fileid::of`].
fn index_by_identity(root: &Path) -> (HashMap<FileId, String>, HashSet<FileId>) {
    let mut owner_of: HashMap<FileId, String> = HashMap::new();
    let mut shared: HashSet<FileId> = HashSet::new();

    let Ok(entries) = std::fs::read_dir(root) else {
        return (owner_of, shared);
    };
    for entry in entries.flatten() {
        let at = entry.path();
        if !at.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let mut files = Vec::new();
        files_under(&at, Path::new(""), &mut files);
        for rel in files {
            let Some(id) = fileid::of(&at.join(&rel)) else {
                continue;
            };
            match owner_of.get(&id) {
                // One file hardlinked into two staging folders. Rare, and it
                // says nothing about which one a deployed name came from, so
                // it is struck out rather than allowed to cast a vote.
                Some(already) if already != &name => {
                    shared.insert(id);
                }
                Some(_) => {}
                None => {
                    owner_of.insert(id, name.clone());
                }
            }
        }
    }
    (owner_of, shared)
}

/// The same index, on content rather than identity: file name and size.
///
/// The fallback for mods that were **copied** into the game rather than linked
/// -- some managers copy, and this program's own deploy copies when staging and
/// the game are on different drives. Far weaker per file: two mods shipping the
/// same vanilla asset collide here, where they could not collide by identity.
/// It is only ever read in aggregate, and a mod attributed this way is marked
/// so the user is told it was matched on content.
fn index_by_content(root: &Path) -> (HashMap<(String, u64), String>, HashSet<(String, u64)>) {
    let mut owner_of: HashMap<(String, u64), String> = HashMap::new();
    let mut shared: HashSet<(String, u64)> = HashSet::new();

    let Ok(entries) = std::fs::read_dir(root) else {
        return (owner_of, shared);
    };
    for entry in entries.flatten() {
        let at = entry.path();
        if !at.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        let mut files = Vec::new();
        files_under(&at, Path::new(""), &mut files);
        for rel in files {
            let Some(key) = content_key(&at.join(&rel), &rel) else {
                continue;
            };
            match owner_of.get(&key) {
                Some(already) if already != &name => {
                    shared.insert(key);
                }
                Some(_) => {}
                None => {
                    owner_of.insert(key, name.clone());
                }
            }
        }
    }
    (owner_of, shared)
}

/// A file's name and size, which is as much as a copy preserves.
///
/// The *relative* path is deliberately not used: a staged mod holds its files
/// under an inner folder of its own, and the game holds them under a different
/// one, so the two agree on the leaf and nothing above it.
fn content_key(at: &Path, rel: &Path) -> Option<(String, u64)> {
    let name = rel.file_name()?.to_string_lossy().to_uppercase();
    let size = std::fs::metadata(at).ok()?.len();
    Some((name, size))
}

/// The top-level names in the mods folder, and the files under each.
fn deployed_names(mods_dir: &Path) -> Vec<(String, Vec<PathBuf>)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(mods_dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == VORTEX_MANIFEST || is_managers_own(&name) {
            continue;
        }
        let at = entry.path();
        let mut files = Vec::new();
        if at.is_dir() {
            files_under(&at, Path::new(""), &mut files);
        } else {
            // A mod that deploys a loose file rather than a folder. Its "list
            // of files under it" is itself, at an empty relative path.
            files.push(PathBuf::new());
        }
        out.push((name, files));
    }
    out
}

/// Whichever source most of a deployed folder's files came from.
///
/// A plain majority, not a plurality: more than half of the files we could
/// identify have to agree. One shared asset pointing at some other mod is not
/// evidence of anything, and attributing a folder on a single vote is how a
/// loadout ends up recording something untrue about the game -- which is the
/// one thing a loadout must never do.
fn winner(votes: HashMap<String, usize>, identified: usize) -> Option<String> {
    let (source, count) = votes.into_iter().max_by_key(|(_, n)| *n)?;
    (count * 2 > identified).then_some(source)
}

/// Work out where each deployed mod came from by reading the disk.
///
/// No manifest, no manager, no naming convention: see the module note.
fn from_disk(
    mods_dir: &Path,
    staging_root: Option<&Path>,
    derived_root: Option<&Path>,
) -> Result<Sources, String> {
    let root = staging_root.ok_or_else(|| {
        format!(
            "nothing in {} was deployed by a mod manager this program can read, \
             and there is no staging folder set to match it against",
            mods_dir.display()
        )
    })?;
    if !root.is_dir() {
        return Err(format!(
            "the staging folder {} is not there, so there is nothing to take these over from",
            root.display()
        ));
    }

    let (by_id, shared_ids) = index_by_identity(root);

    // Our own builds, indexed the same way.
    //
    // A cleaned, mended or merged mod is deployed from `derived`, not from
    // staging, so tracing it lands on this program's own output. Recognising
    // that is not a nicety: without it such a mod falls through to the content
    // fallback, where enough of its files are still byte-identical to the
    // staged original to win the vote -- and it would be written down as
    // running the author's build when it is running ours. Measured on the real
    // library: three mods, all of them cleaned, all of them about to be
    // recorded wrongly.
    let (by_derived, shared_derived) = match derived_root {
        Some(d) if d.is_dir() => index_by_identity(d),
        _ => (HashMap::new(), HashSet::new()),
    };

    let deployed = deployed_names(mods_dir);

    // Identity first for every mod, and content only for the ones identity
    // could not place -- so the weaker signal never overrides the stronger one.
    let mut by_source: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut guessed: HashSet<String> = HashSet::new();
    let mut ours: Vec<String> = Vec::new();
    let mut unplaced: Vec<(String, Vec<PathBuf>)> = Vec::new();

    for (name, files) in deployed {
        let at = mods_dir.join(&name);
        let mut votes: HashMap<String, usize> = HashMap::new();
        let mut mine: HashMap<String, usize> = HashMap::new();
        let mut identified = 0usize;
        for rel in &files {
            let path = if rel.as_os_str().is_empty() {
                at.clone()
            } else {
                at.join(rel)
            };
            let Some(id) = fileid::of(&path) else { continue };
            identified += 1;
            if !shared_derived.contains(&id) {
                if let Some(build) = by_derived.get(&id) {
                    *mine.entry(build.clone()).or_default() += 1;
                }
            }
            if shared_ids.contains(&id) {
                continue;
            }
            if let Some(source) = by_id.get(&id) {
                *votes.entry(source.clone()).or_default() += 1;
            }
        }

        // Ours wins over staging when both match, which happens for a mod only
        // partly rebuilt: whichever files the build did not touch are still
        // links to the staged original. What the game is loading is the build,
        // so the build is the answer.
        if winner(mine, identified).is_some() {
            ours.push(name);
            continue;
        }
        match winner(votes, identified) {
            Some(source) => by_source.entry(source).or_default().push(name),
            None => unplaced.push((name, files)),
        }
    }

    // Only now, and only for what is left: walking the staging tree a second
    // time costs nothing on a library that was deployed by links, which is
    // almost all of them.
    if !unplaced.is_empty() {
        let (by_content, shared_content) = index_by_content(root);
        for (name, files) in unplaced {
            let at = mods_dir.join(&name);
            let mut votes: HashMap<String, usize> = HashMap::new();
            let mut identified = 0usize;
            for rel in &files {
                let (path, leaf) = if rel.as_os_str().is_empty() {
                    (at.clone(), PathBuf::from(&name))
                } else {
                    (at.join(rel), rel.clone())
                };
                let Some(key) = content_key(&path, &leaf) else {
                    continue;
                };
                identified += 1;
                if shared_content.contains(&key) {
                    continue;
                }
                if let Some(source) = by_content.get(&key) {
                    *votes.entry(source.clone()).or_default() += 1;
                }
            }
            if let Some(source) = winner(votes, identified) {
                guessed.insert(name.clone());
                by_source.entry(source).or_default().push(name);
            }
            // Still nothing: it is not from this staging folder at all. It
            // falls through to `unmanaged`, which is the honest answer.
        }
    }

    Ok(Sources {
        manager: None,
        staging_root: Some(root.to_path_buf()),
        by_source,
        guessed,
        ours,
    })
}

/// Where the source-to-deployed mapping came from, before any of it is checked.
///
/// Split out so that *how we worked out where a mod came from* and *whether it
/// is safe to take over* are two separate pieces of code. The second half used
/// to be welded to the Vortex manifest parser, so the day the manifest was not
/// there the verification could not run either -- on a mods folder whose
/// hardlinks were all still perfectly readable.
struct Sources {
    /// the program that deployed this, when we can name it
    manager: Option<String>,
    staging_root: Option<PathBuf>,
    /// staging folder name -> the top-level names it put in the mods folder
    by_source: BTreeMap<String, Vec<String>>,
    /// mods matched on content rather than on file identity, which is weaker
    guessed: HashSet<String>,
    /// mods whose deployed names link into our own `derived` folder
    ours: Vec<String>,
}

/// Look at the mods folder and work out what could be taken over.
///
/// `staging_root` is where the real mod files are expected to live. It is only
/// a starting point: a Vortex manifest names its own and that wins, because a
/// folder someone else filled is not necessarily the one this program would
/// have chosen.
///
/// Read-only. Nothing is written until [`adopt`] is called.
pub fn survey(
    mods_dir: &Path,
    staging_root: Option<&Path>,
    derived_root: Option<&Path>,
) -> Result<Survey, String> {
    // The manifest first only because it is one file to read rather than a walk
    // of two trees. Everything works without it.
    let found = match from_manifest(mods_dir) {
        Some(from_vortex) => from_vortex,
        None => from_disk(mods_dir, staging_root, derived_root)?,
    };

    let Sources {
        manager,
        staging_root,
        by_source,
        guessed,
        ours,
    } = found;
    let staging = staging_root.as_ref().map(|p| p.display().to_string());
    let mut candidates = Vec::new();
    let mut claimed: Vec<String> = Vec::new();

    for (source, mut deployed) in by_source {
        deployed.sort();
        claimed.extend(deployed.iter().cloned());

        // The mod folder the game loads is the display name; a mod that
        // deploys only loose files keeps the staging folder's name.
        let owner = deployed
            .iter()
            .find(|n| mods_dir.join(n).is_dir())
            .cloned()
            .unwrap_or_else(|| source.clone());

        let Some(root) = staging_root.as_ref().map(|r| r.join(&source)) else {
            candidates.push(Candidate {
                owner,
                source: source.clone(),
                deployed,
                files: 0,
                refused: Some("there is no staging folder to take it over from".into()),
                notes: Vec::new(),
            });
            continue;
        };

        if !root.is_dir() {
            candidates.push(Candidate {
                owner,
                source: root.display().to_string(),
                deployed,
                files: 0,
                refused: Some(format!("{} is not there any more", root.display())),
                notes: Vec::new(),
            });
            continue;
        }

        // The staged folder and what is deployed must hold the same files, or
        // we would be recording something untrue about the game.
        let mut staged = Vec::new();
        files_under(&root, Path::new(""), &mut staged);
        let staged_set: BTreeSet<PathBuf> = staged
            .into_iter()
            .filter(|p| {
                !p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(is_managers_own)
            })
            .collect();

        let mut live = BTreeSet::new();
        for name in &deployed {
            let at = mods_dir.join(name);
            if at.is_dir() {
                let mut found = Vec::new();
                files_under(&at, Path::new(name), &mut found);
                live.extend(
                    found.into_iter().filter(|p| {
                        !p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(is_managers_own)
                    }),
                );
            } else if at.is_file() {
                live.insert(PathBuf::from(name));
            }
        }

        // What differs, and whether it matters.
        //
        // A file in the game with no staged copy is serious: removing the mod
        // would delete something nothing has a copy of. A *game asset* staged
        // but not deployed is serious the other way -- redeploying would change
        // what loads. A readme that was staged and never deployed is neither,
        // and refusing a mod over one would leave it unmanaged for nothing.
        // One of the 62 measured mods is exactly that case.
        let mut notes = Vec::new();
        // Said before anything else about this mod, because it qualifies
        // everything that follows: the checks below are exactly as sound as the
        // match they are checking, and this one was made on content.
        if deployed.iter().any(|n| guessed.contains(n)) {
            notes.push(
                "matched by file name and size, not by file identity -- these were copied                  into the game rather than linked, so this is a strong guess rather than a                  fact"
                    .to_string(),
            );
        }
        let unstaged: Vec<&PathBuf> = live.difference(&staged_set).collect();
        let undeployed: Vec<&PathBuf> = staged_set.difference(&live).collect();
        // `deploy::is_game_content` rather than a list of our own, because
        // "would the game read this" is a question `deploy` already answers and
        // is the only one that can answer it correctly: whatever it declines to
        // deploy *cannot* be in the game, so counting such a file as missing
        // refuses a mod for a state that is not reachable.
        //
        // These two rules had drifted apart, and it cost most of a real
        // library. This one counted `.lua` as a game asset; `deploy` lists it
        // as inert and has never deployed one. Nearly every AMUMSS mod ships
        // its build script beside its output, so 24 of 54 traced mods were
        // refused for "1 staged game file is not installed" -- the script, which
        // would not have been installed whoever deployed it.
        let undeployed_assets = undeployed
            .iter()
            .filter(|p| super::deploy::is_game_content(p))
            .count();

        let refused = if live.is_empty() {
            Some("none of its files are in the mods folder".to_string())
        } else if !unstaged.is_empty() {
            Some(format!(
                "{} installed file(s) have no staged copy, so removing it would lose them",
                unstaged.len()
            ))
        } else if undeployed_assets > 0 {
            Some(format!(
                "{undeployed_assets} staged game file(s) are not installed, so the staged \
                 copy is not what the game is loading"
            ))
        } else {
            if !undeployed.is_empty() {
                notes.push(format!(
                    "{} staged file(s) were never deployed and would appear if it is \
                     reinstalled: {}",
                    undeployed.len(),
                    undeployed
                        .iter()
                        .take(3)
                        .map(|p| p.to_string_lossy().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            None
        };

        candidates.push(Candidate {
            owner,
            source: root.display().to_string(),
            files: live.len(),
            deployed,
            refused,
            notes,
        });
    }

    // Anything in the mods folder no manager claims: left alone, but worth
    // saying so, because a cutover does not put those under our care either.
    let mut unmanaged = Vec::new();
    if let Ok(entries) = std::fs::read_dir(mods_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == VORTEX_MANIFEST || claimed.contains(&name) || ours.contains(&name) {
                continue;
            }
            unmanaged.push(name);
        }
    }
    unmanaged.sort();

    candidates.sort_by(|a, b| a.owner.cmp(&b.owner));
    let mut ours = ours;
    ours.sort();
    Ok(Survey {
        manager,
        staging,
        candidates,
        unmanaged,
        ours,
    })
}

/// A mod running one of our builds, and the author's copy to put it back on.
#[derive(Debug, Clone, Serialize)]
pub struct Restorable {
    pub owner: String,
    /// the staging folder that deploys this name, when exactly one does
    pub source: Option<String>,
    /// the names it currently has in the game, which are links into `derived`
    pub deployed: Vec<String>,
    /// why the author's build cannot be found, when it cannot
    pub why_not: Option<String>,
}

impl Restorable {
    pub fn ready(&self) -> bool {
        self.source.is_some()
    }
}

/// Which staging folder deploys a given name.
///
/// Not a guess and not a name heuristic: `deploy` links a staging folder's
/// **top-level entries** into the mods folder under exactly those names, so
/// "the staging folder holding a top-level entry called `owner`" *is* the
/// definition of where `owner` comes from. It is the same rule read backwards.
///
/// Exactly one, or none: two staging folders offering the same name is a
/// genuine ambiguity, and guessing between them would put the wrong mod in the
/// game under the right name -- which is worse than saying so.
fn deployers_of(staging_root: &Path) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let Ok(folders) = std::fs::read_dir(staging_root) else {
        return out;
    };
    for folder in folders.flatten() {
        let at = folder.path();
        if !at.is_dir() {
            continue;
        }
        let source = folder.file_name().to_string_lossy().to_string();
        let Ok(inner) = std::fs::read_dir(&at) else {
            continue;
        };
        for item in inner.flatten() {
            let name = item.file_name().to_string_lossy().to_string();
            // Only what would actually be deployed: a staged readme is not a
            // mod folder, and letting one claim a name would have `Notes.txt`
            // deploy half the library.
            if !deploy::is_game_content(Path::new(&name)) && !item.path().is_dir() {
                continue;
            }
            out.entry(name).or_default().push(source.clone());
        }
    }
    out
}

/// Work out how to put mods running one of our builds back on the author's.
///
/// The answer to [`Survey::ours`]. Those mods cannot be adopted as they stand,
/// because adopting one means guessing which of three builds it is on -- but
/// they do not have to be: the author's copy is still staged and untouched, and
/// putting them back on it makes the question go away entirely. A mod on its
/// author's build is an ordinary mod.
///
/// Read-only. [`restore`] is what acts on this.
pub fn restorable(staging_root: Option<&Path>, mods_dir: &Path, ours: &[String]) -> Vec<Restorable> {
    let deployers = staging_root.map(deployers_of).unwrap_or_default();
    ours.iter()
        .map(|owner| {
            let deployed = deployed_top_level(mods_dir, owner);
            match deployers.get(owner).map(Vec::as_slice) {
                Some([one]) => Restorable {
                    owner: owner.clone(),
                    source: Some(one.clone()),
                    deployed,
                    why_not: None,
                },
                Some(several) => Restorable {
                    owner: owner.clone(),
                    source: None,
                    deployed,
                    why_not: Some(format!(
                        "{} staged mods deploy a folder called {owner}, so which one \
                         the game is running cannot be told apart: {}",
                        several.len(),
                        several.join(", ")
                    )),
                },
                _ => Restorable {
                    owner: owner.clone(),
                    source: None,
                    deployed,
                    why_not: Some(
                        "no staged mod deploys a folder by this name -- the author's copy \
                         is not in your staging folder, so there is nothing to go back to"
                            .into(),
                    ),
                },
            }
        })
        .collect()
}

/// The names this mod currently occupies in the game.
///
/// A mod is often several things -- a folder, a `.lua`, a readme -- and only
/// the ones actually there count, because these are handed to `undeploy`.
fn deployed_top_level(mods_dir: &Path, owner: &str) -> Vec<String> {
    let mut out = Vec::new();
    if mods_dir.join(owner).exists() {
        out.push(owner.to_string());
    }
    out
}

/// What restoring did, or would do.
#[derive(Debug, Clone, Serialize)]
pub struct Restoration {
    /// mods now running the build their author shipped
    pub restored: Vec<String>,
    /// mods left alone, and why
    pub skipped: Vec<(String, String)>,
    pub problems: Vec<String>,
}

/// Put mods running one of our builds back on the author's, and adopt them.
///
/// Two things at once, deliberately: relinking without writing the loadout
/// would leave the game holding mods nothing is responsible for, which is the
/// state this whole module exists to get out of.
///
/// The relink itself is [`loadout::reconcile`] rather than a second copy of
/// deploy-and-undeploy. The entry is written with `built_from` pointing at the
/// *derived* build it is on now, which is what tells reconcile the names in the
/// game are stale and have to be taken out before the staged copy goes in --
/// without it, the names already exist and the deploy refuses on the first one.
pub fn restore(
    plan: &[Restorable],
    staging_root: &Path,
    derived_root: &Path,
    mods_dir: &Path,
    book_path: &Path,
    dry_run: bool,
) -> Result<Restoration, String> {
    let mut book = Loadout::read(book_path);
    let mut out = Restoration {
        restored: Vec::new(),
        skipped: Vec::new(),
        problems: Vec::new(),
    };

    for item in plan {
        let Some(source) = item.source.as_ref() else {
            out.skipped.push((
                item.owner.clone(),
                item.why_not.clone().unwrap_or_else(|| "cannot be traced".into()),
            ));
            continue;
        };
        if book.get(&item.owner).is_some() {
            out.skipped
                .push((item.owner.clone(), "already in your mod list".into()));
            continue;
        }
        let staged = staging_root.join(source);
        if !staged.is_dir() {
            out.skipped
                .push((item.owner.clone(), format!("{} is gone", staged.display())));
            continue;
        }
        book.put(Entry {
            owner: item.owner.clone(),
            source: staged.display().to_string(),
            origin: Some(staged.display().to_string()),
            archive: None,
            variant: Variant::Original,
            replaces: Vec::new(),
            deployed: item.deployed.clone(),
            // The build it is on *now*. See the note above: this is what makes
            // reconcile take the derived links out first.
            built_from: Some(derived_root.join(&item.owner).display().to_string()),
            enabled: true,
            edited: false,
        });
        out.restored.push(item.owner.clone());
    }

    if out.restored.is_empty() {
        return Ok(out);
    }

    let changes = super::loadout::reconcile(&mut book, mods_dir, dry_run);
    out.problems.extend(changes.problems);
    if !dry_run {
        book.write(book_path)?;
    }
    Ok(out)
}

/// Write the survey's adoptable mods into the loadout.
///
/// Changes nothing on disk in the game or in staging: the links that are
/// already there stay exactly as they are. Existing entries are left alone, so
/// running this twice is harmless and a mod this program installed itself is
/// never overwritten by a stale manifest.
pub fn adopt(survey: &Survey, loadout_path: &Path) -> Result<usize, String> {
    let mut book = Loadout::read(loadout_path);
    let mut taken = 0;
    for candidate in survey.candidates.iter().filter(|c| c.adoptable()) {
        if book.get(&candidate.owner).is_some() {
            continue;
        }
        book.put(Entry {
            owner: candidate.owner.clone(),
            source: candidate.source.clone(),
            origin: Some(candidate.source.clone()),
            // Adopted from another manager, so we never saw a download.
            archive: None,
            variant: Variant::Original,
            replaces: Vec::new(),
            deployed: candidate.deployed.clone(),
            built_from: Some(candidate.source.clone()),
            enabled: true,
            edited: false,
        });
        taken += 1;
    }
    book.write(loadout_path)?;
    Ok(taken)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let path = std::env::current_dir()
                .unwrap_or_else(|_| std::env::temp_dir())
                .join(format!("target/nmscheck_adopt_{tag}"));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Dir(path)
        }

        fn file(&self, rel: &str, body: &str) {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }

        fn manifest(&self, staging: &str, rows: &[(&str, &str)]) {
            let files: Vec<String> = rows
                .iter()
                .map(|(rel, source)| {
                    format!(
                        "{{\"relPath\":\"{}\",\"source\":\"{}\"}}",
                        rel.replace('\\', "\\\\"),
                        source
                    )
                })
                .collect();
            self.file(
                &format!("MODS/{VORTEX_MANIFEST}"),
                &format!(
                    "{{\"stagingPath\":\"{}\",\"files\":[{}]}}",
                    staging.replace('\\', "\\\\"),
                    files.join(",")
                ),
            );
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A mod deployed exactly as Vortex leaves one: folder, script, readme.
    fn one_good_mod(dir: &Dir) -> (PathBuf, PathBuf) {
        let staging = dir.0.join("staging");
        let mods = dir.0.join("MODS");
        for (rel, body) in [
            ("staging/Shelters 1.3-2308/Shelters 1.4/GLOBALS/A.EXML", "asset"),
            ("staging/Shelters 1.3-2308/Shelters 1.4.lua", "script"),
            ("staging/Shelters 1.3-2308/Notes.txt", "readme"),
            ("MODS/Shelters 1.4/GLOBALS/A.EXML", "asset"),
            ("MODS/Shelters 1.4.lua", "script"),
            ("MODS/Notes.txt", "readme"),
        ] {
            dir.file(rel, body);
        }
        dir.manifest(
            &staging.display().to_string(),
            &[
                (r"Shelters 1.4\GLOBALS\A.EXML", "Shelters 1.3-2308"),
                (r"Shelters 1.4.lua", "Shelters 1.3-2308"),
                (r"Notes.txt", "Shelters 1.3-2308"),
            ],
        );
        (staging, mods)
    }

    #[test]
    fn a_mod_deployed_by_vortex_is_adoptable_as_it_stands() {
        let dir = Dir::new("good");
        let (_staging, mods) = one_good_mod(&dir);

        let survey = survey(&mods, None, None).unwrap();
        assert_eq!(survey.ready(), 1);
        assert_eq!(survey.refused(), 0);

        let mod_entry = &survey.candidates[0];
        // Named after the folder the game loads, not the archive.
        assert_eq!(mod_entry.owner, "Shelters 1.4");
        assert_eq!(
            mod_entry.deployed,
            vec!["Notes.txt", "Shelters 1.4", "Shelters 1.4.lua"],
            "all three siblings have to come across, or removing it strands two"
        );
    }

    #[test]
    fn adopting_writes_the_loadout_and_touches_nothing_else() {
        let dir = Dir::new("write");
        let (_staging, mods) = one_good_mod(&dir);
        let book = dir.0.join("loadout.json");

        let survey = survey(&mods, None, None).unwrap();
        assert_eq!(adopt(&survey, &book).unwrap(), 1);

        let read_back = Loadout::read(&book);
        let entry = read_back.get("Shelters 1.4").unwrap();
        assert_eq!(entry.variant, Variant::Original);
        assert_eq!(entry.deployed.len(), 3);

        // Every file is exactly where it was.
        assert!(mods.join("Shelters 1.4/GLOBALS/A.EXML").exists());
        assert!(mods.join("Shelters 1.4.lua").exists());
        assert!(dir.0.join("staging/Shelters 1.3-2308/Notes.txt").exists());
    }

    #[test]
    fn adopting_twice_changes_nothing_the_second_time() {
        let dir = Dir::new("twice");
        let (_staging, mods) = one_good_mod(&dir);
        let book = dir.0.join("loadout.json");
        let survey = survey(&mods, None, None).unwrap();

        assert_eq!(adopt(&survey, &book).unwrap(), 1);
        assert_eq!(adopt(&survey, &book).unwrap(), 0);
        assert_eq!(Loadout::read(&book).entries.len(), 1);
    }

    #[test]
    fn a_mod_whose_staged_copy_is_gone_is_refused_not_guessed() {
        let dir = Dir::new("nostage");
        let (staging, mods) = one_good_mod(&dir);
        std::fs::remove_dir_all(staging.join("Shelters 1.3-2308")).unwrap();

        let survey = survey(&mods, None, None).unwrap();
        assert_eq!(survey.ready(), 0);
        assert!(survey.candidates[0]
            .refused
            .as_deref()
            .unwrap()
            .contains("not there any more"));
    }

    #[test]
    fn a_readme_that_was_never_deployed_is_a_note_not_a_refusal() {
        // Real case: one of the 62 mods staged a README.txt that Vortex never
        // deployed. Its game files match exactly, so it is fine to take over.
        let dir = Dir::new("readme");
        let (_staging, mods) = one_good_mod(&dir);
        dir.file("staging/Shelters 1.3-2308/README.txt", "never deployed");

        let survey = survey(&mods, None, None).unwrap();
        assert_eq!(survey.ready(), 1, "{:?}", survey.candidates[0].refused);
        assert!(survey.candidates[0].notes[0].contains("README.txt"));
    }

    #[test]
    fn a_staged_game_file_that_is_not_installed_is_refused() {
        // Here the staged copy really is not what the game loads.
        let dir = Dir::new("assetdrift");
        let (_staging, mods) = one_good_mod(&dir);
        dir.file("staging/Shelters 1.3-2308/Shelters 1.4/GLOBALS/B.EXML", "extra");

        let survey = survey(&mods, None, None).unwrap();
        assert_eq!(survey.ready(), 0);
        assert!(survey.candidates[0].refused.as_deref().unwrap().contains("not installed"));
    }

    #[test]
    fn an_installed_file_with_no_staged_copy_is_refused() {
        // Removing this mod would delete a file nothing has a copy of.
        let dir = Dir::new("drift");
        let (_staging, mods) = one_good_mod(&dir);
        dir.file("MODS/Shelters 1.4/GLOBALS/EXTRA.EXML", "added by hand");

        let survey = survey(&mods, None, None).unwrap();
        assert_eq!(survey.ready(), 0);
        assert!(survey.candidates[0]
            .refused
            .as_deref()
            .unwrap()
            .contains("would lose them"));
    }

    #[test]
    fn the_managers_own_marker_file_is_not_treated_as_a_difference() {
        // Vortex drops `__folder_managed_by_vortex` into every folder it
        // deploys, and it exists only on the game side.
        let dir = Dir::new("marker");
        let (_staging, mods) = one_good_mod(&dir);
        dir.file("MODS/Shelters 1.4/__folder_managed_by_vortex", "vortex");

        let survey = survey(&mods, None, None).unwrap();
        assert_eq!(survey.ready(), 1, "{:?}", survey.candidates[0].refused);
    }

    #[test]
    fn mods_no_manager_claims_are_listed_rather_than_swept_up() {
        let dir = Dir::new("unmanaged");
        let (_staging, mods) = one_good_mod(&dir);
        dir.file("MODS/Hand Installed Mod/GLOBALS/B.EXML", "by hand");

        let survey = survey(&mods, None, None).unwrap();
        assert_eq!(survey.unmanaged, vec!["Hand Installed Mod"]);
        // ...and adopting does not take them.
        let book = dir.0.join("loadout.json");
        adopt(&survey, &book).unwrap();
        assert!(Loadout::read(&book).get("Hand Installed Mod").is_none());
    }

    /// With neither a manifest nor a staging folder there is genuinely nothing
    /// to match against, and the message must not blame Vortex for it -- the
    /// person reading it may never have installed Vortex.
    #[test]
    fn with_nothing_to_match_against_it_says_so_without_naming_a_manager() {
        let dir = Dir::new("nomanifest");
        std::fs::create_dir_all(dir.0.join("MODS")).unwrap();
        let err = survey(&dir.0.join("MODS"), None, None).unwrap_err();
        assert!(
            !err.to_lowercase().contains("vortex"),
            "must not assume Vortex: {err}"
        );
        assert!(err.contains("staging folder"), "{err}");
    }

    // ----------------------------------------------------------------------
    // Taking over a folder with no manifest at all
    //
    // The case that matters for anyone who has never run Vortex, or who ran it
    // and then uninstalled it and took the manifest with them. The links in the
    // mods folder are still there and still readable; these prove we read them.
    // ----------------------------------------------------------------------

    /// Deploy `staged` into `mods` the way a manager does: by hardlink.
    fn link_into_game(staged: &Path, mods: &Path, inner: &str) {
        let from = staged.join(inner);
        let to = mods.join(inner);
        let mut files = Vec::new();
        files_under(&from, Path::new(""), &mut files);
        for rel in files {
            let at = to.join(&rel);
            std::fs::create_dir_all(at.parent().unwrap()).unwrap();
            std::fs::hard_link(from.join(&rel), &at).unwrap();
        }
    }

    #[test]
    fn a_mod_is_traced_to_its_staging_folder_by_the_links_alone() {
        let dir = Dir::new("bylink");
        let staging = dir.0.join("staging");
        let mods = dir.0.join("MODS");
        // The archive folder and the folder the game loads are named
        // differently, as they always are -- nothing here matches by name.
        for (rel, body) in [
            ("staging/Shelters-2308-1-4/Shelters/GLOBALS/A.EXML", "asset a"),
            ("staging/Shelters-2308-1-4/Shelters/GLOBALS/B.EXML", "asset b"),
            ("staging/Other-991-2-0/Other Mod/GLOBALS/C.EXML", "asset c"),
        ] {
            dir.file(rel, body);
        }
        std::fs::create_dir_all(&mods).unwrap();
        link_into_game(&staging.join("Shelters-2308-1-4"), &mods, "Shelters");
        link_into_game(&staging.join("Other-991-2-0"), &mods, "Other Mod");

        let survey = survey(&mods, Some(&staging), None).unwrap();
        assert_eq!(survey.manager, None, "no manager was involved");
        assert_eq!(survey.refused(), 0, "{:?}", survey.candidates);
        assert_eq!(survey.ready(), 2);

        let shelters = survey
            .candidates
            .iter()
            .find(|c| c.owner == "Shelters")
            .expect("found by its links");
        assert!(
            shelters.source.ends_with("Shelters-2308-1-4"),
            "traced to the archive folder, whose name shares nothing with it: {}",
            shelters.source
        );
        assert!(
            shelters.notes.is_empty(),
            "a link is a fact, so nothing to qualify: {:?}",
            shelters.notes
        );
    }

    /// A mod nothing in staging accounts for is left alone, not guessed at.
    #[test]
    fn a_hand_installed_mod_is_not_attributed_to_anything() {
        let dir = Dir::new("byhand");
        let staging = dir.0.join("staging");
        let mods = dir.0.join("MODS");
        dir.file("staging/Shelters-2308-1-4/Shelters/GLOBALS/A.EXML", "asset a");
        std::fs::create_dir_all(&mods).unwrap();
        link_into_game(&staging.join("Shelters-2308-1-4"), &mods, "Shelters");
        dir.file("MODS/Hand Dropped/GLOBALS/Z.EXML", "not ours");

        let survey = survey(&mods, Some(&staging), None).unwrap();
        assert_eq!(survey.ready(), 1);
        assert!(
            survey.unmanaged.contains(&"Hand Dropped".to_string()),
            "{:?}",
            survey.unmanaged
        );
    }

    /// Copied rather than linked -- some managers copy, and our own deploy
    /// copies across drives. Content match finds it, and says that it did.
    #[test]
    fn a_copied_mod_is_matched_on_content_and_marked_as_a_guess() {
        let dir = Dir::new("bycopy");
        let staging = dir.0.join("staging");
        let mods = dir.0.join("MODS");
        for (rel, body) in [
            ("staging/Shelters-2308-1-4/Shelters/GLOBALS/A.EXML", "asset a"),
            ("staging/Shelters-2308-1-4/Shelters/GLOBALS/B.EXML", "asset b"),
            // Written, not linked: a different file holding the same bytes.
            ("MODS/Shelters/GLOBALS/A.EXML", "asset a"),
            ("MODS/Shelters/GLOBALS/B.EXML", "asset b"),
        ] {
            dir.file(rel, body);
        }

        let survey = survey(&mods, Some(&staging), None).unwrap();
        assert_eq!(survey.ready(), 1, "{:?}", survey.candidates);
        let only = &survey.candidates[0];
        assert_eq!(only.owner, "Shelters");
        assert!(
            only.notes.iter().any(|n| n.contains("strong guess")),
            "a content match must not be presented as a fact: {:?}",
            only.notes
        );
    }

    /// One shared asset is not evidence. A folder whose files mostly come from
    /// nowhere we know must not be attributed on the strength of a single
    /// vanilla file that happens to also sit in someone's staging folder.
    #[test]
    fn one_file_in_common_does_not_attribute_a_mod() {
        let dir = Dir::new("minority");
        let staging = dir.0.join("staging");
        let mods = dir.0.join("MODS");
        dir.file("staging/Shelters-2308-1-4/Shelters/GLOBALS/SHARED.EXML", "same");
        std::fs::create_dir_all(mods.join("Stranger/GLOBALS")).unwrap();
        // One file in common, three that are its own.
        dir.file("MODS/Stranger/GLOBALS/SHARED.EXML", "same");
        dir.file("MODS/Stranger/GLOBALS/ONE.EXML", "one");
        dir.file("MODS/Stranger/GLOBALS/TWO.EXML", "two");
        dir.file("MODS/Stranger/GLOBALS/THREE.EXML", "three");

        let survey = survey(&mods, Some(&staging), None).unwrap();
        assert!(
            survey.unmanaged.contains(&"Stranger".to_string()),
            "a minority of shared files is not a source: {:?}",
            survey.candidates
        );
    }

    /// A staged file the deployer would never have deployed is not a mod that
    /// is out of step with its staging folder.
    ///
    /// Nearly every AMUMSS mod ships its `.lua` build script beside its output.
    /// `deploy` lists `lua` as inert and has never put one in the game, so
    /// counting it as a missing game file refused 24 of 54 traced mods on a
    /// real library -- for a state no deploy could ever produce.
    #[test]
    fn a_staged_build_script_is_not_a_missing_game_file() {
        let dir = Dir::new("luascript");
        let staging = dir.0.join("staging");
        let mods = dir.0.join("MODS");
        dir.file("staging/Cool-1-0/Cool Mod/GLOBALS/A.EXML", "asset");
        // Shipped, and deliberately never deployed. See `deploy::INERT`.
        dir.file("staging/Cool-1-0/Cool Mod.lua", "the AMUMSS script");
        std::fs::create_dir_all(&mods).unwrap();
        link_into_game(&staging.join("Cool-1-0"), &mods, "Cool Mod");

        let survey = survey(&mods, Some(&staging), None).unwrap();
        assert_eq!(
            survey.refused(),
            0,
            "the script is not a game file: {:?}",
            survey.candidates
        );
        assert_eq!(survey.ready(), 1);
    }

    /// The rule it defers to still has teeth: a real game asset left undeployed
    /// means the staged copy is not what the game is loading, and that is a
    /// refusal.
    #[test]
    fn an_undeployed_game_asset_is_still_a_refusal() {
        let dir = Dir::new("missingasset");
        let staging = dir.0.join("staging");
        let mods = dir.0.join("MODS");
        dir.file("staging/Cool-1-0/Cool Mod/GLOBALS/A.EXML", "asset");
        dir.file("staging/Cool-1-0/Cool Mod/GLOBALS/B.EXML", "also an asset");
        std::fs::create_dir_all(&mods).unwrap();
        link_into_game(&staging.join("Cool-1-0"), &mods, "Cool Mod");
        // Deployed, then taken out of the game behind our back.
        std::fs::remove_file(mods.join("Cool Mod/GLOBALS/B.EXML")).unwrap();

        let survey = survey(&mods, Some(&staging), None).unwrap();
        assert_eq!(survey.refused(), 1, "{:?}", survey.candidates);
    }

    /// A mod running one of *our* builds is not somebody else's mod.
    ///
    /// The trap this closes: a cleaned build is written into `derived`, but
    /// only the files it actually pruned differ -- everything it did not touch
    /// is still byte-identical to the staged original. So without knowing about
    /// `derived` it falls through to the content fallback, matches its own
    /// staging folder on the untouched files, and gets written down as running
    /// the author's build. Three mods on the real library were about to be.
    #[test]
    fn a_mod_running_our_own_build_is_not_adopted_as_the_authors() {
        let dir = Dir::new("derived");
        let staging = dir.0.join("staging");
        let derived = dir.0.join("derived");
        let mods = dir.0.join("MODS");

        // Staged as the author shipped it.
        dir.file("staging/Cool-1-0/Cool Mod/GLOBALS/A.EXML", "untouched");
        dir.file("staging/Cool-1-0/Cool Mod/GLOBALS/BIG.EXML", "the whole game file");
        // Our cleaned build: one file pruned, the other carried over as-is.
        dir.file("derived/Cool Mod/GLOBALS/A.EXML", "untouched");
        dir.file("derived/Cool Mod/GLOBALS/BIG.EXML", "just the edits");
        std::fs::create_dir_all(&mods).unwrap();
        link_into_game(&derived, &mods, "Cool Mod");

        let survey = survey(&mods, Some(&staging), Some(&derived)).unwrap();
        assert_eq!(
            survey.ours,
            vec!["Cool Mod".to_string()],
            "recognised as our build: {survey:?}"
        );
        assert!(
            survey.candidates.is_empty(),
            "must not be offered for adoption: {:?}",
            survey.candidates
        );
        assert!(
            !survey.unmanaged.contains(&"Cool Mod".to_string()),
            "and not filed as hand-installed either"
        );
    }

    /// Without the derived folder it is exactly the mis-attribution above --
    /// which is what this guards against coming back.
    #[test]
    fn the_same_mod_would_be_mis_attributed_without_the_derived_folder() {
        let dir = Dir::new("derivedblind");
        let staging = dir.0.join("staging");
        let derived = dir.0.join("derived");
        let mods = dir.0.join("MODS");
        dir.file("staging/Cool-1-0/Cool Mod/GLOBALS/A.EXML", "untouched");
        dir.file("staging/Cool-1-0/Cool Mod/GLOBALS/B.EXML", "untouched too");
        dir.file("derived/Cool Mod/GLOBALS/A.EXML", "untouched");
        dir.file("derived/Cool Mod/GLOBALS/B.EXML", "untouched too");
        std::fs::create_dir_all(&mods).unwrap();
        link_into_game(&derived, &mods, "Cool Mod");

        let blind = survey(&mods, Some(&staging), None).unwrap();
        assert_eq!(blind.ready(), 1, "content match claims it");
        let seeing = survey(&mods, Some(&staging), Some(&derived)).unwrap();
        assert_eq!(seeing.ready(), 0, "knowing about derived does not");
        assert_eq!(seeing.ours, vec!["Cool Mod".to_string()]);
    }

    // ----------------------------------------------------------------------
    // Putting a mod back on its author's build
    //
    // The way out of `Survey::ours`. Without it the survey names mods it cannot
    // help with and tells the reader to go and fix each one in the Library --
    // which cannot show them, because they are not in the loadout, which is the
    // very reason they are on that list.
    // ----------------------------------------------------------------------

    /// The file the game is reading, by content, so a test can say *which
    /// build* is deployed rather than merely that something is.
    fn in_game(mods: &Path, rel: &str) -> String {
        std::fs::read_to_string(mods.join(rel)).unwrap_or_else(|e| format!("<{e}>"))
    }

    #[test]
    fn a_mod_on_our_build_goes_back_to_the_authors_and_is_taken_over() {
        let dir = Dir::new("restore");
        let staging = dir.0.join("staging");
        let derived = dir.0.join("derived");
        let mods = dir.0.join("MODS");
        let book = dir.0.join("loadout.json");

        dir.file("staging/Carbon-1231-5-0/Black Carbon/GLOBALS/A.EXML", "the author's");
        dir.file("derived/Black Carbon/GLOBALS/A.EXML", "our cleaned build");
        std::fs::create_dir_all(&mods).unwrap();
        link_into_game(&derived, &mods, "Black Carbon");
        assert_eq!(in_game(&mods, "Black Carbon/GLOBALS/A.EXML"), "our cleaned build");

        let found = survey(&mods, Some(&staging), Some(&derived)).unwrap();
        assert_eq!(found.ours, vec!["Black Carbon".to_string()]);

        let plan = restorable(Some(&staging), &mods, &found.ours);
        assert_eq!(plan.len(), 1);
        assert!(plan[0].ready(), "{:?}", plan[0].why_not);
        assert_eq!(plan[0].source.as_deref(), Some("Carbon-1231-5-0"));

        let done = restore(&plan, &staging, &derived, &mods, &book, false).unwrap();
        assert_eq!(done.restored, vec!["Black Carbon".to_string()]);
        assert!(done.problems.is_empty(), "{:?}", done.problems);

        // The point of the whole exercise: the game is now reading the author's
        // file, not ours.
        assert_eq!(
            in_game(&mods, "Black Carbon/GLOBALS/A.EXML"),
            "the author's",
            "the deployed name must be relinked, not merely re-recorded"
        );

        // And it is ours to manage now, on its original build.
        let entry = Loadout::read(&book).get("Black Carbon").cloned().expect("adopted");
        assert!(matches!(entry.variant, Variant::Original));
        assert!(entry.source.ends_with("Carbon-1231-5-0"));
        assert!(entry.enabled);

        // Which means the survey has nothing left to complain about.
        let again = survey(&mods, Some(&staging), Some(&derived)).unwrap();
        assert!(again.ours.is_empty(), "{:?}", again.ours);
    }

    /// A dry run says what it would do and leaves the game alone.
    #[test]
    fn previewing_a_restore_changes_nothing() {
        let dir = Dir::new("restoredry");
        let staging = dir.0.join("staging");
        let derived = dir.0.join("derived");
        let mods = dir.0.join("MODS");
        let book = dir.0.join("loadout.json");
        dir.file("staging/Carbon-1231-5-0/Black Carbon/GLOBALS/A.EXML", "the author's");
        dir.file("derived/Black Carbon/GLOBALS/A.EXML", "our cleaned build");
        std::fs::create_dir_all(&mods).unwrap();
        link_into_game(&derived, &mods, "Black Carbon");

        let found = survey(&mods, Some(&staging), Some(&derived)).unwrap();
        let plan = restorable(Some(&staging), &mods, &found.ours);
        let done = restore(&plan, &staging, &derived, &mods, &book, true).unwrap();

        assert_eq!(done.restored, vec!["Black Carbon".to_string()]);
        assert_eq!(
            in_game(&mods, "Black Carbon/GLOBALS/A.EXML"),
            "our cleaned build",
            "a preview must not touch the game"
        );
        assert!(!book.exists(), "and must not write the mod list");
    }

    /// A mod whose author's copy is not staged is reported, not guessed at.
    #[test]
    fn a_build_with_no_staged_original_is_skipped_with_a_reason() {
        let dir = Dir::new("restoregone");
        let staging = dir.0.join("staging");
        let derived = dir.0.join("derived");
        let mods = dir.0.join("MODS");
        let book = dir.0.join("loadout.json");
        // Staging holds some other mod entirely.
        dir.file("staging/Something Else-1/Something Else/GLOBALS/Z.EXML", "other");
        dir.file("derived/Orphan/GLOBALS/A.EXML", "our build, no original");
        std::fs::create_dir_all(&mods).unwrap();
        link_into_game(&derived, &mods, "Orphan");

        let found = survey(&mods, Some(&staging), Some(&derived)).unwrap();
        let plan = restorable(Some(&staging), &mods, &found.ours);
        assert!(!plan[0].ready());
        assert!(
            plan[0].why_not.as_deref().unwrap().contains("nothing to go back to"),
            "{:?}",
            plan[0].why_not
        );

        let done = restore(&plan, &staging, &derived, &mods, &book, false).unwrap();
        assert!(done.restored.is_empty());
        assert_eq!(done.skipped.len(), 1);
        assert_eq!(
            in_game(&mods, "Orphan/GLOBALS/A.EXML"),
            "our build, no original",
            "left exactly as it was"
        );
    }

    /// Two staged mods deploying the same folder name is a real ambiguity, and
    /// picking one would put the wrong mod in the game under the right name.
    #[test]
    fn two_staged_mods_claiming_one_name_is_reported_not_guessed() {
        let dir = Dir::new("restoreambig");
        let staging = dir.0.join("staging");
        let derived = dir.0.join("derived");
        let mods = dir.0.join("MODS");
        dir.file("staging/Carbon-a/Black Carbon/GLOBALS/A.EXML", "one author");
        dir.file("staging/Carbon-b/Black Carbon/GLOBALS/A.EXML", "another author");
        dir.file("derived/Black Carbon/GLOBALS/A.EXML", "our build");
        std::fs::create_dir_all(&mods).unwrap();
        link_into_game(&derived, &mods, "Black Carbon");

        let found = survey(&mods, Some(&staging), Some(&derived)).unwrap();
        let plan = restorable(Some(&staging), &mods, &found.ours);
        assert!(!plan[0].ready());
        assert!(
            plan[0].why_not.as_deref().unwrap().contains("cannot be told apart"),
            "{:?}",
            plan[0].why_not
        );
    }

    /// The manifest is a shortcut, not a requirement -- and when it is there it
    /// still wins, because it is one file to read instead of two trees to walk.
    #[test]
    fn a_manifest_is_still_used_when_there_is_one() {
        let dir = Dir::new("stillvortex");
        let (_staging, mods) = one_good_mod(&dir);
        let survey = survey(&mods, None, None).unwrap();
        assert_eq!(survey.manager.as_deref(), Some("Vortex"));
        assert_eq!(survey.ready(), 1);
    }
}
