//! What would installing each archive in a folder actually produce?
//!
//!     cargo run --release --example archive_dump -- DOWNLOADS_DIR MODS_DIR
//!
//! Read-only: it lists and plans, it never extracts.

use std::path::PathBuf;

use anomaly_lib::engine::archive;

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(dir), Some(mods)) = (args.next(), args.next()) else {
        eprintln!("usage: archive_dump DOWNLOADS_DIR MODS_DIR");
        std::process::exit(2);
    };
    let mods_dir = PathBuf::from(mods);

    match archive::find_7z() {
        Some(path) => println!("7-Zip: {}\n", path.display()),
        None => {
            eprintln!("7-Zip was not found");
            std::process::exit(1);
        }
    }

    let mut seen = 0;
    let mut failed = 0;
    for entry in std::fs::read_dir(&dir).expect("cannot read that folder").flatten() {
        let path = entry.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        if !matches!(ext.as_str(), "zip" | "rar" | "7z") {
            continue;
        }
        seen += 1;
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        match archive::preview(&path, &mods_dir) {
            Ok(plan) => {
                println!(
                    "{:<58} -> {:<42} {:>4} files{}",
                    name.chars().take(58).collect::<String>(),
                    plan.owner.chars().take(42).collect::<String>(),
                    plan.files,
                    if plan.collides { "  [already installed]" } else { "" }
                );
                for note in &plan.notes {
                    println!("{:62}   {note}", "");
                }
            }
            Err(err) => {
                failed += 1;
                println!("{:<58} -> FAILED: {err}", name.chars().take(58).collect::<String>());
            }
        }
    }
    println!("\n{seen} archives, {failed} could not be read");
}
