//! One scan of the mods folder, shared by everything that needs it.
//!
//! Reading the library is the expensive part of this program: it walks every
//! file in every mod, hashes them, and parses the XML. Start-up used to do
//! that **three** times over -- once for the conflict report, once for the
//! clean preview, once for the update check -- because each command took a
//! folder and scanned it from scratch. Three passes over the same unchanged
//! directory, contending for the same disk.
//!
//! So the scan is done once and kept. The rules are deliberately simple,
//! because a cache that is clever about staleness is a cache that eventually
//! shows someone a mod they deleted:
//!
//! * [`get`] hands back the last scan of that folder, or takes one.
//! * [`refresh`] always scans, and is what "Rescan library" calls.
//! * [`invalidate`] is called by everything that changes the mods folder.
//!
//! There is no time-based expiry. A scan is valid until this program changes
//! something or the user asks for a new one -- and if something outside
//! changes the folder, "Rescan library" is the answer, the same as it was
//! before any of this existed.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use super::discovery;
use super::hostenv::HostInfo;
use super::model::{Mod, ScanStats};

/// Everything one pass over the mods folder produced.
pub struct Scan {
    pub mods: Vec<Mod>,
    pub stats: ScanStats,
    pub host: HostInfo,
    /// the folder this came from, so a different one is never served from it
    pub root: PathBuf,
}

impl Scan {
    /// The mods the game will actually load, in the order it loads them.
    pub fn active(&self) -> Vec<Mod> {
        self.mods.iter().filter(|m| !m.disabled).cloned().collect()
    }
}

fn cell() -> &'static Mutex<Option<Arc<Scan>>> {
    static CACHE: OnceLock<Mutex<Option<Arc<Scan>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// A poisoned lock means some other thread panicked mid-scan. The cached scan
/// is plain data and cannot be half-updated, so taking it anyway is safe and
/// better than propagating a panic into every command.
fn lock() -> std::sync::MutexGuard<'static, Option<Arc<Scan>>> {
    cell().lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn same(a: &Path, b: &Path) -> bool {
    // Windows paths differ in case and in trailing separators without meaning
    // anything by it.
    let tidy = |p: &Path| {
        p.to_string_lossy()
            .trim_end_matches(['/', '\\'])
            .to_lowercase()
            .replace('/', "\\")
    };
    tidy(a) == tidy(b)
}

/// The last scan of `root`, taking one only if there is not one already.
pub fn get(root: &Path) -> std::io::Result<Arc<Scan>> {
    if let Some(have) = lock().as_ref() {
        if same(&have.root, root) {
            return Ok(Arc::clone(have));
        }
    }
    refresh(root)
}

/// Scan `root` now, whatever is cached.
pub fn refresh(root: &Path) -> std::io::Result<Arc<Scan>> {
    // Deliberately not holding the lock across the scan: it takes seconds, and
    // blocking every other command for its duration is worse than the rare
    // case of two scans starting at once and one result being dropped.
    // Flatten everything, keep none of it. This scan outlives every command,
    // so retaining the property maps means paying for them for the life of the
    // process; `propcache::props_of` fetches a file's share on demand.
    let (mods, stats, host) = discovery::scan_roots_with(&[root.to_path_buf()], true, false)?;
    let scan = Arc::new(Scan {
        mods,
        stats,
        host,
        root: root.to_path_buf(),
    });
    *lock() = Some(Arc::clone(&scan));
    Ok(scan)
}

/// Throw the scan away. Called by anything that changes the mods folder.
pub fn invalidate() {
    *lock() = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cache is one global, so these tests cannot run beside each other.
    fn alone() -> std::sync::MutexGuard<'static, ()> {
        static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
        let guard = ONE_AT_A_TIME
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        invalidate();
        guard
    }

    /// A mods folder with one mod in it, cleaned up on drop.
    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let path = std::env::current_dir()
                .unwrap_or_else(|_| std::env::temp_dir())
                .join(format!("target/nmscheck_scancache_{tag}"));
            let _ = std::fs::remove_dir_all(&path);
            let dir = Dir(path);
            dir.mod_named("A Mod");
            dir
        }

        /// A folder only counts as a mod once it holds something the game
        /// would load, so a readme is not enough.
        fn mod_named(&self, name: &str) {
            let at = self.0.join(name);
            std::fs::create_dir_all(&at).unwrap();
            std::fs::write(
                at.join("TABLE.EXML"),
                "<?xml version=\"1.0\"?>
<Data template=\"GcTable\"></Data>
",
            )
            .unwrap();
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            invalidate();
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_second_ask_is_served_from_the_first_scan() {
        let _alone = alone();
        let dir = Dir::new("reuse");
        let first = get(&dir.0).unwrap();
        let second = get(&dir.0).unwrap();
        assert!(
            Arc::ptr_eq(&first, &second),
            "the second call scanned again instead of reusing"
        );
    }

    #[test]
    fn refreshing_really_does_scan_again() {
        let _alone = alone();
        let dir = Dir::new("refresh");
        let first = get(&dir.0).unwrap();
        let second = refresh(&dir.0).unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn a_change_to_the_folder_is_seen_after_invalidating() {
        let _alone = alone();
        let dir = Dir::new("invalidate");
        assert_eq!(get(&dir.0).unwrap().mods.len(), 1);

        dir.mod_named("Another Mod");
        // Still the old answer: nothing told us anything changed.
        assert_eq!(get(&dir.0).unwrap().mods.len(), 1);

        invalidate();
        assert_eq!(get(&dir.0).unwrap().mods.len(), 2);
    }

    #[test]
    fn another_folder_is_never_served_from_this_ones_scan() {
        let _alone = alone();
        let a = Dir::new("folder_a");
        let b = Dir::new("folder_b");
        let first = get(&a.0).unwrap();
        let second = get(&b.0).unwrap();
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(same(&second.root, &b.0));
    }

    #[test]
    fn a_path_that_differs_only_in_case_or_slashes_is_the_same_folder() {
        assert!(same(
            Path::new(r"D:\Games\No Man's Sky\GAMEDATA\MODS"),
            Path::new(r"d:/games/no man's sky/gamedata/mods/")
        ));
        assert!(!same(
            Path::new(r"D:\Games\A\MODS"),
            Path::new(r"D:\Games\B\MODS")
        ));
    }
}
