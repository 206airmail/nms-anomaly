//! Dump the targets each AMUMSS recipe declares, as JSON.
//!
//! Companion to `exml_dump`, driven by `tools/compare_engines.py`.
//!
//!     cargo run --example lua_dump -- FILE_LIST

use std::path::{Path, PathBuf};

fn main() {
    let Some(list) = std::env::args().nth(1) else {
        eprintln!("usage: lua_dump FILE_LIST");
        std::process::exit(2);
    };

    let text = std::fs::read_to_string(&list).expect("cannot read file list");
    let mut out = serde_json::Map::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let path = PathBuf::from(line);
        let targets: Vec<String> =
            anomaly_lib::engine::luascript::declared_targets(Path::new(&path))
                .into_iter()
                .collect();
        out.insert(
            path.to_string_lossy().into_owned(),
            serde_json::json!(targets),
        );
    }
    println!("{}", serde_json::to_string(&out).unwrap());
}
