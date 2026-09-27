//! Flatten EXML files with the Rust engine and print the result as JSON.
//!
//! Exists so `tools/compare_engines.py` can diff this against
//! `nmscc.exml.parse` over a real mod library. Unit tests share the porter's
//! assumptions; 75 real files written by different mod authors do not.
//!
//!     cargo run --example exml_dump -- FILE_LIST
//!
//! `FILE_LIST` is a UTF-8 text file with one path per line, which avoids both
//! command-line length limits and any quoting argument about paths containing
//! spaces, `&`, or apostrophes — all of which occur in real mod names.

use std::path::{Path, PathBuf};

fn main() {
    let Some(list) = std::env::args().nth(1) else {
        eprintln!("usage: exml_dump FILE_LIST");
        std::process::exit(2);
    };

    let text = std::fs::read_to_string(&list).expect("cannot read file list");
    let paths: Vec<PathBuf> = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect();

    let mut out = serde_json::Map::new();
    for path in paths {
        let doc = anomaly_lib::engine::exml::parse(Path::new(&path));

        let props: serde_json::Map<String, serde_json::Value> = doc
            .props
            .iter()
            .map(|(k, v)| {
                (
                    k.clone(),
                    match v {
                        Some(value) => serde_json::Value::String(value.clone()),
                        None => serde_json::Value::Null,
                    },
                )
            })
            .collect();

        let annotations: serde_json::Map<String, serde_json::Value> = doc
            .annotations
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();

        out.insert(
            path.to_string_lossy().into_owned(),
            serde_json::json!({
                "template": doc.template,
                "mbinc_version": doc.mbinc_version.map(|v| v.to_string()),
                "amumss_version": doc.amumss_version,
                "props": props,
                "annotations": annotations,
                "error": doc.error,
            }),
        );
    }

    println!("{}", serde_json::to_string(&out).unwrap());
}
