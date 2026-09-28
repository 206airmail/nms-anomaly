//! What installing one archive would do, without doing it.
//!
//! `cargo run --example install_probe <archive> [mods-dir]`
use std::path::PathBuf;

use anomaly_lib::engine::{archive, gamefind};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(zip) = args.next().map(PathBuf::from) else {
        eprintln!("usage: install_probe <archive> [mods-dir]");
        return;
    };
    let mods = match args.next() {
        Some(given) => PathBuf::from(given),
        None => match gamefind::find_install().filter(|f| f.has_mods_dir) {
            Some(found) => PathBuf::from(found.mods_dir),
            None => {
                eprintln!("no install found -- pass the mods folder");
                return;
            }
        },
    };
    println!("archive {}", zip.display());
    println!("mods    {}\n", mods.display());

    match archive::preview(&zip, &mods) {
        Ok(plan) => println!("{plan:#?}"),
        Err(e) => println!("REFUSED: {e}"),
    }
}
