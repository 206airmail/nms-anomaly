//! Filesystem scan that turns mod library folders into [`Mod`] objects.
//!
//! Each immediate subdirectory of a scanned root is treated as one mod,
//! matching how AMUMSS lays out its unpacked output. A root that itself
//! contains assets is accepted as a single mod so one folder can be checked on
//! its own.
//!
//! Port of `nmscc/discovery.py`.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use super::model::{FileKind, Mod, ModFile, ScanStats};
use super::paths::{is_asset, normalize_target};
use super::{exml, hostenv, luascript, mbin};

/// Directories that never hold shippable assets.
const SKIP_DIRS: [&str; 4] = [".GIT", "__PYCACHE__", "BACKUP", "_BACKUP"];

/// Bookkeeping files mod managers scatter through the deployment target.
///
/// Vortex alone leaves one marker per directory, which would otherwise
/// dominate the file counts and make every mod look larger than it is. No
/// manager is required -- a hand-installed folder simply has none of these.
const SKIP_FILES: [&str; 8] = [
    "__FOLDER_MANAGED_BY_VORTEX",
    "VORTEX.DEPLOYMENT.JSON",
    "META.INI", // Mod Organizer 2
    "MODLIST.TXT",
    "FILES.SHA256",
    "DESKTOP.INI",
    "THUMBS.DB",
    ".DS_STORE",
];

fn is_noise(filename: &str) -> bool {
    let upper = filename.to_uppercase();
    SKIP_FILES.contains(&upper.as_str())
}

/// Empty marker AMUMSS drops beside its output, e.g. `AMUMSS_v5.6.5.0w.txt`.
fn amumss_marker(filename: &str) -> Option<String> {
    let upper = filename.to_uppercase();
    let stem = upper.strip_prefix("AMUMSS_V")?.strip_suffix(".TXT")?;
    if stem.is_empty() {
        return None;
    }
    // Keep the original casing of the version itself.
    let start = "AMUMSS_v".len();
    let end = filename.len() - ".txt".len();
    let version = &filename[start..end];
    version
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '.')
        .then(|| version.to_string())
}

fn sha1_of(path: &Path) -> String {
    let Ok(bytes) = std::fs::read(path) else {
        return String::new();
    };
    let mut hasher = sha1_smol::Sha1::new();
    hasher.update(&bytes);
    hasher.digest().to_string()
}

fn classify(filename: &str) -> Option<FileKind> {
    let upper = filename.to_uppercase();
    if upper.ends_with(".MBIN") {
        Some(FileKind::Mbin)
    } else if upper.ends_with(".EXML") {
        Some(FileKind::Exml)
    } else if upper.ends_with(".LUA") {
        Some(FileKind::Lua)
    } else if upper.ends_with(".MXML") {
        Some(FileKind::Mxml)
    } else if upper.ends_with(".DDS") {
        Some(FileKind::Dds)
    } else {
        Some(FileKind::Other)
    }
}

/// Localisation ids contributed by an AMUMSS `LocTable.MXML`.
fn loc_ids(path: &Path) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    let Ok(bytes) = std::fs::read(path) else {
        return ids;
    };
    let text = String::from_utf8_lossy(&bytes);
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let Ok(document) = roxmltree::Document::parse(text) else {
        return ids;
    };
    for node in document.descendants().filter(|n| n.has_tag_name("Property")) {
        if node.attribute("name").unwrap_or("").to_uppercase() == "ID" {
            if let Some(value) = node.attribute("value") {
                if !value.is_empty() {
                    ids.insert(value.to_string());
                }
            }
        }
    }
    ids
}

fn load_file(abs_path: &Path, rel_path: &str, deep: bool) -> ModFile {
    let kind = classify(rel_path);
    let size = std::fs::metadata(abs_path).map(|m| m.len()).unwrap_or(0);

    let mut entry = ModFile {
        abs_path: abs_path.to_string_lossy().into_owned(),
        rel_path: rel_path.to_string(),
        kind,
        size,
        ..Default::default()
    };

    if is_asset(rel_path) {
        entry.target = Some(normalize_target(rel_path));
    }

    match kind {
        Some(FileKind::Dds) => entry.sha1 = sha1_of(abs_path),
        Some(FileKind::Mbin) => {
            entry.sha1 = sha1_of(abs_path);
            let header = mbin::read_header(abs_path);
            if header.valid {
                entry.mbinc_version = header.version;
            } else {
                entry.parse_error = header.error;
            }
        }
        Some(FileKind::Exml) => {
            entry.sha1 = sha1_of(abs_path);
            if deep {
                // Flattening is the expensive half of reading a library, and
                // the hash above already says which file this is -- so it is
                // done once per distinct file rather than once per scan.
                let doc = super::propcache::parse(abs_path, &entry.sha1);
                entry.props = doc.props;
                entry.annotations = doc.annotations;
                entry.template = doc.template;
                entry.mbinc_version = doc.mbinc_version;
                entry.amumss_version = doc.amumss_version;
                entry.parse_error = doc.error;
            } else {
                let (version, amumss) = exml::read_stamps(abs_path);
                entry.mbinc_version = version;
                entry.amumss_version = amumss;
            }
        }
        _ => {}
    }

    entry
}

/// Every file under `root`, in Python's `os.walk` order.
///
/// The order is load-bearing: a stale finding quotes the first three stamped
/// files it meets as examples, so a different traversal quotes different
/// files and the two engines disagree. `os.walk` yields *all* files of a
/// directory before descending into any of its subdirectories, whereas a
/// plain sorted walk interleaves them -- `ACTIVATEDCOPPER/` sorts before
/// `ACTIVATEDCOPPER.SCENE.MBIN` and would be entered first.
///
/// Within a directory: files sorted by name (Python's `sorted(filenames)`),
/// subdirectories left in `read_dir` order, which is the same enumeration
/// `os.listdir` gets. Symlinked directories are not followed, matching
/// `os.walk`'s `followlinks=False`.
fn walk_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_into(root, &mut out);
    out
}

fn walk_into(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    let mut files: Vec<PathBuf> = Vec::new();
    let mut dirs: Vec<PathBuf> = Vec::new();

    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            let name = entry.file_name().to_string_lossy().to_uppercase();
            if !SKIP_DIRS.contains(&name.as_str()) {
                dirs.push(entry.path());
            }
        } else if file_type.is_file() {
            files.push(entry.path());
        }
    }

    files.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    out.extend(files);

    for subdir in dirs {
        walk_into(&subdir, out);
    }
}

/// Scan one mod folder.
pub fn scan_mod(root: &Path, name: &str, deep: bool) -> Mod {
    let mut the_mod = Mod {
        name: name.to_string(),
        root: root.to_string_lossy().into_owned(),
        ..Default::default()
    };

    for abs_path in walk_files(root) {
        let filename = abs_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if is_noise(&filename) {
            continue;
        }

        let rel_path = abs_path
            .strip_prefix(root)
            .unwrap_or(&abs_path)
            .to_string_lossy()
            .into_owned();
        let abs_path = abs_path.as_path();

        if let Some(version) = amumss_marker(&filename) {
            the_mod.amumss_version = Some(version);
        }

        let file = load_file(abs_path, &rel_path, deep);

        match file.kind {
            Some(FileKind::Lua) => {
                the_mod
                    .declared_targets
                    .extend(luascript::declared_targets(abs_path));
            }
            Some(FileKind::Mxml) => {
                the_mod.loc_ids.extend(loc_ids(abs_path));
            }
            Some(FileKind::Exml) => {
                if the_mod.amumss_version.is_none() {
                    if let Some(version) = &file.amumss_version {
                        the_mod.amumss_version = Some(version.clone());
                    }
                }
            }
            _ => {}
        }

        the_mod.files.push(file);
    }

    the_mod
}

fn has_assets(path: &Path) -> bool {
    walk_files(path).iter().any(|p| {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        !is_noise(&name) && (is_asset(&name) || name.to_uppercase().ends_with(".LUA"))
    })
}

/// Scan every library root and return the mods, counters and host facts.
pub fn scan_roots(
    roots: &[PathBuf],
    deep: bool,
) -> std::io::Result<(Vec<Mod>, ScanStats, hostenv::HostInfo)> {
    let mut mods: Vec<Mod> = Vec::new();
    let mut seen: HashMap<String, PathBuf> = HashMap::new();
    let mut hosts: Vec<hostenv::HostInfo> = Vec::new();

    for root in roots {
        let root = std::path::absolute(root).unwrap_or_else(|_| root.clone());
        if !root.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                root.to_string_lossy().into_owned(),
            ));
        }

        let mut children: Vec<String> = std::fs::read_dir(&root)?
            .flatten()
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|name| !SKIP_DIRS.contains(&name.to_uppercase().as_str()))
            .collect();
        children.sort();

        let mut mod_dirs: Vec<(PathBuf, String)> = children
            .into_iter()
            .map(|name| (root.join(&name), name))
            .filter(|(path, _)| has_assets(path))
            .collect();

        // A root holding assets directly is itself a single mod.
        if mod_dirs.is_empty() && has_assets(&root) {
            let name = root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            mod_dirs = vec![(root.clone(), name)];
        }

        let host = hostenv::describe(&root);

        for (path, name) in &mod_dirs {
            let mut unique = name.clone();
            let mut suffix = 2;
            while seen.contains_key(&unique) {
                unique = format!("{name} ({suffix})");
                suffix += 1;
            }
            seen.insert(unique.clone(), path.clone());

            let mut the_mod = scan_mod(path, &unique, deep);
            the_mod.source = host.sources.get(name).cloned();
            the_mod.archive = host.archives.get(name).cloned();
            the_mod.disabled = host.is_disabled(name);
            the_mod.priority = host.priority_of(name);
            mods.push(the_mod);
        }

        hosts.push(host);

        // AMUMSS recipes sometimes sit loose in the mods root rather than
        // inside a mod folder. Attribute one to the folder it is named after
        // so its declared targets are not lost.
        let by_upper: HashMap<String, String> = mod_dirs
            .iter()
            .map(|(_, n)| (n.to_uppercase(), n.clone()))
            .collect();

        let mut loose: Vec<String> = std::fs::read_dir(&root)?
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.to_uppercase().ends_with(".LUA"))
            .collect();
        loose.sort();

        for filename in loose {
            let stem = Path::new(&filename)
                .file_stem()
                .map(|s| s.to_string_lossy().to_uppercase())
                .unwrap_or_default();
            let Some(owner) = by_upper.get(&stem) else {
                continue;
            };
            for the_mod in mods.iter_mut() {
                if &the_mod.name == owner {
                    the_mod
                        .declared_targets
                        .extend(luascript::declared_targets(&root.join(&filename)));
                    break;
                }
            }
        }
    }

    let mut stats = ScanStats {
        mods: mods.len(),
        ..Default::default()
    };
    let mut targets: BTreeSet<String> = BTreeSet::new();

    for the_mod in &mods {
        for entry in &the_mod.files {
            stats.files += 1;
            if let Some(target) = &entry.target {
                stats.assets += 1;
                targets.insert(target.clone());
            }
            match entry.kind {
                Some(FileKind::Exml) => stats.exml += 1,
                Some(FileKind::Mbin) => stats.mbin += 1,
                Some(FileKind::Lua) => stats.lua += 1,
                Some(FileKind::Dds) => stats.dds += 1,
                _ => {}
            }
            if let Some(error) = &entry.parse_error {
                stats.parse_errors += 1;
                stats.error_details.push(format!(
                    "{} :: {} -- {}",
                    the_mod.name, entry.rel_path, error
                ));
            }
        }
    }
    stats.targets = targets.len();

    // Only one root normally carries a game install; keep the richest reading.
    let host = hosts
        .into_iter()
        .max_by_key(|h| (h.priorities.len(), h.sources.len()))
        .unwrap_or_default();

    Ok((mods, stats, host))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "nmscheck-disc-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    const FRAGMENT: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
        <!--File created using MBINCompiler version (7.03.0.1)-->\n\
        <Data template=\"GcGameplayGlobals\">\n\
        <Property name=\"GroundRunSpeed\" value=\"12\" />\n\
        </Data>\n";

    #[test]
    fn each_subdirectory_is_one_mod() {
        let root = scratch("subdirs");
        write(&root.join("Alpha").join("GLOBALS").join("A.EXML"), FRAGMENT);
        write(&root.join("Beta").join("GLOBALS").join("B.EXML"), FRAGMENT);

        let (mods, stats, _host) = scan_roots(&[root.clone()], true).unwrap();
        let names: Vec<&str> = mods.iter().map(|m| m.name.as_str()).collect();

        assert_eq!(names, vec!["Alpha", "Beta"]);
        assert_eq!(stats.mods, 2);
        assert_eq!(stats.exml, 2);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn manager_noise_is_not_counted_as_a_file() {
        let root = scratch("noise");
        let mod_dir = root.join("Alpha");
        write(&mod_dir.join("GLOBALS").join("A.EXML"), FRAGMENT);
        write(&mod_dir.join("__folder_managed_by_vortex"), "");
        write(&mod_dir.join("meta.ini"), "[General]");

        let (mods, stats, _host) = scan_roots(&[root.clone()], true).unwrap();
        assert_eq!(stats.files, 1, "only the asset should count");
        assert_eq!(mods[0].files.len(), 1);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn targets_collapse_exml_and_mbin_onto_one_key() {
        let root = scratch("targets");
        write(&root.join("A").join("GLOBALS").join("X.EXML"), FRAGMENT);
        // A second mod shipping the compiled form of the same asset.
        let mbin = root.join("B").join("GLOBALS").join("X.MBIN");
        std::fs::create_dir_all(mbin.parent().unwrap()).unwrap();
        std::fs::write(&mbin, vec![0u8; 64]).unwrap();

        let (_mods, stats, _host) = scan_roots(&[root.clone()], true).unwrap();
        assert_eq!(stats.targets, 1, "EXML and MBIN are the same game asset");
        assert_eq!(stats.assets, 2);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_root_holding_assets_is_itself_one_mod() {
        let root = scratch("bare");
        write(&root.join("GLOBALS").join("A.EXML"), FRAGMENT);

        let (mods, _stats, _host) = scan_roots(&[root.clone()], true).unwrap();
        assert_eq!(mods.len(), 1);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn deep_scan_reads_properties_shallow_does_not() {
        let root = scratch("deep");
        write(&root.join("A").join("GLOBALS").join("X.EXML"), FRAGMENT);

        let (deep, _, _) = scan_roots(&[root.clone()], true).unwrap();
        assert!(!deep[0].files[0].props.is_empty());
        // Either way the version stamp is read, which staleness needs.
        assert_eq!(
            deep[0].files[0].mbinc_version.unwrap().to_string(),
            "7.03.0.1"
        );

        let (shallow, _, _) = scan_roots(&[root.clone()], false).unwrap();
        assert!(shallow[0].files[0].props.is_empty());
        assert_eq!(
            shallow[0].files[0].mbinc_version.unwrap().to_string(),
            "7.03.0.1"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn priority_and_disabled_come_from_the_game() {
        let root = scratch("priority");
        let mods_dir = root.join("GAMEDATA").join("MODS");
        write(&mods_dir.join("Alpha").join("G").join("A.EXML"), FRAGMENT);
        write(&mods_dir.join("Beta").join("G").join("B.EXML"), FRAGMENT);
        write(
            &root.join("Binaries").join("SETTINGS").join("GCMODSETTINGS.MXML"),
            r#"<Data template="GcModSettings">
                 <Property name="DisableAllMods" value="false" />
                 <Property name="Data" value="GcModSettingsInfo">
                   <Property name="Name" value="ALPHA" />
                   <Property name="ModPriority" value="5" />
                   <Property name="Enabled" value="true" />
                 </Property>
                 <Property name="Data" value="GcModSettingsInfo">
                   <Property name="Name" value="BETA" />
                   <Property name="ModPriority" value="9" />
                   <Property name="Enabled" value="false" />
                 </Property>
               </Data>"#,
        );

        let (mods, _stats, host) = scan_roots(&[mods_dir], true).unwrap();
        assert!(host.has_real_order());
        let alpha = mods.iter().find(|m| m.name == "Alpha").unwrap();
        let beta = mods.iter().find(|m| m.name == "Beta").unwrap();
        assert_eq!(alpha.priority, Some(5));
        assert!(!alpha.disabled);
        assert_eq!(beta.priority, Some(9));
        assert!(beta.disabled);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_loose_recipe_is_attributed_to_the_folder_it_names() {
        let root = scratch("loose");
        write(&root.join("Alpha").join("G").join("A.EXML"), FRAGMENT);
        write(
            &root.join("Alpha.lua"),
            r#"["MBIN_FILE_SOURCE"] = "GLOBALS/DECLARED.MBIN""#,
        );

        let (mods, _stats, _host) = scan_roots(&[root.clone()], true).unwrap();
        assert!(mods[0].declared_targets.contains("GLOBALS/DECLARED.MBIN"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_root_is_an_error_not_a_panic() {
        let missing = std::env::temp_dir().join("nmscheck-definitely-not-here");
        assert!(scan_roots(&[missing], true).is_err());
    }

    #[test]
    fn amumss_marker_version_is_picked_up() {
        assert_eq!(amumss_marker("AMUMSS_v5.6.5.0w.txt").as_deref(), Some("5.6.5.0w"));
        assert_eq!(amumss_marker("notes.txt"), None);
        assert_eq!(amumss_marker("AMUMSS_v.txt"), None);
    }
}
