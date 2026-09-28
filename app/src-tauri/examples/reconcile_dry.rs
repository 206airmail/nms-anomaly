//! What a reconcile would do to the real library, without doing any of it.
//!
//! `cargo run --example reconcile_dry [mods-dir] [loadout.json]`
use std::path::PathBuf;

use anomaly_lib::engine::{gamefind, loadout};

fn main() {
    let mut args = std::env::args().skip(1);
    // Detected, never assumed: the game is on whatever drive it is on, and an
    // argument wins so a second install can be checked without moving it.
    let mods = match args.next() {
        Some(given) => PathBuf::from(given),
        None => match gamefind::find_install().filter(|found| found.has_mods_dir) {
            Some(found) => PathBuf::from(found.mods_dir),
            None => {
                eprintln!("no No Man's Sky install found -- pass the mods folder as an argument");
                return;
            }
        },
    };
    let book_path = args.next().map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var("APPDATA").unwrap_or_default())
            .join("NMS Anomaly")
            .join("loadout.json")
    });

    println!("mods    {}", mods.display());
    println!("loadout {}\n", book_path.display());

    let mut book = loadout::Loadout::read(&book_path);
    let changes = loadout::reconcile(&mut book, &mods, true);

    println!("would deploy  ({}):", changes.deployed.len());
    for name in &changes.deployed {
        println!("    {name}");
    }
    println!("would remove  ({}):", changes.removed.len());
    for name in &changes.removed {
        println!("    {name}");
    }
    println!("unchanged     ({})", changes.unchanged.len());
    if !changes.problems.is_empty() {
        println!("problems:");
        for p in &changes.problems {
            println!("    {p}");
        }
    }
}
