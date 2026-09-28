//! Actually install an archive, but into throwaway folders.
//!
//! `cargo run --example install_run_probe <archive>`
//! Nothing real is touched: staging, derived, archives and the "mods folder"
//! are all made under the temp directory and removed at the end.
use std::path::PathBuf;

use anomaly_lib::engine::pipeline::{self, Places};

fn main() {
    let Some(zip) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("usage: install_run_probe <archive>");
        return;
    };
    let root = std::env::temp_dir().join("anomaly_install_probe");
    let _ = std::fs::remove_dir_all(&root);
    let places = Places {
        archives: root.join("archives"),
        staging: root.join("staging"),
        derived: root.join("derived"),
        mods_dir: root.join("MODS"),
    };
    std::fs::create_dir_all(&places.mods_dir).unwrap();
    let book = root.join("loadout.json");

    println!("archive {}", zip.display());
    println!("sandbox {}\n", root.display());

    match pipeline::install_from_file(&zip, &places, &book, false) {
        Ok(done) => {
            println!("OK {done:#?}");
            let mut found = Vec::new();
            walk(&places.mods_dir, &places.mods_dir, &mut found);
            println!("\nin the mods folder ({}):", found.len());
            for f in found {
                println!("    {f}");
            }
        }
        Err(e) => println!("FAILED: {e}"),
    }
    let _ = std::fs::remove_dir_all(&root);
}

fn walk(root: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, base, out);
        } else {
            out.push(
                path.strip_prefix(base)
                    .unwrap_or(&path)
                    .display()
                    .to_string(),
            );
        }
    }
}
