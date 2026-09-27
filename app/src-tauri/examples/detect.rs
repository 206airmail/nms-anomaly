//! Dump what the Rust install detection finds, as JSON.
//!
//! Exists so `tools/compare_engines.py` can diff it against the Python
//! `nmscc.gamefind` on a real machine. Equivalence with the Python is the
//! acceptance test for every ported module, so it needs to be observable
//! from outside the app.
//!
//!     cargo run --example detect

fn main() {
    let installs = anomaly_lib::engine::gamefind::find_installs();
    let chosen = anomaly_lib::engine::gamefind::find_install();
    let payload = serde_json::json!({
        "installs": installs,
        "chosen": chosen.map(|i| i.root),
    });
    println!("{}", serde_json::to_string_pretty(&payload).unwrap());
}
