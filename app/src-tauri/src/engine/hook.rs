//! The recorder's other half: a DLL that has to sit inside the game.
//!
//! No Man's Sky writes almost nothing to `FullLog.txt`, so the only way to find
//! out which mod files it actually opened, what it complained about, and where
//! it crashed is to be *inside* the process while it runs. That is what
//! `xinput9_1_0.dll` does -- it is loaded by the game's own import of XInput,
//! forwards every XInput call on to the real one in System32, and reports what
//! it sees down a pipe to this program. Its source is in `hook/` at the top of
//! this repository; the built DLL ships beside the other tools.
//!
//! **Why that file name.** `NMS.exe` imports `xinput9_1_0.dll` and Windows
//! resolves an import from the program's own folder before System32, so a copy
//! there is loaded with no injector, no launcher and no change to how the user
//! starts the game. It is also the slot other overlay tools reach for, which is
//! the one hazard this module exists to handle carefully.
//!
//! Three rules, each of them a way this could otherwise ruin someone's install:
//!
//! **Never overwrite a DLL that is not ours.** A foreign `xinput9_1_0.dll` is
//! another tool's hook, and replacing it would break that tool silently. Ours is
//! recognised by a marker string compiled into it -- not by size, date or
//! version, which a rebuild changes. Installing over a foreign one happens only
//! when asked twice, and backs the original up first.
//!
//! **Never write while the game is running.** The loader holds the file open, so
//! the copy would fail -- and on the uninstall side, a delete that "succeeds"
//! against an open handle leaves the DLL live in the running process.
//!
//! **Say what is there, don't guess.** [`State`] carries the whole picture,
//! including who owns the slot and whether the copy matches the one we ship, so
//! the screen can explain rather than offer a button that will fail.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{gameproc, tools};

/// The name the game's import table asks for.
pub const DLL_NAME: &str = "xinput9_1_0.dll";

/// Where the built hook sits among the bundled tools.
const SHIPPED: &str = "nmslogger/xinput9_1_0.dll";

/// Point this at a hook DLL to use that one instead of the shipped copy.
const ENV_OVERRIDE: &str = "NMSCHECK_HOOK_DLL";

/// Compiled into our DLL, and the only thing that identifies it.
const MARKER: &[u8] = b"NMSLOGGER_HOOK_MARKER_V1";

/// Where a displaced foreign DLL waits to be put back.
const BACKUP_SUFFIX: &str = ".nmscheck-backup";

/// The folder the hook writes its own raw log and its `config.ini` into.
pub const OUT_DIR_NAME: &str = "NMSLogger";

/// The raw log the DLL keeps even when nothing is listening on the pipe.
pub const RAW_LOG: &str = "hook_latest.log";

/// What is installed in the game, and what can be done about it.
#[derive(Debug, Clone, Serialize)]
pub struct State {
    /// where the DLL goes: beside `NMS.exe`
    pub path: Option<String>,
    /// our copy, the one an install would write. `None` when it did not ship.
    pub source: Option<String>,
    pub installed: bool,
    /// true when what is installed is ours
    pub ours: bool,
    /// true when it is byte-for-byte the copy we ship
    pub up_to_date: bool,
    /// set when another tool owns the slot, in words for the screen
    pub foreign: Option<String>,
    /// true when a displaced foreign DLL is waiting to be restored
    pub backup: bool,
    pub game_running: bool,
    /// where the hook's own raw log lives
    pub out_dir: Option<String>,
    /// why installing or removing cannot happen right now
    pub blocked: Option<String>,
}

impl State {
    /// True when the game would be recorded on its next run.
    pub fn recording(&self) -> bool {
        self.installed && self.ours
    }
}

/// The DLL we would install, wherever this build keeps its tools.
pub fn source() -> Option<PathBuf> {
    tools::find(None, ENV_OVERRIDE, &[SHIPPED])
}

/// Where the hook belongs: beside the game's executable.
pub fn target(bin_dir: &Path) -> PathBuf {
    bin_dir.join(DLL_NAME)
}

/// Where the hook writes its raw log, whether or not we are listening.
pub fn out_dir(bin_dir: &Path) -> PathBuf {
    bin_dir.join(OUT_DIR_NAME)
}

fn backup(bin_dir: &Path) -> PathBuf {
    bin_dir.join(format!("{DLL_NAME}{BACKUP_SUFFIX}"))
}

/// What a file was, last time we looked: length and modification time.
///
/// Enough to notice a replacement without reading the file again. A rebuild
/// changes both; a copy of a different DLL over the top changes at least one.
type Stamp = (u64, std::time::SystemTime);

fn stamp(path: &Path) -> Option<Stamp> {
    let facts = std::fs::metadata(path).ok()?;
    Some((facts.len(), facts.modified().ok()?))
}

/// The last answer [`is_ours`] worked out, and what the file looked like then.
static WAS_OURS: std::sync::Mutex<Option<(PathBuf, Stamp, bool)>> =
    std::sync::Mutex::new(None);

/// The last answer [`same_as_ours`] worked out, for the pair of files.
static WAS_SAME: std::sync::Mutex<Option<(PathBuf, Stamp, Stamp, bool)>> =
    std::sync::Mutex::new(None);

/// True when this file is the hook we ship.
///
/// By content, because a marker compiled into the file is the only thing a
/// rebuild does not change -- but remembered against the file's length and
/// modification time, because [`state`] is asked this every two seconds for as
/// long as the app is open, and a quarter of a megabyte read on that schedule is
/// a cost with nothing to show for it.
pub fn is_ours(path: &Path) -> bool {
    let Some(now) = stamp(path) else {
        return false;
    };
    if let Ok(held) = WAS_OURS.lock() {
        if let Some((was, then, answer)) = held.as_ref() {
            if was == path && *then == now {
                return *answer;
            }
        }
    }
    let answer = match std::fs::read(path) {
        Ok(bytes) => bytes.windows(MARKER.len()).any(|slice| slice == MARKER),
        Err(_) => false,
    };
    if let Ok(mut held) = WAS_OURS.lock() {
        *held = Some((path.to_path_buf(), now, answer));
    }
    answer
}

/// True when the installed file is byte-for-byte the one we would install.
///
/// Remembered the same way, and against *both* files: rebuilding the app's copy
/// has to turn a matching install into one that can be updated.
fn same_as_ours(installed: &Path, source: &Path) -> bool {
    let (Some(there), Some(ours)) = (stamp(installed), stamp(source)) else {
        return false;
    };
    if there.0 != ours.0 {
        return false; // different lengths cannot be the same file
    }
    if let Ok(held) = WAS_SAME.lock() {
        if let Some((was, a, b, answer)) = held.as_ref() {
            if was == installed && *a == there && *b == ours {
                return *answer;
            }
        }
    }
    let answer = match (std::fs::read(installed), std::fs::read(source)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };
    if let Ok(mut held) = WAS_SAME.lock() {
        *held = Some((installed.to_path_buf(), there, ours, answer));
    }
    answer
}

/// Read the slot beside the game and say exactly what is in it.
pub fn state(bin_dir: Option<&Path>) -> State {
    let source = source();
    let running = gameproc::find().is_some();

    let Some(bin_dir) = bin_dir else {
        return State {
            path: None,
            source: source.map(|p| p.display().to_string()),
            installed: false,
            ours: false,
            up_to_date: false,
            foreign: None,
            backup: false,
            game_running: running,
            out_dir: None,
            blocked: Some("the game's folder has not been found -- set it in Settings".into()),
        };
    };

    let path = target(bin_dir);
    let installed = path.is_file();
    let ours = installed && is_ours(&path);
    let up_to_date = ours
        && source
            .as_ref()
            .map(|ours| same_as_ours(&path, ours))
            .unwrap_or(false);

    let foreign = (installed && !ours).then(|| {
        format!(
            "another {DLL_NAME} is already in {} -- some other tool or mod uses the same slot",
            bin_dir.display()
        )
    });

    let blocked = if source.is_none() {
        Some("the hook DLL did not ship with this build, so nothing can be installed".into())
    } else if running {
        Some(format!(
            "{} is running, and Windows holds the DLL open while it is -- close the game first",
            gameproc::GAME_EXE
        ))
    } else {
        None
    };

    State {
        path: Some(path.display().to_string()),
        source: source.map(|p| p.display().to_string()),
        installed,
        ours,
        up_to_date,
        foreign,
        backup: backup(bin_dir).is_file(),
        game_running: running,
        out_dir: Some(out_dir(bin_dir).display().to_string()),
        blocked,
    }
}

/// Put the hook beside the game, so the next run is recorded.
///
/// `force` is the answer to a question the screen has already asked: another
/// tool owns the slot, replace it? The displaced DLL is copied aside first and
/// [`uninstall`] puts it back, so saying yes is reversible.
pub fn install(bin_dir: &Path, force: bool) -> Result<State, String> {
    let source = source().ok_or("the hook DLL did not ship with this build")?;
    if let Some(pid) = gameproc::find() {
        return Err(format!(
            "{} is running (process {pid}). Close the game and try again -- Windows will not let \
             the file be replaced while the game has it open.",
            gameproc::GAME_EXE
        ));
    }
    if !bin_dir.is_dir() {
        return Err(format!("{} is not there", bin_dir.display()));
    }

    let path = target(bin_dir);
    if path.is_file() && !is_ours(&path) {
        if !force {
            return Err(format!(
                "{} already has a {DLL_NAME} that is not ours. Another tool uses the same slot; \
                 installing would break it.",
                bin_dir.display()
            ));
        }
        std::fs::copy(&path, backup(bin_dir))
            .map_err(|err| format!("could not back up the existing {DLL_NAME}: {err}"))?;
    }

    std::fs::copy(&source, &path).map_err(|err| {
        format!(
            "could not write {}: {err}",
            path.display()
        )
    })?;
    // The DLL makes this itself on first run, but making it now means the
    // screen can point at somewhere that exists.
    let _ = std::fs::create_dir_all(out_dir(bin_dir));
    Ok(state(Some(bin_dir)))
}

/// Take the hook back out, and put any displaced DLL back.
///
/// A file in the slot that is not ours is left strictly alone: we did not put
/// it there. That is not an error the user needs to fix, so it reads as one
/// sentence rather than a failure.
pub fn uninstall(bin_dir: &Path) -> Result<State, String> {
    if let Some(pid) = gameproc::find() {
        return Err(format!(
            "{} is running (process {pid}). Close the game first.",
            gameproc::GAME_EXE
        ));
    }

    let path = target(bin_dir);
    if path.is_file() {
        if !is_ours(&path) {
            return Err(format!(
                "the {DLL_NAME} in {} is not ours, so it has been left alone",
                bin_dir.display()
            ));
        }
        std::fs::remove_file(&path).map_err(|err| format!("could not remove the hook: {err}"))?;
    }

    let saved = backup(bin_dir);
    if saved.is_file() {
        std::fs::rename(&saved, &path)
            .map_err(|err| format!("the hook is gone, but the DLL it replaced could not be put back: {err}"))?;
    }
    Ok(state(Some(bin_dir)))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let path = std::env::temp_dir().join(format!("nmscheck_hook_{tag}"));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Dir(path)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A stand-in for the real DLL: the marker is all that identifies it.
    fn ours(path: &Path) {
        let mut body = b"MZ padding padding ".to_vec();
        body.extend_from_slice(MARKER);
        body.extend_from_slice(b" more padding");
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn a_foreign_dll_is_not_mistaken_for_ours() {
        let dir = Dir::new("foreign");
        let path = dir.0.join(DLL_NAME);
        std::fs::write(&path, b"MZ some other tool's hook").unwrap();
        assert!(!is_ours(&path));
    }

    #[test]
    fn our_own_dll_is_recognised_by_its_marker() {
        let dir = Dir::new("mine");
        let path = dir.0.join(DLL_NAME);
        ours(&path);
        assert!(is_ours(&path));
    }

    #[test]
    fn a_missing_file_is_not_ours_rather_than_an_error() {
        assert!(!is_ours(Path::new(r"Z:\nope\xinput9_1_0.dll")));
    }

    #[test]
    fn replacing_the_file_changes_the_answer_despite_the_remembered_one() {
        // The answer is remembered against the file's length and modification
        // time, because the watcher asks every two seconds. A replacement has to
        // be noticed anyway -- otherwise removing our DLL and dropping another
        // tool's in its place would leave us treating theirs as ours, and the
        // next install would overwrite it without asking.
        let dir = Dir::new("swapped");
        let path = dir.0.join(DLL_NAME);
        ours(&path);
        assert!(is_ours(&path));

        std::fs::write(&path, b"MZ another tool, and a different length entirely").unwrap();
        assert!(!is_ours(&path), "the remembered answer outlived the file");
    }

    #[test]
    fn an_installed_copy_of_a_different_length_is_never_called_up_to_date() {
        let dir = Dir::new("lengths");
        let installed = dir.0.join(DLL_NAME);
        let shipped = dir.0.join("shipped.dll");
        ours(&installed);
        let mut longer = std::fs::read(&installed).unwrap();
        longer.extend_from_slice(b" and more");
        std::fs::write(&shipped, longer).unwrap();
        assert!(!same_as_ours(&installed, &shipped));

        std::fs::copy(&shipped, &installed).unwrap();
        assert!(same_as_ours(&installed, &shipped));
    }

    #[test]
    fn installing_over_a_foreign_dll_is_refused_until_asked_twice() {
        let dir = Dir::new("refuse");
        let bin = dir.0.join("Binaries");
        std::fs::create_dir_all(&bin).unwrap();
        let path = target(&bin);
        std::fs::write(&path, b"MZ another tool").unwrap();

        // Two reasons this can be skipped rather than run: the DLL may not have
        // shipped in this checkout, and the refusal has to come from the foreign
        // file rather than from a missing source; and the real game may be
        // running on this machine, which is refused earlier and for a different
        // reason -- the file being locked.
        if source().is_none() || gameproc::find().is_some() {
            return;
        }
        let err = install(&bin, false).unwrap_err();
        assert!(err.contains("not ours"), "{err}");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"MZ another tool",
            "the other tool's DLL must still be exactly where it was"
        );
    }

    #[test]
    fn uninstall_leaves_a_dll_that_is_not_ours_where_it_is() {
        let dir = Dir::new("leave");
        let bin = dir.0.join("Binaries");
        std::fs::create_dir_all(&bin).unwrap();
        let path = target(&bin);
        std::fs::write(&path, b"MZ another tool").unwrap();

        if gameproc::find().is_some() {
            return; // the real game is running on this machine right now
        }
        let err = uninstall(&bin).unwrap_err();
        assert!(err.contains("left alone"), "{err}");
        assert!(path.is_file());
    }

    #[test]
    fn uninstall_restores_the_dll_we_displaced() {
        let dir = Dir::new("restore");
        let bin = dir.0.join("Binaries");
        std::fs::create_dir_all(&bin).unwrap();
        let path = target(&bin);
        ours(&path);
        std::fs::write(bin.join(format!("{DLL_NAME}{BACKUP_SUFFIX}")), b"MZ another tool").unwrap();

        if gameproc::find().is_some() {
            return;
        }
        uninstall(&bin).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"MZ another tool");
    }

    #[test]
    fn uninstalling_when_nothing_is_installed_is_not_a_failure() {
        let dir = Dir::new("absent");
        let bin = dir.0.join("Binaries");
        std::fs::create_dir_all(&bin).unwrap();
        if gameproc::find().is_some() {
            return;
        }
        let after = uninstall(&bin).unwrap();
        assert!(!after.installed);
    }

    #[test]
    fn state_without_a_game_folder_says_so_instead_of_inventing_a_path() {
        let state = state(None);
        assert!(state.path.is_none());
        assert!(state.blocked.unwrap().contains("Settings"));
    }
}
