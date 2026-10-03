//! A library handed over as nothing but its archives, installed from scratch.
//!
//!     cargo run --release --example handoff_sandbox -- ARCHIVES_DIR SANDBOX_DIR
//!
//! Copies every archive into SANDBOX_DIR\archives, and installs what the
//! archives folder offers into SANDBOX_DIR\{staging,derived,MODS} with an empty
//! loadout -- the state of someone who has just installed Anomaly and been
//! given the zips. Never touches the real game. Then checks that nothing is
//! still waiting and that every mod is recorded with its Nexus archive.

use std::path::{Path, PathBuf};

use anomaly_lib::engine::{archivescan, loadout, nexusname, pipeline};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [from, sandbox] = &args[..] else {
        eprintln!("usage: handoff_sandbox ARCHIVES_DIR SANDBOX_DIR");
        std::process::exit(2);
    };
    let sandbox = PathBuf::from(sandbox);
    if sandbox.exists() {
        eprintln!("{} exists; give a fresh folder", sandbox.display());
        std::process::exit(2);
    }
    let places = pipeline::Places {
        archives: sandbox.join("archives"),
        staging: sandbox.join("staging"),
        derived: sandbox.join("derived"),
        mods_dir: sandbox.join("GAMEDATA").join("MODS"),
    };
    for dir in [&places.archives, &places.staging, &places.derived, &places.mods_dir] {
        std::fs::create_dir_all(dir).unwrap();
    }
    for e in std::fs::read_dir(from).unwrap().flatten() {
        if e.path().is_file() {
            std::fs::copy(e.path(), places.archives.join(e.file_name())).unwrap();
        }
    }
    let book_path = sandbox.join("loadout.json");

    let waiting = archivescan::survey(&places.archives, &loadout::Loadout::default());
    println!("waiting before: {}", waiting.len());
    let mut failed = Vec::new();
    for w in &waiting {
        if let Err(e) =
            pipeline::install_from_file(Path::new(&w.archive), &places, &book_path, w.replaces.is_some())
        {
            failed.push(format!("{}: {e}", w.file));
        }
    }
    let book = loadout::Loadout::read(&book_path);
    let after = archivescan::survey(&places.archives, &book);
    let linked = book
        .entries
        .iter()
        .filter(|e| {
            e.archive
                .as_deref()
                .and_then(|a| Path::new(a).file_name())
                .and_then(|f| nexusname::parse(&f.to_string_lossy()))
                .is_some()
        })
        .count();
    let deployed = std::fs::read_dir(&places.mods_dir).map(|d| d.count()).unwrap_or(0);
    println!("installed: {} entries, {deployed} folders in MODS", book.entries.len());
    println!("linked to a Nexus page: {linked}");
    println!("waiting after: {}", after.len());
    for f in &failed {
        println!("FAILED {f}");
    }
}
