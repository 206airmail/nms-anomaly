//! What one start-up costs, stage by stage.
//!
//! Replays exactly what `analyse_library` then `clean_preview` do, so a change
//! meant to make opening the app faster can be checked against a real library
//! rather than against a feeling.
//!
//!   cargo run --example startup_timing -- "<MODS folder>" "<game root>"

use std::path::PathBuf;
use std::time::Instant;

use anomaly_lib::engine::analyze::WinnerRule;
use anomaly_lib::engine::decompile::{self, Decompiler};
use anomaly_lib::engine::vanilla::VanillaSource;
use anomaly_lib::engine::{analyze, drift, merge, prune, scancache};

macro_rules! time {
    ($label:expr, $body:expr) => {{
        let at = Instant::now();
        let value = $body;
        println!("{:<20}{:>7.2}s", $label, at.elapsed().as_secs_f64());
        value
    }};
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mods = PathBuf::from(args.next().expect("usage: <MODS folder> <game root>"));
    let game = PathBuf::from(args.next().expect("usage: <MODS folder> <game root>"));
    let whole = Instant::now();

    let scan = time!("scan", scancache::refresh(&mods).expect("read the mods folder"));
    let mut active: Vec<_> = scan.mods.iter().filter(|m| !m.disabled).cloned().collect();
    let mut decompiler = Decompiler::locate(None, None).expect("MBINCompiler");
    let mut source = VanillaSource::locate(&game, None).expect("hgpaktool and PCBANKS");

    let mut stats = scan.stats.clone();
    stats.decompiled = time!("enrich", decompile::enrich(&mut active, &mut decompiler));
    let roots = vec![mods.to_string_lossy().into_owned()];
    let mut built = time!(
        "analyse",
        analyze::analyse(active.clone(), stats, roots, None, WinnerRule::Last, false, Some(&scan.host))
    );
    let (findings, status) = time!("drift", drift::run(&active, Some(&game), None, None));
    built.drift = findings;
    built.tools = status;
    time!("merge", merge::run(&mut built.conflicts, &active, &mut decompiler, &mut source));
    println!("{:-<27}\n{:<20}{:>7.2}s", "", "report shown", whole.elapsed().as_secs_f64());

    let plans = time!("plan", prune::plan(&active, &mut decompiler, &mut source));
    println!("{:-<27}\n{:<20}{:>7.2}s", "", "actions shown", whole.elapsed().as_secs_f64());
    println!(
        "\n{} plans; MBIN cache {} hit / {} converted; vanilla {} hit / {} unpacked",
        plans.len(),
        decompiler.cache_hits,
        decompiler.converted,
        source.cache_hits,
        source.unpacked
    );
}
