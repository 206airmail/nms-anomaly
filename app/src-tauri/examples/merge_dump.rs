//! Merge every mergeable conflict in a library and verify the result.
//!
//!     cargo run --example merge_dump -- MODS_DIR OUT_DIR [GAME_ROOT]
//!
//! Writes one `.MBIN` per merged asset into OUT_DIR, mirroring the in-game
//! path so the folder can be installed as a mod.

use std::path::PathBuf;

use anomaly_lib::engine::decompile::{self, Decompiler};
use anomaly_lib::engine::vanilla::VanillaSource;
use anomaly_lib::engine::{analyze, analyze::WinnerRule, discovery, gamefind, merge, model::FileKind};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(root), Some(out)) = (args.next(), args.next()) else {
        eprintln!("usage: merge_dump MODS_DIR OUT_DIR [GAME_ROOT]");
        std::process::exit(2);
    };
    let root = PathBuf::from(root);
    let out = PathBuf::from(out);
    let game_root = args
        .next()
        .map(PathBuf::from)
        .or_else(|| gamefind::find_install().map(|i| PathBuf::from(i.root)))
        .expect("no game install found");

    let (mods, stats, host) = discovery::scan_roots(&[root.clone()], true).expect("scan failed");
    let mut active: Vec<_> = mods.into_iter().filter(|m| !m.disabled).collect();

    let mut decompiler = Decompiler::locate(None, None).expect("MBINCompiler not found");
    let mut source = VanillaSource::locate(&game_root, None).expect("hgpaktool not found");
    decompile::enrich(&mut active, &mut decompiler);

    let mut report = analyze::analyse(
        active.clone(),
        stats,
        vec![root.to_string_lossy().into_owned()],
        None,
        WinnerRule::Last,
        false,
        Some(&host),
    );
    merge::run(&mut report.conflicts, &active, &mut decompiler, &mut source);

    let mergeable: Vec<_> = report
        .conflicts
        .iter()
        .filter(|c| c.mergeable == Some(true) && !c.benign)
        .collect();
    println!("{} mergeable conflict(s)", mergeable.len());

    let targets: Vec<String> = mergeable.iter().map(|c| c.target.clone()).collect();
    let extraction = source.fetch(&targets);

    for conflict in mergeable {
        let Some(vanilla_mbin) = extraction.found.get(&conflict.target) else {
            continue;
        };
        let Some(vanilla_xml) = decompiler
            .decompile_file(vanilla_mbin)
            .and_then(|p| std::fs::read_to_string(p).ok())
        else {
            continue;
        };

        let mut copies = Vec::new();
        for name in &conflict.mods {
            let Some(owner) = active.iter().find(|m| &m.name == name) else {
                continue;
            };
            let Some(file) = owner
                .files
                .iter()
                .find(|f| f.target.as_deref() == Some(conflict.target.as_str()))
            else {
                continue;
            };
            let whole = file.kind == Some(FileKind::Mbin);
            let xml = if whole {
                decompiler
                    .decompile(std::path::Path::new(&file.abs_path), &file.sha1)
                    .and_then(|p| std::fs::read_to_string(p).ok())
            } else {
                std::fs::read_to_string(&file.abs_path).ok()
            };
            if let Some(xml) = xml {
                copies.push((name.clone(), xml, whole));
            }
        }

        match merge::build(&conflict.target, &vanilla_xml, &copies) {
            Ok(merged) if !merged.complete() => {
                println!("  {}
    REFUSED: {}", conflict.target, merged.shortfall().unwrap());
                println!("    applied {:?}  grafted {}", merged.applied, merged.grafted);
                for path in merged.skipped.iter().take(3) {
                    println!("      e.g. {path}");
                }
            }
            Ok(merged) => {
                let rel: PathBuf = conflict.target.split('/').collect();
                let dir = out.join(rel.parent().unwrap_or(std::path::Path::new("")));
                let stem = rel
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .trim_end_matches(".MBIN")
                    .to_string();
                match decompiler.compile(&merged.xml, &dir, &stem) {
                    Ok(path) => println!(
                        "  {}\n    host {}  grafted {}  applied {:?}  skipped {}\n    -> {}",
                        conflict.target,
                        merged.host,
                        merged.grafted,
                        merged.applied,
                        merged.skipped.len(),
                        path.display()
                    ),
                    Err(err) => println!("  {}: compile failed: {err}", conflict.target),
                }
            }
            Err(err) => println!("  {}: {err}", conflict.target),
        }
    }
}
