//! What taking over the mods folder would do. Read-only unless told otherwise.
//!
//!     cargo run --release --example cutover -- MODS_DIR [--adopt LOADOUT_PATH]

use std::path::PathBuf;

use anomaly_lib::engine::adopt;

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(root) = args.next() else {
        eprintln!("usage: cutover MODS_DIR [--adopt LOADOUT_PATH]");
        std::process::exit(2);
    };
    let rest: Vec<String> = args.collect();

    // No staging or derived root given: this reads the mods folder and whatever
    // manifest another manager left in it, which is what a cutover starts from.
    let survey = adopt::survey(&PathBuf::from(&root), None, None).unwrap_or_else(|err| {
        eprintln!("{err}");
        std::process::exit(1);
    });

    println!("deployed by: {}", survey.manager.as_deref().unwrap_or("nobody"));
    println!("staging:     {}\n", survey.staging.as_deref().unwrap_or("(unknown)"));

    let mut multi = 0;
    for c in &survey.candidates {
        for note in &c.notes {
            println!("NOTE     {:<44} {note}", c.owner.chars().take(44).collect::<String>());
        }
        if c.refused.is_none() {
            if c.deployed.len() > 1 {
                multi += 1;
            }
            continue;
        }
        println!("REFUSED  {:<44} {}", c.owner.chars().take(44).collect::<String>(),
                 c.refused.as_deref().unwrap_or(""));
    }

    println!("\n{} of {} mods can be taken over as they stand",
             survey.ready(), survey.candidates.len());
    println!("{multi} of those deploy more than one thing (a folder plus a script or notes)");
    if !survey.unmanaged.is_empty() {
        println!("\n{} items no manager claims, left alone:", survey.unmanaged.len());
        for name in survey.unmanaged.iter().take(10) {
            println!("   {name}");
        }
    }

    if rest.first().map(String::as_str) == Some("--adopt") {
        let Some(book) = rest.get(1) else {
            eprintln!("--adopt needs a path to write the loadout to");
            std::process::exit(2);
        };
        let taken = adopt::adopt(&survey, &PathBuf::from(book)).unwrap_or_else(|err| {
            eprintln!("{err}");
            std::process::exit(1);
        });
        println!("\nadopted {taken} mods into {book}");
        println!("nothing in the game or in staging was changed");
    } else {
        println!("\n(nothing was written -- pass --adopt LOADOUT_PATH to record it)");
    }
}
