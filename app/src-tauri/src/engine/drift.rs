//! Detection of scene replacements that move nodes they did not mean to.
//!
//! A mod shipping a whole `.SCENE.MBIN` owns every node in it, so the only way
//! to know which nodes it meant to change is to hold the vanilla file next to
//! it. [`super::vanilla`] makes that cheap; this module does the comparing.
//!
//! **Not every move is a bug.** Plenty of scene mods reposition things on
//! purpose -- a teleporter mod moves teleporters, and on a real library that
//! accounts for the large majority of moved nodes. Reporting every displacement
//! as a fault would bury the real ones.
//!
//! The discriminator is what happened *underneath* the node. Moving something
//! deliberately carries its contents along. An exporter that rebakes a parent's
//! offset down into its children leaves every piece of geometry exactly where
//! it was and moves only the parent -- so the scene still renders correctly and
//! nothing looks wrong in game, right up until engine code reads that node's
//! position as an anchor. A node that moved while everything under it stayed
//! put is therefore the finding; everything else is reported as information.
//!
//! **Why there is no vanilla-free variant check.** An earlier version compared
//! a mod's own copies of an asset family (`HANGAR` / `HANGARGHOST` /
//! `HANGARPIRATE`) on the theory that they should agree about shared nodes.
//! Measured against the real game, they should not: vanilla itself places
//! `HangarA/Approach3a` differently in the normal and pirate hangars, so the
//! check reported a false fault on an untouched library. The idea is recorded
//! here so it is not rediscovered and shipped.

use std::path::Path;

use super::decompile::Decompiler;
use super::model::{FileKind, Mod, ModFile, SceneDrift, Severity};
use super::tools;
use super::scene::{self, Scene};
use super::vanilla::VanillaSource;

/// Assets this check applies to. A scene is the only asset whose meaning
/// depends on where its nodes sit, and the only one compared positionally.
pub const SCENE_SUFFIX: &str = ".SCENE.MBIN";

/// Wholesale scene replacements across the library.
///
/// Only compiled `.MBIN` scenes qualify. A `.SCENE.EXML` from AMUMSS is usually
/// a *sparse* patch naming a few properties, and treating its handful of nodes
/// as a complete scene graph would report every node it omits as deleted.
pub fn scene_assets(mods: &[Mod]) -> Vec<(&Mod, &ModFile)> {
    let mut out = Vec::new();
    for entry in mods {
        for file in entry.assets() {
            let is_scene = file
                .target
                .as_deref()
                .map(|t| t.ends_with(SCENE_SUFFIX))
                .unwrap_or(false);
            if file.kind == Some(FileKind::Mbin) && is_scene {
                out.push((entry, file));
            }
        }
    }
    out
}

/// Decompile an MBIN and parse it as a scene graph.
fn scene_of(path: &Path, sha1: &str, decompiler: &mut Decompiler) -> Option<Scene> {
    let xml_path = if sha1.is_empty() {
        decompiler.decompile_file(path)?
    } else {
        decompiler.decompile(path, sha1)?
    };
    let parsed = scene::parse(&xml_path);
    parsed.error.is_none().then_some(parsed)
}

/// Compare every scene replacement against the game's own copy.
///
/// Returns an empty list when the machine lacks either tool, so the engine
/// degrades to its previous behaviour rather than failing.
pub fn detect(
    mods: &[Mod],
    decompiler: Option<&mut Decompiler>,
    source: Option<&mut VanillaSource>,
) -> Vec<SceneDrift> {
    let (Some(decompiler), Some(source)) = (decompiler, source) else {
        return Vec::new();
    };
    let assets = scene_assets(mods);
    if assets.is_empty() {
        return Vec::new();
    }

    let targets: Vec<String> = assets
        .iter()
        .filter_map(|(_m, f)| f.target.clone())
        .collect();
    let extraction = source.fetch(&targets);

    let mut findings = Vec::new();
    for (owner, file) in assets {
        let Some(target) = file.target.as_deref() else {
            continue;
        };
        // No counterpart in PCBANKS: the mod ships an asset the game does not
        // have. That is not drift, and saying otherwise would flag every
        // genuinely new model a mod adds.
        let Some(vanilla_mbin) = extraction.found.get(target) else {
            continue;
        };
        let Some(before) = scene_of(vanilla_mbin, "", decompiler) else {
            continue;
        };
        let Some(after) = scene_of(Path::new(&file.abs_path), &file.sha1, decompiler) else {
            continue;
        };

        let diff = scene::diff(&before, &after);
        if diff.clean() {
            continue;
        }
        let rebaked = diff.moved.iter().any(|m| m.contents_held);
        findings.push(SceneDrift {
            mod_name: owner.name.clone(),
            rel_path: file.rel_path.clone(),
            target: target.to_string(),
            moved: diff.moved,
            added: diff.added.len(),
            removed: diff.removed.len(),
            reference: "vanilla".to_string(),
            severity: if rebaked {
                Severity::Major
            } else {
                Severity::Info
            },
        });
    }

    findings.sort_by(|a, b| {
        let rank = |d: &SceneDrift| u8::from(d.severity == Severity::Info);
        let worst = |d: &SceneDrift| d.moved.first().map(|m| m.distance()).unwrap_or(0.0);
        rank(a)
            .cmp(&rank(b))
            .then_with(|| {
                worst(b)
                    .partial_cmp(&worst(a))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| a.mod_name.cmp(&b.mod_name))
    });
    findings
}

/// Locate the tools, run the scene check, and report what was available.
///
/// This is the entry point callers should use. [`detect`] returns an empty list
/// when a tool is missing, which is indistinguishable from a clean library --
/// the [`tools::Status`] returned here is what lets the UI say "not checked"
/// instead of implying nothing is wrong.
pub fn run(
    mods: &[Mod],
    game_root: Option<&Path>,
    mbincompiler: Option<&str>,
    hgpaktool: Option<&str>,
) -> (Vec<SceneDrift>, tools::Status) {
    let mut decompiler = Decompiler::locate(mbincompiler, None);
    let mut source = game_root.and_then(|root| VanillaSource::locate(root, hgpaktool));

    let mut status = tools::Status {
        mbincompiler: decompiler.as_ref().map(|d| d.exe.display().to_string()),
        hgpaktool: source.as_ref().map(|s| s.exe.display().to_string()),
        scene_check: false,
        scene_check_note: None,
    };

    // Say which piece is missing rather than "unavailable": the two have
    // different fixes, and the game not being found is not the same problem as
    // a tool not being installed.
    status.scene_check_note = match (&decompiler, &source, game_root) {
        (None, _, _) if status.hgpaktool.is_none() => {
            Some("MBINCompiler and hgpaktool were not found".to_string())
        }
        (None, _, _) => Some("MBINCompiler was not found".to_string()),
        (_, None, None) => Some("no game install was found to compare against".to_string()),
        (_, None, Some(_)) => {
            Some("hgpaktool was not found, or the game has no PCBANKS folder".to_string())
        }
        _ => None,
    };

    if status.scene_check_note.is_some() {
        return (Vec::new(), status);
    }

    let findings = detect(mods, decompiler.as_mut(), source.as_mut());
    status.scene_check = true;
    (findings, status)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mod_with(files: Vec<ModFile>) -> Mod {
        Mod {
            name: "m".to_string(),
            files,
            ..Default::default()
        }
    }

    fn file(target: &str, kind: FileKind) -> ModFile {
        ModFile {
            target: Some(target.to_string()),
            kind: Some(kind),
            ..Default::default()
        }
    }

    #[test]
    fn only_compiled_scenes_are_candidates() {
        let mods = vec![mod_with(vec![
            file("MODELS/A/HANGAR.SCENE.MBIN", FileKind::Mbin),
            // a sparse AMUMSS patch of a scene is not a whole scene
            file("MODELS/A/OTHER.SCENE.MBIN", FileKind::Exml),
            file("METADATA/REWARDTABLE.MBIN", FileKind::Mbin),
        ])];
        let found = scene_assets(&mods);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].1.target.as_deref(),
            Some("MODELS/A/HANGAR.SCENE.MBIN")
        );
    }

    #[test]
    fn without_tools_the_check_is_skipped_not_failed() {
        let mods = vec![mod_with(vec![file(
            "MODELS/A/HANGAR.SCENE.MBIN",
            FileKind::Mbin,
        )])];
        assert!(detect(&mods, None, None).is_empty());
    }

    #[test]
    fn a_skipped_run_says_so_rather_than_looking_clean() {
        // The whole point: no findings plus no explanation reads as a healthy
        // library, which is the one answer the tool must never give by accident.
        let mods = vec![mod_with(vec![file(
            "MODELS/A/HANGAR.SCENE.MBIN",
            FileKind::Mbin,
        )])];
        let (findings, status) = run(&mods, None, Some("Z:\nope.exe"), Some("Z:\nope.exe"));
        assert!(findings.is_empty());
        assert!(!status.scene_check);
        assert!(status.scene_check_note.is_some());
        assert!(status.mbincompiler.is_none());
    }
}
