//! Dry-run putting this machine's own-build mods back on the author's.
//!
//! Read-only: `restore` is called with `dry_run: true`, so nothing in the game
//! folder moves and no mod list is written.
use std::path::PathBuf;

use anomaly_lib::engine::{adopt, gamefind};

fn main() {
    let mut args = std::env::args().skip(1);
    let staging = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"D:\NMSMods"));
    let derived = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"D:\NMSAnomaly\derived"));
    let mods = gamefind::find_install()
        .map(|i| i.mods_dir)
        .expect("no No Man's Sky install found");

    let survey = adopt::survey(&mods, Some(&staging), Some(&derived)).expect("survey");
    let plan = adopt::restorable(Some(&staging), &mods, &survey.ours);

    println!("on a build of ours: {}\n", survey.ours.len());
    for item in &plan {
        match (&item.source, &item.why_not) {
            (Some(source), _) => println!("  CAN  {}\n         <- {source}", item.owner),
            (None, Some(why)) => println!("  NO   {}\n         {why}", item.owner),
            _ => {}
        }
    }

    let book = std::env::temp_dir().join("restore_probe_loadout.json");
    let _ = std::fs::remove_file(&book);
    let done = adopt::restore(&plan, &staging, &derived, &mods, &book, true).expect("dry run");
    println!(
        "\nwould restore {} | would skip {} | problems {}",
        done.restored.len(),
        done.skipped.len(),
        done.problems.len()
    );
    for p in &done.problems {
        println!("  PROBLEM {p}");
    }
    assert!(!book.exists(), "a dry run must not write the mod list");
    println!("(dry run: nothing was changed)");
}
