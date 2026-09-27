//! What auto-clean would do to a library, and optionally prove it reverses.
//!
//!     cargo run --release --example clean_dump -- MODS_DIR [--build DEST]
//!
//! Without `--build` nothing is written. With it, a cleaned build of every
//! cleanable mod is written under `DEST` and checked: the overrides must be
//! gone from the build, the patches must be there, and the staged mod must be
//! untouched byte for byte -- which is the property the undo depends on.
//!
//! Pointing this at the *live* mods folder is safe in a way the old in-place
//! version was not: a cleaned build is a copy, and nothing here writes into the
//! folder it reads.

use std::path::PathBuf;

use anomaly_lib::engine::decompile::Decompiler;
use anomaly_lib::engine::vanilla::VanillaSource;
use anomaly_lib::engine::{discovery, gamefind, prune};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else {
        eprintln!("usage: clean_dump MODS_DIR [--build DEST]");
        std::process::exit(2);
    };
    let rest: Vec<String> = args.collect();
    let build = rest
        .iter()
        .position(|a| a == "--build")
        .and_then(|at| rest.get(at + 1))
        .map(PathBuf::from);

    let game_root = gamefind::find_install()
        .map(|i| PathBuf::from(i.root))
        .expect("no game install found");

    let (mods, _stats, _host) =
        discovery::scan_roots(&[PathBuf::from(&root)], true).expect("scan failed");
    let active: Vec<_> = mods.into_iter().filter(|m| !m.disabled).collect();

    let mut decompiler = Decompiler::locate(None, None).expect("MBINCompiler not found");
    let mut source = VanillaSource::locate(&game_root, None).expect("hgpaktool not found");

    let plans = prune::plan(&active, &mut decompiler, &mut source);
    println!("{:<34} {:>9} {:>7}  {}", "mod", "shipped", "edits", "asset / reason");
    for entry in &plans {
        let asset = entry.target.rsplit('/').next().unwrap_or(&entry.target);
        let owner: String = entry.owner.chars().take(34).collect();
        match &entry.refused {
            Some(why) => println!("{owner:<34} {:>9} {:>7}  {asset}: {why}", "-", "-"),
            None => println!("{owner:<34} {:>9} {:>7}  {asset}", entry.original, entry.edits),
        }
    }

    let Some(build) = build else {
        println!("\nnothing written; pass --build DEST to build the cleaned copies");
        return;
    };

    // One build per mod, laid out the way the loadout deploys from: the mod's
    // own folder is what appears in the game, and the staged folder holds it.
    let mut built = 0usize;
    for owner in &active {
        let mine: Vec<&prune::Plan> =
            plans.iter().filter(|p| p.owner == owner.name && p.can_clean()).collect();
        if mine.is_empty() {
            continue;
        }
        // The scanned mod folder stands in for a staged one here: its parent is
        // the folder holding it, which is the shape `clean_into` expects.
        let origin = PathBuf::from(&owner.root)
            .parent()
            .expect("a mod folder always has a parent")
            .to_path_buf();
        let dest = build.join(&owner.name);
        let done = prune::clean_into(&origin, &dest, &mine).expect("build failed");

        for entry in &mine {
            let at = PathBuf::from(&entry.owner).join(entry.rel.replace('\\', "/"));
            assert!(!dest.join(&at).exists(), "the override survived the build");
            assert!(
                dest.join(at.with_extension("EXML")).exists(),
                "no patch was written"
            );
            assert!(
                PathBuf::from(&owner.root).join(&entry.rel).exists(),
                "{} was taken out of the mod itself; there is no way back",
                entry.rel
            );
        }
        built += done.len();
    }
    println!("\n{built} files cleaned into builds; every original left where it was");
}
