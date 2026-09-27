//! What one start-up costs, stage by stage.
//!
//! Replays exactly what `analyse_library` then `clean_preview` do, so a change
//! meant to make opening the app faster can be checked against a real library
//! rather than against a feeling.
//!
//!   cargo run --example startup_timing -- "<MODS folder>" "<game root>" [retain]
//!
//! Reports memory as well as time. A third argument of `retain` keeps the
//! flattened properties on every file, which is what the scan used to do;
//! without it they stay on disk and are fetched per file. That is the A/B for
//! the change, and it has to be measured here rather than from the running app,
//! which idles at 5 MB either way because it has not scanned yet.

use std::path::PathBuf;
use std::time::Instant;

use anomaly_lib::engine::analyze::WinnerRule;
use anomaly_lib::engine::decompile::{self, Decompiler};
use anomaly_lib::engine::vanilla::VanillaSource;
use anomaly_lib::engine::{analyze, discovery, drift, merge, prune, scancache};

/// Current and peak working set, in MB.
#[cfg(windows)]
fn memory() -> (f64, f64) {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set: usize,
        working_set: usize,
        rest: [usize; 6],
    }
    extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(h: isize, c: *mut Counters, cb: u32) -> i32;
    }
    let mut c = Counters {
        cb: std::mem::size_of::<Counters>() as u32,
        ..Default::default()
    };
    unsafe {
        K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb);
    }
    const MB: f64 = 1024.0 * 1024.0;
    (c.working_set as f64 / MB, c.peak_working_set as f64 / MB)
}

#[cfg(not(windows))]
fn memory() -> (f64, f64) {
    (0.0, 0.0)
}

fn show_memory(label: &str) {
    let (now, peak) = memory();
    println!("{label:<20}{now:>4.0} MB now, {peak:.0} MB peak");
}

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

    let retain = args.next().is_some_and(|a| a == "retain");
    let scan = if retain {
        // What the scan used to do, kept so the cost of retaining can be
        // measured again rather than remembered.
        time!("scan (retaining)", {
            let (found, stats, host) =
                discovery::scan_roots(&[mods.clone()], true).expect("read the mods folder");
            std::sync::Arc::new(scancache::Scan {
                mods: found,
                stats,
                host,
                root: mods.clone(),
            })
        })
    } else {
        time!("scan", scancache::refresh(&mods).expect("read the mods folder"))
    };
    show_memory("  after scan");
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
    // A digest of the report, so the two modes can be compared for AGREEMENT and
    // not merely for speed. Loading properties later must not change a single
    // conflict or clash; if it does, the saving is worthless.
    {
        let clashes: usize = built.conflicts.iter().map(|c| c.clashes.len()).sum();
        let mergeable = built.conflicts.iter().filter(|c| c.mergeable == Some(true)).count();
        let benign = built.conflicts.iter().filter(|c| c.benign).count();
        println!(
            "{:<20}{} conflicts, {} clashes, {} benign, {} mergeable",
            "  report", built.conflicts.len(), clashes, benign, mergeable
        );
    }
    let (findings, status) = time!("drift", drift::run(&active, Some(&game), None, None));
    built.drift = findings;
    built.tools = status;
    time!("merge", merge::run(&mut built.conflicts, &active, &mut decompiler, &mut source));
    println!("{:-<27}\n{:<20}{:>7.2}s", "", "report shown", whole.elapsed().as_secs_f64());

    let plans = time!("plan", prune::plan(&active, &mut decompiler, &mut source));
    println!("{:-<27}\n{:<20}{:>7.2}s", "", "actions shown", whole.elapsed().as_secs_f64());
    show_memory("  after actions");
    println!(
        "\n{} plans; MBIN cache {} hit / {} converted; vanilla {} hit / {} unpacked",
        plans.len(),
        decompiler.cache_hits,
        decompiler.converted,
        source.cache_hits,
        source.unpacked
    );
}
