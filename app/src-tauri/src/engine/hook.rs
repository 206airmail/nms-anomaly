//! The recorder's other half: code that has to sit inside the game.
//!
//! No Man's Sky writes almost nothing to `FullLog.txt`, so the only way to find
//! out which mod files it actually opened, what it complained about, and where
//! it crashed is to be *inside* the process while it runs.
//!
//! **This is now two files, not one.** The thing the game loads is Atlas, a
//! standalone plugin host with its own repository; the recording itself is an
//! Atlas plugin. Installing means putting both in place:
//!
//! | file | where | what it is |
//! |---|---|---|
//! | `xinput9_1_0.dll` | beside `NMS.exe` | Atlas, the host |
//! | `anomaly_recorder.dll` | `Binaries\Atlas\plugins\` | our recorder |
//!
//! **Why it had to be split.** `NMS.exe` imports `xinput9_1_0.dll` and Windows
//! resolves an import from the program's own folder before System32, so a copy
//! there is loaded with no injector and no change to how the game is started.
//! Only one file can have that name. Atlas and a separate recorder DLL could
//! therefore never both be installed, and the recorder became a plugin. Anomaly
//! now uses exactly the public plugin contract a third party would.
//!
//! Three rules, each of them a way this could otherwise ruin someone's install:
//!
//! **Never overwrite a DLL that is not ours.** A foreign `xinput9_1_0.dll` is
//! another tool's hook, and replacing it would break that tool silently. Ours is
//! recognised by a marker string compiled into it -- not by size, date or
//! version, which a rebuild changes. Two markers count as ours: Atlas's, and the
//! **old combined hook's**, because an install predating the split is still our
//! own file and is safe to upgrade in place.
//!
//! **Never write while the game is running.** The loader holds the file open, so
//! the copy would fail -- and on the uninstall side, a delete that "succeeds"
//! against an open handle leaves the DLL live in the running process.
//!
//! **Say what is there, don't guess.** [`State`] carries the whole picture,
//! including which half is missing, so the screen can explain rather than offer
//! a button that will fail.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{gameproc, tools};

/// The name the game's import table asks for.
pub const DLL_NAME: &str = "xinput9_1_0.dll";

/// The recorder plugin's file name.
pub const PLUGIN_NAME: &str = "anomaly_recorder.dll";

/// Atlas keeps its own folder beside the game's executable.
pub const HOST_DIR_NAME: &str = "Atlas";

/// Where Atlas looks for plugins, under [`HOST_DIR_NAME`].
pub const PLUGIN_DIR_NAME: &str = "plugins";

/// Where the built host and plugin sit among the bundled tools.
const SHIPPED_HOST: &str = "atlas/xinput9_1_0.dll";
const SHIPPED_PLUGIN: &str = "atlas/anomaly_recorder.dll";

/// Point these at a DLL to use that one instead of the shipped copy.
const ENV_OVERRIDE: &str = "NMSCHECK_HOOK_DLL";
const ENV_OVERRIDE_PLUGIN: &str = "NMSCHECK_RECORDER_DLL";

/// Compiled into Atlas, and the only thing that identifies it.
const ATLAS_MARKER: &[u8] = b"ATLAS_NMS_PLUGIN_HOST_V1";

/// Compiled into the recorder -- and, before the split, into the combined DLL
/// that used to occupy the slot. Finding it *in the slot* therefore means an
/// old install that predates Atlas, which is ours and safe to replace.
const RECORDER_MARKER: &[u8] = b"NMSLOGGER_HOOK_MARKER_V1";

/// Where a displaced foreign DLL waits to be put back.
const BACKUP_SUFFIX: &str = ".nmscheck-backup";

/// The folder the recorder writes its own raw log and its `config.ini` into.
/// Unchanged across the split, so an existing install keeps its settings.
pub const OUT_DIR_NAME: &str = "NMSLogger";

/// The raw log the recorder keeps even when nothing is listening on the pipe.
pub const RAW_LOG: &str = "hook_latest.log";

/// Who owns the slot beside the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Empty,
    /// Atlas: what we install now.
    Atlas,
    /// The combined DLL from before the split. Ours, and upgradable in place.
    Legacy,
    /// Somebody else's tool.
    Foreign,
}

impl Slot {
    /// True when we put it there, so replacing it needs no permission.
    fn ours(self) -> bool {
        matches!(self, Slot::Atlas | Slot::Legacy)
    }
}

/// What is installed in the game, and what can be done about it.
#[derive(Debug, Clone, Serialize)]
pub struct State {
    /// where the host goes: beside `NMS.exe`
    pub path: Option<String>,
    /// our copy of the host, the one an install would write
    pub source: Option<String>,
    /// where the recorder plugin goes
    pub plugin_path: Option<String>,
    /// **both halves are in place.** Not just the host.
    pub installed: bool,
    /// true when whatever occupies the slot is ours
    pub ours: bool,
    /// true when both halves are byte-for-byte the copies we ship
    pub up_to_date: bool,
    /// set when another tool owns the slot, in words for the screen
    pub foreign: Option<String>,
    /// true when a displaced foreign DLL is waiting to be restored
    pub backup: bool,
    pub game_running: bool,
    /// where the recorder's own raw log lives
    pub out_dir: Option<String>,
    /// why installing or removing cannot happen right now
    pub blocked: Option<String>,
    /// the host alone, for a message that can say which half is missing
    pub host_installed: bool,
    /// the plugin alone
    pub plugin_installed: bool,
    /// set when the slot holds the pre-split combined DLL
    pub legacy: bool,
}

impl State {
    /// True when the game would be recorded on its next run.
    ///
    /// Both halves are required: Atlas alone loads and records nothing, and the
    /// plugin alone is a file nothing will ever open.
    pub fn recording(&self) -> bool {
        self.installed && self.ours
    }
}

/// The host we would install, wherever this build keeps its tools.
pub fn source() -> Option<PathBuf> {
    tools::find(None, ENV_OVERRIDE, &[SHIPPED_HOST])
}

/// The recorder plugin we would install.
pub fn plugin_source() -> Option<PathBuf> {
    tools::find(None, ENV_OVERRIDE_PLUGIN, &[SHIPPED_PLUGIN])
}

/// Where the host belongs: beside the game's executable.
pub fn target(bin_dir: &Path) -> PathBuf {
    bin_dir.join(DLL_NAME)
}

/// Where Atlas loads plugins from.
pub fn plugin_dir(bin_dir: &Path) -> PathBuf {
    bin_dir.join(HOST_DIR_NAME).join(PLUGIN_DIR_NAME)
}

/// Where the recorder plugin belongs.
pub fn plugin_target(bin_dir: &Path) -> PathBuf {
    plugin_dir(bin_dir).join(PLUGIN_NAME)
}

/// Where the recorder writes its raw log, whether or not we are listening.
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

/// The last answer [`slot`] worked out, and what the file looked like then.
static WAS_OURS: std::sync::Mutex<Option<(PathBuf, Stamp, Slot)>> =
    std::sync::Mutex::new(None);

/// The last answer [`same_as_ours`] worked out, for both files involved.
static WAS_SAME: std::sync::Mutex<Option<(PathBuf, Stamp, Stamp, bool)>> =
    std::sync::Mutex::new(None);

fn contains(bytes: &[u8], marker: &[u8]) -> bool {
    bytes.windows(marker.len()).any(|slice| slice == marker)
}

/// Who owns the slot, by content.
///
/// By content, because a marker compiled into the file is the only thing a
/// rebuild does not change -- but remembered against the file's length and
/// modification time, because [`state`] is asked this every two seconds for as
/// long as the app is open, and a quarter of a megabyte read on that schedule is
/// a cost with nothing to show for it.
pub fn slot(path: &Path) -> Slot {
    let Some(now) = stamp(path) else {
        return Slot::Empty;
    };
    if let Ok(held) = WAS_OURS.lock() {
        if let Some((was, then, answer)) = held.as_ref() {
            if was == path && *then == now {
                return *answer;
            }
        }
    }
    let answer = match std::fs::read(path) {
        Ok(bytes) if contains(&bytes, ATLAS_MARKER) => Slot::Atlas,
        // Checked second: the recorder plugin also carries this marker, but the
        // plugin never occupies the slot. In the slot it means the pre-split
        // combined DLL.
        Ok(bytes) if contains(&bytes, RECORDER_MARKER) => Slot::Legacy,
        Ok(_) => Slot::Foreign,
        Err(_) => Slot::Empty,
    };
    if let Ok(mut held) = WAS_OURS.lock() {
        *held = Some((path.to_path_buf(), now, answer));
    }
    answer
}

/// True when what is in the slot is ours, whichever generation it is.
pub fn is_ours(path: &Path) -> bool {
    slot(path).ours()
}

/// True when the plugin file is our recorder rather than some other plugin that
/// happens to share the name.
fn plugin_is_ours(path: &Path) -> bool {
    match std::fs::read(path) {
        Ok(bytes) => contains(&bytes, RECORDER_MARKER),
        Err(_) => false,
    }
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

/// Compare without the cache. Used for the plugin, which is asked about far less
/// often than the slot and does not justify a second remembered answer.
fn same_file(a: &Path, b: &Path) -> bool {
    match (stamp(a), stamp(b)) {
        (Some(x), Some(y)) if x.0 == y.0 => {
            matches!((std::fs::read(a), std::fs::read(b)), (Ok(p), Ok(q)) if p == q)
        }
        _ => false,
    }
}

/// Read the slot beside the game and say exactly what is in it.
pub fn state(bin_dir: Option<&Path>) -> State {
    let source = source();
    let plugin_src = plugin_source();
    let running = gameproc::find().is_some();

    let Some(bin_dir) = bin_dir else {
        return State {
            path: None,
            source: source.map(|p| p.display().to_string()),
            plugin_path: None,
            installed: false,
            ours: false,
            up_to_date: false,
            foreign: None,
            backup: false,
            game_running: running,
            out_dir: None,
            blocked: Some("the game's folder has not been found -- set it in Settings".into()),
            host_installed: false,
            plugin_installed: false,
            legacy: false,
        };
    };

    let path = target(bin_dir);
    let which = slot(&path);
    let host_installed = which == Slot::Atlas;
    let plugin_path = plugin_target(bin_dir);
    let plugin_installed = plugin_path.is_file() && plugin_is_ours(&plugin_path);

    // Both halves, or it records nothing. A legacy DLL in the slot is ours but
    // is not Atlas, so it does not count as installed -- it counts as an
    // upgrade waiting to happen, which `ours` and `legacy` together say.
    let installed = host_installed && plugin_installed;
    let ours = which.ours();

    let up_to_date = installed
        && source
            .as_ref()
            .map(|ours| same_as_ours(&path, ours))
            .unwrap_or(false)
        && plugin_src
            .as_ref()
            .map(|ours| same_file(&plugin_path, ours))
            .unwrap_or(false);

    let foreign = (which == Slot::Foreign).then(|| {
        format!(
            "another {DLL_NAME} is already in {} -- some other tool or mod uses the same slot",
            bin_dir.display()
        )
    });

    let blocked = if source.is_none() || plugin_src.is_none() {
        Some("the recorder did not ship with this build, so nothing can be installed".into())
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
        plugin_path: Some(plugin_path.display().to_string()),
        installed,
        ours,
        up_to_date,
        foreign,
        backup: backup(bin_dir).is_file(),
        game_running: running,
        out_dir: Some(out_dir(bin_dir).display().to_string()),
        blocked,
        host_installed,
        plugin_installed,
        legacy: which == Slot::Legacy,
    }
}

/// Put both halves beside the game, so the next run is recorded.
///
/// `force` is the answer to a question the screen has already asked: another
/// tool owns the slot, replace it? The displaced DLL is copied aside first and
/// [`uninstall`] puts it back, so saying yes is reversible.
///
/// A pre-split combined DLL in the slot is *not* foreign and needs no `force`:
/// it is our own file, and replacing it with Atlas plus the plugin is the
/// upgrade.
pub fn install(bin_dir: &Path, force: bool) -> Result<State, String> {
    let source = source().ok_or("the recorder did not ship with this build")?;
    let plugin_src = plugin_source().ok_or("the recorder plugin did not ship with this build")?;
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
    if slot(&path) == Slot::Foreign {
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

    // The plugin first. If writing the host succeeded and the plugin then
    // failed, the game would load a host with nothing to record -- which looks
    // exactly like working. Failing before the slot is touched leaves the
    // install in whatever state it was already in.
    let plugins = plugin_dir(bin_dir);
    std::fs::create_dir_all(&plugins)
        .map_err(|err| format!("could not make {}: {err}", plugins.display()))?;
    let plugin_path = plugin_target(bin_dir);
    std::fs::copy(&plugin_src, &plugin_path)
        .map_err(|err| format!("could not write {}: {err}", plugin_path.display()))?;

    std::fs::copy(&source, &path)
        .map_err(|err| format!("could not write {}: {err}", path.display()))?;

    // The recorder makes this itself on first run, but making it now means the
    // screen can point at somewhere that exists.
    let _ = std::fs::create_dir_all(out_dir(bin_dir));
    Ok(state(Some(bin_dir)))
}

/// Take both halves back out, and put any displaced DLL back.
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
    match slot(&path) {
        Slot::Foreign => {
            return Err(format!(
                "the {DLL_NAME} in {} is not ours, so it has been left alone",
                bin_dir.display()
            ));
        }
        Slot::Atlas | Slot::Legacy => {
            std::fs::remove_file(&path)
                .map_err(|err| format!("could not remove the host: {err}"))?;
        }
        Slot::Empty => {}
    }

    // Only our own plugin. Another plugin in Atlas's folder belongs to somebody
    // else and is none of our business, and the folder itself is left in place
    // because Atlas may be back.
    let plugin_path = plugin_target(bin_dir);
    if plugin_path.is_file() && plugin_is_ours(&plugin_path) {
        std::fs::remove_file(&plugin_path)
            .map_err(|err| format!("the host is gone, but the recorder plugin could not be removed: {err}"))?;
    }

    let saved = backup(bin_dir);
    if saved.is_file() {
        std::fs::rename(&saved, &path)
            .map_err(|err| format!("the recorder is gone, but the DLL it replaced could not be put back: {err}"))?;
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

    fn with_marker(path: &Path, marker: &[u8]) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut body = b"MZ padding padding ".to_vec();
        body.extend_from_slice(marker);
        body.extend_from_slice(b" more padding");
        std::fs::write(path, body).unwrap();
    }

    fn atlas(path: &Path) {
        with_marker(path, ATLAS_MARKER)
    }
    fn recorder(path: &Path) {
        with_marker(path, RECORDER_MARKER)
    }

    #[test]
    fn a_foreign_dll_is_not_mistaken_for_ours() {
        let dir = Dir::new("foreign");
        let path = dir.0.join(DLL_NAME);
        std::fs::write(&path, b"MZ some other hook entirely").unwrap();
        assert_eq!(slot(&path), Slot::Foreign);
        assert!(!is_ours(&path));
    }

    #[test]
    fn atlas_in_the_slot_is_recognised() {
        let dir = Dir::new("atlas");
        let path = dir.0.join(DLL_NAME);
        atlas(&path);
        assert_eq!(slot(&path), Slot::Atlas);
        assert!(is_ours(&path));
    }

    #[test]
    fn the_pre_split_combined_dll_is_ours_and_upgradable() {
        // Someone who installed before Atlas existed has the old combined DLL
        // in the slot, and it carries the recorder marker. Treating it as
        // foreign would make the app refuse to touch its own file and ask the
        // user to confirm replacing "another tool".
        let dir = Dir::new("legacy");
        let path = dir.0.join(DLL_NAME);
        recorder(&path);
        assert_eq!(slot(&path), Slot::Legacy);
        assert!(is_ours(&path), "our own older file must not read as foreign");
    }

    #[test]
    fn a_missing_file_is_empty_rather_than_an_error() {
        let nowhere = Path::new(r"Z:\nope\xinput9_1_0.dll");
        assert_eq!(slot(nowhere), Slot::Empty);
        assert!(!is_ours(nowhere));
    }

    #[test]
    fn replacing_the_file_changes_the_answer_despite_the_remembered_one() {
        // The answer is remembered against the file length and modification
        // time, because the watcher asks every two seconds. A replacement has
        // to be noticed anyway -- otherwise removing our DLL and dropping
        // another tool in its place would leave us treating theirs as ours,
        // and the next install would overwrite it without asking.
        let dir = Dir::new("swapped");
        let path = dir.0.join(DLL_NAME);
        atlas(&path);
        assert!(is_ours(&path));

        std::fs::write(&path, b"MZ another tool, and a different length entirely").unwrap();
        assert!(!is_ours(&path), "the remembered answer outlived the file");
    }

    #[test]
    fn the_host_alone_is_not_installed() {
        // Atlas with no plugin loads and records nothing, which from the
        // outside looks exactly like a working install. It must not read as
        // one.
        let dir = Dir::new("hostonly");
        let bin = dir.0.join("Binaries");
        std::fs::create_dir_all(&bin).unwrap();
        atlas(&target(&bin));

        let found = state(Some(&bin));
        assert!(found.host_installed);
        assert!(!found.plugin_installed);
        assert!(!found.installed, "one half is not an install");
        assert!(!found.recording());
        assert!(found.ours, "the slot is still ours");
    }

    #[test]
    fn the_plugin_alone_is_not_installed() {
        let dir = Dir::new("pluginonly");
        let bin = dir.0.join("Binaries");
        std::fs::create_dir_all(&bin).unwrap();
        recorder(&plugin_target(&bin));

        let found = state(Some(&bin));
        assert!(!found.host_installed);
        assert!(found.plugin_installed);
        assert!(!found.installed, "a plugin nothing loads is not an install");
    }

    #[test]
    fn both_halves_together_are_an_install() {
        let dir = Dir::new("both");
        let bin = dir.0.join("Binaries");
        std::fs::create_dir_all(&bin).unwrap();
        atlas(&target(&bin));
        recorder(&plugin_target(&bin));

        let found = state(Some(&bin));
        assert!(found.installed);
        assert!(found.recording());
    }

    #[test]
    fn a_plugin_of_the_same_name_that_is_not_ours_does_not_count() {
        let dir = Dir::new("otherplugin");
        let bin = dir.0.join("Binaries");
        std::fs::create_dir_all(&bin).unwrap();
        atlas(&target(&bin));
        let p = plugin_target(&bin);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"MZ a plugin that is not ours").unwrap();

        assert!(!state(Some(&bin)).plugin_installed);
    }

    #[test]
    fn installing_writes_both_halves() {
        let dir = Dir::new("writeboth");
        let bin = dir.0.join("Binaries");
        std::fs::create_dir_all(&bin).unwrap();
        // Skipped when the DLLs did not ship in this checkout, or the real game
        // is running -- which is refused earlier, for a different reason.
        if source().is_none() || plugin_source().is_none() || gameproc::find().is_some() {
            return;
        }
        let after = install(&bin, false).unwrap();
        assert!(target(&bin).is_file(), "the host was not written");
        assert!(plugin_target(&bin).is_file(), "the plugin was not written");
        assert!(after.installed);
        assert!(after.up_to_date);
    }

    #[test]
    fn installing_over_a_foreign_dll_is_refused_until_asked_twice() {
        let dir = Dir::new("refuse");
        let bin = dir.0.join("Binaries");
        std::fs::create_dir_all(&bin).unwrap();
        let path = target(&bin);
        std::fs::write(&path, b"MZ another tool").unwrap();

        if source().is_none() || plugin_source().is_none() || gameproc::find().is_some() {
            return;
        }
        let err = install(&bin, false).unwrap_err();
        assert!(err.contains("not ours"), "{err}");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"MZ another tool",
            "the other tool DLL must still be exactly where it was"
        );
    }

    #[test]
    fn upgrading_from_the_combined_dll_needs_no_permission() {
        // The upgrade path for every install that predates Atlas. Asking the
        // user to confirm replacing "another tool" would be alarming and wrong.
        let dir = Dir::new("upgrade");
        let bin = dir.0.join("Binaries");
        std::fs::create_dir_all(&bin).unwrap();
        recorder(&target(&bin));

        if source().is_none() || plugin_source().is_none() || gameproc::find().is_some() {
            return;
        }
        let after = install(&bin, false).expect("upgrading our own file must not need force");
        assert!(after.installed);
        assert!(!after.legacy, "the slot should hold Atlas now");
        assert!(
            !backup(&bin).is_file(),
            "our own old file is replaced, not backed up as if it were foreign"
        );
    }

    #[test]
    fn uninstall_takes_both_halves_out() {
        let dir = Dir::new("removeboth");
        let bin = dir.0.join("Binaries");
        std::fs::create_dir_all(&bin).unwrap();
        atlas(&target(&bin));
        recorder(&plugin_target(&bin));

        if gameproc::find().is_some() {
            return;
        }
        let after = uninstall(&bin).unwrap();
        assert!(!target(&bin).is_file());
        assert!(!plugin_target(&bin).is_file());
        assert!(!after.installed);
    }

    #[test]
    fn uninstall_leaves_another_plugin_alone() {
        let dir = Dir::new("keepother");
        let bin = dir.0.join("Binaries");
        std::fs::create_dir_all(&bin).unwrap();
        atlas(&target(&bin));
        let mine = plugin_target(&bin);
        recorder(&mine);
        let theirs = plugin_dir(&bin).join("somebody_else.dll");
        std::fs::write(&theirs, b"MZ not ours").unwrap();

        if gameproc::find().is_some() {
            return;
        }
        uninstall(&bin).unwrap();
        assert!(!mine.is_file());
        assert!(
            theirs.is_file(),
            "another author plugin is not ours to delete"
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
        atlas(&path);
        std::fs::write(backup(&bin), b"MZ another tool").unwrap();

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
        let found = state(None);
        assert!(found.path.is_none());
        assert!(found.blocked.unwrap().contains("Settings"));
    }
}

