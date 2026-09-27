//! Run the settings migration against a *copy* of the real app data folder.
//!
//! The unit tests prove the rules on files this program wrote itself. This
//! proves them on the files actually sitting in `%APPDATA%\com.nsbro.nmscheck`
//! right now -- which is the only version of the migration that matters, and
//! the one that only runs once.
//!
//!     cargo run --example settings_migrate
//!
//! Copies first and never touches the original, so it can be run before the
//! app is ever started and again afterwards.

use std::path::PathBuf;

use anomaly_lib::engine::settings::Settings;

fn main() {
    let live = match std::env::args().nth(1) {
        Some(given) => PathBuf::from(given),
        None => match std::env::var("APPDATA") {
            Ok(dir) => PathBuf::from(dir).join("com.nsbro.nmscheck"),
            Err(_) => {
                eprintln!("no APPDATA; pass the folder as an argument");
                std::process::exit(2);
            }
        },
    };
    if !live.is_dir() {
        eprintln!("{} is not there", live.display());
        std::process::exit(2);
    }

    let work = std::env::temp_dir().join("nmscheck_settings_migrate");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    for name in ["settings.json", "nexus.key", "presets.json"] {
        let from = live.join(name);
        if from.is_file() {
            std::fs::copy(&from, work.join(name)).unwrap();
            println!("copied   {name}");
        } else {
            println!("absent   {name}");
        }
    }

    let path = work.join("settings.json");
    let found = Settings::read(&path);

    println!();
    println!("key      {}", if found.has_key() { "carried over" } else { "none" });
    println!("presets  {}", found.presets.len());
    println!("active   {:?}", found.active_preset);
    println!("mods_dir {:?}", found.mods_dir);
    println!("staging  {:?}", found.staging_dir);
    println!(
        "left     {:?}",
        ["nexus.key", "presets.json"]
            .iter()
            .filter(|n| work.join(n).exists())
            .collect::<Vec<_>>()
    );

    // Reading again must be the same answer: nothing left to migrate, and
    // nothing lost by the write the first read did.
    assert_eq!(Settings::read(&path), found, "a second read disagreed");
    println!();
    println!("on disk now:");
    println!(
        "{}",
        std::fs::read_to_string(&path)
            .unwrap()
            // The whole point of the exercise is not to print it.
            .replace(found.nexus_key.as_deref().unwrap_or("\0no key\0"), "<the key>")
    );

    let _ = std::fs::remove_dir_all(&work);
}
