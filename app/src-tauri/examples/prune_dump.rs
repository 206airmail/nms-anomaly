//! Rewrite every whole-file replacement in a library as a sparse patch.
//!
//!     cargo run --release --example prune_dump -- MODS_DIR [OUT_DIR]
//!
//! With no OUT_DIR nothing is written; it just reports what pruning would do.

use std::path::PathBuf;

use anomaly_lib::engine::decompile::Decompiler;
use anomaly_lib::engine::vanilla::VanillaSource;
use anomaly_lib::engine::{discovery, gamefind, model::FileKind, prune};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else {
        eprintln!("usage: prune_dump MODS_DIR [OUT_DIR]");
        std::process::exit(2);
    };
    let out = args.next().map(PathBuf::from);
    let game_root = gamefind::find_install()
        .map(|i| PathBuf::from(i.root))
        .expect("no game install found");

    let (mods, _stats, _host) =
        discovery::scan_roots(&[PathBuf::from(root)], true).expect("scan failed");
    let active: Vec<_> = mods.into_iter().filter(|m| !m.disabled).collect();

    let mut decompiler = Decompiler::locate(None, None).expect("MBINCompiler not found");
    let mut source = VanillaSource::locate(&game_root, None).expect("hgpaktool not found");

    // Whole-file replacements only: a sparse patch is already the good shape.
    let mut subjects: Vec<(String, String, String, String)> = Vec::new();
    for owner in &active {
        for file in owner.files.iter().filter(|f| f.kind == Some(FileKind::Mbin)) {
            if let Some(target) = &file.target {
                subjects.push((
                    owner.name.clone(),
                    target.clone(),
                    file.abs_path.clone(),
                    file.sha1.clone(),
                ));
            }
        }
    }
    let targets: Vec<String> = subjects.iter().map(|(_, t, _, _)| t.clone()).collect();
    let extraction = source.fetch(&targets);

    println!("{:<30} {:>9} {:>7} {:>8}  asset", "mod", "original", "edits", "cut");
    let mut total_before = 0usize;
    let mut total_after = 0usize;

    for (owner, target, path, sha1) in &subjects {
        let Some(vanilla_mbin) = extraction.found.get(target) else {
            continue; // an asset the game does not ship; nothing to prune against
        };
        let Some(vanilla_xml) = decompiler
            .decompile_file(vanilla_mbin)
            .and_then(|p| std::fs::read_to_string(p).ok())
        else {
            continue;
        };
        let Some(mod_xml) = decompiler
            .decompile(std::path::Path::new(path), sha1)
            .and_then(|p| std::fs::read_to_string(p).ok())
        else {
            continue;
        };

        // This example only walks compiled overrides, so the input is a whole file.
        match prune::prune(target, &vanilla_xml, &mod_xml, true) {
            Ok(p) => {
                total_before += p.original;
                total_after += p.edits;
                println!(
                    "{:<30} {:>9} {:>7} {:>7.1}%  {}",
                    owner.chars().take(30).collect::<String>(),
                    p.original,
                    p.edits,
                    p.reduction(),
                    target.rsplit('/').next().unwrap_or(target),
                );
                if let Some(dir) = &out {
                    if p.empty() {
                        continue;
                    }
                    let rel: PathBuf = target.split('/').collect();
                    let into = dir
                        .join(owner)
                        .join(rel.parent().unwrap_or(std::path::Path::new("")));
                    let stem = rel
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .trim_end_matches(".MBIN")
                        .to_string();
                    std::fs::create_dir_all(&into).ok();
                    std::fs::write(into.join(format!("{stem}.EXML")), &p.xml).ok();
                }
            }
            Err(err) => println!("{:<30} {}: {err}", owner, target),
        }
    }
    println!(
        "\ntotal: {total_before} properties shipped -> {total_after} actually changed"
    );
}
