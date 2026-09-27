//! Keeping copies of the save, because nothing else can rebuild one.
//!
//! Every other thing this program manages is replaceable: a mod can be
//! downloaded again, a merge rebuilt from its parents, a cleaned file thrown away
//! and made afresh. A save cannot. It is also the file most exposed to the risks
//! this program deals in -- a crash mid-write, a mod that makes the game
//! unhappy, a disk that fills up -- and No Man's Sky keeps exactly one copy of it.
//!
//! So: while a session is being recorded, every save the game finishes writing is
//! copied aside, and the last few copies are kept. **This works with the hook
//! installed or not**, because it is only looking at files; the hook merely makes
//! it prompt rather than periodic.
//!
//! Three things the implementation is careful about:
//!
//! **A slot is two files.** The game writes `save3.hg` with `mf_save3.hg`
//! beside it -- data and manifest -- and a restore that mixed one moment's data
//! with another's manifest would be worse than no restore at all. So a copy is
//! taken per *slot*, both files together, into a folder named for the moment.
//!
//! **Never copy a file the game is still writing.** A file touched in the last
//! couple of seconds is left alone; the next pass takes it. A torn copy that
//! looks like a backup is the one outcome worse than having none.
//!
//! **A restore is itself reversible.** Putting a copy back first takes a copy of
//! what is being replaced, so a restore of the wrong moment is undone by
//! restoring the one it made. And it refuses outright while the game is running,
//! because the game holds the save in memory and would write over it on its way
//! out.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{clock, gameproc};

/// How the game names its saves.
const EXT: &str = ".hg";

/// The manifest that goes with a save of the same slot.
const MANIFEST: &str = "mf_";

/// A file this new is probably still being written. Two seconds is long against
/// the write itself -- a 27 MB save takes a few hundred milliseconds -- and short
/// against how often this runs.
const SETTLE_MS: i64 = 2_000;

/// Copies kept per slot when the user has not chosen a number.
///
/// Ten, because this is cheaper than it sounds: measured on a real install, a
/// whole save folder is 1.2 MB across ten files -- No Man's Sky compresses its
/// saves -- so ten copies of every slot costs about twelve megabytes. The limit
/// exists to bound the folder, not to ration it.
pub const KEEP_DEFAULT: u32 = 10;

/// One kept copy of one slot: both of its files, from one moment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Kept {
    /// the account folder the save belongs to, e.g. `st_76561198…`
    pub account: String,
    /// the slot, as the game names it: `save3`, or `save` for the first
    pub slot: String,
    /// when the copy was taken, in unix milliseconds
    pub at_ms: i64,
    /// local time, spelled out
    pub at: String,
    /// where the copy is
    pub path: String,
    /// the files in it, with their sizes
    pub files: Vec<(String, u64)>,
    pub bytes: u64,
    /// true when this copy was taken to make a restore reversible
    pub before_restore: bool,
}

/// What a backup pass did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Passed {
    pub taken: Vec<Kept>,
    /// slots that had not changed since the last copy
    pub unchanged: usize,
    /// copies deleted because they were older than the limit
    pub pruned: usize,
    /// what stopped a slot being copied, in words
    pub problems: Vec<String>,
}

/// Every save folder on this machine, one per account the game has seen.
///
/// The game keeps them under `%APPDATA%\HelloGames\NMS\`, named for the Steam
/// account or `DefaultUser` elsewhere. Any folder with a `.hg` file in it counts,
/// so a layout this code has never seen still gets backed up.
pub fn save_dirs() -> Vec<PathBuf> {
    let Ok(appdata) = std::env::var("APPDATA") else {
        return Vec::new();
    };
    let root = PathBuf::from(appdata).join("HelloGames").join("NMS");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && holds_saves(path))
        .collect();
    found.sort();
    found
}

fn holds_saves(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries.flatten().any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .to_lowercase()
                    .ends_with(EXT)
            })
        })
        .unwrap_or(false)
}

/// The slot a save file belongs to: `mf_save3.hg` and `save3.hg` are both
/// `save3`. `None` for a `.hg` that is not a save at all.
fn slot_of(name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    let stem = lower.strip_suffix(EXT)?;
    let stem = stem.strip_prefix(MANIFEST).unwrap_or(stem);
    if !stem.starts_with("save") {
        return None;
    }
    // `save` on its own is the first slot; anything after it must be its number.
    let tail = &stem[4..];
    if tail.is_empty() || tail.chars().all(|c| c.is_ascii_digit()) {
        Some(stem.to_string())
    } else {
        None
    }
}

/// The files of one slot in a save folder, with their sizes and times.
fn slot_files(dir: &Path, slot: &str) -> Vec<(PathBuf, u64, i64)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(PathBuf, u64, i64)> = entries
        .flatten()
        .filter(|entry| {
            slot_of(&entry.file_name().to_string_lossy()).as_deref() == Some(slot)
        })
        .filter_map(|entry| {
            let facts = entry.metadata().ok()?;
            Some((entry.path(), facts.len(), modified_ms(&facts)))
        })
        .collect();
    found.sort();
    found
}

fn modified_ms(facts: &std::fs::Metadata) -> i64 {
    facts
        .modified()
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_millis() as i64)
        .unwrap_or(0)
}

/// Copy anything that has changed since the last copy, and prune the old ones.
///
/// Safe to call often -- every ten seconds while a game runs, say: a slot that
/// has not changed costs one directory listing and nothing else.
pub fn back_up(dirs: &[PathBuf], root: &Path, keep: u32) -> Passed {
    let mut passed = Passed::default();
    let now = clock::now_ms();

    for dir in dirs {
        let account = dir
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "saves".to_string());

        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut slots: Vec<String> = entries
            .flatten()
            .filter_map(|entry| slot_of(&entry.file_name().to_string_lossy()))
            .collect();
        slots.sort();
        slots.dedup();

        for slot in slots {
            let files = slot_files(dir, &slot);
            if files.is_empty() {
                continue;
            }
            // Mid-write: come back to it.
            if files.iter().any(|(_, _, when)| now - *when < SETTLE_MS) {
                continue;
            }
            let newest = files.iter().map(|(_, _, when)| *when).max().unwrap_or(0);
            let here: Vec<(String, u64)> = files
                .iter()
                .map(|(path, len, _)| {
                    (
                        path.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default(),
                        *len,
                    )
                })
                .collect();

            // Unchanged is the common case, and is decided from what is already
            // on disk rather than from anything remembered in memory -- so it
            // still holds after a restart, and cannot drift from the truth.
            let mine = kept_for(root, &account, &slot);
            if mine
                .first()
                .map(|last| last.files == here && last.at_ms >= newest)
                .unwrap_or(false)
            {
                passed.unchanged += 1;
                continue;
            }

            match take_copy(&files, root, &account, &slot, newest, false) {
                Ok(kept) => passed.taken.push(kept),
                Err(why) => passed.problems.push(why),
            }
            passed.pruned += prune(root, &account, &slot, keep);
        }
    }
    passed
}

/// What a copy records about itself, beside the files.
///
/// The folder is named for the save's own moment, which is what a person looks
/// for -- but a name is not a record: two saves written inside the same second
/// would want the same name, and a folder's own timestamp says when it was
/// *copied*, not what it holds. Comparing the copy time against the save's would
/// then call every unchanged slot changed, or every changed one unchanged,
/// depending on which way the clocks fell.
#[derive(Serialize, Deserialize)]
struct Note {
    /// the save's modification time when this copy was taken
    source_ms: i64,
    #[serde(default)]
    before_restore: bool,
}

/// The note's file name, which is never one of the save's own files.
const NOTE: &str = "taken.json";

/// Copy one slot's files into a folder named for the save's own moment.
fn take_copy(
    files: &[(PathBuf, u64, i64)],
    root: &Path,
    account: &str,
    slot: &str,
    when_ms: i64,
    before_restore: bool,
) -> Result<Kept, String> {
    let stamp = clock::local(when_ms).stamp();
    let base = if before_restore {
        format!("{stamp}_replaced")
    } else {
        stamp
    };
    let slot_dir = root.join(account).join(slot);
    // Two saves in one second, or two copies of one moment: the second gets its
    // own folder rather than overwriting the first.
    let mut into = slot_dir.join(&base);
    let mut nth = 2;
    while into.exists() {
        into = slot_dir.join(format!("{base}_{nth}"));
        nth += 1;
    }
    std::fs::create_dir_all(&into).map_err(|err| format!("could not make {}: {err}", into.display()))?;

    let mut copied = Vec::new();
    let mut bytes = 0;
    for (path, _, _) in files {
        let Some(file) = path.file_name() else { continue };
        let to = into.join(file);
        let size = std::fs::copy(path, &to)
            .map_err(|err| format!("could not copy {}: {err}", path.display()))?;
        copied.push((file.to_string_lossy().to_string(), size));
        bytes += size;
    }
    let note = Note {
        source_ms: when_ms,
        before_restore,
    };
    if let Ok(text) = serde_json::to_string(&note) {
        let _ = std::fs::write(into.join(NOTE), text);
    }
    Ok(Kept {
        account: account.to_string(),
        slot: slot.to_string(),
        at_ms: when_ms,
        at: clock::local(when_ms).written(),
        path: into.display().to_string(),
        files: copied,
        bytes,
        before_restore,
    })
}

/// Copies of one slot, newest first.
fn kept_for(root: &Path, account: &str, slot: &str) -> Vec<Kept> {
    let dir = root.join(account).join(slot);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut found: Vec<Kept> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| read_copy(&entry.path(), account, slot))
        .collect();
    found.sort_by_key(|kept| std::cmp::Reverse(kept.at_ms));
    found
}

/// Read one copy off disk: its note says what it holds, the rest is the copy.
fn read_copy(dir: &Path, account: &str, slot: &str) -> Option<Kept> {
    let name = dir.file_name()?.to_string_lossy().to_string();
    let note: Option<Note> = super::read_json(&dir.join(NOTE));
    let before_restore = note
        .as_ref()
        .map(|note| note.before_restore)
        .unwrap_or_else(|| name.contains("_replaced"));
    let mut files: Vec<(String, u64)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let facts = entry.metadata().ok()?;
            let name = entry.file_name().to_string_lossy().to_string();
            // The note is ours, not the game's: it must never be put back into a
            // save folder by a restore.
            (facts.is_file() && name != NOTE).then_some((name, facts.len()))
        })
        .collect();
    if files.is_empty() {
        return None;
    }
    // The save's own moment, from the note. A copy written by an older build has
    // none, and then the folder's time is the best available answer.
    let at_ms = note
        .map(|note| note.source_ms)
        .or_else(|| std::fs::metadata(dir).ok().map(|f| modified_ms(&f)))
        .unwrap_or(0);
    files.sort();
    Some(Kept {
        account: account.to_string(),
        slot: slot.to_string(),
        at_ms,
        at: clock::local(at_ms).written(),
        path: dir.display().to_string(),
        bytes: files.iter().map(|(_, len)| len).sum(),
        files,
        before_restore,
    })
}

/// Delete the copies of one slot past the newest `keep`.
///
/// A copy taken to make a restore reversible is never pruned by count: it is the
/// way back from a decision, and the whole point is that it is there later.
fn prune(root: &Path, account: &str, slot: &str, keep: u32) -> usize {
    let mine = kept_for(root, account, slot);
    let mut gone = 0;
    let mut kept = 0;
    for copy in mine {
        if copy.before_restore {
            continue;
        }
        kept += 1;
        if kept > keep.max(1) && std::fs::remove_dir_all(&copy.path).is_ok() {
            gone += 1;
        }
    }
    gone
}

/// Every kept copy, newest first, across every account and slot.
pub fn list(root: &Path) -> Vec<Kept> {
    let Ok(accounts) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for account in accounts.flatten().filter(|e| e.path().is_dir()) {
        let name = account.file_name().to_string_lossy().to_string();
        let Ok(slots) = std::fs::read_dir(account.path()) else {
            continue;
        };
        for slot in slots.flatten().filter(|e| e.path().is_dir()) {
            let slot_name = slot.file_name().to_string_lossy().to_string();
            found.extend(kept_for(root, &name, &slot_name));
        }
    }
    found.sort_by_key(|kept| std::cmp::Reverse(kept.at_ms));
    found
}

/// What is on disk in total, for a screen that offers to delete some of it.
pub fn total_bytes(root: &Path) -> u64 {
    list(root).iter().map(|kept| kept.bytes).sum()
}

/// Delete one kept copy.
pub fn forget(kept: &Kept) -> Result<(), String> {
    std::fs::remove_dir_all(&kept.path)
        .map_err(|err| format!("could not delete {}: {err}", kept.path))
}

/// Put a kept copy back, and keep what it replaced.
///
/// Refused while the game is running: No Man's Sky holds the save in memory and
/// writes it out as it goes, so a file swapped underneath it would be overwritten
/// within minutes -- and the player would have been told it worked.
pub fn restore(kept: &Kept, dirs: &[PathBuf], root: &Path) -> Result<Vec<String>, String> {
    if let Some(pid) = gameproc::find() {
        return Err(format!(
            "{} is running (process {pid}). Close the game first -- it keeps the save in memory \
             and would write over anything put back now.",
            gameproc::GAME_EXE
        ));
    }
    let live = dirs
        .iter()
        .find(|dir| {
            dir.file_name()
                .map(|name| name.to_string_lossy() == kept.account)
                .unwrap_or(false)
        })
        .ok_or_else(|| {
            format!(
                "the save folder this copy came from is not there any more ({})",
                kept.account
            )
        })?;

    // What is about to be replaced, kept first, so this is reversible.
    let now = slot_files(live, &kept.slot);
    if !now.is_empty() {
        let when = now.iter().map(|(_, _, at)| *at).max().unwrap_or(0);
        take_copy(&now, root, &kept.account, &kept.slot, when, true)?;
    }

    let from = PathBuf::from(&kept.path);
    let mut put_back = Vec::new();
    for (file, _) in &kept.files {
        let source = from.join(file);
        let to = live.join(file);
        std::fs::copy(&source, &to)
            .map_err(|err| format!("could not put {} back: {err}", to.display()))?;
        put_back.push(to.display().to_string());
    }
    Ok(put_back)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let path = std::env::temp_dir().join(format!("nmscheck_savewatch_{tag}"));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Dir(path)
        }

        fn save(&self, rel: &str, body: &str) -> PathBuf {
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

    /// Make a file look older than the settle window, so a pass will take it.
    fn age(path: &Path) {
        age_by(path, 60);
    }

    fn age_by(path: &Path, seconds: u64) {
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(seconds);
        let file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
        file.set_modified(old).unwrap();
    }

    #[test]
    fn a_manifest_belongs_to_the_same_slot_as_its_save() {
        assert_eq!(slot_of("save3.hg").as_deref(), Some("save3"));
        assert_eq!(slot_of("mf_save3.hg").as_deref(), Some("save3"));
        // The first slot has no number at all.
        assert_eq!(slot_of("save.hg").as_deref(), Some("save"));
        assert_eq!(slot_of("MF_SAVE12.HG").as_deref(), Some("save12"));
        assert!(slot_of("something.hg").is_none());
        assert!(slot_of("savegame_backup.hg").is_none());
        assert!(slot_of("save3.txt").is_none());
    }

    #[test]
    fn both_files_of_a_slot_are_copied_together() {
        // The trap: a restore that mixed one moment's data with another moment's
        // manifest would be worse than having no copy at all.
        let dir = Dir::new("pair");
        let live = dir.0.join("st_1");
        age(&dir.save("st_1/save3.hg", "data"));
        age(&dir.save("st_1/mf_save3.hg", "manifest"));
        age(&dir.save("st_1/save7.hg", "other slot"));
        age(&dir.save("st_1/mf_save7.hg", "other manifest"));
        let root = dir.0.join("backups");

        let passed = back_up(&[live], &root, 5);
        assert_eq!(passed.taken.len(), 2, "one copy per slot: {:?}", passed);
        let slot3 = passed.taken.iter().find(|k| k.slot == "save3").unwrap();
        assert_eq!(slot3.files.len(), 2);
        assert!(slot3.files.iter().any(|(name, _)| name == "save3.hg"));
        assert!(slot3.files.iter().any(|(name, _)| name == "mf_save3.hg"));
    }

    #[test]
    fn a_slot_that_has_not_changed_is_not_copied_again() {
        let dir = Dir::new("unchanged");
        let live = dir.0.join("st_1");
        age(&dir.save("st_1/save1.hg", "data"));
        let root = dir.0.join("backups");

        assert_eq!(back_up(&[live.clone()], &root, 5).taken.len(), 1);
        let again = back_up(&[live], &root, 5);
        assert!(again.taken.is_empty(), "{:?}", again);
        assert_eq!(again.unchanged, 1);
    }

    #[test]
    fn a_changed_save_is_copied_again_and_both_copies_are_kept() {
        let dir = Dir::new("changed");
        let live = dir.0.join("st_1");
        let path = dir.save("st_1/save1.hg", "first");
        age(&path);
        let root = dir.0.join("backups");
        back_up(&[live.clone()], &root, 5);

        std::fs::write(&path, "second, and longer").unwrap();
        age_by(&path, 5);   // later than the first copy, still past the settle window
        let passed = back_up(&[live], &root, 5);
        assert_eq!(passed.taken.len(), 1, "{:?}", passed);
        assert_eq!(list(&root).len(), 2);
    }

    #[test]
    fn a_file_the_game_is_still_writing_is_left_for_the_next_pass() {
        let dir = Dir::new("midwrite");
        let live = dir.0.join("st_1");
        dir.save("st_1/save1.hg", "being written right now");
        let root = dir.0.join("backups");

        let passed = back_up(&[live], &root, 5);
        assert!(
            passed.taken.is_empty(),
            "a torn copy that looks like a backup is worse than none"
        );
    }

    #[test]
    fn only_the_newest_copies_are_kept() {
        let dir = Dir::new("prune");
        let live = dir.0.join("st_1");
        let path = dir.save("st_1/save1.hg", "0");
        let root = dir.0.join("backups");

        for round in 1..=4 {
            std::fs::write(&path, format!("body {round}")).unwrap();
            age(&path);
            // Each copy needs its own moment, since the moment names the folder.
            let when = clock::now_ms() - (5 - round as i64) * 60_000;
            let files = slot_files(&live, "save1");
            take_copy(&files, &root, "st_1", "save1", when, false).unwrap();
        }
        assert_eq!(list(&root).len(), 4);
        assert_eq!(prune(&root, "st_1", "save1", 2), 2);
        assert_eq!(list(&root).len(), 2);
    }

    #[test]
    fn restoring_keeps_what_it_replaced() {
        let dir = Dir::new("restore");
        let live = dir.0.join("st_1");
        let data = dir.save("st_1/save1.hg", "the good save");
        let manifest = dir.save("st_1/mf_save1.hg", "the good manifest");
        age(&data);
        age(&manifest);
        let root = dir.0.join("backups");
        let kept = back_up(&[live.clone()], &root, 5).taken.remove(0);

        std::fs::write(&data, "a later, worse save").unwrap();
        if gameproc::find().is_some() {
            return; // the real game is running on this machine
        }
        let put_back = restore(&kept, &[live.clone()], &root).unwrap();
        assert_eq!(put_back.len(), 2);
        assert_eq!(std::fs::read_to_string(&data).unwrap(), "the good save");

        // And the copy it replaced is there, so the restore itself can be undone.
        let replaced: Vec<Kept> = list(&root).into_iter().filter(|k| k.before_restore).collect();
        assert_eq!(replaced.len(), 1);
        let body = std::fs::read_to_string(
            PathBuf::from(&replaced[0].path).join("save1.hg"),
        )
        .unwrap();
        assert_eq!(body, "a later, worse save");
    }

    #[test]
    fn a_copy_taken_before_a_restore_is_not_pruned_away() {
        let dir = Dir::new("keepreplaced");
        let live = dir.0.join("st_1");
        let path = dir.save("st_1/save1.hg", "body");
        age(&path);
        let root = dir.0.join("backups");
        let files = slot_files(&live, "save1");
        take_copy(&files, &root, "st_1", "save1", clock::now_ms() - 60_000, true).unwrap();
        take_copy(&files, &root, "st_1", "save1", clock::now_ms(), false).unwrap();

        assert_eq!(prune(&root, "st_1", "save1", 1), 0);
        assert_eq!(list(&root).len(), 2, "the way back out of a restore stays");
    }

    #[test]
    fn restoring_into_an_account_that_is_gone_says_so() {
        let dir = Dir::new("gone");
        let kept = Kept {
            account: "st_nobody".into(),
            slot: "save1".into(),
            at_ms: 0,
            at: "then".into(),
            path: dir.0.display().to_string(),
            files: vec![("save1.hg".into(), 4)],
            bytes: 4,
            before_restore: false,
        };
        if gameproc::find().is_some() {
            return;
        }
        let err = restore(&kept, &[], &dir.0).unwrap_err();
        assert!(err.contains("not there any more"), "{err}");
    }
}
