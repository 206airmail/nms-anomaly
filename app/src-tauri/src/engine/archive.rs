//! Install a mod from the archive it was downloaded as.
//!
//! Extraction is delegated to 7-Zip. That is a deliberate choice rather than a
//! shortcut: Nexus mods for this game ship as `.zip`, `.7z` and `.rar`, and
//! RAR's format licence forbids using its reference code to *create* an
//! implementation, so a pure-Rust `.rar` reader is not something to reach for
//! casually. 7-Zip reads all three, is already on this machine, and if it is
//! missing we say so instead of half-working on two formats out of three.
//!
//! # The shape of a mod archive
//!
//! The game loads `GAMEDATA\MODS\<Some Mod>\GLOBALS\...`, so every install has
//! to end up with a mod folder. Archives arrive in three shapes:
//!
//! ```text
//! Cool Mod/GLOBALS/...            already a mod folder     -> keep as is
//! Cool Mod/..., Cool Mod.lua      a mod folder and its kit -> keep as is
//! GLOBALS/..., METADATA/...       a mod's insides          -> wrap in a folder
//! MODS/Cool Mod/...               packaging                -> strip, then as is
//! ```
//!
//! The second shape is the common one -- 36 of the 62 mods in the measured
//! library ship a folder, a `.lua` and a readme side by side -- and the three
//! have to stay siblings, because AMUMSS looks for its script next to the
//! folder it builds. [`plan_layout`] tells these apart.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

/// Folder names that are packaging, not the mod.
///
/// An author who zips from inside their game folder catches one of these; the
/// mod is the thing underneath.
const WRAPPERS: [&str; 3] = ["MODS", "GAMEDATA", "PCBANKS"];

/// Folders the game reads *inside* a mod.
///
/// Taken from the measured library rather than from documentation: `GLOBALS`
/// is by far the commonest and appears as both `GLOBALS` and `Globals`, and 23
/// of the 62 installed mods contain no `MODELS` or `METADATA` at all. One of
/// these at an archive's top level means the archive holds a mod's *insides*
/// and needs a folder around them.
const ASSET_ROOTS: [&str; 12] = [
    "GLOBALS",
    "METADATA",
    "MODELS",
    "TEXTURES",
    "SCENES",
    "AUDIO",
    "FONTS",
    "LANGUAGE",
    "UI",
    "SHADERS",
    "EFFECTS",
    "ENTITIES",
];

/// Extensions the game itself loads, as opposed to notes shipped alongside.
const ASSET_FILES: [&str; 4] = [".EXML", ".MBIN", ".MXML", ".DDS"];

fn is_asset_root(name: &str) -> bool {
    ASSET_ROOTS.iter().any(|r| name.eq_ignore_ascii_case(r))
}

fn is_asset_file(name: &str) -> bool {
    let upper = name.to_uppercase();
    ASSET_FILES.iter().any(|e| upper.ends_with(e))
}

/// Where 7-Zip is, if it is anywhere.
///
/// Order matters: the copy we ship wins over whatever the machine happens to
/// have, so a release behaves the same everywhere. A user who wants their own
/// build still overrides everything with `NMSCHECK_7Z`.
pub fn find_7z() -> Option<PathBuf> {
    if let Ok(from_env) = std::env::var("NMSCHECK_7Z") {
        let path = PathBuf::from(from_env);
        if path.is_file() {
            return Some(path);
        }
    }
    // The bundled copy, under `tools/sevenzip/` beside the other binaries.
    // `7z.exe` needs `7z.dll` next to it for anything beyond the 7z format, so
    // they ship together and are found together.
    for dir in super::tools::dirs() {
        let bundled = dir.join("sevenzip").join(if cfg!(windows) { "7z.exe" } else { "7zz" });
        if bundled.is_file() {
            return Some(bundled);
        }
    }
    let candidates = [
        r"C:\Program Files\7-Zip\7z.exe",
        r"C:\Program Files (x86)\7-Zip\7z.exe",
        "/usr/bin/7z",
        "/usr/bin/7zz",
        "/usr/local/bin/7z",
    ];
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
        .or_else(|| {
            // On PATH under any of its several names.
            for name in ["7z", "7zz", "7za"] {
                if Command::new(name).arg("i").output().is_ok() {
                    return Some(PathBuf::from(name));
                }
            }
            None
        })
}

/// What installing an archive would do, before it does it.
#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    /// the folder name the mod will have in the mods directory
    pub owner: String,
    /// how many files it holds
    pub files: usize,
    /// true when a folder of that name is already installed
    pub collides: bool,
    /// anything the user should know before pressing the button
    pub notes: Vec<String>,
}

/// List an archive's entries without extracting it.
fn list(seven: &Path, archive: &Path) -> Result<Vec<String>, String> {
    let out = Command::new(seven)
        .args(["l", "-ba", "-slt"])
        .arg(archive)
        .output()
        .map_err(|e| format!("could not run 7-Zip: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "7-Zip could not read {}: {}",
            archive.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    // `-slt` prints a block per entry; we want Path, and only for files.
    let text = String::from_utf8_lossy(&out.stdout);
    let mut paths = Vec::new();
    let mut path: Option<String> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Path = ") {
            path = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("Attributes = ") {
            let is_dir = rest.contains('D');
            if let Some(p) = path.take() {
                if !is_dir {
                    paths.push(p.replace('/', "\\"));
                }
            }
        }
    }
    Ok(paths)
}

/// How an archive's contents should be laid out in the mods folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// what to call the mod: the folder it creates, or the archive's own name
    pub owner: String,
    /// leading path segments that are packaging and get dropped
    pub strip: usize,
    /// true when the contents are a mod's insides and need a folder round them
    pub wrap: bool,
}

/// Work out where an archive's contents belong. See the module docs.
pub fn plan_layout(entries: &[String], archive_stem: &str) -> Layout {
    let rows: Vec<Vec<String>> = entries
        .iter()
        .map(|p| {
            p.split('\\')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        })
        .collect();

    let mut strip = 0usize;
    loop {
        let here: Vec<&Vec<String>> = rows.iter().filter(|r| r.len() > strip).collect();
        if here.is_empty() {
            return Layout {
                owner: archive_stem.to_string(),
                strip,
                wrap: true,
            };
        }

        // The distinct names at this level, and whether each one is a file.
        let mut names: Vec<(&str, bool)> = Vec::new();
        for row in &here {
            let name = row[strip].as_str();
            let is_file = row.len() == strip + 1;
            if !names.iter().any(|(n, _)| *n == name) {
                names.push((name, is_file));
            }
        }

        // A single folder wrapping everything, and it is only packaging.
        if names.len() == 1 && !names[0].1 && WRAPPERS.contains(&names[0].0.to_uppercase().as_str())
        {
            strip += 1;
            continue;
        }

        // Everything here is something the *game* reads, so this is the inside
        // of a mod and wants a folder around it.
        let all_game_content = names.iter().all(|(name, is_file)| {
            if *is_file {
                is_asset_file(name)
            } else {
                is_asset_root(name)
            }
        });
        if all_game_content {
            return Layout {
                owner: archive_stem.to_string(),
                strip,
                wrap: true,
            };
        }

        // Otherwise this level is already shaped for the mods folder. Name it
        // after its only folder when there is one, so the library reads well.
        let folders: Vec<&str> = names.iter().filter(|(_, f)| !f).map(|(n, _)| *n).collect();
        let owner = match folders.as_slice() {
            [only] => (*only).to_string(),
            _ => archive_stem.to_string(),
        };
        return Layout {
            owner,
            strip,
            wrap: false,
        };
    }
}

/// What installing this archive would produce.
pub fn preview(archive: &Path, mods_dir: &Path) -> Result<Plan, String> {
    let seven = find_7z().ok_or(
        "7-Zip was not found. Install it from 7-zip.org, or set NMSCHECK_7Z to its 7z.exe.",
    )?;
    let entries = list(&seven, archive)?;
    if entries.is_empty() {
        return Err("that archive has no files in it".into());
    }
    let stem = archive
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Mod")
        .to_string();
    let layout = plan_layout(&entries, &stem);
    let owner = layout.owner.clone();

    let mut notes = Vec::new();
    if layout.wrap {
        notes.push(format!(
            "The archive holds a mod's contents rather than a mod folder, so they go into \
             a folder called {owner}."
        ));
    }
    if !entries.iter().any(|e| is_asset_file(e)) {
        notes.push(
            "No .EXML or .MBIN inside, so this may be a .pak-based mod the game will not read \
             loose, or documentation only."
                .into(),
        );
    }

    Ok(Plan {
        files: entries.len(),
        collides: mods_dir.join(&owner).exists(),
        owner,
        notes,
    })
}

/// Extract an archive into the staging folder as one mod.
///
/// `staging_dir/<owner>` ends up holding exactly what should appear in the
/// mods folder, at the same relative paths -- so deploying is a straight copy
/// of its contents and never has to think about shape again.
///
/// Extracts to a temporary folder first, so a failure part-way leaves staging
/// untouched rather than half-written.
pub fn install(archive: &Path, staging_dir: &Path, overwrite: bool) -> Result<Plan, String> {
    let plan = preview(archive, staging_dir)?;
    let target = staging_dir.join(&plan.owner);
    if target.exists() && !overwrite {
        return Err(format!(
            "{} is already installed. Remove it first, or choose to replace it.",
            plan.owner
        ));
    }

    let seven = find_7z().ok_or("7-Zip was not found")?;
    let scratch = staging_dir.join(format!(".nmscheck-installing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).map_err(|e| format!("could not stage: {e}"))?;

    let ran = Command::new(&seven)
        .arg("x")
        .arg(archive)
        .arg(format!("-o{}", scratch.display()))
        .args(["-y", "-bso0", "-bse0"])
        .output()
        .map_err(|e| format!("could not run 7-Zip: {e}"));

    let result = (|| {
        let ran = ran?;
        if !ran.status.success() {
            return Err(format!(
                "7-Zip could not extract it: {}",
                String::from_utf8_lossy(&ran.stderr).trim()
            ));
        }

        let entries = list(&seven, archive)?;
        let stem = archive.file_stem().and_then(|s| s.to_str()).unwrap_or("Mod");
        let layout = plan_layout(&entries, stem);

        // Walk down the packaging levels the plan identified.
        let mut source = scratch.clone();
        for _ in 0..layout.strip {
            let mut dirs = std::fs::read_dir(&source)
                .map_err(|e| e.to_string())?
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir());
            let Some(only) = dirs.next() else { break };
            if dirs.next().is_some() {
                break; // more than one: the plan said this level is content
            }
            source = only;
        }

        if target.exists() {
            std::fs::remove_dir_all(&target)
                .map_err(|e| format!("could not replace {}: {e}", plan.owner))?;
        }

        if layout.wrap {
            // A mod's insides: put a folder round them, so the staged mod is
            // `<owner>/<owner>/GLOBALS/...` and deploys to `MODS/<owner>/...`.
            let inner = target.join(&plan.owner);
            std::fs::create_dir_all(&inner)
                .map_err(|e| format!("could not make {}: {e}", plan.owner))?;
            for entry in std::fs::read_dir(&source)
                .map_err(|e| e.to_string())?
                .flatten()
            {
                std::fs::rename(entry.path(), inner.join(entry.file_name()))
                    .map_err(|e| format!("could not lay out {}: {e}", plan.owner))?;
            }
        } else {
            std::fs::rename(&source, &target)
                .map_err(|e| format!("could not put {} in place: {e}", plan.owner))?;
        }
        Ok(())
    })();

    let _ = std::fs::remove_dir_all(&scratch);
    result?;
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn layout(items: &[&str], stem: &str) -> (String, usize, bool) {
        let l = plan_layout(&paths(items), stem);
        (l.owner, l.strip, l.wrap)
    }

    #[test]
    fn an_archive_that_is_already_a_mod_folder_is_left_alone() {
        assert_eq!(
            layout(
                &["Cool Mod\\GLOBALS\\A.EXML", "Cool Mod\\readme.txt"],
                "Cool Mod 1.0-123"
            ),
            ("Cool Mod".into(), 0, false)
        );
    }

    #[test]
    fn a_mods_insides_get_a_folder_round_them() {
        // `GLOBALS` at the top level means these are the contents of a mod,
        // and the game needs `MODS\<name>\GLOBALS\...`.
        assert_eq!(
            layout(&["GLOBALS\\A.EXML", "METADATA\\B.EXML"], "Cool Mod 1.0-123"),
            ("Cool Mod 1.0-123".into(), 0, true)
        );
        // Lower case too: the measured library has both spellings.
        assert_eq!(
            layout(&["Globals\\A.EXML"], "Tiny Mod"),
            ("Tiny Mod".into(), 0, true)
        );
        // A loose asset with nothing round it is the same situation.
        assert_eq!(layout(&["A.EXML"], "Tiny Mod"), ("Tiny Mod".into(), 0, true));
    }

    #[test]
    fn a_script_beside_the_folder_keeps_them_as_siblings() {
        // The commonest real shape: 36 of 62 installed mods look like this,
        // and the `.lua` has to end up beside the folder, not inside it.
        assert_eq!(
            layout(
                &[
                    "Unpredictable Shelters 1.4\\MODELS\\A.EXML",
                    "Unpredictable Shelters 1.4.lua",
                    "Installation Notes for Unpredictable Shelters.txt",
                ],
                "Unpredictable Shelters 1.3-2308-1-3-1738789143",
            ),
            ("Unpredictable Shelters 1.4".into(), 0, false)
        );
    }

    #[test]
    fn a_wrapper_folder_is_looked_through_not_installed() {
        // An author who zipped from GAMEDATA would otherwise give us a mod
        // folder called "MODS", which the game ignores.
        assert_eq!(
            layout(&["MODS\\Cool Mod\\GLOBALS\\A.EXML"], "whatever"),
            ("Cool Mod".into(), 1, false)
        );
        assert_eq!(
            layout(&["GAMEDATA\\MODS\\Cool Mod\\A.EXML"], "whatever"),
            ("Cool Mod".into(), 2, false)
        );
    }

    #[test]
    fn two_mods_side_by_side_keep_the_archives_name() {
        // Nothing sensible to do but name it after the archive; both folders
        // still deploy as siblings, which is what the game wants.
        assert_eq!(
            layout(
                &["Mod A\\GLOBALS\\X.EXML", "Mod B\\GLOBALS\\Y.EXML"],
                "Pack 2.0"
            ),
            ("Pack 2.0".into(), 0, false)
        );
    }

    #[test]
    fn an_empty_archive_does_not_panic() {
        assert_eq!(layout(&[], "Nothing"), ("Nothing".into(), 0, true));
    }
}
