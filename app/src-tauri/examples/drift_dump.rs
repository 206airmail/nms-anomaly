//! Run the scene-drift check with the Rust engine and print it as JSON.
//!
//! Needs MBINCompiler and hgpaktool (both in the checkout's `tools/`) and a
//! real game install, since the whole point is comparing a mod's scene against
//! the one inside PCBANKS.
//!
//!     cargo run --example drift_dump -- MODS_DIR [GAME_ROOT]
//!
//! `GAME_ROOT` defaults to whatever install detection finds, which is what the
//! app itself does.

use std::path::PathBuf;

use anomaly_lib::engine::{discovery, drift, gamefind};

fn main() {
    let Some(root) = std::env::args().nth(1) else {
        eprintln!("usage: drift_dump MODS_DIR [GAME_ROOT]");
        std::process::exit(2);
    };
    let root = PathBuf::from(root);

    let game_root = match std::env::args().nth(2) {
        Some(path) => PathBuf::from(path),
        None => match gamefind::find_install() {
            Some(install) => PathBuf::from(install.root),
            None => {
                eprintln!("no game install found; pass GAME_ROOT explicitly");
                std::process::exit(2);
            }
        },
    };

    let (mods, _stats, _host) =
        discovery::scan_roots(&[root], true).expect("scan failed");
    let active: Vec<_> = mods.into_iter().filter(|m| !m.disabled).collect();

    let (findings, status) = drift::run(&active, Some(&game_root), None, None);
    if !status.scene_check {
        eprintln!(
            "warning: scene check skipped: {}",
            status.scene_check_note.as_deref().unwrap_or("unknown reason")
        );
    }

    let payload: Vec<_> = findings
        .iter()
        .map(|d| {
            serde_json::json!({
                "mod": d.mod_name,
                "target": d.target,
                "severity": d.severity.as_str(),
                "added": d.added,
                "removed": d.removed,
                "moved": d.moved.iter().map(|m| serde_json::json!({
                    "path": m.path,
                    "kind": m.kind,
                    "before": [m.before.0, m.before.1, m.before.2],
                    "after": [m.after.0, m.after.1, m.after.2],
                    // Full precision on purpose: this dump is compared against
                    // another engine, and rounding here before the comparison
                    // rounds again can shift a value across a tie boundary.
                    "distance": m.distance(),
                    "axis": m.axis(),
                    "contents_held": m.contents_held,
                    "rotated_only": m.rotated_only,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();

    println!("{}", serde_json::to_string_pretty(&payload).unwrap());
}
