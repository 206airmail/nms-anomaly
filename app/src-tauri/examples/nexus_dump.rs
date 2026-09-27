//! What the Vortex manifest tells us about where each mod came from.
//!
//!     cargo run --release --example nexus_dump -- MODS_DIR
//!
//! Update checking stands on this: if the mod id cannot be recovered here, the
//! API has nothing to be asked about that mod.

use std::path::PathBuf;

use anomaly_lib::engine::{discovery, nexusname};

fn main() {
    let Some(root) = std::env::args().nth(1) else {
        eprintln!("usage: nexus_dump MODS_DIR");
        std::process::exit(2);
    };

    let (mods, _stats, _host) =
        discovery::scan_roots(&[PathBuf::from(root)], true).expect("scan failed");

    let tsv = std::env::args().any(|a| a == "--tsv");

    let mut known = 0;
    let mut unknown: Vec<String> = Vec::new();
    if !tsv {
        println!("{:<38} {:>7}  {:<12} {}", "mod", "nexus", "version", "uploaded");
    }
    for m in &mods {
        match m.archive.as_deref().and_then(nexusname::parse) {
            Some(found) => {
                known += 1;
                if tsv {
                    println!("{}	{}	{}	{}	{}", m.name, found.mod_id, found.version, found.uploaded, m.archive.as_deref().unwrap_or(""));
                    continue;
                }
                println!(
                    "{:<38} {:>7}  {:<12} {}",
                    m.name.chars().take(38).collect::<String>(),
                    found.mod_id,
                    found.version.chars().take(12).collect::<String>(),
                    found.uploaded,
                );
            }
            None => unknown.push(format!(
                "{}  <=  {}",
                m.name,
                m.archive.as_deref().unwrap_or("(no recorded archive)")
            )),
        }
    }

    println!("\n{known} of {} mods can be checked for updates", mods.len());
    if !unknown.is_empty() {
        println!("\ncannot be identified:");
        for line in &unknown {
            println!("  {line}");
        }
    }
}
