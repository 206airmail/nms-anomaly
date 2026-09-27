//! Scan and analyse a mod library with the Rust engine, printing the same JSON
//! the Python `nmscheck --json` writes.
//!
//! This is the end-to-end acceptance check: `tools/compare_engines.py` diffs
//! it field by field against the Python on a real library, so a divergence in
//! any module shows up as a failing check rather than a wrong verdict in the
//! UI.
//!
//!     cargo run --example analyze_dump -- MODS_DIR

use std::path::PathBuf;

use anomaly_lib::engine::decompile::{self, Decompiler};
use anomaly_lib::engine::vanilla::VanillaSource;
use anomaly_lib::engine::{analyze, discovery, drift, gamefind, merge, report, tools};

fn main() {
    let Some(root) = std::env::args().nth(1) else {
        eprintln!("usage: analyze_dump MODS_DIR");
        std::process::exit(2);
    };
    let root = PathBuf::from(root);

    let (mods, stats, host) =
        discovery::scan_roots(&[root.clone()], true).expect("scan failed");

    // The CLI analyses only the mods the game has switched on, so match it.
    let mut active: Vec<_> = mods.iter().filter(|m| !m.disabled).cloned().collect();
    let disabled: Vec<String> = mods
        .iter()
        .filter(|m| m.disabled)
        .map(|m| m.name.clone())
        .collect();

    // --no-decompile keeps the pure-analysis comparison in compare_engines.py
    // honest: the Python side there runs without enrichment too.
    let mut stats = stats;
    if !std::env::args().any(|a| a == "--no-decompile") {
        if let Some(d) = Decompiler::locate(None, None).as_mut() {
            stats.decompiled = decompile::enrich(&mut active, d);
        }
    }
    let scenes = active.clone();

    let mut built = analyze::analyse(
        active,
        stats,
        vec![std::path::absolute(&root)
            .unwrap_or(root)
            .to_string_lossy()
            .into_owned()],
        None,
        analyze::WinnerRule::Last,
        false,
        Some(&host),
    );
    built.disabled = disabled;

    // Scene drift needs MBINCompiler, hgpaktool and a real install. It is
    // best-effort here for the same reason it is in the app: a machine without
    // the tools should still get the rest of the report. `--no-drift` skips it
    // even when they are present, so `compare_engines.py` can diff the pure
    // analysis without paying for decompilation.
    if std::env::args().any(|a| a == "--no-drift") {
        built.tools = tools::Status::skipped("skipped with --no-drift");
    } else {
        let game_root = gamefind::find_install().map(|i| PathBuf::from(i.root));
        let (findings, status) = drift::run(&scenes, game_root.as_deref(), None, None);
        built.drift = findings;
        built.tools = status;

        if let (Some(root), Some(d)) = (
            game_root.as_deref(),
            Decompiler::locate(None, None).as_mut(),
        ) {
            if let Some(mut src) = VanillaSource::locate(root, None) {
                merge::run(&mut built.conflicts, &scenes, d, &mut src);
            }
        }
    }

    println!("{}", serde_json::to_string(&report::to_json(&built)).unwrap());
}
