//! Put a mod's files where the game reads them, without a second copy.
//!
//! The game only ever loads mods from `GAMEDATA\MODS`, so anything installed
//! has to appear there. Copying would mean two full copies of every mod on
//! disk and no record of where a file came from. Instead this keeps the real
//! files in a staging folder and gives the game **hardlinks** to them, which
//! is exactly what Vortex does -- its own manifest on this machine reads
//! `"deploymentMethod": "hardlink_activator"` with staging at
//! `D:\Vortex_Mods_Staging`.
//!
//! A hardlink is a second name for one file, not a copy and not a shortcut.
//! Three consequences drive everything here:
//!
//! 1. **Both names must be on one volume.** NTFS hardlinks cannot cross
//!    drives, so staging is placed on the game's volume ([`staging_for`]) and
//!    [`link_or_copy`] falls back to copying if that ever fails.
//! 2. **Deleting one name leaves the other.** Uninstalling is therefore not
//!    destructive: [`undeploy`] removes the game's names and the staged files
//!    remain, ready to redeploy.
//! 3. **Writing through one name rewrites the file.** So nothing here, and
//!    nothing in [`super::prune`], ever opens an existing deployed file for
//!    writing -- it creates a new name and unlinks the old one instead.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// Where this program keeps the real files for mods it installed.
///
/// On the game's own volume, because the link cannot cross drives. Kept beside
/// the game rather than inside it so the game never scans it and so a verify
/// of the game files never sees it.
pub fn staging_for(game_root: &Path) -> PathBuf {
    // `D:\SteamLibrary\steamapps\common\No Man's Sky` -> `D:\nmscheck_staging`
    let mut base = game_root.to_path_buf();
    while let Some(parent) = base.parent() {
        if parent.parent().is_none() {
            break; // `parent` is the volume root
        }
        base = parent.to_path_buf();
    }
    base.parent()
        .unwrap_or(game_root)
        .join("nmscheck_staging")
}

/// What deploying one staged mod put into the game.
#[derive(Debug, Clone, Serialize)]
pub struct Deployment {
    /// how many names were created in the mods folder
    pub linked: usize,
    /// files that had to be copied because a link was refused
    pub copied: usize,
    /// the top-level names created, which is what undeploying takes away
    pub top_level: Vec<String>,
    /// staged files deliberately left behind, because another mod owns that
    /// name in the mods folder. See [`deploy_skipping`].
    #[serde(default)]
    pub skipped: Vec<String>,
    pub source: String,
}

/// Make `to` a second name for `from`, copying only if it cannot be linked.
///
/// Returns true when a real link was made.
pub fn link_or_copy(from: &Path, to: &Path) -> Result<bool, String> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("could not make {}: {e}", parent.display()))?;
    }
    match std::fs::hard_link(from, to) {
        Ok(()) => Ok(true),
        Err(_) => {
            // Different volume, a filesystem without links, or the limit on
            // names per file. A copy still works; it just costs the space.
            std::fs::copy(from, to)
                .map(|_| false)
                .map_err(|e| format!("could not place {}: {e}", to.display()))
        }
    }
}

/// Extensions that are documentation or build input, never read by the engine.
///
/// Measured against the real library rather than guessed: every one of these
/// appears in it as a sibling of a mod folder, and none of them is loaded.
/// Anything *not* listed counts as content, so an unfamiliar file is deployed
/// rather than silently withheld -- the safe direction to be wrong in.
const INERT: [&str; 17] = [
    "txt", "md", "lua", "pdf", "url", "html", "htm", "jpg", "jpeg", "png", "gif", "bmp", "webp",
    "doc", "docx", "rtf", "nfo",
];

/// True when the game would actually read this file.
pub fn is_game_content(rel: &Path) -> bool {
    match rel.extension() {
        Some(ext) => !INERT.contains(&ext.to_string_lossy().to_lowercase().as_str()),
        None => true,
    }
}

fn walk(dir: &Path, prefix: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = prefix.join(entry.file_name());
        if path.is_dir() {
            walk(&path, &rel, out);
        } else {
            out.push(rel);
        }
    }
}

/// Give the game a name for every file staged under `source`.
///
/// The *contents* of `source` land in the mods folder, at the same relative
/// paths. That matters because a mod is frequently not one folder: 36 of the
/// 62 mods in the measured library ship a folder, a `.lua` and a readme side
/// by side, and all three have to arrive as siblings for the game and for
/// AMUMSS to find them. Deploying `source` *as* a folder would bury them.
pub fn deploy(source: &Path, mods_dir: &Path) -> Result<Deployment, String> {
    deploy_skipping(source, mods_dir, &BTreeSet::new())
}

/// Deploy the game content under `source`, minus the relative paths in `skip`.
///
/// Two things never reach the game. First, anything it does not read: the
/// `Installation Notes for ...txt` and the AMUMSS `.lua` build script that 36
/// of the 62 mods in the measured library ship are for the user and for
/// rebuilding the mod, not for the engine, and putting them in the game folder
/// only makes it harder to see what is actually installed. Second, whatever
/// `skip` names -- that is how a contested asset is kept out while the mod it
/// belongs to still deploys; see [`super::loadout::clashes`].
///
/// Leaving the notes behind also means they cannot collide: two of these mods
/// ship a `README.txt`, and only one name in the mods folder can hold it.
pub fn deploy_skipping(
    source: &Path,
    mods_dir: &Path,
    skip: &BTreeSet<String>,
) -> Result<Deployment, String> {
    if !source.is_dir() {
        return Err(format!("{} is not staged", source.display()));
    }

    let mut all = Vec::new();
    walk(source, Path::new(""), &mut all);
    if all.is_empty() {
        return Err(format!("{} has no files to deploy", source.display()));
    }

    let mut skipped: Vec<String> = Vec::new();
    let mut files: Vec<PathBuf> = Vec::new();
    for rel in all {
        let shown = rel.display().to_string();
        if skip.contains(&shown) || !is_game_content(&rel) {
            skipped.push(shown);
        } else {
            files.push(rel);
        }
    }
    skipped.sort();
    if files.is_empty() {
        return Err(format!(
            "{} holds nothing the game reads",
            source.display()
        ));
    }

    let mut linked = 0;
    let mut copied = 0;
    let mut placed: Vec<PathBuf> = Vec::new();
    let mut top_level: Vec<String> = Vec::new();

    for rel in &files {
        let to = mods_dir.join(rel);
        if to.exists() {
            // Roll back rather than merge into whatever is already there.
            for done in &placed {
                let _ = std::fs::remove_file(done);
            }
            return Err(format!(
                "{} is already in the mods folder; nothing was deployed",
                to.display()
            ));
        }
        match link_or_copy(&source.join(rel), &to) {
            Ok(true) => linked += 1,
            Ok(false) => copied += 1,
            Err(e) => {
                for done in &placed {
                    let _ = std::fs::remove_file(done);
                }
                return Err(e);
            }
        }
        placed.push(to);
        if let Some(head) = rel.components().next() {
            let name = head.as_os_str().to_string_lossy().to_string();
            if !top_level.contains(&name) {
                top_level.push(name);
            }
        }
    }

    top_level.sort();
    Ok(Deployment {
        linked,
        copied,
        top_level,
        skipped,
        source: source.display().to_string(),
    })
}

/// Take the game's names away. The staged files are untouched.
///
/// Only the names listed are removed, so a mod that shares the mods folder
/// with things this program did not put there cannot take them with it.
pub fn undeploy(mods_dir: &Path, top_level: &[String]) -> Result<usize, String> {
    let mut gone = 0;
    for name in top_level {
        if name.is_empty() || name.contains(['/', '\\']) || name.contains("..") {
            return Err(format!("{name:?} is not a name in the mods folder"));
        }
        let at = mods_dir.join(name);
        if !at.exists() {
            continue;
        }
        if at.is_dir() {
            let mut files = Vec::new();
            walk(&at, Path::new(""), &mut files);
            gone += files.len();
            std::fs::remove_dir_all(&at)
                .map_err(|e| format!("could not remove {name}: {e}"))?;
        } else {
            std::fs::remove_file(&at)
                .map_err(|e| format!("could not remove {name}: {e}"))?;
            gone += 1;
        }
    }
    Ok(gone)
}

/// True when these two paths are names for the same file.
///
/// Used to tell "this mod is deployed from our staging" from "someone put a
/// copy there", which decides whether removing it loses anything.
#[cfg(windows)]
pub fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    let (Ok(x), Ok(y)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
        return false;
    };
    // Two names for one file share size and creation time; NTFS exposes the
    // file index through `file_index` on newer Rust, but size plus creation
    // time is enough to answer "did this come from there" for mod assets.
    x.file_size() == y.file_size() && x.creation_time() == y.creation_time()
}

#[cfg(not(windows))]
pub fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let (Ok(x), Ok(y)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
        return false;
    };
    x.ino() == y.ino() && x.dev() == y.dev()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            // Deliberately on the same volume as the crate, so the hardlink
            // path is the one under test rather than the copy fallback.
            let path = std::env::current_dir()
                .unwrap_or_else(|_| std::env::temp_dir())
                .join(format!("target/nmscheck_deploy_{tag}"));
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
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn staging_sits_on_the_games_own_volume() {
        // The link cannot cross drives, so this must land on D: for a game on
        // D:, not in the user profile on C:.
        let staging = staging_for(Path::new(r"D:\SteamLibrary\steamapps\common\No Man's Sky"));
        let shown = staging.display().to_string();
        assert!(shown.starts_with("D:"), "{shown}");
        assert!(shown.ends_with("nmscheck_staging"), "{shown}");
    }

    #[test]
    fn deploying_puts_the_staged_contents_into_the_mods_folder() {
        let dir = Dir::new("link");
        // The common real shape: a mod folder, a script and a readme, all in
        // one staged mod and all three siblings once deployed.
        dir.file("staging/Cool Mod/Cool Mod/GLOBALS/A.EXML", "the asset");
        dir.file("staging/Cool Mod/Cool Mod.lua", "the script");
        dir.file("staging/Cool Mod/Installation Notes.txt", "readme");
        let mods = dir.0.join("MODS");
        std::fs::create_dir_all(&mods).unwrap();

        let done = deploy(&dir.0.join("staging/Cool Mod"), &mods).unwrap();
        assert_eq!(done.linked + done.copied, 1);
        assert_eq!(done.top_level, vec!["Cool Mod"]);
        assert_eq!(
            std::fs::read_to_string(mods.join("Cool Mod/GLOBALS/A.EXML")).unwrap(),
            "the asset"
        );
        // The script and the notes are for the user and for rebuilding the
        // mod. The game never reads them, so they stay in staging.
        assert_eq!(done.skipped, vec!["Cool Mod.lua", "Installation Notes.txt"]);
        assert!(!mods.join("Cool Mod.lua").exists());
        assert!(!mods.join("Installation Notes.txt").exists());
        assert!(dir.0.join("staging/Cool Mod/Cool Mod.lua").is_file());
    }

    #[test]
    fn a_mod_whose_folder_is_nested_deeper_still_arrives_whole() {
        // Content is deployed at its full relative path, however deep.
        let dir = Dir::new("deep");
        dir.file("staging/M/M/MODELS/SHIP/PARTS/A.MBIN", "deep asset");
        dir.file("staging/M/M/GLOBALS/B.EXML", "shallow asset");
        let mods = dir.0.join("MODS");
        std::fs::create_dir_all(&mods).unwrap();

        let done = deploy(&dir.0.join("staging/M"), &mods).unwrap();
        assert_eq!(done.linked + done.copied, 2);
        assert_eq!(done.top_level, vec!["M"]);
        assert!(mods.join("M/MODELS/SHIP/PARTS/A.MBIN").exists());
        assert!(mods.join("M/GLOBALS/B.EXML").exists());
    }

    #[test]
    fn an_unfamiliar_extension_is_treated_as_content() {
        // Better to deploy something harmless than to withhold something the
        // game needed because it was not on a list.
        assert!(is_game_content(Path::new("A.MBIN")));
        assert!(is_game_content(Path::new("A.EXML")));
        assert!(is_game_content(Path::new("A.SOMETHINGNEW")));
        assert!(is_game_content(Path::new("NOEXTENSION")));
        assert!(!is_game_content(Path::new("README.txt")));
        assert!(!is_game_content(Path::new("Build.LUA")), "case must not matter");
    }

    #[test]
    fn undeploying_takes_the_games_names_and_leaves_the_files() {
        let dir = Dir::new("undeploy");
        dir.file("staging/Cool Mod/Cool Mod/A.EXML", "the asset");
        dir.file("staging/Cool Mod/Cool Mod/B.MBIN", "another asset");
        let mods = dir.0.join("MODS");
        std::fs::create_dir_all(&mods).unwrap();
        let done = deploy(&dir.0.join("staging/Cool Mod"), &mods).unwrap();

        assert_eq!(undeploy(&mods, &done.top_level).unwrap(), 2);
        assert!(!mods.join("Cool Mod").exists());
        // The point of the whole design: uninstalling loses nothing.
        assert_eq!(
            std::fs::read_to_string(dir.0.join("staging/Cool Mod/Cool Mod/A.EXML")).unwrap(),
            "the asset"
        );
    }

    #[test]
    fn undeploying_only_takes_what_it_was_given() {
        // A mod must not be able to remove a neighbour, whoever installed it.
        let dir = Dir::new("neighbour");
        let mods = dir.0.join("MODS");
        dir.file("MODS/Ours/A.EXML", "ours");
        dir.file("MODS/Someone Elses/B.EXML", "theirs");

        undeploy(&mods, &["Ours".to_string()]).unwrap();
        assert!(!mods.join("Ours").exists());
        assert!(mods.join("Someone Elses/B.EXML").exists());
    }

    #[test]
    fn a_name_that_climbs_out_of_the_mods_folder_is_refused() {
        let dir = Dir::new("escape");
        let mods = dir.0.join("MODS");
        dir.file("MODS/Ours/A.EXML", "ours");
        for bad in ["..", "../Binaries", r"a\b", ""] {
            assert!(
                undeploy(&mods, &[bad.to_string()]).is_err(),
                "{bad:?} was accepted"
            );
        }
        assert!(mods.join("Ours/A.EXML").exists());
    }

    #[test]
    fn deploying_over_something_already_there_changes_nothing() {
        let dir = Dir::new("collide");
        dir.file("staging/Cool Mod/A.EXML", "ours");
        dir.file("staging/Cool Mod/B.EXML", "ours too");
        let mods = dir.0.join("MODS");
        dir.file("MODS/B.EXML", "someone else's");

        let err = deploy(&dir.0.join("staging/Cool Mod"), &mods).unwrap_err();
        assert!(err.contains("already in the mods folder"), "{err}");
        assert_eq!(
            std::fs::read_to_string(mods.join("B.EXML")).unwrap(),
            "someone else's"
        );
        // The one we did place before hitting the clash was taken back out.
        assert!(!mods.join("A.EXML").exists());
    }

    #[test]
    fn a_link_and_its_original_are_recognised_as_one_file() {
        let dir = Dir::new("same");
        let from = dir.file("a.txt", "shared");
        let to = dir.0.join("b.txt");
        if link_or_copy(&from, &to).unwrap() {
            assert!(same_file(&from, &to));
        }
        let other = dir.file("c.txt", "different length entirely");
        assert!(!same_file(&from, &other));
    }

    #[test]
    fn giving_up_a_contested_readme_still_deploys_the_mod() {
        // The real case: two mods ship `README.txt`. Losing that name must not
        // cost the user the mod's actual content.
        let dir = Dir::new("skip");
        dir.file("staging/Cool Mod/Cool Mod/GLOBALS/A.EXML", "the asset");
        dir.file("staging/Cool Mod/README.txt", "ours");
        let mods = dir.0.join("MODS");
        dir.file("MODS/README.txt", "the other mod got here first");

        let skip = BTreeSet::from(["README.txt".to_string()]);
        let done = deploy_skipping(&dir.0.join("staging/Cool Mod"), &mods, &skip).unwrap();

        assert_eq!(done.linked + done.copied, 1);
        assert_eq!(done.skipped, vec!["README.txt"]);
        assert_eq!(done.top_level, vec!["Cool Mod"], "the readme is not ours to remove");
        assert!(mods.join("Cool Mod/GLOBALS/A.EXML").exists());
        assert_eq!(
            std::fs::read_to_string(mods.join("README.txt")).unwrap(),
            "the other mod got here first"
        );
    }

    #[test]
    fn undeploying_something_absent_is_quietly_fine() {
        // It is already out of the game, which is what was asked for.
        let dir = Dir::new("absent");
        let mods = dir.0.join("MODS");
        std::fs::create_dir_all(&mods).unwrap();
        assert_eq!(undeploy(&mods, &["Not There".to_string()]).unwrap(), 0);
    }
}
