//! Take a folder of staged mods and give the game links to all of them.
//!
//! This is the recovery path after a manager purge: the staged mods survive,
//! the game folder is empty, and every mod has to be deployed again and
//! recorded so this program knows it owns them.
//!
//!     cargo run --release --example restore_library -- <STAGING> <MODS_DIR> <LOADOUT>
//!
//! Reports and writes nothing without `--write`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anomaly_lib::engine::deploy;
use anomaly_lib::engine::loadout::{self, Entry, Loadout, Variant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let write = args.iter().any(|a| a == "--write");
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if positional.len() < 3 {
        eprintln!(
            "usage: restore_library <STAGING> <MODS_DIR> <LOADOUT> [--write]\n\
             \n\
             Deploys every mod folder under STAGING into MODS_DIR as hardlinks\n\
             and records them in LOADOUT. Without --write, only reports."
        );
        std::process::exit(2);
    }
    let staging = PathBuf::from(positional[0]);
    let mods_dir = PathBuf::from(positional[1]);
    let book = PathBuf::from(positional[2]);

    let staged = match collect(&staging) {
        Ok(found) => found,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    if staged.is_empty() {
        eprintln!("no mods found under {}", staging.display());
        std::process::exit(1);
    }

    let mut book_data = Loadout::read(&book);
    for (owner, source) in &staged {
        book_data.put(Entry {
            owner: owner.clone(),
            source: source.display().to_string(),
            // These mods are on the build their authors shipped, which is the
            // staged folder itself.
            origin: Some(source.display().to_string()),
            // Not known from the staging tree alone: the download that produced
            // this folder may have been deleted, and inventing a path would make
            // a later delete offer to remove a file that is not there.
            archive: None,
            variant: Variant::Original,
            replaces: Vec::new(),
            deployed: Vec::new(),
            built_from: None,
            enabled: true,
            edited: false,
        });
    }

    println!("staging:  {}", staging.display());
    println!("game:     {}", mods_dir.display());
    println!("mods:     {}\n", staged.len());

    let changes = loadout::reconcile(&mut book_data, &mods_dir, !write);

    for clash in &changes.clashes {
        println!(
            "CLASH    {}\n         kept by {}, given up by {}",
            clash.rel,
            clash.keeper,
            clash.losers.join(", ")
        );
    }
    for problem in &changes.problems {
        println!("PROBLEM  {problem}");
    }
    if !changes.clashes.is_empty() || !changes.problems.is_empty() {
        println!();
    }

    println!("{} deployed", changes.deployed.len());
    if !changes.unchanged.is_empty() {
        println!("{} already correct", changes.unchanged.len());
    }
    if !changes.removed.is_empty() {
        println!("{} removed", changes.removed.len());
    }

    if write {
        match book_data.write(&book) {
            Ok(()) => println!("\nrecorded in {}", book.display()),
            Err(e) => {
                eprintln!("\ndeployed, but could not record it: {e}");
                std::process::exit(1);
            }
        }
    } else {
        println!("\n(nothing was written -- pass --write to deploy)");
    }

    if !changes.problems.is_empty() {
        std::process::exit(1);
    }
}

/// Every immediate subfolder of `staging` that holds something the game reads.
///
/// The mod's name is the folder the game will see, which is the one *inside*
/// the staged folder -- the staged folder itself is named after the archive
/// and carries a mod id and a timestamp the game has no use for.
fn collect(staging: &Path) -> Result<BTreeMap<String, PathBuf>, String> {
    let entries = std::fs::read_dir(staging)
        .map_err(|e| format!("cannot read {}: {e}", staging.display()))?;

    let mut out = BTreeMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(owner) = content_folder(&path) else {
            continue;
        };
        if let Some(earlier) = out.insert(owner.clone(), path.clone()) {
            return Err(format!(
                "two staged folders both install {owner}:\n  {}\n  {}",
                earlier.display(),
                path.display()
            ));
        }
    }
    Ok(out)
}

/// The name the game will see for this staged mod.
fn content_folder(staged: &Path) -> Option<String> {
    let mut found = None;
    for entry in std::fs::read_dir(staged).ok()?.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        collect_files(&path, &mut files);
        if !files.iter().any(|f| deploy::is_game_content(f)) {
            continue;
        }
        if found.is_some() {
            return None; // more than one content folder: not the simple shape
        }
        found = Some(entry.file_name().to_string_lossy().to_string());
    }
    found
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out);
        } else {
            out.push(path);
        }
    }
}
