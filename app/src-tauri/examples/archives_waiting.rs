//! What the archives folder holds that is not installed yet.
//!
//!     cargo run --example archives_waiting -- ARCHIVES_DIR [LOADOUT]
//!
//! With no loadout, everything is new: what a fresh install handed only the
//! archives would be offered.

use std::path::Path;

use anomaly_lib::engine::{archivescan, loadout};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(dir) = args.next() else {
        eprintln!("usage: archives_waiting ARCHIVES_DIR [LOADOUT]");
        std::process::exit(2);
    };
    let book = args
        .next()
        .map(|p| loadout::Loadout::read(Path::new(&p)))
        .unwrap_or_default();
    let found = archivescan::survey(Path::new(&dir), &book);
    for w in &found {
        println!(
            "{:<8} {:<10} {}{}",
            w.mod_id.map(|i| i.to_string()).unwrap_or_else(|| "-".into()),
            w.version.as_deref().unwrap_or("-"),
            w.file,
            w.replaces.as_deref().map(|r| format!("   (replaces {r})")).unwrap_or_default()
        );
    }
    println!("\n{} waiting", found.len());
}
