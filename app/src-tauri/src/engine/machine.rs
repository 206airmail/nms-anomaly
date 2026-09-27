//! What the game was running on, and what changed since last time.
//!
//! "It was fine yesterday" is the commonest thing a player knows and the hardest
//! thing for them to act on, because between yesterday and today several things
//! moved at once: mods came and went, the graphics driver updated itself
//! overnight, Windows patched, the game itself patched. Any of those can be the
//! answer, and none of them announces itself.
//!
//! So each recorded session carries a short description of the machine it ran on,
//! and the next session compares itself against the last one. The result is not a
//! diagnosis -- it is a suspect list, which is the honest thing to offer: *"this
//! is the first session since the driver went from 566.36 to 571.96, and since
//! you installed Better Deposit Colors."*
//!
//! Everything here is read from the registry and from files this program already
//! knows about. Nothing is asked of the network, nothing is installed, and a value
//! that cannot be read is `None` rather than a guess -- a comparison against a
//! missing value reports nothing rather than reporting a change.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::{gamefind, hostenv};

/// What the game ran on, as far as it can be read cheaply.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Machine {
    /// the display adapter, e.g. `NVIDIA GeForce RTX 4080 Laptop GPU`
    #[serde(default)]
    pub gpu: Option<String>,
    /// its driver version, as the vendor numbers it (`571.96`, not `32.0.15.7196`)
    #[serde(default)]
    pub driver: Option<String>,
    /// Windows, e.g. `11 24H2 (26200.1234)`
    #[serde(default)]
    pub windows: Option<String>,
    /// the game's own build, from the store that installed it
    #[serde(default)]
    pub game_build: Option<String>,
    /// mod folders the game is set to load, in load order
    #[serde(default)]
    pub mods: Vec<String>,
    /// free space on the volume the game is on, in bytes
    #[serde(default)]
    pub game_disk_free: Option<u64>,
    /// free space where the saves are
    #[serde(default)]
    pub save_disk_free: Option<u64>,
}

impl Machine {
    /// Read everything, for a session about to start.
    pub fn now(game_root: Option<&Path>, save_dir: Option<&Path>) -> Machine {
        Machine {
            gpu: display_adapter().map(|(name, _)| name),
            driver: display_adapter().and_then(|(_, driver)| driver),
            windows: windows_version(),
            game_build: game_root.and_then(steam_build),
            mods: game_root.map(enabled_mods).unwrap_or_default(),
            game_disk_free: game_root.and_then(free_space),
            save_disk_free: save_dir.and_then(free_space),
        }
    }
}

/// What is different between two sessions' machines, in sentences.
///
/// Only differences are reported, and only where both sides know the value: a
/// field that was not readable last time says nothing this time, because "it
/// changed" would be a lie about a gap in our own records.
pub fn changes(before: &Machine, now: &Machine) -> Vec<String> {
    let mut said = Vec::new();

    if let (Some(was), Some(is)) = (&before.driver, &now.driver) {
        if was != is {
            said.push(format!("the graphics driver changed: {was} to {is}"));
        }
    }
    if let (Some(was), Some(is)) = (&before.gpu, &now.gpu) {
        if was != is {
            said.push(format!("the graphics card changed: {was} to {is}"));
        }
    }
    if let (Some(was), Some(is)) = (&before.windows, &now.windows) {
        if was != is {
            said.push(format!("Windows updated: {was} to {is}"));
        }
    }
    if let (Some(was), Some(is)) = (&before.game_build, &now.game_build) {
        if was != is {
            said.push(format!("the game updated: build {was} to {is}"));
        }
    }

    // Mods are compared as sets, because a reordering is not an installation --
    // and the load order is reported separately, since it decides who wins.
    if !before.mods.is_empty() || !now.mods.is_empty() {
        let was: std::collections::BTreeSet<&String> = before.mods.iter().collect();
        let is: std::collections::BTreeSet<&String> = now.mods.iter().collect();
        let added: Vec<&&String> = is.difference(&was).collect();
        let gone: Vec<&&String> = was.difference(&is).collect();
        if !added.is_empty() {
            said.push(format!("{} added: {}", plural(added.len(), "mod"), names(&added)));
        }
        if !gone.is_empty() {
            said.push(format!(
                "{} no longer loaded: {}",
                plural(gone.len(), "mod"),
                names(&gone)
            ));
        }
        if added.is_empty() && gone.is_empty() && before.mods != now.mods {
            said.push("the mods are the same, but their load order changed".to_string());
        }
    }

    // Disk space is a number that moves constantly, so only the threshold that
    // matters is reported: a save is tens of megabytes and a game update is
    // gigabytes.
    if let Some(free) = now.save_disk_free {
        if free < 2 * 1024 * 1024 * 1024 {
            said.push(format!(
                "the drive holding your saves has only {} free",
                spell(free)
            ));
        }
    }
    said
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// Up to four names, and a count for the rest: a list of thirty is not a sentence.
fn names(these: &[&&String]) -> String {
    let shown: Vec<String> = these.iter().take(4).map(|name| (**name).clone()).collect();
    if these.len() > shown.len() {
        format!("{}, and {} more", shown.join(", "), these.len() - shown.len())
    } else {
        shown.join(", ")
    }
}

fn spell(bytes: u64) -> String {
    let gb = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        format!("{} MB", bytes / (1024 * 1024))
    }
}

/// The mod folders the game is set to load, in the order it loads them.
fn enabled_mods(game_root: &Path) -> Vec<String> {
    let settings = hostenv::read_mod_settings(game_root);
    if settings.disable_all {
        return Vec::new();
    }
    let mut order: Vec<(i64, String)> = settings
        .priorities
        .iter()
        .filter(|(name, _)| !settings.is_disabled(name))
        .map(|(name, priority)| (*priority, name.clone()))
        .collect();
    order.sort();
    order.into_iter().map(|(_, name)| name).collect()
}

/// Free bytes on the volume a path is on.
#[cfg(windows)]
fn free_space(path: &Path) -> Option<u64> {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    // The path itself may not exist yet; its volume does.
    let root = path.components().next().map(|first| {
        let mut base = std::path::PathBuf::from(first.as_os_str());
        base.push("");
        base
    })?;
    let wide = HSTRING::from(root.to_string_lossy().to_string());
    let mut free = 0u64;
    unsafe { GetDiskFreeSpaceExW(&wide, Some(&mut free), None, None) }.ok()?;
    Some(free)
}

#[cfg(not(windows))]
fn free_space(_path: &Path) -> Option<u64> {
    None
}

/// The display adapter and its driver version, from the registry.
///
/// Read from the driver's own key rather than through WMI: WMI costs hundreds of
/// milliseconds and a COM apartment, and this runs when a game session starts.
///
/// The vendor number is what a person recognises. NVIDIA's `DriverVersion` is
/// `32.0.15.7196` for what the control panel calls `571.96` -- the last five
/// digits, split -- so it is converted; other vendors are shown as they are.
#[cfg(windows)]
fn display_adapter() -> Option<(String, Option<String>)> {
    use winreg::enums::*;
    use winreg::RegKey;

    const CLASS: &str = r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";
    let local = RegKey::predef(HKEY_LOCAL_MACHINE);
    let adapters = local.open_subkey_with_flags(CLASS, KEY_READ).ok()?;

    for name in adapters.enum_keys().flatten() {
        // Only the numbered subkeys are adapters.
        if !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(adapter) = adapters.open_subkey_with_flags(&name, KEY_READ) else {
            continue;
        };
        let Ok(desc) = adapter.get_value::<String, _>("DriverDesc") else {
            continue;
        };
        let raw: Option<String> = adapter.get_value("DriverVersion").ok();
        return Some((desc, raw.map(|version| vendor_version(&version))));
    }
    None
}

#[cfg(not(windows))]
fn display_adapter() -> Option<(String, Option<String>)> {
    None
}

/// `32.0.15.7196` as NVIDIA's own `571.96`; anything else unchanged.
///
/// Only NVIDIA's shape is converted, and only exactly: the third and fourth parts
/// together being six digits (`15` + `7196`). An Intel or AMD version has other
/// widths, and guessing at those produced numbers that matched nothing a user
/// could see anywhere -- `24.20.11001.4003` came out as `140.03`. A version shown
/// as the vendor never wrote it is worse than one shown raw.
fn vendor_version(raw: &str) -> String {
    let parts: Vec<&str> = raw.split('.').collect();
    if parts.len() == 4 {
        let digits = format!("{}{}", parts[2], parts[3]);
        if digits.len() == 6 && digits.chars().all(|c| c.is_ascii_digit()) {
            let tail = &digits[1..];
            return format!("{}.{}", &tail[..3], &tail[3..]);
        }
    }
    raw.to_string()
}

/// Windows as a person would name it: `11 24H2 (26200.1234)`.
#[cfg(windows)]
fn windows_version() -> Option<String> {
    use winreg::enums::*;
    use winreg::RegKey;

    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", KEY_READ)
        .ok()?;
    let build: String = key.get_value("CurrentBuildNumber").ok()?;
    let revision: u32 = key.get_value("UBR").unwrap_or(0);
    let display: Option<String> = key.get_value("DisplayVersion").ok();
    // Build 22000 and above is Windows 11, whatever the registry's ProductName
    // still says -- it says 10 on every Windows 11 machine.
    let major = if build.parse::<u32>().unwrap_or(0) >= 22_000 {
        "11"
    } else {
        "10"
    };
    Some(match display {
        Some(display) => format!("{major} {display} ({build}.{revision})"),
        None => format!("{major} ({build}.{revision})"),
    })
}

#[cfg(not(windows))]
fn windows_version() -> Option<String> {
    None
}

/// The game's build, from Steam's own record of what it installed.
///
/// Steam writes `buildid` into the app manifest beside the install, and it
/// changes on every patch -- which is what makes "the game updated" sayable at
/// all. A GOG or Game Pass install has no equivalent, and then this is `None`.
fn steam_build(game_root: &Path) -> Option<String> {
    // ...\steamapps\common\No Man's Sky -> ...\steamapps\appmanifest_275850.acf
    let steamapps = game_root.parent()?.parent()?;
    let manifest = steamapps.join(format!("appmanifest_{}.acf", gamefind::STEAM_APP_ID));
    let text = std::fs::read_to_string(manifest).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("\"buildid\"") {
            return rest
                .trim()
                .trim_matches('"')
                .split('"')
                .find(|part| !part.trim().is_empty())
                .map(|build| build.trim().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bare() -> Machine {
        Machine::default()
    }

    #[test]
    fn nvidias_driver_number_is_shown_the_way_the_vendor_writes_it() {
        assert_eq!(vendor_version("32.0.15.7196"), "571.96");
        assert_eq!(vendor_version("31.0.15.5222"), "552.22");
        // Anything that is not that shape is left exactly as it was, rather than
        // squeezed into a number the vendor never printed.
        assert_eq!(vendor_version("24.20.11001.4003"), "24.20.11001.4003");
        assert_eq!(vendor_version("something else"), "something else");
    }

    #[test]
    fn a_driver_update_is_reported_as_a_suspect() {
        let before = Machine {
            driver: Some("566.36".into()),
            ..bare()
        };
        let now = Machine {
            driver: Some("571.96".into()),
            ..bare()
        };
        let said = changes(&before, &now);
        assert_eq!(said.len(), 1);
        assert!(said[0].contains("566.36 to 571.96"), "{said:?}");
    }

    #[test]
    fn a_value_we_could_not_read_reports_nothing_rather_than_a_change() {
        // The trap: treating a missing reading as "it changed" would fill the
        // suspect list with our own gaps.
        let before = Machine {
            driver: None,
            ..bare()
        };
        let now = Machine {
            driver: Some("571.96".into()),
            ..bare()
        };
        assert!(changes(&before, &now).is_empty());
    }

    #[test]
    fn mods_are_compared_as_a_set_and_order_is_reported_apart() {
        let before = Machine {
            mods: vec!["ALPHA".into(), "BETA".into()],
            ..bare()
        };
        let added = Machine {
            mods: vec!["ALPHA".into(), "BETA".into(), "GAMMA".into()],
            ..bare()
        };
        let said = changes(&before, &added);
        assert_eq!(said.len(), 1);
        assert!(said[0].starts_with("1 mod added: GAMMA"), "{said:?}");

        let reordered = Machine {
            mods: vec!["BETA".into(), "ALPHA".into()],
            ..bare()
        };
        let said = changes(&before, &reordered);
        assert_eq!(said.len(), 1);
        assert!(said[0].contains("load order changed"), "{said:?}");
    }

    #[test]
    fn a_removed_mod_is_reported_too() {
        let before = Machine {
            mods: vec!["ALPHA".into(), "BETA".into()],
            ..bare()
        };
        let now = Machine {
            mods: vec!["ALPHA".into()],
            ..bare()
        };
        let said = changes(&before, &now);
        assert!(said[0].contains("no longer loaded: BETA"), "{said:?}");
    }

    #[test]
    fn a_long_list_of_mods_is_summarised_rather_than_recited() {
        let before = bare();
        let now = Machine {
            mods: (1..=9).map(|n| format!("MOD{n}")).collect(),
            ..bare()
        };
        let said = changes(&before, &now);
        assert!(said[0].starts_with("9 mods added: MOD1, MOD2, MOD3, MOD4, and 5 more"), "{said:?}");
    }

    #[test]
    fn nothing_changed_says_nothing() {
        let same = Machine {
            driver: Some("571.96".into()),
            windows: Some("11 24H2 (26200.1)".into()),
            game_build: Some("25442159".into()),
            mods: vec!["ALPHA".into()],
            save_disk_free: Some(500 * 1024 * 1024 * 1024),
            ..bare()
        };
        assert!(changes(&same, &same).is_empty());
    }

    #[test]
    fn a_nearly_full_save_drive_is_worth_saying_on_its_own() {
        let now = Machine {
            save_disk_free: Some(900 * 1024 * 1024),
            ..bare()
        };
        let said = changes(&bare(), &now);
        assert_eq!(said.len(), 1);
        assert!(said[0].contains("only 900 MB free"), "{said:?}");
    }

    #[test]
    fn the_game_build_is_read_out_of_steams_own_manifest() {
        let dir = std::env::temp_dir().join("nmscheck_machine_acf");
        let _ = std::fs::remove_dir_all(&dir);
        let game = dir.join("steamapps").join("common").join("No Man's Sky");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::write(
            dir.join("steamapps").join("appmanifest_275850.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"275850\"\n\t\"buildid\"\t\t\"25442159\"\n}\n",
        )
        .unwrap();

        assert_eq!(steam_build(&game).as_deref(), Some("25442159"));
        // A folder with no manifest is not an error.
        assert!(steam_build(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reading_this_machine_does_not_fail() {
        // Whatever it can read here, it must not panic and must not invent: this
        // runs on every session start.
        let found = Machine::now(None, None);
        assert!(found.mods.is_empty());
        if let Some(driver) = &found.driver {
            assert!(!driver.is_empty());
        }
    }
}
