//! Run the manifest-free adoption against a real library.
//!
//! The case that matters: a mods folder full of hardlinks whose manager is gone,
//! or was never there. Read-only -- it surveys and prints, and writes nothing.
//!
//! ```text
//! cargo run --example adopt_probe [staging] [derived]
//! ```
use std::path::PathBuf;

use anomaly_lib::engine::{adopt, gamefind};

fn main() {
    // Overridable, so this is not welded to one machine.
    let mut args = std::env::args().skip(1);
    let staging = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("D:\\NMSMods"));
    let derived = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("D:\\nmscheck_staging\\derived"));

    let mods = gamefind::find_install()
        .map(|i| i.mods_dir)
        .expect("no No Man's Sky install found");

    println!("mods    {}", mods.display());
    println!("staging {}", staging.display());
    println!("derived {}\n", derived.display());

    let started = std::time::Instant::now();
    let survey = adopt::survey(&mods, Some(&staging), Some(&derived)).expect("survey");
    let took = started.elapsed();

    println!(
        "manager: {:?}   traced: {}   adoptable: {}   refused: {}   ours: {}   unmanaged: {}   in {:?}\n",
        survey.manager,
        survey.candidates.len(),
        survey.ready(),
        survey.refused(),
        survey.ours.len(),
        survey.unmanaged.len(),
        took
    );

    for c in survey.candidates.iter().filter(|c| c.refused.is_some()) {
        println!("REFUSED  {}  -- {}", c.owner, c.refused.clone().unwrap());
    }
    for c in survey
        .candidates
        .iter()
        .filter(|c| c.notes.iter().any(|n| n.contains("strong guess")))
    {
        println!("GUESSED  {}", c.owner);
    }
    for name in &survey.ours {
        println!("OURS     {name}");
    }
    for name in &survey.unmanaged {
        println!("UNMANAGED  {name}");
    }
}
