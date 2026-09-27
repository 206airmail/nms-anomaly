//! The name Nexus shows for a mod, remembered on disk.
//!
//! # Why this exists at all
//!
//! A mod's folder is not its name. The game and every other part of this
//! program refer to a mod by its folder in `GAMEDATA\MODS`, and on a library
//! installed from Nexus that folder is the archive's stem:
//!
//! ```text
//! Buy All Corvette Parts Cosmos 7.01 Non Expedition 4447 1.2 2026-09-11T13-48Z L5bXM34yG
//! gFreighter Perfect Frigates-2468-6-0-5-0a-1758072172
//! ```
//!
//! Nobody recognises their own mods in that list. [`super::nexusname`] can
//! recover the display name Nexus baked into the archive for free and offline,
//! and that is the right fallback -- but it is the name *at the moment that
//! file was uploaded*. An author who renames their mod, or who publishes a new
//! file under a title the old archive never saw, leaves that stale. The name
//! on the page is the true one.
//!
//! # Why it is a cache and not a lookup
//!
//! Asking the API costs one request per mod page. On a sixty-mod library that
//! is sixty requests of an allowance of two thousand an hour -- affordable
//! once, wasteful every launch, and unavailable offline. So the answer is
//! written to disk and never asked for twice: a name is resolved once, and
//! [`Names::missing`] is what the next run asks about, which is normally
//! nothing.
//!
//! There is deliberately no expiry. A cached name that is a month out of date
//! is a mod title, not a version number -- the cost of it being stale is that
//! a row reads slightly wrong, and the cost of expiring it is sixty requests
//! on a timer the user never asked for. [`Names::forget`] exists for when
//! someone wants them fetched again.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// One remembered name.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Named {
    /// the title as the mod's Nexus page gives it
    pub name: String,
    /// unix seconds, for answering "when did we learn this" and nothing else
    #[serde(default)]
    pub at: u64,
}

/// Nexus mod id -> the name its page shows.
///
/// Keyed by id rather than by folder on purpose: two folders can come from one
/// page (`Better Ship Teleport Module Range` and `Better Ship Transfer Range`
/// are both mod 1201), and a folder can be renamed without the page changing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Names {
    /// serialised with string keys, because JSON object keys are strings
    #[serde(default)]
    by_id: BTreeMap<String, Named>,
}

impl Names {
    /// Read the remembered names, or an empty set when there are none yet.
    ///
    /// An unreadable or corrupt file is the same as an empty one: names are a
    /// convenience, and failing to start because a cache did not parse would
    /// be a worse bug than showing folder names for one run.
    pub fn load(path: &Path) -> Names {
        super::read_json(path).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        std::fs::write(path, text)
    }

    pub fn get(&self, mod_id: u64) -> Option<&str> {
        self.by_id.get(&mod_id.to_string()).map(|n| n.name.as_str())
    }

    /// Remember a name. An empty or blank name is not a name, and is ignored:
    /// the API returns `null` for a hidden or deleted page, and writing that
    /// as a title would replace a usable archive name with nothing.
    pub fn put(&mut self, mod_id: u64, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        self.by_id.insert(
            mod_id.to_string(),
            Named {
                name: name.to_string(),
                at: now(),
            },
        );
    }

    /// Which of `wanted` we have never resolved, in a stable order.
    ///
    /// Deduplicated, because several folders routinely share one page and the
    /// caller should not spend two requests on mod 1201.
    pub fn missing(&self, wanted: impl IntoIterator<Item = u64>) -> Vec<u64> {
        let mut out: Vec<u64> = wanted
            .into_iter()
            .filter(|id| self.get(*id).is_none())
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Drop everything, so the next resolve asks Nexus again.
    pub fn forget(&mut self) {
        self.by_id.clear();
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(std::path::PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let path = std::env::current_dir()
                .unwrap()
                .join(format!("target/nmscheck_namecache_{tag}"));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Dir(path)
        }

        fn file(&self) -> std::path::PathBuf {
            self.0.join("names.json")
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_name_survives_a_round_trip_to_disk() {
        let dir = Dir::new("roundtrip");
        let mut names = Names::default();
        names.put(3699, "Accelerated Settlements");
        names.save(&dir.file()).unwrap();

        let back = Names::load(&dir.file());
        assert_eq!(back.get(3699), Some("Accelerated Settlements"));
        assert_eq!(back.len(), 1);
    }

    #[test]
    fn nothing_on_disk_is_an_empty_set_rather_than_a_failure() {
        let dir = Dir::new("absent");
        assert!(Names::load(&dir.file()).is_empty());
    }

    #[test]
    fn a_corrupt_file_is_an_empty_set_rather_than_a_failure() {
        let dir = Dir::new("corrupt");
        std::fs::write(dir.file(), "{ this is not json").unwrap();
        assert!(Names::load(&dir.file()).is_empty());
    }

    #[test]
    fn a_file_written_by_notepad_still_parses() {
        // `read_json` strips the BOM; see engine::read_json on why that
        // mattered enough to cost a loadout once.
        let dir = Dir::new("bom");
        std::fs::write(
            dir.file(),
            "\u{feff}{\"by_id\":{\"1180\":{\"name\":\"Better Freighter Entry and Exit\",\"at\":0}}}",
        )
        .unwrap();
        assert_eq!(
            Names::load(&dir.file()).get(1180),
            Some("Better Freighter Entry and Exit")
        );
    }

    #[test]
    fn only_what_we_have_never_seen_is_asked_about() {
        let mut names = Names::default();
        names.put(3699, "Accelerated Settlements");
        assert_eq!(names.missing([3699, 1180, 2361]), vec![1180, 2361]);
    }

    #[test]
    fn two_folders_from_one_page_cost_one_request() {
        // Both `Better Ship Teleport Module Range` and `Better Ship Transfer
        // Range` are mod 1201. Asking twice would be a wasted request and a
        // duplicated row of work.
        let names = Names::default();
        assert_eq!(names.missing([1201, 1201, 1201]), vec![1201]);
    }

    #[test]
    fn a_blank_name_is_not_remembered_as_one() {
        // The API returns no name for a hidden, deleted or moderated page.
        // Storing that would replace a usable archive-derived name with
        // nothing, which is worse than never having asked.
        let mut names = Names::default();
        names.put(4417, "   ");
        assert!(names.get(4417).is_none());
        assert!(names.is_empty());
    }

    #[test]
    fn a_name_is_stored_trimmed() {
        let mut names = Names::default();
        names.put(4417, "  Salvage Rights\n");
        assert_eq!(names.get(4417), Some("Salvage Rights"));
    }

    #[test]
    fn forgetting_makes_everything_askable_again() {
        let mut names = Names::default();
        names.put(3699, "Accelerated Settlements");
        names.forget();
        assert_eq!(names.missing([3699]), vec![3699]);
    }
}
