//! What the Library list will actually show, assembled exactly as the app does.
//!
//!     cargo run --example names_live -- LOADOUT NAMES_CACHE MODS_DIR //!         [STAGING_DIR] [ARCHIVES_DIR] [KEY_FILE]
//!
//! This exists because a mod's name has several possible sources and the app
//! picks between them at runtime, so the only honest check is to assemble the
//! same answer from the same files on disk. It reads the loadout first (which
//! knows about mods that are switched off *and* about mods this program has
//! fixed, whose deployed build names nobody), fills gaps from the downloads and
//! then from the staging tree, and applies the resolved page titles -- giving up
//! a page title that names more than one mod, which is the case `display_name`
//! alone cannot see.
//!
//! Read-only unless a key file is given, in which case it resolves the pages it
//! has never seen and writes them to the cache, which is what `resolve_names`
//! does inside the app. The key is read from a file so it does not land in a
//! shell history.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anomaly_lib::engine::{library, loadout::Loadout, namecache::Names, nexus, scancache};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(book_path), Some(cache_path), Some(mods_dir)) =
        (args.next(), args.next(), args.next())
    else {
        eprintln!("usage: names_live LOADOUT NAMES_CACHE MODS_DIR [STAGING_DIR] [ARCHIVES_DIR]");
        std::process::exit(2);
    };
    let staging = args.next();

    let book = Loadout::read(Path::new(&book_path));
    let mut cache = Names::load(Path::new(&cache_path));

    // The app's `archive_index`, in full: the download, then the folder the
    // author's build is staged under, then whatever is deployed now -- which for
    // a mod we have cleaned or mended is a build of ours and names nobody --
    // then the downloads folder matched by name, then the staging tree.
    let by_slug = std::env::args()
        .nth(5)
        .map(|path| library::archives_by_slug(&PathBuf::from(path)))
        .unwrap_or_default();

    let mut staged: BTreeMap<String, String> = BTreeMap::new();
    for e in &book.entries {
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
    if let Some(path) = staging.as_deref() {
        for (owner, archive) in library::staged_archives(&PathBuf::from(path)) {
            staged.entry(owner).or_insert(archive);
        }
    }

    let scan = scancache::get(Path::new(&mods_dir)).unwrap_or_else(|err| {
        eprintln!("could not read {mods_dir}: {err}");
        std::process::exit(1);
    });

    let mut owners: Vec<(String, Option<String>)> = scan
        .mods
        .iter()
        .map(|m| {
            (
                m.name.clone(),
                library::archive_of(m, &staged).map(str::to_string),
            )
        })
        .collect();
    let seen: BTreeSet<&str> = scan.mods.iter().map(|m| m.name.as_str()).collect();
    let mut off = 0usize;
    for entry in &book.entries {
        if !seen.contains(entry.owner.as_str()) {
            off += 1;
            owners.push((
                entry.owner.clone(),
                Some(library::archive_stem(&entry.source).to_string()),
            ));
        }
    }

    // With a key, ask about every page never seen before. Costs one request
    // each and nothing on a second run, because the answers are kept.
    if let Some(key_file) = std::env::args().nth(6) {
        let key = std::fs::read_to_string(&key_file)
            .unwrap_or_else(|err| {
                eprintln!("cannot read {key_file}: {err}");
                std::process::exit(2);
            })
            .trim()
            .to_string();
        let api = nexus::Api::new(key);
        let ids: Vec<u64> = owners
            .iter()
            .filter_map(|(_, a)| a.as_deref())
            .filter_map(anomaly_lib::engine::nexusname::parse)
            .map(|f| f.mod_id)
            .collect();
        let wanted = cache.missing(ids);
        eprintln!("resolving {} page(s) never seen before", wanted.len());
        for mod_id in wanted {
            match api.page(mod_id) {
                Ok((page, _)) => match page.name.as_deref() {
                    Some(name) => cache.put(mod_id, name),
                    None => eprintln!("   {mod_id}: the page offers no name"),
                },
                Err(err) => eprintln!("   {mod_id}: {err}"),
            }
        }
        match cache.save(Path::new(&cache_path)) {
            Ok(()) => eprintln!("cached {} name(s)
", cache.len()),
            Err(err) => eprintln!("could not write {cache_path}: {err}"),
        }
    }

    let shown = library::names_for(owners.clone(), &cache);

    // Sorted the way the list sorts: by the name displayed, not by the folder.
    let mut rows: Vec<(&String, &String)> = shown.iter().collect();
    rows.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));

    let mut renamed = 0usize;
    for (owner, name) in &rows {
        if owner != name {
            renamed += 1;
            println!("{name:<52} <- {owner}");
        } else {
            println!("{name:<52}");
        }
    }

    // A name shared by two rows is the failure worth counting: a list you cannot
    // tell apart is worse than one that is merely ugly.
    let mut tally: BTreeMap<&str, usize> = BTreeMap::new();
    for (_, name) in &rows {
        *tally.entry(name.as_str()).or_insert(0) += 1;
    }
    let clashes: Vec<(&&str, &usize)> = tally.iter().filter(|(_, n)| **n > 1).collect();

    println!(
        "\n{} rows, {renamed} showing a name rather than a folder, {off} switched off",
        rows.len()
    );
    if clashes.is_empty() {
        println!("no two rows share a name");
    } else {
        println!("AMBIGUOUS:");
        for (name, n) in clashes {
            println!("   {n} rows all read {name:?}");
        }
    }
}
