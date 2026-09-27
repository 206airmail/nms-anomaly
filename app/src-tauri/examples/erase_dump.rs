//! What deleting each mod would remove. Writes nothing, touches nothing.
//!
//!     cargo run --example erase_dump -- LOADOUT MODS_DIR STAGING_DIR ARCHIVES_DIR \
//!         DERIVED_DIR [OWNER]
//!
//! The case worth checking here is a mod this program has **fixed**. Such a mod
//! lives in two places at once -- the staged copy its author shipped, and the
//! cleaned or mended build we deploy in its place -- and deleting only the one
//! that happens to be deployed would leave the other occupying its full size on
//! disk forever, under a name the user has just been told is gone.

use anomaly_lib::engine::{erase, loadout::Loadout};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(book), Some(mods_dir), Some(staging), Some(archives), Some(derived)) = (
        args.next(),
        args.next(),
        args.next(),
        args.next(),
        args.next(),
    ) else {
        eprintln!(
            "usage: erase_dump LOADOUT MODS_DIR STAGING_DIR ARCHIVES_DIR DERIVED_DIR [OWNER]"
        );
        std::process::exit(2);
    };
    let only = args.next();

    let roots = erase::Roots::of(
        std::path::Path::new(&staging),
        std::path::Path::new(&derived),
        std::path::Path::new(&archives),
        std::path::Path::new(&mods_dir),
        None,
    );

    let book = Loadout::read(std::path::Path::new(&book));
    for entry in &book.entries {
        if let Some(want) = only.as_deref() {
            if entry.owner != want {
                continue;
            }
        }
        // Both answers: keeping the download, and reclaiming it too.
        for with_archive in [false, true] {
            let plan = erase::plan(entry, &roots, with_archive);
            if !with_archive {
                println!(
                    "{}  [{:?}]\n   staged from: {}\n   deployed as: {}",
                    plan.owner, entry.variant, entry.origin_path().display(), entry.source
                );
            }
            print!("   {} ", if with_archive { "with download:   " } else { "keeping download:" });
            if plan.items.is_empty() {
                println!("nothing on disk");
            } else {
                let parts: Vec<String> = plan
                    .items
                    .iter()
                    .map(|i| format!("{:?} {} files", i.what, i.files))
                    .collect();
                println!("{}  ({} bytes)", parts.join(" + "), plan.bytes);
            }
            if with_archive {
                for w in &plan.warnings {
                    println!("      note: {w}");
                }
                for r in &plan.refused {
                    println!("      REFUSED (outside our folders): {r}");
                }
                println!();
            }
        }
    }
}
