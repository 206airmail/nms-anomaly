//! Adding and removing mods, and saying who owns what on disk.
//!
//! A No Man's Sky mod is rarely one folder. `Unpredictable Shelters 1.4` is a
//! folder, a `.lua` beside it and a readme beside that, all three installed
//! together and all three needing to leave together. Guessing at membership by
//! name would strand files; when a mod manager has deployed the folder, its
//! manifest says exactly which files came from which archive, so that is what
//! [`members`] reads, falling back to the name only when there is no manifest.
//!
//! # Removing is a move, not a delete
//!
//! Everything [`remove`] takes out goes to a dated folder in the app's own
//! data directory, with the layout preserved, so it can be put back by hand
//! even if this program never runs again. Nothing here calls `remove_file` on
//! a mod.
//!
//! # This does not make the game's mod manager agree
//!
//! If Vortex (or another manager) deployed these files, it still believes it
//! owns them. It will redeploy what we remove and purge what we add on its
//! next deploy. [`Identity::managed_by`] reports that so the UI can say so,
//! rather than quietly doing work that gets undone.

use std::path::Path;

use serde::Serialize;

use super::hostenv::{self, VORTEX_MANIFEST};
use super::model::Mod;
use super::namecache::Names;
use super::nexusname;

/// What can be said about an installed mod without asking anyone.
#[derive(Debug, Clone, Serialize)]
pub struct Identity {
    /// the mod folder's name, which is how the game and every other part of
    /// this program refers to it
    pub owner: String,
    /// the mod's title as a person would recognise it. See [`display_name`]:
    /// the page, else the archive, else the folder -- never empty.
    pub name: String,
    pub root: String,
    pub mod_id: Option<u64>,
    /// the version recorded for the installed archive. Can be stale -- see
    /// [`super::nexus`] on reinstalls over an existing entry.
    pub version: Option<String>,
    pub page: Option<String>,
    pub priority: Option<i64>,
    pub disabled: bool,
    pub files: usize,
    pub assets: usize,
    /// the mod manager that deployed this, when one did
    pub managed_by: Option<String>,
}

/// Mod folder in the game -> the archive its files are staged from.
///
/// # Why this has to exist
///
/// The archive name is the only record of a mod's Nexus id, its version and
/// its title, and there are two places it can come from. Vortex writes it into
/// a manifest, which is what [`super::hostenv`] reads. But once the user has
/// cut over to this program, Vortex is gone and so is its manifest -- and the
/// mod folders in the game are *not* named after their archives. They are
/// named after whatever folder was inside the archive, which is frequently
/// nothing like it:
///
/// ```text
/// staging: Smooth glyphs-3071-1-1-1739735732
/// game:    MOD.PM_PortalGlyphs
/// ```
///
/// So the mapping is read back off the staging tree, whose shape *is* the
/// record: one folder per archive, holding the folders it deploys. Measured on
/// a real 62-mod library where the manifest had already been purged and every
/// other source knew nothing.
pub fn staged_archives(staging: &Path) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(staging) else {
        return out;
    };
    for entry in entries.flatten() {
        let from = entry.path();
        if !from.is_dir() {
            continue;
        }
        let Some(archive) = from.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Ok(inner) = std::fs::read_dir(&from) else {
            continue;
        };
        for deployed in inner.flatten() {
            if !deployed.path().is_dir() {
                continue;
            }
            if let Some(owner) = deployed.file_name().to_str() {
                // First writer wins, so a folder deployed by two archives keeps
                // the one read first rather than flipping between runs.
                out.entry(owner.to_string()).or_insert_with(|| archive.to_string());
            }
        }
    }
    out
}

/// The archive one installed mod came from, from whichever record knows.
///
/// The manifest first, because a manager that is still deploying is the
/// authority on what it deployed; the staging tree after it, which is what is
/// left once that manager is gone.
pub fn archive_of<'a>(m: &'a Mod, staged: &'a Staged) -> Option<&'a str> {
    m.archive
        .as_deref()
        .or_else(|| staged.get(&m.name).map(String::as_str))
}

/// What [`staged_archives`] returns: mod folder -> archive it is staged from.
pub type Staged = std::collections::BTreeMap<String, String>;

/// The Nexus mod id for one installed mod, when something records its archive.
pub fn mod_id_of(m: &Mod, staged: &Staged) -> Option<u64> {
    archive_of(m, staged).and_then(nexusname::parse).map(|f| f.mod_id)
}

/// Every Nexus mod id in a library, deduplicated.
///
/// What [`super::namecache::Names::missing`] is asked about, so a resolve knows
/// which pages it has never looked at.
pub fn mod_ids(mods: &[Mod], staged: &Staged) -> Vec<u64> {
    let mut out: Vec<u64> = mods.iter().filter_map(|m| mod_id_of(m, staged)).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// The title Nexus baked into this mod's archive name.
///
/// Per *file*, which is what makes it the answer when a page name is ambiguous.
/// Used verbatim, version and all: `Better Deposit Colors 2.9` looks like a name
/// with a version stuck on the end and trimming it is tempting, but Nexus writes
/// the display name into the archive, so that *is* the title its author chose.
/// Trimming would make it disagree with the page it is meant to match.
fn archive_title(m: &Mod, staged: &Staged) -> Option<String> {
    archive_of(m, staged)
        .and_then(nexusname::parse)
        .map(|f| f.name)
        .filter(|name| !name.trim().is_empty())
}

/// The name to put in front of a person, in order of how true it is.
///
/// 1. the title on the mod's Nexus page, once resolved. Follows a rename.
/// 2. the title Nexus baked into the archive name. Free, offline, and right for
///    every mod whose author has not renamed it -- so this alone fixes the list
///    before a single request is made.
/// 3. the folder. All there is for a mod installed by hand, and correct for it.
///
/// **This is the name for one mod considered alone.** Where a whole library is
/// being shown, use [`display_names`], which resolves the case this cannot see:
/// a page name shared by several mods.
pub fn display_name(m: &Mod, names: &Names, staged: &Staged) -> String {
    page_title(m, names, staged)
        .or_else(|| archive_title(m, staged))
        .unwrap_or_else(|| m.name.clone())
}

/// The title of this mod's Nexus page, if we have resolved it.
fn page_title(m: &Mod, names: &Names, staged: &Staged) -> Option<String> {
    let id = mod_id_of(m, staged)?;
    names.get(id).map(str::to_string)
}

/// Folder -> display name for a whole library, for the parts of the UI that
/// hold a folder name and need something to show.
///
/// # Why this is not just `display_name` in a loop
///
/// Because a Nexus page can host several *different* mods as separate files,
/// and the page title then names all of them. Measured on a real library: page
/// 1371 covers No Metrics Lines, No Speed Halo, No Speed Lines and No Warp
/// Flash, so preferring the page title turned four distinct rows into four rows
/// reading "No Ship Speed Effects 4.3". Page 1201 did the same to two more.
/// That is worse than the folder names it replaced -- a list you cannot tell
/// apart is not an improvement on a list that is ugly.
///
/// So the page title is preferred only while it is *unique in this library*.
/// Where several mods claim one, each falls back to its archive title, which is
/// per file and does distinguish them, and to the folder after that. Uniqueness
/// is a property of the set, which is why it cannot be decided one mod at a
/// time.
pub fn display_names(mods: &[Mod], names: &Names, staged: &Staged) -> Staged {
    names_for(
        mods.iter()
            .map(|m| (m.name.clone(), archive_of(m, staged).map(str::to_string))),
        names,
    )
}

/// The same naming, for any set of mods described as (folder, archive).
///
/// Taken as pairs rather than as `Mod`s so that a mod which is **switched off**
/// can be named too. Such a mod has no files in the game, so it is in no scan --
/// and it is precisely the one a person needs to find in the list in order to
/// switch it back on. The loadout knows about it; the game folder does not.
pub fn names_for(
    entries: impl IntoIterator<Item = (String, Option<String>)>,
    names: &Names,
) -> Staged {
    let known: Vec<(String, Option<nexusname::ArchiveName>)> = entries
        .into_iter()
        .map(|(owner, archive)| (owner, archive.as_deref().and_then(nexusname::parse)))
        .collect();

    // How many mods each page title would name. A title claimed twice names
    // neither of them.
    let mut claims: std::collections::BTreeMap<&str, usize> =
        std::collections::BTreeMap::new();
    for (_, found) in &known {
        if let Some(title) = found.as_ref().and_then(|f| names.get(f.mod_id)) {
            *claims.entry(title).or_insert(0) += 1;
        }
    }

    known
        .iter()
        .map(|(owner, found)| {
            let page = found
                .as_ref()
                .and_then(|f| names.get(f.mod_id))
                .filter(|title| claims.get(*title).copied().unwrap_or(0) == 1)
                .map(str::to_string);
            let name = page
                .or_else(|| {
                    found
                        .as_ref()
                        .map(|f| f.name.clone())
                        .filter(|n| !n.trim().is_empty())
                })
                .unwrap_or_else(|| owner.clone());
            (owner.clone(), name)
        })
        .collect()
}

/// Letters and digits only, lowercased: `Increased S Class Chance` and
/// `IncreasedSClassChance` are the same mod written two ways.
///
/// Only ever used to match a folder against an archive *we* downloaded, never
/// to match two mods against each other.
fn slug(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Downloaded archives, indexed by the slug of the title inside their names.
///
/// # Why a mod ever needs this
///
/// A mod's Nexus identity normally rides in the name of the folder it is staged
/// under. It does not always: a folder can be staged under the name of the mod
/// *inside* the archive rather than the archive's own name, and then nothing in
/// staging knows which page it came from. The download is still sitting in the
/// archives folder with the id and version in its name, so that is where the
/// answer is.
///
/// # What it refuses to guess
///
/// A slug claimed by more than one Nexus id is dropped. Matching by name is a
/// heuristic, and the cost of getting it wrong is reporting a version -- and an
/// update -- from somebody else's mod, which is worse than reporting none. Two
/// downloads of the *same* id are not a conflict: that is a re-download, and
/// the newest by file name wins.
pub fn archives_by_slug(archives: &Path) -> std::collections::BTreeMap<String, String> {
    let mut found: std::collections::BTreeMap<String, Vec<(u64, String)>> =
        std::collections::BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(archives) else {
        return std::collections::BTreeMap::new();
    };
    for entry in entries.flatten() {
        if entry.path().is_dir() {
            continue;
        }
        let Some(file) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some(parsed) = nexusname::parse(&file) else {
            continue;
        };
        found
            .entry(slug(&parsed.name))
            .or_default()
            .push((parsed.mod_id, file));
    }

    found
        .into_iter()
        .filter_map(|(key, mut hits)| {
            let first = hits.first()?.0;
            if hits.iter().any(|(id, _)| *id != first) {
                return None; // one name, two pages: refuse to choose
            }
            hits.sort_by(|a, b| a.1.cmp(&b.1));
            Some((key, hits.pop()?.1))
        })
        .collect()
}

/// The archive that produced a staged folder, when only the downloads know.
///
/// `owner` is the mod folder the game sees; `origin` the folder it is staged
/// under, which is tried too because either may be the one that resembles the
/// download's name.
pub fn archive_for_slug(
    owner: &str,
    origin: Option<&str>,
    by_slug: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    [Some(owner), origin]
        .into_iter()
        .flatten()
        .find_map(|name| by_slug.get(&slug(name)).cloned())
}

/// What to call a merge, given the mods it stands in for.
///
/// A merge has no Nexus page and no archive -- it is something this program
/// built -- so the only honest name for it is what it does. Its folder is
/// `zzz_nmscheck_METADATA_REALITY_TABLES_REWARDTABLE`, which is unique and
/// stable and no help at all to a person reading a list.
///
/// `shown` supplies the parents' own display names, so a merge of two mods
/// reads in the same words the rest of the list uses for them.
pub fn merged_label(replaces: &[String], shown: &Staged) -> String {
    let named: Vec<&str> = replaces
        .iter()
        .map(|owner| shown.get(owner).map(String::as_str).unwrap_or(owner.as_str()))
        .collect();
    match named.len() {
        0 => "Merged mod".to_string(),
        1 => format!("Merged: {}", named[0]),
        2 => format!("Merged: {} + {}", named[0], named[1]),
        // Three or more would run past the width of the list; the detail pane
        // names them all.
        n => format!("Merged: {} + {} and {} more", named[0], named[1], n - 2),
    }
}

/// The archive stem out of a staged mod's recorded path.
///
/// The loadout records `source` as a full path into the staging folder, and only
/// its last segment carries the Nexus id, version and title.
pub fn archive_stem(source: &str) -> &str {
    source
        .trim_end_matches(['/', '\\'])
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(source)
}

/// Everything installed, in the order the game loads it.
pub fn identities(mods_dir: &Path, mods: &[Mod], names: &Names, staged: &Staged) -> Vec<Identity> {
    let host = hostenv::describe(mods_dir);
    // The whole-library naming, so this agrees with every other surface about
    // what a mod is called -- including where a shared page title had to be
    // given up on.
    let shown = display_names(mods, names, staged);
    let mut out: Vec<Identity> = mods
        .iter()
        .map(|m| {
            let found = archive_of(m, staged).and_then(nexusname::parse);
            Identity {
                owner: m.name.clone(),
                name: shown.get(&m.name).cloned().unwrap_or_else(|| m.name.clone()),
                root: m.root.clone(),
                mod_id: found.as_ref().map(|f| f.mod_id),
                version: found.as_ref().map(|f| f.version.clone()),
                page: found.as_ref().map(|f| {
                    format!("https://www.nexusmods.com/nomanssky/mods/{}", f.mod_id)
                }),
                priority: m.priority,
                disabled: m.disabled,
                files: m.files.len(),
                assets: m.targets().len(),
                managed_by: host
                    .archives
                    .contains_key(&m.name)
                    .then(|| host.manager.clone())
                    .flatten(),
            }
        })
        .collect();
    // Load order, with the unregistered last: that is the order they take
    // effect in, and it is the order the rest of the UI shows them in.
    out.sort_by(|a, b| match (a.priority, b.priority) {
        (Some(x), Some(y)) => x.cmp(&y).then_with(|| a.owner.cmp(&b.owner)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.owner.cmp(&b.owner),
    });
    out
}

/// Every path in `mods_dir` that belongs to one mod, relative to `mods_dir`.
///
/// Prefers the deployment manifest, which records the archive each deployed
/// file came from: that catches the loose `.lua` and the readme that sit
/// beside the folder rather than inside it. Without a manifest, falls back to
/// the folder plus anything sharing its name, which is the convention AMUMSS
/// mods follow.
pub fn members(mods_dir: &Path, owner: &str) -> Vec<String> {
    if let Some(found) = manifest_members(mods_dir, owner) {
        return found;
    }

    let mut out = Vec::new();
    if mods_dir.join(owner).is_dir() {
        out.push(owner.to_string());
    }
    if let Ok(entries) = std::fs::read_dir(mods_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            // `Unpredictable Shelters 1.4.lua` beside `Unpredictable Shelters 1.4`
            if name != owner && name.starts_with(owner) && name[owner.len()..].starts_with('.') {
                out.push(name.to_string());
            }
        }
    }
    out.sort();
    out
}

/// The manifest's answer, or `None` when there is no manifest to ask.
fn manifest_members(mods_dir: &Path, owner: &str) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(mods_dir.join(VORTEX_MANIFEST)).ok()?;
    let data: serde_json::Value = serde_json::from_str(&text).ok()?;
    let files = data.get("files")?.as_array()?;

    let top = |rel: &str| -> String {
        rel.replace('/', "\\")
            .split('\\')
            .next()
            .unwrap_or("")
            .to_string()
    };

    // The archive this mod's folder came from, then everything else from it.
    let source = files.iter().find_map(|e| {
        let rel = e.get("relPath")?.as_str()?;
        (top(rel) == owner).then(|| e.get("source")?.as_str().map(str::to_string))?
    })?;

    let mut out: Vec<String> = Vec::new();
    for entry in files {
        let Some(rel) = entry.get("relPath").and_then(|v| v.as_str()) else {
            continue;
        };
        if entry.get("source").and_then(|v| v.as_str()) != Some(source.as_str()) {
            continue;
        }
        // One entry per top-level item: moving the folder takes its contents.
        let head = top(rel);
        if !head.is_empty() && !out.contains(&head) {
            out.push(head);
        }
    }
    out.sort();
    (!out.is_empty()).then_some(out)
}

/// What a removal did, in enough detail to undo it by hand.
#[derive(Debug, Clone, Serialize)]
pub struct Removal {
    pub owner: String,
    /// paths relative to the mods folder, as they were
    pub moved: Vec<String>,
    /// where they went
    pub trash: String,
    /// the manager that will put this back unless it is told, if any
    pub managed_by: Option<String>,
}

/// Move one mod out of the mods folder, keeping every byte.
pub fn remove(mods_dir: &Path, owner: &str, trash_root: &Path) -> Result<Removal, String> {
    if owner.is_empty() || owner.contains(['/', '\\']) || owner.contains("..") {
        return Err(format!("{owner:?} is not a mod folder name"));
    }
    let moving = members(mods_dir, owner);
    if moving.is_empty() {
        return Err(format!("nothing in the mods folder belongs to {owner}"));
    }

    let trash = trash_root.join(stamp()).join(owner);
    std::fs::create_dir_all(&trash).map_err(|e| format!("could not make {}: {e}", trash.display()))?;

    let mut moved = Vec::new();
    for rel in &moving {
        let from = mods_dir.join(rel);
        if !from.exists() {
            continue;
        }
        let to = trash.join(rel);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        // A rename across volumes fails; the mods folder and app data are
        // routinely on different drives here, so fall back to copy-then-drop.
        if std::fs::rename(&from, &to).is_err() {
            copy_tree(&from, &to).map_err(|e| format!("could not move {rel}: {e}"))?;
            if from.is_dir() {
                std::fs::remove_dir_all(&from).map_err(|e| format!("could not clear {rel}: {e}"))?;
            } else {
                std::fs::remove_file(&from).map_err(|e| format!("could not clear {rel}: {e}"))?;
            }
        }
        moved.push(rel.clone());
    }

    let host = hostenv::read_vortex_manifest(mods_dir);
    Ok(Removal {
        owner: owner.to_string(),
        moved,
        trash: trash.display().to_string(),
        managed_by: host.manager,
    })
}

/// Put back what one removal took out.
pub fn restore(mods_dir: &Path, trash: &Path) -> Result<Vec<String>, String> {
    let mut back = Vec::new();
    let entries = std::fs::read_dir(trash).map_err(|e| format!("cannot read {}: {e}", trash.display()))?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let to = mods_dir.join(&name);
        if to.exists() {
            return Err(format!(
                "{} is already back in the mods folder; nothing was restored",
                name.to_string_lossy()
            ));
        }
        if std::fs::rename(entry.path(), &to).is_err() {
            copy_tree(&entry.path(), &to).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_dir_all(entry.path());
        }
        back.push(name.to_string_lossy().to_string());
    }
    Ok(back)
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_file() {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(from, to)?;
        return Ok(());
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        copy_tree(&entry.path(), &to.join(entry.file_name()))?;
    }
    Ok(())
}

/// `2026-09-24T14-31-02`, sortable and legal in a path on every platform.
fn stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Civil date from a unix timestamp, Howard Hinnant's algorithm.
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}-{:02}-{:02}",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let path = std::env::temp_dir().join(format!("nmscheck_library_{tag}"));
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

    /// A mod as discovery would report it: a folder, and the archive it came
    /// from when a manager recorded one.
    fn installed(folder: &str, archive: Option<&str>) -> Mod {
        Mod {
            name: folder.to_string(),
            archive: archive.map(str::to_string),
            ..Default::default()
        }
    }

    /// No staging tree in reach; the mod's own record is all there is.
    fn nowhere() -> Staged {
        Staged::new()
    }

    #[test]
    fn the_page_name_wins_over_the_archive_and_the_folder() {
        let m = installed(
            "Salvage Rights 1.3 4417 1.3 2026-09-23T20-08Z QR3q07yEQ",
            Some("Salvage Rights 1.3 4417 1.3 2026-09-23T20-08Z QR3q07yEQ"),
        );
        let mut names = Names::default();
        names.put(4417, "Salvage Rights");
        assert_eq!(display_name(&m, &names, &nowhere()), "Salvage Rights");
    }

    #[test]
    fn without_a_resolved_page_the_archive_name_is_used() {
        // This is the case for a library that has never been online, and it
        // is already an enormous improvement on the folder.
        let m = installed(
            "Buy All Corvette Parts 4447 1.2 2026-09-11T13-48Z L5bXM34yG",
            Some("Buy All Corvette Parts 4447 1.2 2026-09-11T13-48Z L5bXM34yG"),
        );
        assert_eq!(
            display_name(&m, &Names::default(), &nowhere()),
            "Buy All Corvette Parts"
        );
    }

    #[test]
    fn a_version_the_author_put_in_the_title_is_kept() {
        // Nexus writes the *display name* into the archive, so the version
        // here is part of the title its author chose. Trimming it would make
        // this row disagree with the page it is supposed to match.
        let m = installed(
            "Better Deposit Colors 2.9 2361 2.9 2026-09-10T18-59Z xDwj9UAeU",
            Some("Better Deposit Colors 2.9 2361 2.9 2026-09-10T18-59Z xDwj9UAeU"),
        );
        assert_eq!(
            display_name(&m, &Names::default(), &nowhere()),
            "Better Deposit Colors 2.9"
        );
    }

    #[test]
    fn a_mod_installed_by_hand_keeps_its_folder_name() {
        let m = installed("Increased S Class Chance", None);
        assert_eq!(
            display_name(&m, &Names::default(), &nowhere()),
            "Increased S Class Chance"
        );
    }

    #[test]
    fn an_archive_that_did_not_come_from_nexus_falls_back_to_the_folder() {
        let m = installed("MyHandmadeMod", Some("MyHandmadeMod.zip"));
        assert_eq!(display_name(&m, &Names::default(), &nowhere()), "MyHandmadeMod");
    }

    #[test]
    fn a_name_is_never_empty() {
        // Every other part of the UI puts this straight on screen, so an
        // empty string would render as a blank row rather than as a mod.
        for m in [
            installed("Folder Only", None),
            installed("Folder Only", Some("")),
            installed("Folder Only", Some("not a nexus archive")),
        ] {
            assert!(!display_name(&m, &Names::default(), &nowhere()).is_empty());
        }
    }

    #[test]
    fn a_page_title_shared_by_two_mods_is_given_up_for_one_that_tells_them_apart() {
        // The real shape of this: page 1201 hosts two different mods as two
        // files, so its title names both. Measured on a live library, where
        // page 1371 did it to four rows at once. Showing the page title here
        // would produce a list you cannot tell apart, which is worse than the
        // folder names it replaced.
        let a = installed(
            "Teleport Module Range",
            Some("Better Ship Teleport Module Range 6.0-1201-6-0-1751870410"),
        );
        let b = installed(
            "Transfer Range",
            Some("Better Ship Transfer Range 5.9-1201-5-9-1738187904"),
        );
        let mut names = Names::default();
        names.put(1201, "Better Ship Transfer Ranges 6.0");

        let map = display_names(&[a, b], &names, &nowhere());
        assert_eq!(
            map["Teleport Module Range"],
            "Better Ship Teleport Module Range 6.0"
        );
        assert_eq!(map["Transfer Range"], "Better Ship Transfer Range 5.9");
    }

    #[test]
    fn a_page_title_claimed_by_only_one_mod_is_still_preferred() {
        // The common case, and the reason for asking Nexus at all: page 4307 is
        // published as "HPO Medium" but its page is called something a person
        // would recognise.
        let m = installed("HPO Medium", Some("HPO Medium 4307 1 2026-06-27T17-59Z x"));
        let mut names = Names::default();
        names.put(4307, "Hazard Protection Overhaul");

        let map = display_names(&[m], &names, &nowhere());
        assert_eq!(map["HPO Medium"], "Hazard Protection Overhaul");
    }

    #[test]
    fn one_mod_installed_twice_still_shows_its_page_title() {
        // Two folders, one file, one page: this is the same mod deployed twice,
        // not two mods sharing a page, so the title is not ambiguous -- and the
        // archive title would be identical anyway.
        let a = installed("Copy A", Some("Fast Actions 7.0-1081-7-0-1738172730"));
        let b = installed("Copy B", Some("Fast Actions 7.0-1081-7-0-1738172730"));
        let mut names = Names::default();
        names.put(1081, "Fast Actions 7.1");

        let map = display_names(&[a, b], &names, &nowhere());
        // Both fall back to the archive title, which names this file exactly.
        assert_eq!(map["Copy A"], "Fast Actions 7.0");
        assert_eq!(map["Copy B"], "Fast Actions 7.0");
    }

    #[test]
    fn every_page_in_a_library_is_asked_about_once() {
        let mods = vec![
            installed("a", Some("A 3699 1.0 2026-06-24T22-04Z x")),
            installed("b", Some("B 1201 1.0 2026-06-24T22-04Z x")),
            installed("c", Some("C 1201 2.0 2026-06-24T22-04Z x")),
            installed("d", None),
        ];
        assert_eq!(mod_ids(&mods, &nowhere()), vec![1201, 3699]);
    }

    #[test]
    fn the_map_covers_every_folder_including_the_unrecognised_ones() {
        let mods = vec![
            installed("Ship 6.0-1201-6-0-1751870410", Some("Ship 6.0-1201-6-0-1751870410")),
            installed("Increased S Class Chance", None),
        ];
        let map = display_names(&mods, &Names::default(), &nowhere());
        assert_eq!(map.len(), 2);
        assert_eq!(map["Ship 6.0-1201-6-0-1751870410"], "Ship 6.0");
        assert_eq!(map["Increased S Class Chance"], "Increased S Class Chance");
    }

    #[test]
    fn a_download_names_a_mod_whose_staged_folder_does_not() {
        // The real case: this mod is staged under the name of the folder inside
        // its archive, so nothing in staging knows it is Nexus mod 3141. The
        // download still does.
        let dir = Dir::new("bydownload");
        dir.file("IncreasedSClassChance-3141-6-12-1762790084.rar", "x");

        let by_slug = archives_by_slug(&dir.0);
        let found = archive_for_slug("Increased S Class Chance", None, &by_slug);
        assert_eq!(
            found.as_deref(),
            Some("IncreasedSClassChance-3141-6-12-1762790084.rar")
        );

        // And that is enough to recover the whole identity.
        let m = installed("Increased S Class Chance", found.as_deref());
        assert_eq!(mod_id_of(&m, &nowhere()), Some(3141));
        assert_eq!(display_name(&m, &Names::default(), &nowhere()), "IncreasedSClassChance");
    }

    #[test]
    fn re_downloads_of_one_mod_are_not_a_conflict() {
        // A browser writes "name (2).rar" beside "name.rar". Same page, same
        // file: there is nothing to be ambiguous about.
        let dir = Dir::new("redownload");
        dir.file("IncreasedSClassChance-3141-6-12-1762790084.rar", "x");
        dir.file("IncreasedSClassChance-3141-6-13-1762790099.rar", "x");

        let by_slug = archives_by_slug(&dir.0);
        assert_eq!(
            archive_for_slug("Increased S Class Chance", None, &by_slug).as_deref(),
            Some("IncreasedSClassChance-3141-6-13-1762790099.rar"),
            "the newer download should win"
        );
    }

    #[test]
    fn two_pages_sharing_a_name_are_refused_rather_than_guessed() {
        // Reporting a version -- and an update -- from somebody else's mod is
        // worse than reporting none.
        let dir = Dir::new("ambiguous");
        dir.file("Shelters-1111-1-0-1738789143.rar", "x");
        dir.file("Shelters-2222-1-0-1738789144.rar", "x");

        let by_slug = archives_by_slug(&dir.0);
        assert!(archive_for_slug("Shelters", None, &by_slug).is_none());
    }

    #[test]
    fn something_that_is_not_a_nexus_download_is_ignored() {
        let dir = Dir::new("notnexus");
        dir.file("MyHandmadeMod.zip", "x");
        dir.file("notes.txt", "x");
        assert!(archives_by_slug(&dir.0).is_empty());
    }

    #[test]
    fn a_missing_downloads_folder_is_not_an_error() {
        assert!(archives_by_slug(Path::new("nowhere at all")).is_empty());
    }

    #[test]
    fn punctuation_and_case_do_not_stop_a_download_matching_its_folder() {
        assert_eq!(slug("Increased S Class Chance"), "increasedsclasschance");
        assert_eq!(slug("IncreasedSClassChance"), "increasedsclasschance");
        assert_eq!(slug("MOD.PM_PortalGlyphs"), "modpmportalglyphs");
        assert_eq!(slug("gFreighter Perfect Frigates  6.0.5.0a"), "gfreighterperfectfrigates605 0a".replace(' ', ""));
    }

    #[test]
    fn a_merge_is_named_for_what_it_stands_in_for() {
        let mut shown = Staged::new();
        shown.insert("folder-a".into(), "Better Rewards".into());
        shown.insert("folder-b".into(), "Fast Refiners".into());

        assert_eq!(
            merged_label(&["folder-a".into(), "folder-b".into()], &shown),
            "Merged: Better Rewards + Fast Refiners"
        );
        assert_eq!(
            merged_label(&["folder-a".into()], &shown),
            "Merged: Better Rewards"
        );
        assert_eq!(
            merged_label(
                &["folder-a".into(), "folder-b".into(), "folder-c".into()],
                &shown
            ),
            "Merged: Better Rewards + Fast Refiners and 1 more"
        );
    }

    #[test]
    fn a_merge_of_mods_we_cannot_name_falls_back_to_their_folders() {
        assert_eq!(
            merged_label(&["Mod A".into(), "Mod B".into()], &Staged::new()),
            "Merged: Mod A + Mod B"
        );
        assert_eq!(merged_label(&[], &Staged::new()), "Merged mod");
    }

    #[test]
    fn an_archive_is_read_out_of_the_path_the_loadout_records() {
        // The loadout stores `source` as a full path into staging; only its last
        // segment carries the id, the version and the title.
        assert_eq!(
            archive_stem("D:\\NMSMods\\Galactic Positioning System 2144 7.03-3.0.1 2026-09-18T15-42Z AeW3Tdr2W"),
            "Galactic Positioning System 2144 7.03-3.0.1 2026-09-18T15-42Z AeW3Tdr2W"
        );
        assert_eq!(archive_stem("/mnt/d/NMSMods/Cheap Paint 3.9-2384-3-9-1741105786"), "Cheap Paint 3.9-2384-3-9-1741105786");
        // A trailing separator must not swallow the name.
        assert_eq!(archive_stem("D:\\NMSMods\\Fast Refiners 2016 7.00 2026-09-09T19-33Z x\\"), "Fast Refiners 2016 7.00 2026-09-09T19-33Z x");
        // Not a path at all: already the stem.
        assert_eq!(archive_stem("Increased S Class Chance"), "Increased S Class Chance");
        assert_eq!(archive_stem(""), "");
    }

    #[test]
    fn a_mod_that_is_switched_off_is_still_named() {
        // It has no files in the game, so it is in no scan and there is no `Mod`
        // to describe it -- which is why the naming takes pairs. This is the mod
        // a person is looking for when they want to switch it back on.
        let map = names_for(
            [(
                "alchemist_GPS".to_string(),
                Some("Galactic Positioning System 2144 7.03-3.0.1 2026-09-18T15-42Z x".to_string()),
            )],
            &Names::default(),
        );
        assert_eq!(map["alchemist_GPS"], "Galactic Positioning System");
    }

    #[test]
    fn a_folder_with_no_archive_at_all_names_itself() {
        let map = names_for([("Increased S Class Chance".to_string(), None)], &Names::default());
        assert_eq!(map["Increased S Class Chance"], "Increased S Class Chance");
    }

    #[test]
    fn the_staging_tree_names_a_mod_the_manifest_no_longer_records() {
        // The live case after a cutover: no manifest, and the mod folder in the
        // game shares nothing with the archive it came from.
        let dir = Dir::new("staged");
        dir.file(
            "Smooth glyphs-3071-1-1-1739735732/MOD.PM_PortalGlyphs/A.EXML",
            "a",
        );
        let staged = staged_archives(&dir.0);
        assert_eq!(
            staged.get("MOD.PM_PortalGlyphs").map(String::as_str),
            Some("Smooth glyphs-3071-1-1-1739735732")
        );

        let m = installed("MOD.PM_PortalGlyphs", None);
        assert_eq!(display_name(&m, &Names::default(), &staged), "Smooth glyphs");
        assert_eq!(mod_id_of(&m, &staged), Some(3071));
    }

    #[test]
    fn a_manifest_outranks_the_staging_tree() {
        // A manager that is still deploying is the authority on what it
        // deployed; the staging tree is what is left once it is gone.
        let dir = Dir::new("both");
        dir.file("Stale 1.0-1-1-0-1738789143/Shelters/A.EXML", "a");
        let staged = staged_archives(&dir.0);
        let m = installed(
            "Shelters",
            Some("Current 3699 2.0 2026-06-24T22-04Z YjQ5seZ66"),
        );
        assert_eq!(mod_id_of(&m, &staged), Some(3699));
    }

    #[test]
    fn a_staging_folder_with_no_mod_in_it_contributes_nothing() {
        let dir = Dir::new("empty");
        dir.file("Just An Archive-1-1-0-1738789143/readme.txt", "hi");
        assert!(staged_archives(&dir.0).is_empty());
    }

    #[test]
    fn a_missing_staging_folder_is_not_an_error() {
        assert!(staged_archives(Path::new("nowhere at all")).is_empty());
    }

    #[test]
    fn a_stamp_is_a_real_date() {
        let s = stamp();
        assert_eq!(s.len(), 19, "{s}");
        assert!(s.starts_with("20"), "{s}");
        assert!(!s.contains(':'), "a colon is illegal in a Windows path: {s}");
    }

    #[test]
    fn without_a_manifest_the_folder_takes_its_namesakes_with_it() {
        let dir = Dir::new("namesakes");
        dir.file("Shelters 1.4/MODELS/A.EXML", "a");
        dir.file("Shelters 1.4.lua", "lua");
        dir.file("Shelters 1.4.txt", "readme");
        // Belongs to a different mod that merely starts with the same words.
        dir.file("Shelters 1.4 Extra/B.EXML", "b");
        dir.file("Unrelated.lua", "no");

        let found = members(&dir.0, "Shelters 1.4");
        assert_eq!(found, vec!["Shelters 1.4", "Shelters 1.4.lua", "Shelters 1.4.txt"]);
    }

    #[test]
    fn the_manifest_finds_files_that_share_no_name_with_the_folder() {
        let dir = Dir::new("manifest");
        dir.file("Shelters 1.4/MODELS/A.EXML", "a");
        dir.file("Shelters 1.4.lua", "lua");
        dir.file("Installation Notes for Shelters.txt", "readme");
        dir.file("Someone Else/B.EXML", "b");
        dir.file(
            VORTEX_MANIFEST,
            r#"{"files":[
                {"relPath":"Shelters 1.4\\MODELS\\A.EXML","source":"Shelters 1.3-2308-1-3-1738789143"},
                {"relPath":"Shelters 1.4.lua","source":"Shelters 1.3-2308-1-3-1738789143"},
                {"relPath":"Installation Notes for Shelters.txt","source":"Shelters 1.3-2308-1-3-1738789143"},
                {"relPath":"Someone Else\\B.EXML","source":"Other-1-1-1.zip"}
            ]}"#,
        );

        let found = members(&dir.0, "Shelters 1.4");
        assert_eq!(
            found,
            vec![
                "Installation Notes for Shelters.txt",
                "Shelters 1.4",
                "Shelters 1.4.lua",
            ],
            "the readme shares no name with the folder and only the manifest knows"
        );
    }

    #[test]
    fn removing_moves_every_byte_and_restoring_puts_it_back() {
        let dir = Dir::new("roundtrip");
        dir.file("Shelters 1.4/MODELS/A.EXML", "contents of a");
        dir.file("Shelters 1.4.lua", "the script");
        let trash_root = dir.0.join("_trash");

        let done = remove(&dir.0, "Shelters 1.4", &trash_root).unwrap();
        assert_eq!(done.moved, vec!["Shelters 1.4", "Shelters 1.4.lua"]);
        assert!(!dir.0.join("Shelters 1.4").exists());
        assert!(!dir.0.join("Shelters 1.4.lua").exists());

        let back = restore(&dir.0, Path::new(&done.trash)).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(
            std::fs::read_to_string(dir.0.join("Shelters 1.4/MODELS/A.EXML")).unwrap(),
            "contents of a"
        );
        assert_eq!(
            std::fs::read_to_string(dir.0.join("Shelters 1.4.lua")).unwrap(),
            "the script"
        );
    }

    #[test]
    fn restoring_over_a_reinstalled_mod_refuses_rather_than_overwriting() {
        let dir = Dir::new("collide");
        dir.file("Mod/A.EXML", "original");
        let trash_root = dir.0.join("_trash");
        let done = remove(&dir.0, "Mod", &trash_root).unwrap();

        dir.file("Mod/A.EXML", "a newer copy the user installed since");
        assert!(restore(&dir.0, Path::new(&done.trash)).is_err());
        assert_eq!(
            std::fs::read_to_string(dir.0.join("Mod/A.EXML")).unwrap(),
            "a newer copy the user installed since"
        );
    }

    #[test]
    fn a_name_that_climbs_out_of_the_mods_folder_is_refused() {
        let dir = Dir::new("escape");
        dir.file("Mod/A.EXML", "a");
        let trash = dir.0.join("_trash");
        assert!(remove(&dir.0, "..", &trash).is_err());
        assert!(remove(&dir.0, "../Binaries", &trash).is_err());
        assert!(remove(&dir.0, "", &trash).is_err());
    }

    #[test]
    fn removing_something_that_is_not_there_says_so() {
        let dir = Dir::new("absent");
        dir.file("Mod/A.EXML", "a");
        assert!(remove(&dir.0, "Not Installed", &dir.0.join("_trash")).is_err());
    }
}
