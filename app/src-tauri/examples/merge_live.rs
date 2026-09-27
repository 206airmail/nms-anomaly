//! Merge one conflict for real, in a sandbox, and check the whole life of it.
//!
//!     cargo run --release --example merge_live -- MODS_DIR SANDBOX TARGET [GAME_ROOT]
//!
//! `MODS_DIR` is only ever **read**: the mods that contest `TARGET` are copied
//! into `SANDBOX/MODS` and everything happens there, so this can be pointed at a
//! real library without changing it.
//!
//! What it checks, in order, is the whole reason a merge has a loadout entry at
//! all:
//!
//! 1. merging puts the merge in the game and holds its inputs out of it;
//! 2. deactivating the merge hands the inputs straight back;
//! 3. reactivating takes them away again;
//! 4. deleting the merge removes only the build we made -- never an input --
//!    and the inputs come back for good.

use std::path::{Path, PathBuf};

use anomaly_lib::engine::analyze::WinnerRule;
use anomaly_lib::engine::{
    analyze, decompile::Decompiler, discovery, erase, loadout, merge, pipeline, vanilla::VanillaSource,
};

fn copy_tree(from: &Path, to: &Path) {
    if from.is_dir() {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            copy_tree(&entry.path(), &to.join(entry.file_name()));
        }
    } else if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).unwrap();
        std::fs::copy(from, to).unwrap();
    }
}

fn names_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    out.sort();
    out
}

fn check(what: &str, ok: bool) {
    println!("   {} {what}", if ok { "ok  " } else { "FAIL" });
    if !ok {
        std::process::exit(1);
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(real_mods), Some(sandbox), Some(target)) = (args.next(), args.next(), args.next())
    else {
        eprintln!("usage: merge_live MODS_DIR SANDBOX TARGET [GAME_ROOT]");
        std::process::exit(2);
    };
    let game_root = args
        .next()
        .map(PathBuf::from)
        .or_else(|| anomaly_lib::engine::gamefind::find_install().map(|i| i.root))
        .expect("no game install found");

    let sandbox = PathBuf::from(&sandbox);
    let _ = std::fs::remove_dir_all(&sandbox);
    let places = pipeline::Places {
        archives: sandbox.join("archives"),
        staging: sandbox.join("staging"),
        derived: sandbox.join("derived"),
        mods_dir: sandbox.join("MODS"),
    };
    std::fs::create_dir_all(&places.mods_dir).unwrap();
    let book_path = sandbox.join("loadout.json");

    // --- work out who contests the target, reading the real library ---------
    let real = PathBuf::from(&real_mods);
    let (mods, stats, host) = discovery::scan_roots(&[real.clone()], true).expect("scan failed");
    let mut decompiler = Decompiler::locate(None, None).expect("MBINCompiler not found");
    let mut source = VanillaSource::locate(&game_root, None).expect("hgpaktool not found");

    let mut enriched = mods.clone();
    anomaly_lib::engine::decompile::enrich(&mut enriched, &mut decompiler);
    let built = analyze::analyse(
        enriched,
        stats,
        vec![real.to_string_lossy().into_owned()],
        None,
        WinnerRule::Last,
        false,
        Some(&host),
    );
    let contenders: Vec<String> = built
        .conflicts
        .iter()
        .find(|c| c.target == target)
        .unwrap_or_else(|| {
            eprintln!("no conflict on {target}");
            std::process::exit(1);
        })
        .mods
        .clone();
    println!("{target}\n   contested by {}\n", contenders.join(", "));

    // --- copy just those mods into the sandbox and stage them ---------------
    for name in &contenders {
        copy_tree(&real.join(name), &places.staging.join(name).join(name));
        copy_tree(&real.join(name), &places.mods_dir.join(name));
    }
    let mut book = loadout::Loadout::default();
    for name in &contenders {
        book.put(loadout::Entry {
            owner: name.clone(),
            source: places.staging.join(name).display().to_string(),
            origin: Some(places.staging.join(name).display().to_string()),
            archive: None,
            variant: loadout::Variant::Original,
            replaces: Vec::new(),
            deployed: vec![name.clone()],
            built_from: Some(places.staging.join(name).display().to_string()),
            enabled: true,
            edited: false,
        });
    }
    book.write(&book_path).unwrap();

    // --- merge, through exactly the code the app runs ------------------------
    let (sandbox_mods, sandbox_stats, sandbox_host) =
        discovery::scan_roots(&[places.mods_dir.clone()], true).expect("sandbox scan failed");
    let mut active = sandbox_mods.clone();
    anomaly_lib::engine::decompile::enrich(&mut active, &mut decompiler);
    let mut sandbox_built = analyze::analyse(
        active.clone(),
        sandbox_stats,
        vec![places.mods_dir.to_string_lossy().into_owned()],
        None,
        WinnerRule::Last,
        false,
        Some(&sandbox_host),
    );
    merge::run(
        &mut sandbox_built.conflicts,
        &active,
        &mut decompiler,
        &mut source,
    );
    let conflict = sandbox_built
        .conflicts
        .iter()
        .find(|c| c.target == target)
        .expect("the conflict did not survive into the sandbox");

    let done = pipeline::record_merge(
        conflict,
        &active,
        &places,
        &book_path,
        &mut decompiler,
        &mut source,
    )
    .unwrap_or_else(|err| {
        eprintln!("merge refused: {err}");
        std::process::exit(1);
    });

    println!("merged as {}", done.owner);
    println!("   in the game now: {:?}\n", names_in(&places.mods_dir));

    println!("1. merging puts the merge in and holds its inputs out");
    let now = names_in(&places.mods_dir);
    check("the merge is in the game", now.contains(&done.owner));
    for name in &contenders {
        check(&format!("{name} is held out"), !now.contains(name));
        check(
            &format!("{name}'s own files are untouched"),
            places.staging.join(name).join(name).exists(),
        );
    }

    println!("\n2. deactivating the merge hands the inputs back");
    let mut book = loadout::Loadout::read(&book_path);
    book.entries
        .iter_mut()
        .find(|e| e.owner == done.owner)
        .unwrap()
        .enabled = false;
    let changes = loadout::reconcile(&mut book, &places.mods_dir, false);
    book.write(&book_path).unwrap();
    check("no problems", changes.problems.is_empty());
    let now = names_in(&places.mods_dir);
    check("the merge is out", !now.contains(&done.owner));
    for name in &contenders {
        check(&format!("{name} is back"), now.contains(name));
    }

    println!("\n3. reactivating takes them away again");
    let mut book = loadout::Loadout::read(&book_path);
    book.entries
        .iter_mut()
        .find(|e| e.owner == done.owner)
        .unwrap()
        .enabled = true;
    let changes = loadout::reconcile(&mut book, &places.mods_dir, false);
    book.write(&book_path).unwrap();
    check("no problems", changes.problems.is_empty());
    let now = names_in(&places.mods_dir);
    check("the merge is back in", now.contains(&done.owner));
    for name in &contenders {
        check(&format!("{name} is held out again"), !now.contains(name));
    }

    println!("\n4. deleting the merge removes only our build, and they come back");
    let roots = erase::Roots::of(
        &places.staging,
        &places.derived,
        &places.archives,
        &places.mods_dir,
        None,
    );
    let mut book = loadout::Loadout::read(&book_path);
    let entry = book.get(&done.owner).cloned().unwrap();
    let plan = erase::plan(&entry, &roots, true);
    check(
        &format!("deletes exactly one thing, not the inputs ({} item(s))", plan.items.len()),
        plan.items.len() == 1,
    );
    erase::erase(&plan, &roots, &mut book).unwrap();
    let changes = loadout::reconcile(&mut book, &places.mods_dir, false);
    book.write(&book_path).unwrap();
    check("no problems", changes.problems.is_empty());

    let now = names_in(&places.mods_dir);
    check("the merge is gone from the game", !now.contains(&done.owner));
    check("the merge is gone from the list", book.get(&done.owner).is_none());
    for name in &contenders {
        check(&format!("{name} is back in the game"), now.contains(name));
        check(&format!("{name} is still installed"), book.get(name).is_some());
        check(
            &format!("{name}'s files survived"),
            places.staging.join(name).join(name).exists(),
        );
    }

    println!("\nall good -- sandbox left at {}", sandbox.display());
}
