//! Find the No Man's Sky install, whichever launcher put it there.
//!
//! Port of `nmscc/gamefind.py`; see that file for the reasoning. Sources are
//! asked cheapest and most authoritative first:
//!
//! 1. the `NMS_GAME_DIR` environment variable, which overrides everything
//! 2. **Steam** — the per-app uninstall key records `InstallLocation` outright
//! 3. **Steam** — failing that, walk the library list to the app manifest
//! 4. **GOG Galaxy** — every game is registered under `GOG.com\Games`
//! 5. **Epic** — one JSON manifest per installed game
//! 6. **Microsoft Store / Game Pass** — `<drive>:\XboxGames\<title>\Content`
//! 7. a short sweep of the usual folders on every fixed drive
//!
//! Every candidate is confirmed by looking for a `GAMEDATA` folder inside it,
//! so a stale registry entry pointing at a deleted install is discarded rather
//! than reported. Nothing here fails: a machine with no game simply yields
//! nothing and the caller asks the user for a path.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// Steam's app id for No Man's Sky.
pub const STEAM_APP_ID: &str = "275850";

/// How the game titles itself, used where a source is searched by name.
pub const GAME_TITLE: &str = "No Man's Sky";

/// The mod folder, relative to the install root. Note this is *not*
/// `GAMEDATA/PCBANKS/MODS`, which is the old pak location.
pub const MODS_SUBPATH: [&str; 2] = ["GAMEDATA", "MODS"];

/// Set this to skip detection entirely.
pub const ENV_OVERRIDE: &str = "NMS_GAME_DIR";

/// An install, and how it was found.
#[derive(Debug, Clone, Serialize)]
pub struct Install {
    pub root: PathBuf,
    pub source: String,
    pub mods_dir: PathBuf,
    pub has_mods_dir: bool,
}

impl Install {
    fn new(root: PathBuf, source: impl Into<String>) -> Self {
        let mods_dir = mods_dir_of(&root);
        Self {
            has_mods_dir: mods_dir.is_dir(),
            mods_dir,
            root,
            source: source.into(),
        }
    }
}

pub fn mods_dir_of(root: &Path) -> PathBuf {
    let mut dir = root.to_path_buf();
    for part in MODS_SUBPATH {
        dir.push(part);
    }
    dir
}

/// True when `root` really is a No Man's Sky install.
///
/// Guards against a registry entry left behind by an uninstall, and against a
/// launcher pointing at a folder that has since been emptied.
pub fn looks_like_game(root: &Path) -> bool {
    root.join("GAMEDATA").is_dir()
}

// ---------------------------------------------------------------------------
// registry
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod registry {
    use winreg::enums::*;
    use winreg::RegKey;

    /// Read one string value, trying both registry views.
    pub fn read(hive: isize, subkey: &str, name: &str) -> Option<String> {
        for flags in [KEY_READ | KEY_WOW64_64KEY, KEY_READ | KEY_WOW64_32KEY] {
            let root = RegKey::predef(hive);
            if let Ok(key) = root.open_subkey_with_flags(subkey, flags) {
                if let Ok(value) = key.get_value::<String, _>(name) {
                    let trimmed = value.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_string());
                    }
                }
            }
        }
        None
    }

    /// Child key names of `subkey`, across both registry views.
    pub fn subkeys(hive: isize, subkey: &str) -> Vec<String> {
        let mut seen = Vec::new();
        for flags in [KEY_READ | KEY_WOW64_64KEY, KEY_READ | KEY_WOW64_32KEY] {
            let root = RegKey::predef(hive);
            if let Ok(key) = root.open_subkey_with_flags(subkey, flags) {
                for name in key.enum_keys().flatten() {
                    if !seen.contains(&name) {
                        seen.push(name);
                    }
                }
            }
        }
        seen
    }
}

#[cfg(not(windows))]
mod registry {
    pub fn read(_hive: isize, _subkey: &str, _name: &str) -> Option<String> {
        None
    }
    pub fn subkeys(_hive: isize, _subkey: &str) -> Vec<String> {
        Vec::new()
    }
}

#[cfg(windows)]
const HKLM: isize = winreg::enums::HKEY_LOCAL_MACHINE as isize;
#[cfg(windows)]
const HKCU: isize = winreg::enums::HKEY_CURRENT_USER as isize;
#[cfg(not(windows))]
const HKLM: isize = 0;
#[cfg(not(windows))]
const HKCU: isize = 0;

// ---------------------------------------------------------------------------
// Steam
// ---------------------------------------------------------------------------

/// The most direct answer: Steam writes the path per installed app.
fn steam_uninstall_entry(out: &mut Vec<Install>) {
    let key = format!(
        r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\Steam App {STEAM_APP_ID}"
    );
    if let Some(path) = registry::read(HKLM, &key, "InstallLocation") {
        out.push(Install::new(PathBuf::from(path), "Steam (registry)"));
    }
}

/// Where Steam itself is installed.
fn steam_clients() -> Vec<PathBuf> {
    let mut found = Vec::new();
    for (hive, key, name) in [
        (HKCU, r"Software\Valve\Steam", "SteamPath"),
        (HKLM, r"SOFTWARE\Valve\Steam", "InstallPath"),
    ] {
        if let Some(value) = registry::read(hive, key, name) {
            found.push(PathBuf::from(value));
        }
    }

    if let Some(home) = home_dir() {
        for relative in [
            [".steam", "steam"].as_slice(),
            [".local", "share", "Steam"].as_slice(),
            ["Library", "Application Support", "Steam"].as_slice(),
        ] {
            let mut path = home.clone();
            for part in relative {
                path.push(part);
            }
            found.push(path);
        }
    }
    found
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// Every library folder a Steam client knows about.
///
/// `libraryfolders.vdf` has been through several shapes; rather than write a
/// VDF parser, pull out anything quoted that looks like a path. A wrong guess
/// costs one failed `is_dir` and nothing else.
fn steam_libraries(client: &Path) -> Vec<PathBuf> {
    let mut libraries = vec![client.to_path_buf()];

    let vdf = client.join("steamapps").join("libraryfolders.vdf");
    let Ok(text) = std::fs::read_to_string(&vdf) else {
        return libraries;
    };

    // Current format: "path"  "D:\\SteamLibrary".  Legacy: "1"  "D:\\Library".
    for (key, value) in quoted_pairs(&text) {
        let is_path_key = key.eq_ignore_ascii_case("path") || key.chars().all(|c| c.is_ascii_digit());
        if !is_path_key {
            continue;
        }
        let candidate = value.replace("\\\\", "\\");
        if candidate.contains('\\') || candidate.contains('/') {
            libraries.push(PathBuf::from(candidate));
        }
    }
    libraries
}

/// Every `"key"  "value"` pair in a VDF-ish blob, in order.
fn quoted_pairs(text: &str) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for line in text.lines() {
        let mut fields = line.split('"').skip(1).step_by(2);
        if let (Some(key), Some(value)) = (fields.next(), fields.next()) {
            pairs.push((key.to_string(), value.to_string()));
        }
    }
    pairs
}

/// Resolve the app manifest in `library` to an install folder.
fn steam_manifest(library: &Path) -> Option<PathBuf> {
    let manifest = library
        .join("steamapps")
        .join(format!("appmanifest_{STEAM_APP_ID}.acf"));
    let text = std::fs::read_to_string(&manifest).ok()?;

    let folder = quoted_pairs(&text)
        .into_iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("installdir"))
        .map(|(_, value)| value)
        .unwrap_or_else(|| GAME_TITLE.to_string());

    Some(library.join("steamapps").join("common").join(folder))
}

/// Fallback for when the uninstall key is missing or stale.
fn steam_library_walk(out: &mut Vec<Install>) {
    let mut seen: HashSet<String> = HashSet::new();
    for client in steam_clients() {
        if !client.is_dir() {
            continue;
        }
        for library in steam_libraries(&client) {
            let key = library.to_string_lossy().to_lowercase();
            if !seen.insert(key) {
                continue;
            }
            if let Some(root) = steam_manifest(&library) {
                out.push(Install::new(root, "Steam (library folder)"));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// other launchers
// ---------------------------------------------------------------------------

/// GOG Galaxy registers each game by id; match on the title it stores.
///
/// Searching by name rather than by product id means this keeps working if the
/// game is ever re-issued under a different id.
fn gog(out: &mut Vec<Install>) {
    let base = r"SOFTWARE\GOG.com\Games";
    for game_id in registry::subkeys(HKLM, base) {
        let key = format!("{base}\\{game_id}");
        let name = registry::read(HKLM, &key, "gameName").unwrap_or_default();
        if !name.to_lowercase().contains(&GAME_TITLE.to_lowercase()) {
            continue;
        }
        if let Some(path) = registry::read(HKLM, &key, "path") {
            out.push(Install::new(PathBuf::from(path), "GOG Galaxy"));
        }
    }
}

/// Epic keeps one JSON manifest per installed game.
fn epic(out: &mut Vec<Install>) {
    let program_data =
        std::env::var("PROGRAMDATA").unwrap_or_else(|_| r"C:\ProgramData".to_string());
    let dir = Path::new(&program_data)
        .join("Epic")
        .join("EpicGamesLauncher")
        .join("Data")
        .join("Manifests");

    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("item") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let name = data
            .get("DisplayName")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if !name.to_lowercase().contains(&GAME_TITLE.to_lowercase()) {
            continue;
        }
        if let Some(location) = data.get("InstallLocation").and_then(|v| v.as_str()) {
            out.push(Install::new(
                PathBuf::from(location),
                "Epic Games Launcher",
            ));
        }
    }
}

/// Drive roots to sweep.
///
/// Only fixed disks: probing a disconnected network drive can block for
/// seconds, and the game is not going to be on a DVD.
#[cfg(windows)]
fn fixed_drives() -> Vec<PathBuf> {
    // DRIVE_FIXED. Declared here rather than pulling in a winapi crate for two
    // symbols.
    const DRIVE_FIXED: u32 = 3;
    extern "system" {
        fn GetLogicalDrives() -> u32;
        fn GetDriveTypeW(root: *const u16) -> u32;
    }

    let mask = unsafe { GetLogicalDrives() };
    let mut drives = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        let wide: Vec<u16> = root.encode_utf16().chain(std::iter::once(0)).collect();
        if unsafe { GetDriveTypeW(wide.as_ptr()) } == DRIVE_FIXED {
            drives.push(PathBuf::from(root));
        }
    }
    drives
}

#[cfg(not(windows))]
fn fixed_drives() -> Vec<PathBuf> {
    vec![PathBuf::from("/")]
}

/// Microsoft Store / Game Pass.
///
/// The Xbox app installs into `<drive>:\XboxGames\<title>\Content`. The older
/// `WindowsApps` location is also checked, though its ACLs usually stop
/// anything — including the game's own mod loading — from reading it.
fn xbox(out: &mut Vec<Install>) {
    for drive in fixed_drives() {
        let content = drive.join("XboxGames").join(GAME_TITLE).join("Content");
        if content.is_dir() {
            out.push(Install::new(content, "Microsoft Store (Xbox app)"));
        }
    }

    let program_files =
        std::env::var("PROGRAMFILES").unwrap_or_else(|_| r"C:\Program Files".to_string());
    let apps = Path::new(&program_files).join("WindowsApps");
    if let Ok(entries) = std::fs::read_dir(&apps) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with("HelloGames.NoMansSky_") && entry.path().is_dir() {
                out.push(Install::new(
                    entry.path(),
                    "Microsoft Store (WindowsApps)",
                ));
            }
        }
    }
}

/// A short sweep of conventional folders on every fixed drive.
fn usual_places(out: &mut Vec<Install>) {
    let relatives: [&[&str]; 7] = [
        &["Steam", "steamapps", "common", GAME_TITLE],
        &["SteamLibrary", "steamapps", "common", GAME_TITLE],
        &["Games", GAME_TITLE],
        &["GOG Games", GAME_TITLE],
        &["Program Files (x86)", "Steam", "steamapps", "common", GAME_TITLE],
        &["Program Files (x86)", "GOG Galaxy", "Games", GAME_TITLE],
        &["Program Files", "GOG Galaxy", "Games", GAME_TITLE],
    ];
    for drive in fixed_drives() {
        for relative in relatives {
            let mut path = drive.clone();
            for part in relative {
                path.push(part);
            }
            out.push(Install::new(path, "common install folder"));
        }
    }
}

// ---------------------------------------------------------------------------
// public surface
// ---------------------------------------------------------------------------

/// Every install found, best source first, deduplicated and confirmed.
///
/// Order matters to the caller: several installs must never be scanned
/// together, because a mod present in two of them would be reported as
/// conflicting with its own other copy.
pub fn find_installs() -> Vec<Install> {
    let mut candidates: Vec<Install> = Vec::new();

    if let Some(override_path) = std::env::var_os(ENV_OVERRIDE) {
        candidates.push(Install::new(
            PathBuf::from(override_path),
            format!("{ENV_OVERRIDE} environment variable"),
        ));
    }
    steam_uninstall_entry(&mut candidates);
    steam_library_walk(&mut candidates);
    gog(&mut candidates);
    epic(&mut candidates);
    xbox(&mut candidates);
    usual_places(&mut candidates);

    let mut found: Vec<Install> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for candidate in candidates {
        let root = std::fs::canonicalize(&candidate.root)
            .unwrap_or_else(|_| candidate.root.clone());
        // canonicalize gives a \\?\ prefix on Windows; keep the plain form for
        // display and use the canonical one only to deduplicate.
        let key = root.to_string_lossy().to_lowercase();
        if seen.contains(&key) || !looks_like_game(&candidate.root) {
            continue;
        }
        seen.insert(key);
        found.push(candidate);
    }
    found
}

/// The install to use: one that has a MODS folder, else the first found.
pub fn find_install() -> Option<Install> {
    let installs = find_installs();
    installs
        .iter()
        .find(|i| i.has_mods_dir)
        .cloned()
        .or_else(|| installs.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gamedata_is_what_marks_an_install() {
        let dir = std::env::temp_dir().join(format!("nmscheck-gf-{}", std::process::id()));
        let game = dir.join("real");
        std::fs::create_dir_all(game.join("GAMEDATA")).unwrap();
        let empty = dir.join("not-a-game");
        std::fs::create_dir_all(&empty).unwrap();

        assert!(looks_like_game(&game));
        assert!(!looks_like_game(&empty));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mods_dir_is_gamedata_mods_not_pcbanks() {
        let root = Path::new("C:").join("game");
        let mods = mods_dir_of(&root);
        assert!(mods.ends_with(Path::new("GAMEDATA").join("MODS")));
        assert!(!mods.to_string_lossy().contains("PCBANKS"));
    }

    #[test]
    fn library_list_is_read_in_both_vdf_formats() {
        let dir = std::env::temp_dir().join(format!("nmscheck-vdf-{}", std::process::id()));
        let client = dir.join("Steam");
        std::fs::create_dir_all(client.join("steamapps")).unwrap();
        let other = dir.join("SteamLibrary");

        let vdf = format!(
            "\"libraryfolders\"\n{{\n\t\"0\"\n\t{{\n\t\t\"path\"\t\t\"{}\"\n\t}}\n\t\"1\"\t\t\"{}\"\n}}\n",
            client.to_string_lossy().replace('\\', "\\\\"),
            other.to_string_lossy().replace('\\', "\\\\"),
        );
        std::fs::write(client.join("steamapps").join("libraryfolders.vdf"), vdf).unwrap();

        let libraries: Vec<String> = steam_libraries(&client)
            .iter()
            .map(|p| p.to_string_lossy().to_lowercase())
            .collect();

        assert!(libraries.contains(&client.to_string_lossy().to_lowercase()));
        assert!(libraries.contains(&other.to_string_lossy().to_lowercase()));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn app_manifest_gives_the_install_folder() {
        let dir = std::env::temp_dir().join(format!("nmscheck-acf-{}", std::process::id()));
        let steamapps = dir.join("steamapps");
        std::fs::create_dir_all(&steamapps).unwrap();
        std::fs::write(
            steamapps.join(format!("appmanifest_{STEAM_APP_ID}.acf")),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"275850\"\n\t\"installdir\"\t\t\"No Man's Sky\"\n}\n",
        )
        .unwrap();

        let resolved = steam_manifest(&dir).unwrap();
        assert_eq!(resolved, steamapps.join("common").join("No Man's Sky"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn manifest_for_another_library_is_ignored() {
        let dir = std::env::temp_dir().join(format!("nmscheck-noacf-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("steamapps")).unwrap();
        assert!(steam_manifest(&dir).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
