//! Scan a mod library with the Rust engine and print the result as JSON.
//!
//! Driven by `tools/compare_engines.py`. Emits the per-mod facts the analysis
//! depends on, so a divergence in the walk is caught before it can turn into
//! a wrong conflict verdict.
//!
//!     cargo run --example scan_dump -- MODS_DIR [--fast]

use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else {
        eprintln!("usage: scan_dump MODS_DIR [--fast]");
        std::process::exit(2);
    };
    let deep = !std::env::args().any(|a| a == "--fast");

    let (mods, stats, host) =
        anomaly_lib::engine::discovery::scan_roots(&[PathBuf::from(root)], deep)
            .expect("scan failed");

    let mods_json: Vec<serde_json::Value> = mods
        .iter()
        .map(|m| {
            serde_json::json!({
                "name": m.name,
                "files": m.files.len(),
                "targets": m.targets().iter().collect::<Vec<_>>(),
                "declared_targets": m.declared_targets.iter().collect::<Vec<_>>(),
                "loc_ids": m.loc_ids.len(),
                "amumss_version": m.amumss_version,
                "source": m.source,
                "disabled": m.disabled,
                "priority": m.priority,
                "max_version": m.max_version().map(|v| v.to_string()),
                "min_version": m.min_version().map(|v| v.to_string()),
                "props_total": m.files.iter().map(|f| f.props.len()).sum::<usize>(),
                "annotated_files": m.files.iter().filter(|f| f.annotated()).count(),
            })
        })
        .collect();

    println!(
        "{}",
        serde_json::to_string(&serde_json::json!({
            "mods": mods_json,
            "stats": stats,
            "real_load_order": host.has_real_order(),
            "manager": host.manager,
            "disable_all": host.disable_all,
        }))
        .unwrap()
    );
}
