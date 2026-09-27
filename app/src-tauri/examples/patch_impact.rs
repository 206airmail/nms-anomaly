//! What the last game update changed underneath the mods.
//!
//!     cargo run --example patch_impact                 # the real comparison
//!     cargo run --example patch_impact -- --demo       # prove it without one
//!
//! Plain, the survey needs two cached game builds, so on a machine that has not
//! seen an update yet it reports that and stops -- which is the honest answer,
//! not a clean bill of health.
//!
//! `--demo` earns its keep because of that. It fabricates the missing build:
//! real vanilla assets out of the live cache, decompiled, a handful of values
//! altered, compiled back to `.MBIN` with the same MBINCompiler the app uses,
//! and laid down in a temporary cache as the *older* build. The survey then runs
//! against the real library with the real toolchain, and the findings it prints
//! are the ones a genuine update would produce. Nothing is written to the live
//! cache -- see `vanilla::CACHE_OVERRIDE` -- so a fake build cannot survive the
//! run and be reported as a real patch later.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anomaly_lib::engine::{
    decompile::Decompiler,
    gamefind,
    model::{FileKind, Mod},
    patchdiff, scancache, vanilla,
};

fn main() {
    let demo = std::env::args().any(|a| a == "--demo");

    let Some(install) = gamefind::find_install() else {
        eprintln!("no No Man's Sky install found");
        std::process::exit(2);
    };
    let scan = match scancache::refresh(Path::new(&install.mods_dir)) {
        Ok(scan) => scan,
        Err(err) => {
            eprintln!("could not read {}: {err}", install.mods_dir.display());
            std::process::exit(2);
        }
    };
    let Some(mut decompiler) = Decompiler::locate(None, None) else {
        eprintln!("MBINCompiler was not found; drop it in tools/");
        std::process::exit(2);
    };

    let temp = if demo {
        match stage_demo(&scan.active(), &mut decompiler) {
            Ok(dir) => {
                println!("(demo: fabricated an older build in {})\n", dir.display());
                Some(dir)
            }
            Err(err) => {
                eprintln!("could not stage the demo: {err}");
                std::process::exit(2);
            }
        }
    } else {
        None
    };

    let impact = patchdiff::latest(&scan.active(), &mut decompiler);
    report(&impact);

    // The whole point of the override: the fake build goes away.
    if let Some(dir) = temp {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

fn report(impact: &patchdiff::Impact) {
    println!("=== builds ===");
    for (label, build) in [("from", &impact.from), ("to", &impact.to)] {
        match build {
            Some(b) => println!(
                "  {label:<5}{}  {} paks, first seen {}{}",
                &b.key[..b.key.len().min(16)],
                b.paks,
                anomaly_lib::engine::clock::local(b.first_seen_ms).written(),
                if b.dated { "" } else { "  (dated by folder mtime)" }
            ),
            None => println!("  {label:<5}(none)"),
        }
    }
    println!("\ncompared    : {} assets", impact.compared);
    println!("patch moved : {} of them", impact.touched);
    println!("uncomparable: {}", impact.uncomparable.len());
    println!("\n{}\n", impact.verdict);

    for file in &impact.files {
        println!(
            "[{}] {} :: {}",
            file.severity, file.owner, file.rel_path
        );
        println!("    {}", file.summary);
        let show = |label: &str, rows: &[patchdiff::Moved]| {
            for m in rows.iter().take(6) {
                println!(
                    "      {label:<10} {}\n                 game {:?} -> {:?}, mod says {:?}",
                    m.path, m.before, m.after, m.mod_value
                );
            }
            if rows.len() > 6 {
                println!("      ... and {} more", rows.len() - 6);
            }
        };
        show("REVERTS", &file.judged.reverts);
        show("DROPS", &file.judged.drops);
        show("DEAD", &file.judged.dead);
        if !file.judged.overridden.is_empty() {
            println!(
                "      overridden {} (deliberate, listed for context)",
                file.judged.overridden.len()
            );
        }
        println!();
    }

    if !impact.unaffected.is_empty() {
        println!(
            "unaffected  : {} mods checked and clear{}",
            impact.unaffected.len(),
            if impact.unaffected.len() <= 8 {
                format!(" -- {}", impact.unaffected.join(", "))
            } else {
                String::new()
            }
        );
    }
}

/// Build a plausible "previous game build" out of the real one.
///
/// The newer build is the live cache, verbatim. The older build is the same,
/// except that for a few assets a mod replaces whole, the mod's own copy stands
/// in as what the game used to ship.
///
/// That is not an arbitrary way to fabricate a difference -- it *is* the
/// scenario. A mod ships a whole table because it was built against a game that
/// looked like that table. Pretending the older build looked like the mod
/// therefore produces exactly the finding a real update produces: every property
/// the mod still carries at the old value is one the patch changed and the mod
/// takes back, which is the `SalvageRights` shape. Nudging a random float, which
/// this did first, proved the plumbing and found nothing, because the values it
/// happened to move were ones no mod sets.
///
/// One artefact to read past: nodes the mod *adds* show up as `DEAD`. That is
/// the fabrication, not a misclassification -- the mod is standing in for the old
/// game here, so anything it invented is present in the older build and absent
/// from the newer one, which is the definition of a property the patch removed.
/// On a real pair of builds a mod's own additions appear in neither, and
/// `judge` returns early on them.
fn stage_demo(mods: &[Mod], decompiler: &mut Decompiler) -> Result<PathBuf, String> {
    let live = vanilla::builds();
    let newest = live
        .last()
        .ok_or("nothing in the vanilla cache yet; run a scan first")?;
    let real_home = vanilla::cache_home();
    let source = real_home.join(&newest.key);

    let temp =
        std::env::temp_dir().join(format!("nmscheck-patchdiff-demo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&temp);
    let (old_dir, new_dir) = (temp.join("bbb_older"), temp.join("ccc_newer"));

    let mut assets = Vec::new();
    collect(&source, &source, &mut assets);
    assets.sort();
    if assets.is_empty() {
        return Err(format!("no cached assets under {}", source.display()));
    }

    // Which cached assets a mod ships a compiled whole-file copy of. Those are
    // the ones that can stand in for an older game, because they are complete
    // tables in the game's own format.
    let mut stand_ins: BTreeMap<String, (String, PathBuf)> = BTreeMap::new();
    for owner in mods {
        for file in owner.assets() {
            let Some(target) = file.target.as_deref() else {
                continue;
            };
            if file.kind != Some(FileKind::Mbin) {
                continue;
            }
            let cached = target.split('/').fold(source.clone(), |at, p| at.join(p));
            if cached.is_file() {
                stand_ins.insert(
                    target.to_string(),
                    (owner.name.clone(), PathBuf::from(&file.abs_path)),
                );
            }
        }
    }
    if stand_ins.is_empty() {
        return Err(
            "no mod in this library ships a compiled whole-file copy of a cached asset, so \
             there is nothing to stand in for an older game"
                .into(),
        );
    }

    let chosen: Vec<(String, (String, PathBuf))> =
        stand_ins.into_iter().take(3).collect();

    for rel in &assets {
        copy_into(&source.join(rel), &new_dir.join(rel))?;
    }

    let picked: BTreeMap<PathBuf, &(String, PathBuf)> = chosen
        .iter()
        .map(|(target, who)| {
            (
                target.split('/').fold(PathBuf::new(), |at, p| at.join(p)),
                who,
            )
        })
        .collect();

    for rel in &assets {
        match picked.get(rel) {
            Some((owner, shipped)) => {
                println!(
                    "  older build: {} stands in for {}",
                    owner,
                    rel.display()
                );
                copy_into(shipped, &old_dir.join(rel))?;
            }
            None => copy_into(&source.join(rel), &old_dir.join(rel))?,
        }
    }

    // `decompiler` is threaded through so the demo fails loudly here rather than
    // inside the survey if MBINCompiler cannot read a mod's copy at all.
    for (_, (owner, shipped)) in &chosen {
        if decompiler.decompile_file(shipped).is_none() {
            return Err(format!(
                "MBINCompiler could not read {}'s copy at {}",
                owner,
                shipped.display()
            ));
        }
    }

    for (dir, key, when) in [
        (&old_dir, "bbb_older", 1_700_000_000_000i64),
        (&new_dir, "ccc_newer", 1_759_000_000_000i64),
    ] {
        let stamp = serde_json::json!({
            "key": key,
            "first_seen_ms": when,
            "paks": 0,
            "bytes": 0,
            "dated": true,
        });
        std::fs::write(
            dir.join(vanilla::STAMP_FILE),
            serde_json::to_string_pretty(&stamp).unwrap(),
        )
        .map_err(|e| e.to_string())?;
    }

    // Set for the rest of the process, so `cache_home` finds the fake pair and
    // the real cache is never written to.
    std::env::set_var(vanilla::CACHE_OVERRIDE, &temp);
    Ok(temp)
}
fn collect(root: &Path, dir: &Path, into: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() {
            if !name.starts_with('_') {
                collect(root, &path, into);
            }
        } else if name.to_uppercase().ends_with(".MBIN") {
            if let Ok(rel) = path.strip_prefix(root) {
                into.push(rel.to_path_buf());
            }
        }
    }
}

fn copy_into(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::copy(from, to).map_err(|e| e.to_string())?;
    Ok(())
}
