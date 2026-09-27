//! Where everything lives, when finding it automatically is not enough.
//!
//! Detection ([`super::gamefind`]) handles the common installs, but it cannot
//! handle all of them: a copy moved by hand, a launcher nobody has heard of, a
//! second install kept for testing. So every path detection produces can be
//! overridden, and the override always wins.
//!
//! Two rules shape this module.
//!
//! **A setting that is not set is not a value.** Every override is an
//! `Option`, never an empty string standing in for one, because "" and "not
//! chosen" mean different things and confusing them is how a manager ends up
//! deploying to the drive root. [`Settings::read`] discards blanks on the way
//! in, so a cleared text box behaves as unset rather than as a path to
//! nowhere.
//!
//! **The user is told where a path came from.** [`resolve`] reports, per path,
//! whether it was chosen or detected and whether it actually exists, so the
//! settings screen can show "detected" beside one and "not found" beside
//! another instead of presenting a guess as a fact.
//!
//! # One file, the API key included
//!
//! This used to be three files -- `settings.json`, `nexus.key` and
//! `presets.json` -- on the reasoning that a credential must not end up in
//! something a user would paste into a bug report. That reasoning assumed
//! settings get shared, and they do not: nothing in this program exports them,
//! and every path in here is specific to one machine, so a settings file is
//! not a thing anyone has a reason to hand to anyone else. What *is* shared is
//! a mod list, and that has its own format in [`super::collection`] which
//! carries no settings at all.
//!
//! So everything the user chooses lives here, in one file, and
//! [`Settings::read`] folds the two older files in the first time it finds
//! them. What is deliberately *not* here is everything the program merely
//! records: the loadout (rewritten on every deploy) and the Nexus name cache
//! (rewritten on every resolve). Putting those in would mean every deploy
//! rewrote the file holding the API key.
//!
//! The key still never reaches the screen. [`Settings::without_secrets`] is
//! what the settings command hands the front end, and [`Settings::keeping`]
//! puts back what the front end was never given, so a round trip through the
//! settings form cannot erase a key or a preset it never saw.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::deploy;
use super::gamefind;
use super::preset::Preset;

/// Everything the user can override, and nothing they cannot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    /// the game's install folder, holding `GAMEDATA`
    #[serde(default)]
    pub game_root: Option<String>,
    /// the executable, used to launch the game and to read its version
    #[serde(default)]
    pub game_exe: Option<String>,
    /// the folder the game loads mods from
    #[serde(default)]
    pub mods_dir: Option<String>,
    /// where extracted mods are kept. Must be on the game's volume or the
    /// hardlinks become copies -- [`resolve`] says so when it is not.
    #[serde(default)]
    pub staging_dir: Option<String>,
    /// where downloaded archives are kept
    #[serde(default)]
    pub archives_dir: Option<String>,
    #[serde(default = "yes")]
    pub check_updates_on_start: bool,
    /// Off by default: a mod built for an older game version usually still
    /// works, so listing them all as problems buries the real ones.
    #[serde(default)]
    pub show_outdated: bool,
    #[serde(default = "yes")]
    pub block_ads: bool,
    /// Record what the game says while it runs, using the hook in the game.
    ///
    /// On by default, and it costs nothing while the game is not running: the
    /// pipe is only opened once `NMS.exe` is there. It does nothing at all until
    /// the hook is installed, which is a separate, explicit act.
    #[serde(default = "yes")]
    pub record_sessions: bool,
    /// How many session logs to keep. The oldest go when a new one finishes.
    #[serde(default = "two_dozen")]
    pub keep_sessions: u32,
    /// Keep copies of the save as the game writes it.
    ///
    /// On by default, and the one feature here that protects something
    /// irreplaceable. It needs no hook: copies are taken by watching the save
    /// folder, so it works whether or not the recorder is installed.
    #[serde(default = "yes")]
    pub backup_saves: bool,
    /// How many copies of each save slot to keep.
    #[serde(default = "a_few")]
    pub keep_save_backups: u32,
    /// the preset currently applied, for the UI to show as active
    #[serde(default)]
    pub active_preset: Option<String>,
    /// the saved mod lists. Ordered by name, see [`Settings::put_preset`].
    #[serde(default)]
    pub presets: Vec<Preset>,
    /// the Nexus personal API key, read-only and specific to one account.
    ///
    /// Never sent to the front end -- see [`Settings::without_secrets`].
    #[serde(default)]
    pub nexus_key: Option<String>,
}

fn yes() -> bool {
    true
}

/// Enough sessions to cover "it started crashing this week", not so many that a
/// folder of megabyte logs grows without anyone deciding to keep them.
fn two_dozen() -> u32 {
    25
}

/// How many copies of each save slot to keep. See [`super::savewatch`] for why
/// this can afford to be generous: a whole save folder measured 1.2 MB.
fn a_few() -> u32 {
    super::savewatch::KEEP_DEFAULT
}

/// The defaults are [`Settings::fresh`], not all-zeroes: deriving `Default`
/// here would silently turn update checking and ad blocking *off* for anyone
/// whose settings file is missing a field.
impl Default for Settings {
    fn default() -> Self {
        Settings::fresh()
    }
}

impl Settings {
    /// Read the file, treating anything unreadable as "nothing set yet".
    ///
    /// A corrupt settings file must not stop the program starting -- the user
    /// would have no way in to fix it.
    ///
    /// Also folds in the two files this one replaced, the first time it finds
    /// them. That happens here rather than in a one-off startup step because
    /// this is the only function every caller goes through: a migration the
    /// settings screen ran would leave anything reading settings on a
    /// background thread looking at the old, keyless file until the user
    /// happened to open that screen.
    pub fn read(path: &Path) -> Settings {
        let mut found: Settings = super::read_json(path).unwrap_or_else(Settings::fresh);
        found.tidy();
        if found.absorb_older_files(path) {
            // Best effort: a settings file we cannot write is not a reason to
            // refuse to run, and the values are already correct in memory.
            let _ = found.write(path);
        }
        found
    }

    /// Take in `nexus.key` and `presets.json`, and delete them once taken.
    ///
    /// Returns true when something moved, which is the caller's signal to save.
    /// Deleting is part of the migration rather than left for later: two copies
    /// of a key, one of which is no longer read, is exactly the state where a
    /// user revokes the wrong one and cannot work out why it made no difference.
    fn absorb_older_files(&mut self, path: &Path) -> bool {
        let Some(dir) = path.parent() else {
            return false;
        };
        let mut moved = false;

        let key_file = dir.join("nexus.key");
        if let Ok(text) = std::fs::read_to_string(&key_file) {
            if self.nexus_key.is_none() {
                let key = text.trim();
                if !key.is_empty() {
                    self.nexus_key = Some(key.to_string());
                }
            }
            let _ = std::fs::remove_file(&key_file);
            moved = true;
        }

        let presets_file = dir.join("presets.json");
        if presets_file.exists() {
            #[derive(Deserialize)]
            struct OldPresets {
                #[serde(default)]
                presets: Vec<Preset>,
                #[serde(default)]
                active: Option<String>,
            }
            if let Some(old) = super::read_json::<OldPresets>(&presets_file) {
                if self.presets.is_empty() {
                    self.presets = old.presets;
                    // Only adopt the old active marker when this file has no
                    // opinion: `active_preset` predates the move and may
                    // already be right.
                    self.active_preset = self.active_preset.take().or(old.active);
                }
            }
            let _ = std::fs::remove_file(&presets_file);
            moved = true;
        }

        moved
    }

    /// The defaults, with the flags at their intended values.
    pub fn fresh() -> Settings {
        Settings {
            game_root: None,
            game_exe: None,
            mods_dir: None,
            staging_dir: None,
            archives_dir: None,
            check_updates_on_start: true,
            show_outdated: false,
            block_ads: true,
            record_sessions: true,
            keep_sessions: two_dozen(),
            backup_saves: true,
            keep_save_backups: a_few(),
            active_preset: None,
            presets: Vec::new(),
            nexus_key: None,
        }
    }

    // -- presets ------------------------------------------------------------

    pub fn preset(&self, name: &str) -> Option<&Preset> {
        self.presets.iter().find(|p| p.name == name)
    }

    /// Add or replace a preset by name.
    pub fn put_preset(&mut self, preset: Preset) {
        match self.presets.iter_mut().find(|p| p.name == preset.name) {
            Some(slot) => *slot = preset,
            None => self.presets.push(preset),
        }
        self.presets.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// Remove a preset. Removing the active one leaves nothing active, rather
    /// than changing what is deployed -- deleting a list is not a request to
    /// uninstall the mods on it.
    pub fn forget_preset(&mut self, name: &str) {
        self.presets.retain(|p| p.name != name);
        if self.active_preset.as_deref() == Some(name) {
            self.active_preset = None;
        }
    }

    // -- what the front end may see, and what it may write ------------------

    /// A copy with the API key removed, for sending to the screen.
    ///
    /// The key is the one value here that is worth something to someone else,
    /// and the settings form has no use for it: connecting and disconnecting
    /// go through their own commands, which report the *account* instead. So
    /// it is never handed out, and [`Settings::has_key`] answers the only
    /// question the screen actually asks of it.
    pub fn without_secrets(&self) -> Settings {
        Settings {
            nexus_key: None,
            ..self.clone()
        }
    }

    pub fn has_key(&self) -> bool {
        self.nexus_key.is_some()
    }

    /// Take the fields the settings form owns, keeping the ones it never sees.
    ///
    /// Without this, saving the form would write back the `None` key and empty
    /// preset list that [`Settings::without_secrets`] handed it, disconnecting
    /// the account and deleting every saved mod list as a side effect of
    /// ticking a checkbox.
    /// `active_preset` is kept too: it is set by switching presets, not by this
    /// form, so a settings screen opened before a switch would otherwise save
    /// its stale idea of which list is on.
    pub fn keeping(mut self, stored: &Settings) -> Settings {
        self.nexus_key = stored.nexus_key.clone();
        self.presets = stored.presets.clone();
        self.active_preset = stored.active_preset.clone();
        self
    }

    /// A cleared text box is not a path. Blank and whitespace-only overrides
    /// become "unset", so clearing a field restores detection.
    fn tidy(&mut self) {
        for slot in [
            &mut self.game_root,
            &mut self.game_exe,
            &mut self.mods_dir,
            &mut self.staging_dir,
            &mut self.archives_dir,
            &mut self.active_preset,
            &mut self.nexus_key,
        ] {
            if slot.as_deref().map(str::trim).unwrap_or("").is_empty() {
                *slot = None;
            } else if let Some(text) = slot {
                *text = text.trim().to_string();
            }
        }
    }

    /// Save, via a temporary file, so a crash mid-write cannot leave a
    /// half-written settings file behind.
    ///
    /// This mattered less when the file held only paths that could be set
    /// again in a minute. It now holds the API key and every saved mod list,
    /// and it is written by background threads as well as by the settings
    /// screen, so a torn write is a real loss rather than an inconvenience.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        let mut clean = self.clone();
        clean.tidy();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string_pretty(&clean).map_err(|e| e.to_string())?;

        let temp = path.with_extension("json.writing");
        std::fs::write(&temp, text).map_err(|e| format!("could not save settings: {e}"))?;
        std::fs::rename(&temp, path).map_err(|e| {
            let _ = std::fs::remove_file(&temp);
            format!("could not save settings: {e}")
        })
    }
}

/// One resolved path, and enough about it to explain itself on screen.
#[derive(Debug, Clone, Serialize)]
pub struct Place {
    pub path: Option<String>,
    /// true when the user set this, false when it was detected
    pub chosen: bool,
    pub exists: bool,
    /// why this path is a problem, when it is
    pub problem: Option<String>,
}

impl Place {
    fn missing(problem: &str) -> Place {
        Place {
            path: None,
            chosen: false,
            exists: false,
            problem: Some(problem.to_string()),
        }
    }

    fn at(path: PathBuf, chosen: bool) -> Place {
        Place {
            exists: path.exists(),
            problem: None,
            path: Some(path.display().to_string()),
            chosen,
        }
    }

    fn complain(mut self, problem: impl Into<String>) -> Place {
        self.problem = Some(problem.into());
        self
    }

    pub fn as_path(&self) -> Option<PathBuf> {
        self.path.as_deref().map(PathBuf::from)
    }
}

/// Every path the program needs, with overrides applied and gaps named.
#[derive(Debug, Clone, Serialize)]
pub struct Resolved {
    pub game_root: Place,
    pub game_exe: Place,
    pub mods_dir: Place,
    pub staging: Place,
    pub archives: Place,
    /// how the install was found, for the settings screen to show
    pub detected_from: Option<String>,
    /// true when nothing is missing and nothing is misplaced
    pub ready: bool,
}

/// The executable's name inside the install, on each platform that has one.
const EXE_NAMES: [&str; 2] = ["Binaries/NMS.exe", "NMS.exe"];

/// Work out every path, preferring what the user chose over what was found.
pub fn resolve(settings: &Settings) -> Resolved {
    let detected = gamefind::find_install();

    let game_root = match settings.game_root.as_deref() {
        Some(chosen) => {
            let path = PathBuf::from(chosen);
            let place = Place::at(path.clone(), true);
            if !place.exists {
                place.complain("that folder does not exist")
            } else if !path.join("GAMEDATA").is_dir() {
                place.complain("no GAMEDATA folder here, so this is not the game's install folder")
            } else {
                place
            }
        }
        None => match &detected {
            Some(found) => Place::at(found.root.clone(), false),
            None => Place::missing("could not find the game -- set its folder below"),
        },
    };

    let mods_dir = match settings.mods_dir.as_deref() {
        Some(chosen) => {
            let place = Place::at(PathBuf::from(chosen), true);
            if place.exists {
                place
            } else {
                place.complain("that folder does not exist")
            }
        }
        None => match game_root.as_path() {
            Some(root) => {
                let path = gamefind::mods_dir_of(&root);
                let place = Place::at(path, false);
                if place.exists {
                    place
                } else {
                    place.complain("the game has no MODS folder yet; it is made when the first mod is installed")
                }
            }
            None => Place::missing("depends on the game folder"),
        },
    };

    let game_exe = match settings.game_exe.as_deref() {
        Some(chosen) => {
            let place = Place::at(PathBuf::from(chosen), true);
            if place.exists {
                place
            } else {
                place.complain("that file does not exist")
            }
        }
        None => match game_root.as_path() {
            Some(root) => EXE_NAMES
                .iter()
                .map(|rel| root.join(rel))
                .find(|p| p.is_file())
                .map(|p| Place::at(p, false))
                .unwrap_or_else(|| Place::missing("could not find the game's program -- set it below")),
            None => Place::missing("depends on the game folder"),
        },
    };

    // Staging and archives default beside the game rather than in the user
    // profile, because a hardlink cannot cross volumes and the profile is
    // routinely on a different drive from the game.
    let base = game_root.as_path().map(|root| deploy::staging_for(&root));
    let staging = pick(
        settings.staging_dir.as_deref(),
        base.as_ref().map(|b| b.join("staging")),
    );
    let archives = pick(
        settings.archives_dir.as_deref(),
        base.as_ref().map(|b| b.join("archives")),
    );

    // The one setting that can be wrong in a way that costs disk rather than
    // failing outright: links become copies across volumes, silently.
    let staging = match (staging.as_path(), mods_dir.as_path()) {
        (Some(s), Some(m)) if !same_volume(&s, &m) => staging.complain(
            "this is on a different drive from the game, so mods will be copied instead of linked, using twice the space",
        ),
        _ => staging,
    };

    let ready = game_root.path.is_some()
        && game_root.problem.is_none()
        && mods_dir.path.is_some()
        && mods_dir.problem.is_none();

    Resolved {
        detected_from: detected.map(|d| d.source),
        game_root,
        game_exe,
        mods_dir,
        staging,
        archives,
        ready,
    }
}

fn pick(chosen: Option<&str>, fallback: Option<PathBuf>) -> Place {
    match chosen {
        Some(text) => Place::at(PathBuf::from(text), true),
        None => match fallback {
            Some(path) => Place::at(path, false),
            None => Place::missing("depends on the game folder"),
        },
    }
}

/// Whether two paths are on one volume, which decides link versus copy.
fn same_volume(a: &Path, b: &Path) -> bool {
    fn volume(p: &Path) -> Option<String> {
        let text = p.display().to_string();
        // `D:\...` on Windows; on anything else treat it as one volume, since
        // this only guards the hardlink fallback and a wrong guess there costs
        // nothing but a missing warning.
        let mut chars = text.chars();
        match (chars.next(), chars.next()) {
            (Some(letter), Some(':')) => Some(letter.to_ascii_uppercase().to_string()),
            _ => None,
        }
    }
    match (volume(a), volume(b)) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let path = std::env::temp_dir().join(format!("nmscheck_settings_{tag}"));
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

    #[test]
    fn a_missing_settings_file_gives_the_intended_defaults() {
        let dir = Dir::new("fresh");
        let found = Settings::read(&dir.0.join("nope.json"));
        assert!(found.check_updates_on_start);
        assert!(found.block_ads);
        assert!(
            !found.show_outdated,
            "a mod built for an older game usually still works; listing them all buries the real problems"
        );
        assert!(found.mods_dir.is_none());
        assert!(found.record_sessions);
        assert!(
            found.keep_sessions >= 1,
            "keeping zero sessions would delete each log as soon as it was written"
        );
    }

    #[test]
    fn a_settings_file_written_before_recording_existed_still_records() {
        // The trap: deriving these two from `Default` would leave anyone with an
        // older settings file silently not recording, with nothing on screen to
        // explain why.
        let dir = Dir::new("older");
        let path = dir.0.join("settings.json");
        std::fs::write(&path, r#"{"mods_dir":"D:\\Game\\MODS"}"#).unwrap();
        let found = Settings::read(&path);
        assert!(found.record_sessions);
        assert_eq!(found.keep_sessions, two_dozen());
        assert!(found.backup_saves, "the one setting protecting something irreplaceable");
        assert_eq!(found.keep_save_backups, a_few());
    }

    #[test]
    fn a_corrupt_settings_file_does_not_lock_the_user_out() {
        let dir = Dir::new("corrupt");
        let path = dir.0.join("settings.json");
        std::fs::write(&path, "{ this is not json").unwrap();
        let found = Settings::read(&path);
        assert!(found.check_updates_on_start, "fell back to defaults");
    }

    #[test]
    fn clearing_a_box_restores_detection_rather_than_setting_an_empty_path() {
        let dir = Dir::new("blank");
        let path = dir.0.join("settings.json");
        std::fs::write(
            &path,
            r#"{"mods_dir":"   ","game_root":"","staging_dir":"D:\\Staging"}"#,
        )
        .unwrap();
        let found = Settings::read(&path);
        assert_eq!(found.mods_dir, None);
        assert_eq!(found.game_root, None);
        assert_eq!(found.staging_dir.as_deref(), Some(r"D:\Staging"));
    }

    #[test]
    fn settings_saved_by_a_windows_tool_still_load() {
        // Same trap as the loadout: a BOM would send every setting back to
        // its default, quietly turning off whatever the user had chosen.
        let dir = Dir::new("bom");
        let path = dir.0.join("settings.json");
        std::fs::write(&path, "\u{feff}{\"mods_dir\":\"D:\\\\Game\\\\MODS\",\"block_ads\":false}")
            .unwrap();

        let back = Settings::read(&path);
        assert_eq!(back.mods_dir.as_deref(), Some(r"D:\Game\MODS"));
        assert!(!back.block_ads, "the chosen value survived, not the default");
    }

    #[test]
    fn settings_survive_a_round_trip() {
        let dir = Dir::new("roundtrip");
        let path = dir.0.join("settings.json");
        let mut want = Settings::fresh();
        want.mods_dir = Some(r"D:\Game\GAMEDATA\MODS".into());
        want.show_outdated = true;
        want.block_ads = false;
        want.write(&path).unwrap();

        let back = Settings::read(&path);
        assert_eq!(back, want);
    }

    #[test]
    fn the_key_file_and_the_presets_file_are_folded_in_and_then_gone() {
        // The whole point of the move: one place to look, and exactly one copy
        // of the key -- two, one of which is no longer read, is how a user
        // revokes the wrong key and cannot work out why nothing changed.
        let dir = Dir::new("absorb");
        let path = dir.0.join("settings.json");
        std::fs::write(&path, r#"{"mods_dir":"D:\\Game\\MODS"}"#).unwrap();
        std::fs::write(dir.0.join("nexus.key"), "  abc123\n").unwrap();
        std::fs::write(
            dir.0.join("presets.json"),
            r#"{"presets":[{"name":"Visuals","enabled":["A"]}],"active":"Visuals"}"#,
        )
        .unwrap();

        let found = Settings::read(&path);
        assert_eq!(found.nexus_key.as_deref(), Some("abc123"));
        assert_eq!(found.preset("Visuals").unwrap().enabled, vec!["A"]);
        assert_eq!(found.active_preset.as_deref(), Some("Visuals"));
        assert!(!dir.0.join("nexus.key").exists());
        assert!(!dir.0.join("presets.json").exists());

        // And it stuck: reading again, with nothing left to migrate, is the
        // same answer.
        assert_eq!(Settings::read(&path), found);
    }

    #[test]
    fn migrating_never_overwrites_a_key_already_in_settings() {
        let dir = Dir::new("absorb_conflict");
        let path = dir.0.join("settings.json");
        std::fs::write(&path, r#"{"nexus_key":"the new one"}"#).unwrap();
        std::fs::write(dir.0.join("nexus.key"), "the old one").unwrap();

        let found = Settings::read(&path);
        assert_eq!(found.nexus_key.as_deref(), Some("the new one"));
        assert!(!dir.0.join("nexus.key").exists(), "stale copy taken away");
    }

    #[test]
    fn the_key_is_not_handed_to_the_screen_and_saving_does_not_lose_it() {
        // The trap: the form saves back whatever it was given. Given a blanked
        // key and an empty preset list, a plain save would disconnect the
        // account and delete every saved mod list as a side effect of ticking
        // a checkbox.
        let mut stored = Settings::fresh();
        stored.nexus_key = Some("secret".into());
        stored.put_preset(crate::engine::preset::Preset {
            name: "Visuals".into(),
            enabled: vec!["A".into()],
            note: None,
        });
        stored.active_preset = Some("Visuals".into());

        let shown = stored.without_secrets();
        assert_eq!(shown.nexus_key, None);
        assert!(stored.has_key());

        let mut edited = shown.clone();
        edited.block_ads = false;
        edited.presets.clear();
        edited.active_preset = None;

        let saved = edited.keeping(&stored);
        assert!(!saved.block_ads, "the field the form does own was taken");
        assert_eq!(saved.nexus_key.as_deref(), Some("secret"));
        assert_eq!(saved.presets.len(), 1);
        assert_eq!(saved.active_preset.as_deref(), Some("Visuals"));
    }

    #[test]
    fn a_half_written_settings_file_is_never_what_is_read_back() {
        // Written by background threads as well as by the settings screen, and
        // it now holds the API key and every saved list, so a torn write is a
        // real loss rather than an inconvenience.
        let dir = Dir::new("atomic");
        let path = dir.0.join("settings.json");
        let mut want = Settings::fresh();
        want.nexus_key = Some("secret".into());
        want.write(&path).unwrap();

        assert!(!dir.0.join("settings.json.writing").exists());
        assert_eq!(Settings::read(&path).nexus_key.as_deref(), Some("secret"));
    }

    #[test]
    fn a_chosen_game_folder_without_gamedata_is_named_as_wrong() {
        let dir = Dir::new("notgame");
        let mut settings = Settings::fresh();
        settings.game_root = Some(dir.0.display().to_string());
        let found = resolve(&settings);

        assert!(found.game_root.chosen);
        assert!(found.game_root.exists);
        assert!(
            found.game_root.problem.as_deref().unwrap().contains("GAMEDATA"),
            "{:?}",
            found.game_root.problem
        );
        assert!(!found.ready);
    }

    #[test]
    fn a_chosen_path_that_is_not_there_says_so_rather_than_being_used() {
        let mut settings = Settings::fresh();
        settings.mods_dir = Some(r"Q:\nowhere\MODS".into());
        let found = resolve(&settings);
        assert!(found.mods_dir.chosen);
        assert!(!found.mods_dir.exists);
        assert!(found.mods_dir.problem.is_some());
        assert!(!found.ready);
    }

    #[test]
    fn staging_on_another_drive_is_flagged_because_links_become_copies() {
        let mut settings = Settings::fresh();
        settings.mods_dir = Some(r"D:\Game\GAMEDATA\MODS".into());
        settings.staging_dir = Some(r"C:\Staging".into());
        let found = resolve(&settings);
        let said = found.staging.problem.unwrap_or_default();
        assert!(said.contains("different drive"), "{said}");
        assert!(said.contains("twice the space"), "{said}");
    }

    #[test]
    fn staging_on_the_same_drive_is_not_flagged() {
        let mut settings = Settings::fresh();
        settings.mods_dir = Some(r"D:\Game\GAMEDATA\MODS".into());
        settings.staging_dir = Some(r"D:\Staging".into());
        assert!(resolve(&settings).staging.problem.is_none());
    }

    #[test]
    fn volumes_are_compared_without_regard_to_case() {
        assert!(same_volume(Path::new(r"D:\a"), Path::new(r"d:\b")));
        assert!(!same_volume(Path::new(r"C:\a"), Path::new(r"D:\b")));
        // No drive letter: assume one volume rather than warn wrongly.
        assert!(same_volume(Path::new("/home/a"), Path::new("/mnt/b")));
    }
}
