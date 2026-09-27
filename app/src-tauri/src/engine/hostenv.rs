//! Facts about the installed game and the mod manager, when they are there.
//!
//! Modern No Man's Sky (Cosmos 7.x) loads loose *sparse EXML patches* straight
//! out of `GAMEDATA/MODS/<ModFolder>/` and applies them natively -- no pak, no
//! AMUMSS, no MBINCompiler. Two sources of truth may live alongside that
//! folder. **Neither is required** -- a hand-installed mod folder has neither,
//! and the analysis is the same either way -- but both sharpen the report:
//!
//! `vortex.deployment.json`
//!   Written by Vortex, for the people who use it. It maps every deployed file
//!   back to the Nexus archive it came from, so a folder called
//!   `alchemist_GPS` can be reported under its real mod name and version.
//!
//! `Binaries/SETTINGS/GCMODSETTINGS.MXML`
//!   Written by the *game* on a run with mods present. It records which mods
//!   are enabled and in what order, which is the real answer to "who wins a
//!   clash". It does not exist until the game has been launched with mods, so
//!   every reader here degrades gracefully to empty.
//!
//! Port of `nmscc/hostenv.py`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use indexmap::IndexMap;

/// Vortex drops this manifest in the deployment target.
pub const VORTEX_MANIFEST: &str = "vortex.deployment.json";

/// Game-written mod list, relative to the install root.
pub const MOD_SETTINGS: [&str; 3] = ["Binaries", "SETTINGS", "GCMODSETTINGS.MXML"];

/// Whatever could be learned about the surrounding installation.
#[derive(Debug, Default, Clone)]
pub struct HostInfo {
    /// mod folder name -> Nexus archive it was deployed from, tidied for
    /// display: the id, version, timestamp and hash trimmed off the end
    pub sources: HashMap<String, String>,
    /// the same mapping, untouched. The four fields `sources` drops are where
    /// the Nexus mod id and the installed version live, so update checking
    /// reads this one. See [`super::nexusname`].
    pub archives: HashMap<String, String>,
    /// upper-cased mod name -> ModPriority, straight from the game
    pub priorities: IndexMap<String, i64>,
    /// upper-cased names of mods the game has switched off
    pub disabled: Vec<String>,
    /// the game-wide "turn everything off" switch
    pub disable_all: bool,
    pub staging_path: Option<String>,
    pub manager: Option<String>,
    pub settings_path: Option<PathBuf>,
}

impl HostInfo {
    pub fn has_real_order(&self) -> bool {
        !self.priorities.is_empty()
    }

    /// ModPriority for a mod folder, matched case-insensitively.
    pub fn priority_of(&self, mod_name: &str) -> Option<i64> {
        self.priorities.get(&mod_name.to_uppercase()).copied()
    }

    pub fn is_disabled(&self, mod_name: &str) -> bool {
        self.disable_all || self.disabled.contains(&mod_name.to_uppercase())
    }

    /// False for a folder the game has not registered yet.
    pub fn is_known(&self, mod_name: &str) -> bool {
        self.priorities.contains_key(&mod_name.to_uppercase())
    }
}

/// Trim the bookkeeping Vortex appends to an archive name.
///
/// Entries look like `"Refiner Wiki Slots normal 3718 1.4.1
/// 2026-09-22T00-46Z lN9fGp87U"`: name, quality, Nexus id, version, timestamp,
/// hash. The trailing four fields are noise in a report, so drop them when
/// they match that shape and leave anything unexpected untouched.
fn clean_source(raw: &str) -> String {
    let parts: Vec<&str> = raw.split_whitespace().collect();
    if parts.len() >= 4 {
        let stamp = parts[parts.len() - 2];
        let first_four: String = stamp.chars().take(4).collect();
        if stamp.contains('T') && first_four.len() == 4 && first_four.chars().all(|c| c.is_ascii_digit())
        {
            let kept = parts[..parts.len() - 4].join(" ");
            let kept = kept.trim();
            if !kept.is_empty() {
                return kept.to_string();
            }
            return raw.to_string();
        }
    }
    raw.to_string()
}

/// Read `vortex.deployment.json` from `mods_dir`, if it is there.
pub fn read_vortex_manifest(mods_dir: &Path) -> HostInfo {
    let mut info = HostInfo::default();
    let path = mods_dir.join(VORTEX_MANIFEST);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return info;
    };
    let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) else {
        return info;
    };

    info.manager = Some("Vortex".to_string());
    info.staging_path = data
        .get("stagingPath")
        .and_then(|v| v.as_str())
        .map(str::to_string);

    if let Some(files) = data.get("files").and_then(|v| v.as_array()) {
        for entry in files {
            let rel = entry.get("relPath").and_then(|v| v.as_str()).unwrap_or("");
            let source = entry.get("source").and_then(|v| v.as_str()).unwrap_or("");
            if rel.is_empty() || source.is_empty() {
                continue;
            }
            let folder = rel
                .replace('/', "\\")
                .split('\\')
                .next()
                .unwrap_or("")
                .to_string();
            info
                .archives
                .entry(folder.clone())
                .or_insert_with(|| source.to_string());
            info.sources
                .entry(folder)
                .or_insert_with(|| clean_source(source));
        }
    }
    info
}

/// Read the game's own mod list, if the game has written one yet.
///
/// The file is `GcModSettings`: a `DisableAllMods` switch followed by one
/// `GcModSettingsInfo` block per mod holding an upper-cased `Name` (which is
/// the folder name), a `ModPriority` integer and an `Enabled` flag.
pub fn read_mod_settings(game_root: &Path) -> HostInfo {
    let mut info = HostInfo::default();
    let mut path = game_root.to_path_buf();
    for part in MOD_SETTINGS {
        path.push(part);
    }
    if !path.is_file() {
        return info;
    }

    let Ok(bytes) = std::fs::read(&path) else {
        return info;
    };
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let Ok(document) = roxmltree::Document::parse(text) else {
        return info;
    };

    info.settings_path = Some(path);

    // First DisableAllMods anywhere in the document wins.
    for node in document.descendants().filter(|n| n.has_tag_name("Property")) {
        if node.attribute("name") == Some("DisableAllMods") {
            info.disable_all = node
                .attribute("value")
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            break;
        }
    }

    for block in document.descendants().filter(|n| n.has_tag_name("Property")) {
        if block.attribute("value") != Some("GcModSettingsInfo") {
            continue;
        }
        let mut name: Option<String> = None;
        let mut priority: Option<i64> = None;
        let mut enabled = true;

        for child in block.children().filter(|c| c.is_element()) {
            let key = child.attribute("name").unwrap_or("").to_uppercase();
            let value = child.attribute("value");
            match key.as_str() {
                "NAME" => name = value.map(str::to_string),
                "MODPRIORITY" => priority = value.and_then(|v| v.trim().parse::<i64>().ok()),
                "ENABLED" => {
                    enabled = !value.map(|v| v.eq_ignore_ascii_case("false")).unwrap_or(false)
                }
                _ => {}
            }
        }

        let Some(name) = name.filter(|n| !n.is_empty()) else {
            continue;
        };
        let key = name.to_uppercase();
        // A block with no readable priority keeps its document position, which
        // is the order the game wrote them in.
        let fallback = info.priorities.len() as i64;
        info.priorities.insert(key.clone(), priority.unwrap_or(fallback));
        if !enabled {
            info.disabled.push(key);
        }
    }
    info
}

/// Combine every available source of host information.
///
/// `mods_dir` is expected to be `<game>/GAMEDATA/MODS`; the game root is
/// derived from it so the settings file can be found without a second
/// argument.
pub fn describe(mods_dir: &Path) -> HostInfo {
    let mut info = read_vortex_manifest(mods_dir);

    let game_root = mods_dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| mods_dir.to_path_buf());

    let settings = read_mod_settings(&game_root);
    info.priorities = settings.priorities;
    info.disabled = settings.disabled;
    info.disable_all = settings.disable_all;
    info.settings_path = settings.settings_path;
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nmscheck-host-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_settings(game_root: &Path, body: &str) {
        let dir = game_root.join("Binaries").join("SETTINGS");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("GCMODSETTINGS.MXML"), body).unwrap();
    }

    const TWO_MODS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Data template="GcModSettings">
  <Property name="DisableAllMods" value="false" />
  <Property name="Data" value="GcModSettingsInfo" _index="1">
    <Property name="Name" value="ALPHA" />
    <Property name="ModPriority" value="7" />
    <Property name="Enabled" value="true" />
  </Property>
  <Property name="Data" value="GcModSettingsInfo" _index="2">
    <Property name="Name" value="BETA" />
    <Property name="ModPriority" value="2" />
    <Property name="Enabled" value="false" />
  </Property>
</Data>"#;

    #[test]
    fn reads_priorities_and_disabled_flags() {
        let root = scratch("settings");
        write_settings(&root, TWO_MODS);
        let info = read_mod_settings(&root);

        assert!(info.has_real_order());
        assert_eq!(info.priority_of("alpha"), Some(7));
        assert_eq!(info.priority_of("Beta"), Some(2));
        assert!(!info.is_disabled("ALPHA"));
        assert!(info.is_disabled("BETA"));
        assert!(!info.disable_all);
        assert!(info.is_known("Alpha"));
        assert!(!info.is_known("Gamma"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn disable_all_switches_every_mod_off() {
        let root = scratch("disableall");
        write_settings(&root, &TWO_MODS.replace(
            r#"<Property name="DisableAllMods" value="false" />"#,
            r#"<Property name="DisableAllMods" value="true" />"#,
        ));
        let info = read_mod_settings(&root);

        assert!(info.disable_all);
        assert!(info.is_disabled("ALPHA"), "DisableAllMods overrides Enabled");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_settings_file_is_not_an_error() {
        let root = scratch("nosettings");
        let info = read_mod_settings(&root);
        assert!(!info.has_real_order());
        assert!(info.settings_path.is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn describe_finds_settings_two_levels_up_from_mods() {
        let root = scratch("describe");
        write_settings(&root, TWO_MODS);
        let mods = root.join("GAMEDATA").join("MODS");
        std::fs::create_dir_all(&mods).unwrap();

        let info = describe(&mods);
        assert_eq!(info.priority_of("ALPHA"), Some(7));
        // No manifest present: a hand-installed library needs no manager.
        assert!(info.manager.is_none());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn vortex_manifest_maps_folders_to_archive_names() {
        let dir = scratch("vortex");
        std::fs::write(
            dir.join(VORTEX_MANIFEST),
            r#"{"stagingPath": "D:\\Staging", "files": [
                 {"relPath": "RefinerWikiSlots\\METADATA\\X.MBIN",
                  "source": "Refiner Wiki Slots normal 3718 1.4.1 2026-09-22T00-46Z lN9fGp87U"}
               ]}"#,
        )
        .unwrap();

        let info = read_vortex_manifest(&dir);
        assert_eq!(info.manager.as_deref(), Some("Vortex"));
        assert_eq!(info.staging_path.as_deref(), Some(r"D:\Staging"));
        // Only the trailing four fields (id, version, timestamp, hash) are
        // dropped; the quality word stays, matching the Python exactly.
        assert_eq!(
            info.sources.get("RefinerWikiSlots").map(String::as_str),
            Some("Refiner Wiki Slots normal"),
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unexpected_source_shape_is_left_alone() {
        assert_eq!(clean_source("Just A Name"), "Just A Name");
        assert_eq!(clean_source("One"), "One");
    }

    #[test]
    fn no_manifest_means_no_manager_not_a_failure() {
        let dir = scratch("nomanifest");
        let info = read_vortex_manifest(&dir);
        assert!(info.manager.is_none());
        assert!(info.sources.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
