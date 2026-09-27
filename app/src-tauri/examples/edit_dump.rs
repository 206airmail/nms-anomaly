//! What the value editor would offer for one mod, and whether writing it works.
//!
//!     cargo run --release --example edit_dump -- STAGED_MOD_DIR [PATH=VALUE ...]
//!
//! `STAGED_MOD_DIR` is the mod as its author shipped it — a folder in the
//! staging root, holding the folder(s) it puts in the game. With no assignments
//! it reports every property the mod changes, with the game's own value beside
//! it, which is the read the editor opens with.
//!
//! Given `PATH=VALUE` it also copies the mod to a scratch folder, applies the
//! values there and reports what was written. Nothing in the library, staging
//! or the game folder is touched either way, which is what makes this safe to
//! run against a real install.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anomaly_lib::engine::decompile::Decompiler;
use anomaly_lib::engine::edit::{self, Change, ModEdits};
use anomaly_lib::engine::gamefind;
use anomaly_lib::engine::vanilla::VanillaSource;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else {
        eprintln!("usage: edit_dump STAGED_MOD_DIR [PATH=VALUE ...]");
        std::process::exit(2);
    };
    let root = PathBuf::from(root);
    let wanted: Vec<String> = args.collect();

    // The folders this mod puts in the game: the top level of the staged copy.
    let members: Vec<String> = std::fs::read_dir(&root)
        .unwrap_or_else(|err| {
            eprintln!("could not read {}: {err}", root.display());
            std::process::exit(1);
        })
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    println!("mod:     {}", root.display());
    println!("folders: {}\n", members.join(", "));

    let install = gamefind::find_install().unwrap_or_else(|| {
        eprintln!("no No Man's Sky install found");
        std::process::exit(1);
    });
    let mut decompiler = Decompiler::locate(None, None).unwrap_or_else(|| {
        eprintln!("MBINCompiler was not found");
        std::process::exit(1);
    });
    let mut source = VanillaSource::locate(&PathBuf::from(&install.root), None)
        .unwrap_or_else(|| {
            eprintln!("hgpaktool was not found, or the game has no PCBANKS folder");
            std::process::exit(1);
        });

    let owner = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let started = std::time::Instant::now();
    let survey = edit::survey(&owner, &root, &members, &mut decompiler, &mut source, None);
    println!("read in {:?}\n", started.elapsed());

    let mut total = 0usize;
    for asset in &survey.assets {
        if let Some(why) = &asset.refused {
            println!("  {}  REFUSED: {why}", asset.rel);
            continue;
        }
        total += asset.fields.len();
        println!(
            "  {}  {} changed{}{}",
            asset.rel,
            asset.fields.len(),
            if asset.whole_file { "  [whole file]" } else { "" },
            if asset.in_game { "" } else { "  [not in the game]" },
        );
        // A handful, not all of them: a scene override runs to five figures.
        for field in asset.fields.iter().take(8) {
            println!(
                "      {:<62} game {:<16} mod {}",
                field.path,
                field.vanilla.as_deref().unwrap_or("—"),
                field.author,
            );
        }
        if asset.fields.len() > 8 {
            println!("      … {} more", asset.fields.len() - 8);
        }
    }
    println!("\n{total} properties this mod changes");

    if wanted.is_empty() {
        return;
    }

    // --- and now write them, into a scratch copy -----------------------------
    let dest = std::env::temp_dir().join(format!("anomaly_edit_dump/{owner}"));
    let _ = std::fs::remove_dir_all(&dest);
    edit::copy_tree(&root, &dest).unwrap_or_else(|err| {
        eprintln!("{err}");
        std::process::exit(1);
    });

    let mut edits: ModEdits = BTreeMap::new();
    for pair in &wanted {
        let Some((path, value)) = pair.split_once('=') else {
            eprintln!("expected PATH=VALUE, got {pair}");
            std::process::exit(2);
        };
        // Which asset holds it, and what the author has there, both from the
        // survey: an assignment naming a property no asset has is a mistake
        // worth catching here rather than as a `lost` line later.
        let found = survey.assets.iter().find_map(|asset| {
            asset
                .fields
                .iter()
                .find(|f| f.path == path)
                .map(|f| (asset.file.clone(), f.author.clone()))
        });
        let Some((file, was)) = found else {
            eprintln!("no asset in this mod changes {path}");
            std::process::exit(1);
        };
        edits
            .entry(file)
            .or_default()
            .insert(path.to_string(), Change { value: value.to_string(), was });
    }

    println!("\nwriting into {}", dest.display());
    match edit::apply_into(&dest, &edits, &mut decompiler) {
        Ok(applied) => {
            for line in &applied.done {
                println!("  {line}");
            }
            for line in &applied.lost {
                println!("  LOST: {line}");
            }
        }
        Err(why) => {
            eprintln!("  failed: {why}");
            std::process::exit(1);
        }
    }

    // Read the copy back through the same survey, so the check is "does the
    // editor now see the value it just wrote" rather than "did a write return
    // Ok". A whole-file asset went out through MBINCompiler and came back in
    // through it, which is the part most worth confirming.
    let after = edit::survey(&owner, &dest, &members, &mut decompiler, &mut source, None);
    println!("\nreading the written copy back:");
    for pair in &wanted {
        let (path, value) = pair.split_once('=').unwrap_or((pair.as_str(), ""));
        let now = after
            .assets
            .iter()
            .flat_map(|a| &a.fields)
            .find(|f| f.path == path)
            .map(|f| f.author.clone());
        match now {
            Some(now) => println!(
                "  {}  {path} = {now}",
                if now.trim() == value.trim() { "OK  " } else { "WRONG" },
            ),
            None => println!("  GONE  {path} is no longer a change this mod makes"),
        }
    }
}
