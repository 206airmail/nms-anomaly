//! Diagnose, and optionally mend, a malformed mod file.
//!
//!     cargo run --release --example repair_dump -- <FILE.EXML> [--write OUT]

use std::path::PathBuf;

use anomaly_lib::engine::{exml, repair};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first().map(PathBuf::from) else {
        eprintln!("usage: repair_dump <FILE.EXML> [--write OUT]");
        std::process::exit(2);
    };
    let out = args
        .iter()
        .position(|a| a == "--write")
        .and_then(|at| args.get(at + 1))
        .map(PathBuf::from);

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("cannot read {}: {e}", path.display());
            std::process::exit(1);
        }
    };

    println!("file:   {}", path.display());
    println!("lines:  {}", text.lines().count());
    match exml::parse_str(&text) {
        Ok(_) => println!("parses: yes"),
        Err(e) => println!("parses: NO -- {e}"),
    }

    let Some(fault) = repair::diagnose(&text) else {
        println!("\nnothing here is safe to mend automatically");
        return;
    };

    println!("\nDIAGNOSIS");
    println!("  {}", fault.summary);
    for one in &fault.unclosed {
        println!("  - {one}");
    }
    println!(
        "  confidence: {}",
        if fault.confident {
            "high -- everything after it is indented inside it"
        } else {
            "low -- the layout does not settle where it belongs"
        }
    );

    match repair::repair(&text) {
        Ok(fixed) => {
            println!("\nREPAIR");
            println!(
                "  {} line(s) -> {} line(s)",
                text.lines().count(),
                fixed.lines().count()
            );
            match exml::parse_str(&fixed) {
                Ok(_) => println!("  the mended file parses"),
                Err(e) => println!("  STILL BROKEN: {e}"),
            }
            for (n, line) in fixed.lines().enumerate() {
                if line.trim() == "</Property>" && n + 2 >= fixed.lines().count() {
                    println!("  added at line {}: {line:?}", n + 1);
                }
            }
            if let Some(dest) = out {
                match std::fs::write(&dest, &fixed) {
                    Ok(()) => println!("\nwritten to {}", dest.display()),
                    Err(e) => eprintln!("\ncould not write: {e}"),
                }
            } else {
                println!("\n(nothing written -- pass --write OUT to save)");
            }
        }
        Err(e) => println!("\ncannot mend: {e}"),
    }
}
