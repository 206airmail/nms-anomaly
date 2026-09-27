//! Measure load order from a recorded session, against the real game.
//!
//! This is the check that retires the winner-rule assumption, run from outside
//! the app so the answer can be seen without a window: it reads a session log,
//! groups the `modfile loaded` lines by asset, and lines each sequence up
//! against the `ModPriority` values the game itself wrote.
//!
//!     cargo run --example observe -- <path-to-session.log>
//!
//! With no argument the newest log in the app's sessions folder is used.

use std::path::{Path, PathBuf};

use anomaly_lib::engine::{gamefind, hostenv, observed};

fn newest_log() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var("APPDATA").ok()?)
        .join("NMS Anomaly")
        .join("sessions");
    let mut logs: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("log")))
        .collect();
    logs.sort();
    logs.pop()
}

fn main() {
    let arg = std::env::args().nth(1).map(PathBuf::from);
    let Some(log) = arg.or_else(newest_log) else {
        eprintln!("no session log given, and none found in the app's sessions folder");
        std::process::exit(2);
    };
    let Ok(text) = std::fs::read_to_string(&log) else {
        eprintln!("could not read {}", log.display());
        std::process::exit(2);
    };

    let install = gamefind::find_install();
    let (host, known) = match &install {
        Some(i) => {
            let root = Path::new(&i.root);
            let host = hostenv::read_mod_settings(root);
            let known: Vec<String> = std::fs::read_dir(&i.mods_dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .collect();
            (host, known)
        }
        None => (hostenv::HostInfo::default(), Vec::new()),
    };

    let loads = observed::loads(&text);
    let applied = observed::applied(&loads, &known, &host);
    let measured = observed::measure(
        &log.display().to_string(),
        &applied,
        loads.len(),
        anomaly_lib::engine::analyze::WinnerRule::Last,
        anomaly_lib::engine::clock::now_ms(),
    );

    println!("log        : {}", log.display());
    println!("loads      : {}", measured.loads);
    println!("contested  : {}", measured.contested);
    println!("testable   : {}", measured.testable);
    println!("direction  : {}", measured.direction.as_str());
    println!("consistent : {}", measured.consistent());
    println!();
    println!("{}", measured.basis());
    println!();
    for r in &measured.readings {
        println!(
            "  if {:<50} -> winner rule \"{}\"{}",
            r.model,
            r.rule,
            if r.is_current { "   <-- what the tool uses" } else { "" }
        );
    }
    println!();

    for seq in &applied {
        let spelled: Vec<String> = seq
            .order
            .iter()
            .zip(&seq.priorities)
            .map(|(name, p)| match p {
                Some(n) => format!("{name} [{n}]"),
                None => format!("{name} [unregistered]"),
            })
            .collect();
        println!(
            "{}{}\n    {}",
            seq.target,
            if seq.testable() { "" } else { "   (untestable)" },
            spelled.join("\n -> ")
        );
    }

    if !measured.exceptions.is_empty() {
        println!("\nAGAINST THE RULE:");
        for e in &measured.exceptions {
            println!("  {} [{}] {:?} {:?}", e.target, e.direction, e.order, e.priorities);
        }
    }
}
