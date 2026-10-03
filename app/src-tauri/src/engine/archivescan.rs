//! The archives in the archives folder that are not installed yet.
//!
//! This is what makes a library portable as nothing but its downloads. Hand
//! someone the original archives, they drop them into their archives folder,
//! and every one of them is offered for install -- already linked to its Nexus
//! page, because Nexus writes the mod id, the version and the upload time into
//! every archive name (see [`super::nexusname`]). No loadout, no preset and no
//! staging tree has to travel with them.
//!
//! The same folder is where this program keeps its own downloads, so most of
//! what is in it on a working library is installed already, and some of it is
//! an older download of a mod that has since been updated. Neither is offered:
//!
//! - an archive the loadout records is installed;
//! - so is one whose Nexus file -- mod id *and* title, because one page can
//!   host several mods -- is installed at the same upload or a later one;
//! - of several downloads of one Nexus file, only the newest is offered.
//!
//! What is left is either new, or newer than what is installed, and the second
//! kind says which mod it replaces.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use super::{loadout, nexus, nexusname};

/// One archive waiting to be installed.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Waiting {
    /// the full path, which is what the install takes
    pub archive: String,
    /// the file name, for showing
    pub file: String,
    /// what the name says about the mod, when it came from Nexus
    pub mod_id: Option<u64>,
    pub title: Option<String>,
    pub version: Option<String>,
    /// the installed mod this is a newer download of
    pub replaces: Option<String>,
}

const KINDS: [&str; 3] = ["zip", "rar", "7z"];

/// Which Nexus file an archive is a download of, as far as its name says.
fn identity(file: &str) -> Option<(u64, String)> {
    nexusname::parse(file).map(|n| (n.mod_id, n.name.trim().to_lowercase()))
}

/// When an archive was uploaded, as the start of the span its name allows.
///
/// Comparable across both naming conventions, which the raw stamp strings are
/// not: `2026-09-13T06-30Z` and `1757165929` sort in no useful order.
fn uploaded(file: &str) -> Option<i64> {
    nexusname::parse(file)
        .and_then(|n| nexus::upload_span(&n.uploaded))
        .map(|span| span.start)
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Every archive in `dir` that installing would add something for.
pub fn survey(dir: &Path, book: &loadout::Loadout) -> Vec<Waiting> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<String> = entries
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|name| {
            Path::new(name)
                .extension()
                .and_then(|x| x.to_str())
                .is_some_and(|x| KINDS.iter().any(|k| x.eq_ignore_ascii_case(k)))
        })
        .collect();
    files.sort();

    // What is installed, by file name and by Nexus file. Names compared without
    // case: Windows does, and an archive copied over by hand can change it.
    let recorded: std::collections::BTreeSet<String> = book
        .entries
        .iter()
        .filter_map(|e| e.archive.as_deref())
        .map(|a| file_name(a).to_lowercase())
        .collect();
    let mut installed: BTreeMap<(u64, String), (String, Option<i64>)> = BTreeMap::new();
    for entry in &book.entries {
        if entry.variant == loadout::Variant::Merged {
            continue;
        }
        let Some(name) = entry.archive.as_deref().map(file_name) else {
            continue;
        };
        if let Some(key) = identity(&name) {
            installed.insert(key, (entry.owner.clone(), uploaded(&name)));
        }
    }

    // The newest download of each Nexus file; anything unidentifiable stands
    // alone, since there is nothing to say two of them are the same mod.
    let mut newest: BTreeMap<(u64, String), String> = BTreeMap::new();
    let mut loose: Vec<String> = Vec::new();
    for file in files {
        if recorded.contains(&file.to_lowercase()) {
            continue;
        }
        match identity(&file) {
            Some(key) => {
                let keep = match newest.get(&key) {
                    Some(held) => uploaded(&file) > uploaded(held),
                    None => true,
                };
                if keep {
                    newest.insert(key, file);
                }
            }
            None => loose.push(file),
        }
    }

    let mut out = Vec::new();
    for (key, file) in newest {
        let mut replaces = None;
        if let Some((owner, at)) = installed.get(&key) {
            // Installed at the same upload or later: a second copy of what is
            // there (`... (2).zip`), or an old download left behind. Unknown
            // times on either side mean we cannot say it is newer, so it is
            // not offered -- offering it would downgrade as often as upgrade.
            match (uploaded(&file), at) {
                (Some(this), Some(have)) if this > *have => replaces = Some(owner.clone()),
                _ => continue,
            }
        }
        let parsed = nexusname::parse(&file);
        out.push(Waiting {
            archive: dir.join(&file).display().to_string(),
            mod_id: parsed.as_ref().map(|p| p.mod_id),
            title: parsed.as_ref().map(|p| p.name.clone()),
            version: parsed.map(|p| p.version),
            replaces,
            file,
        });
    }
    for file in loose {
        out.push(Waiting {
            archive: dir.join(&file).display().to_string(),
            mod_id: None,
            title: None,
            version: None,
            replaces: None,
            file,
        });
    }
    out.sort_by(|a, b| {
        let key = |w: &Waiting| w.title.clone().unwrap_or_else(|| w.file.clone()).to_lowercase();
        key(a).cmp(&key(b))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(std::path::PathBuf);

    impl Dir {
        fn with(tag: &str, files: &[&str]) -> Dir {
            let path = std::env::temp_dir().join(format!("anomaly_archivescan_{tag}"));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            for f in files {
                std::fs::write(path.join(f), b"x").unwrap();
            }
            Dir(path)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn entry(owner: &str, archive: &str) -> loadout::Entry {
        loadout::Entry {
            owner: owner.into(),
            source: format!("staging/{owner}"),
            origin: None,
            archive: Some(format!(r"D:\NMSAnomaly\archives\{archive}")),
            variant: loadout::Variant::Original,
            replaces: Vec::new(),
            deployed: vec![owner.into()],
            built_from: None,
            enabled: true,
            edited: false,
        }
    }

    fn files(found: &[Waiting]) -> Vec<&str> {
        found.iter().map(|w| w.file.as_str()).collect()
    }

    #[test]
    fn a_fresh_install_is_offered_every_archive_once_linked_to_nexus() {
        let dir = Dir::with(
            "fresh",
            &[
                "Better Rewards 1460 7.05 2026-09-30T17-43Z N2LUkQgvG.zip",
                "Better Freighter Entry and Exit 1.8-1180-1-8-1780817964.zip",
                "notes.txt",
            ],
        );
        let found = survey(&dir.0, &loadout::Loadout::default());
        assert_eq!(found.len(), 2, "{found:?}");
        let rewards = found.iter().find(|w| w.mod_id == Some(1460)).unwrap();
        assert_eq!(rewards.title.as_deref(), Some("Better Rewards"));
        assert_eq!(rewards.version.as_deref(), Some("7.05"));
        assert_eq!(rewards.replaces, None);
        assert!(found.iter().any(|w| w.mod_id == Some(1180) && w.version.as_deref() == Some("1.8")));
    }

    #[test]
    fn what_is_installed_is_not_offered_again_even_as_a_second_copy() {
        let dir = Dir::with(
            "installed",
            &[
                "Refiner Wiki Slots normal 3718 1.4.2 2026-09-29T11-29Z SxEYBfk7E.zip",
                "Refiner Wiki Slots normal 3718 1.4.2 2026-09-29T11-29Z SxEYBfk7E (2).zip",
                "Refiner Wiki Slots normal 3718 1.4.1 2026-09-22T00-46Z lN9fGp87U.zip",
            ],
        );
        let book = loadout::Loadout {
            entries: vec![entry(
                "RefinerWikiSlots_normal",
                "Refiner Wiki Slots normal 3718 1.4.2 2026-09-29T11-29Z SxEYBfk7E.zip",
            )],
        };
        assert!(survey(&dir.0, &book).is_empty());
    }

    #[test]
    fn only_the_newest_download_is_offered_and_it_names_what_it_replaces() {
        let dir = Dir::with(
            "newer",
            &[
                "Refiner Wiki Slots normal 3718 1.4.1 2026-09-22T00-46Z lN9fGp87U.zip",
                "Refiner Wiki Slots normal 3718 1.4.2 2026-09-29T11-29Z SxEYBfk7E.zip",
                "Refiner Wiki Slots legend 3718 1.4.2 2026-09-29T11-30Z L5bXM34EZ.zip",
            ],
        );
        let book = loadout::Loadout {
            entries: vec![entry(
                "RefinerWikiSlots",
                "Refiner Wiki Slots normal 3718 1.4.0 2026-09-22T00-41Z AeW3TdrET.zip",
            )],
        };
        let found = survey(&dir.0, &book);
        assert_eq!(
            files(&found),
            [
                "Refiner Wiki Slots legend 3718 1.4.2 2026-09-29T11-30Z L5bXM34EZ.zip",
                "Refiner Wiki Slots normal 3718 1.4.2 2026-09-29T11-29Z SxEYBfk7E.zip",
            ]
        );
        assert_eq!(found[0].replaces, None, "legend is a different mod on the same page");
        assert_eq!(found[1].replaces.as_deref(), Some("RefinerWikiSlots"));
    }

    #[test]
    fn an_older_download_than_what_is_installed_is_not_a_downgrade_offer() {
        let dir = Dir::with(
            "older",
            &["Refiner Wiki Slots normal 3718 1.4.1 2026-09-22T00-46Z lN9fGp87U.zip"],
        );
        let book = loadout::Loadout {
            entries: vec![entry(
                "RefinerWikiSlots_normal",
                "Refiner Wiki Slots normal 3718 1.4.2 2026-09-29T11-29Z SxEYBfk7E.zip",
            )],
        };
        assert!(survey(&dir.0, &book).is_empty());
    }

    #[test]
    fn an_archive_from_somewhere_else_is_still_offered_just_not_linked() {
        let dir = Dir::with("loose", &["my own tweak.7z"]);
        let found = survey(&dir.0, &loadout::Loadout::default());
        assert_eq!(files(&found), ["my own tweak.7z"]);
        assert_eq!(found[0].mod_id, None);
    }

    #[test]
    fn a_missing_folder_is_nothing_waiting_not_an_error() {
        let found = survey(Path::new(r"Z:\no\such\folder"), &loadout::Loadout::default());
        assert!(found.is_empty());
    }
}
