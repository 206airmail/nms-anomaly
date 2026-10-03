//! The conflict-checking engine.
//!
//! This is the implementation: the app calls it directly and its JSON is the
//! contract the front end reads. It was ported from the Python in `nmscc/`,
//! which is kept only as a cross-check -- `tools/compare_engines.py` diffs the
//! two on a real library, so a mistake in this code shows up as a failing
//! check rather than a wrong answer in the UI.
//!
//! Ported: paths, versions, MBIN headers, install detection, EXML property
//! flattening, AMUMSS recipes, host environment, discovery, analysis,
//! MBINCompiler and hgpaktool bridges, and scene drift.

/// Read a JSON file this program owns, tolerating a byte-order mark.
///
/// Every Windows tool a user is likely to reach for writes UTF-8 *with* a BOM
/// -- Notepad does, and PowerShell's `Set-Content -Encoding utf8` does -- and
/// `serde_json` rejects the file outright when it finds one. The readers here
/// all fall back to "nothing recorded yet" rather than failing, so without
/// this a single hand-edit in Notepad would silently empty the user's mod
/// list and the next reconcile would treat the whole library as unknown.
///
/// Measured, not guessed: this cost a loadout of 62 mods during the cutover.
pub fn read_json<T: serde::de::DeserializeOwned>(path: &std::path::Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(text.strip_prefix('\u{feff}').unwrap_or(&text)).ok()
}

pub mod adopt;
pub mod analyze;
pub mod archive;
pub mod archivescan;
pub mod clock;
pub mod collection;
pub mod crashinfo;
pub mod decompile;
pub mod download;
pub mod edit;
pub mod drift;
pub mod deploy;
pub mod discovery;
pub mod erase;
pub mod exml;
pub mod exmltree;
pub mod fileid;
pub mod gamefind;
pub mod gameproc;
pub mod hook;
pub mod hostenv;
pub mod library;
pub mod loadout;
pub mod luascript;
pub mod machine;
pub mod mbin;
pub mod merge;
pub mod model;
pub mod namecache;
pub mod nexus;
pub mod nexusname;
pub mod nxm;
pub mod observed;
pub mod paths;
pub mod patchdiff;
pub mod pipe;
pub mod pipeline;
pub mod preset;
pub mod propcache;
pub mod prune;
pub mod repair;
pub mod report;
pub mod savewatch;
pub mod scancache;
pub mod scene;
pub mod sessionlog;
pub mod settings;
pub mod tools;
pub mod vanilla;
pub mod version;
