//! Ask Nexus whether anything in the mods folder is out of date.
//!
//!     cargo run --release --example update_check -- MODS_DIR KEY_FILE \
//!         [STAGING_DIR] [LOADOUT] [ARCHIVES_DIR]
//!
//! The key is read from a file rather than the command line so it does not
//! land in a shell history.
//!
//! Give the staging folder on a library this program has taken over: the mod
//! folders in the game are not named after their archives, and once Vortex's
//! manifest is gone the staging tree is the only thing that knows which archive
//! -- and therefore which Nexus page -- each one came from.
//!
//! Give the loadout and the downloads folder as well to cover the mods this
//! program has *fixed*. A cleaned, mended or merged mod is deployed from a build
//! of ours that carries no Nexus name at all, so without the loadout's `origin`
//! it drops out of update checking silently -- exactly the mod whose updates you
//! most want to hear about.

use std::path::{Path, PathBuf};

use anomaly_lib::engine::{discovery, library, nexus};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(root), Some(key_file)) = (args.next(), args.next()) else {
        eprintln!("usage: update_check MODS_DIR KEY_FILE [STAGING_DIR] [LOADOUT] [ARCHIVES_DIR]");
        std::process::exit(2);
    };
    let staging = args.next();
    let book_path = args.next();
    let archives = args.next();

    let key = std::fs::read_to_string(&key_file)
        .unwrap_or_else(|e| {
            eprintln!("cannot read {key_file}: {e}");
            std::process::exit(2);
        })
        .trim()
        .to_string();

    let api = nexus::Api::new(key);
    match api.whoami() {
        Ok(who) => println!(
            "key belongs to {} ({})\n",
            who.name,
            if who.is_premium { "premium" } else { "not premium" }
        ),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }

    let (mods, _stats, _host) =
        discovery::scan_roots(&[PathBuf::from(root)], true).expect("scan failed");
    println!("checking {} mods\n", mods.len());

    // The app's `archive_index`, in the same order.
    let by_slug = archives
        .map(|path| library::archives_by_slug(&PathBuf::from(path)))
        .unwrap_or_default();
    let mut staged = library::Staged::new();
    if let Some(path) = book_path.as_deref() {
        for e in anomaly_lib::engine::loadout::Loadout::read(Path::new(path)).entries {
            let named = e
                .archive
                .as_deref()
                .map(library::archive_stem)
                .map(str::to_string)
                .into_iter()
                .chain(e.origin.as_deref().map(|o| library::archive_stem(o).to_string()))
                .chain(std::iter::once(library::archive_stem(&e.source).to_string()))
                .find(|n| anomaly_lib::engine::nexusname::parse(n).is_some())
                .or_else(|| {
                    library::archive_for_slug(
                        &e.owner,
                        e.origin.as_deref().map(library::archive_stem),
                        &by_slug,
                    )
                });
            if let Some(name) = named {
                staged.insert(e.owner.clone(), name);
            }
        }
    }
    if let Some(path) = staging.as_deref() {
        for (owner, archive) in library::staged_archives(&PathBuf::from(path)) {
            staged.entry(owner).or_insert(archive);
        }
    }
    let (checks, budget) = nexus::check_all(&api, &mods, &staged, |done, total| {
        if done % 10 == 0 || done == total {
            eprintln!("  {done}/{total} pages");
        }
    });

    let mut current = 0;
    let mut other: Vec<&nexus::Check> = Vec::new();
    for c in &checks {
        match c.standing {
            nexus::Standing::Current => current += 1,
            _ => other.push(c),
        }
    }

    for c in &other {
        match &c.standing {
            nexus::Standing::Outdated {
                latest_version,
                latest_name,
                page,
                ..
            } => println!(
                "UPDATE    {:<44} {} -> {}\n          {latest_name}\n          {page}",
                c.owner,
                c.recorded_version.as_deref().unwrap_or("?"),
                latest_version
            ),
            nexus::Standing::RecordStale { actual_version } => println!(
                // Not "Vortex says": the archive is recorded by whichever of the
                // manifest or the staging tree knows, and after a cutover it is
                // the staging tree.
                "STALE     {:<44} recorded as {}, the folder holds {actual_version}",
                c.owner,
                c.recorded_version.as_deref().unwrap_or("?")
            ),
            nexus::Standing::Withdrawn { installed_version } => println!(
                "WITHDRAWN {:<44} {installed_version} is no longer offered on Nexus",
                c.owner
            ),
            nexus::Standing::Unknown { reason } => {
                println!("UNKNOWN   {:<44} {reason}", c.owner)
            }
            nexus::Standing::Current => {}
        }
    }

    println!(
        "\n{current} up to date, {} needing a look",
        other.len()
    );
    println!(
        "budget left: {} this hour, {} today",
        budget.hourly_remaining, budget.daily_remaining
    );
}
