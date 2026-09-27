//! The clean preview as JSON, exactly as the `clean_preview` command returns it.
//!
//!     cargo run --release --example clean_json -- MODS_DIR [GAME_ROOT]
//!
//! Writes nothing. It exists so the front end's ranking can be checked against a
//! real library without a running app: `buildActions` takes the report and these
//! plans, and which of them it decides is important is the whole question.
//!
//! The already-cleaned entries the command adds are not here: those are read off
//! the loadout, which belongs to a running app rather than to the engine.

use std::path::PathBuf;

use anomaly_lib::engine::decompile::Decompiler;
use anomaly_lib::engine::vanilla::VanillaSource;
use anomaly_lib::engine::{decompile, discovery, prune};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else {
        eprintln!("usage: clean_json MODS_DIR [GAME_ROOT]");
        std::process::exit(2);
    };
    let game_root = args
        .next()
        .map(PathBuf::from)
        .or_else(|| anomaly_lib::engine::gamefind::find_install().map(|i| i.root))
        .expect("no game install found");

    let (mods, _stats, _host) =
        discovery::scan_roots(&[PathBuf::from(&root)], true).expect("scan failed");
    let mut decompiler = Decompiler::locate(None, None).expect("MBINCompiler not found");
    let mut source = VanillaSource::locate(&game_root, None).expect("hgpaktool not found");

    let mut enriched = mods;
    decompile::enrich(&mut enriched, &mut decompiler);

    let plans = prune::plan(&enriched, &mut decompiler, &mut source);
    println!("{}", serde_json::to_string_pretty(&plans).unwrap());
}
