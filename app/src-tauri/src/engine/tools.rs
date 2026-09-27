//! Where the external tools live, and whether they were found.
//!
//! The engine shells out to two binaries. MBINCompiler turns a compiled
//! `.MBIN` back into XML; hgpaktool pulls the vanilla copy of an asset out of
//! the game's archives. Without both, the scene check cannot run at all.
//!
//! **A packaged build has to be told where they are.** In development they sit
//! in the checkout's `tools/`, which `CARGO_MANIFEST_DIR` finds. In a bundle
//! they are Tauri resources, and only the running app can resolve that
//! directory -- so `lib.rs` calls [`set_bundled_dir`] once at startup and
//! everything downstream picks it up.
//!
//! **Missing tools must be visible, not silent.** If discovery fails, the scene
//! check quietly finds nothing, and a report with no findings is
//! indistinguishable from a clean library. [`Status`] therefore travels with
//! the report so the UI can say the check did not run instead of implying
//! everything is fine.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use serde::Serialize;

/// Set once by the app; the directory holding the bundled binaries.
static BUNDLED: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Tell the engine where a packaged build keeps its tools.
///
/// Safe to call more than once; the last value wins. Never called in tests or
/// by the example binaries, which fall back to the checkout's `tools/`.
pub fn set_bundled_dir(dir: PathBuf) {
    if let Ok(mut slot) = BUNDLED.write() {
        *slot = Some(dir);
    }
}

/// Directories to search for a tool, most specific first.
pub fn dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();

    // A bundle: whatever the app resolved its resource directory to.
    if let Ok(slot) = BUNDLED.read() {
        if let Some(dir) = slot.as_ref() {
            out.push(dir.clone());
        }
    }

    // A released build that keeps the tools beside the executable.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.join("tools"));
            out.push(dir.to_path_buf());
        }
    }

    // A `cargo run` from the repo. `tools/` holds the redistributable copies
    // and comes first; the venv below it holds the pip-installed hgpaktool
    // launcher, which has an absolute path to this checkout baked into it and
    // therefore only works here.
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    if let Some(project) = manifest.parent().and_then(|p| p.parent()) {
        out.push(project.join("tools"));
        out.push(project.join("tools").join("venv").join("Scripts"));
        out.push(project.join("tools").join("venv").join("bin"));
    }
    out
}

/// Find the first existing file named any of `names` in [`dirs`], then `PATH`.
///
/// A path given explicitly -- as an argument or through `env_var` -- is
/// honoured exactly: if it does not exist the result is `None` rather than a
/// silent fall back to some other copy. Someone who named a path and got a
/// different binary would have no way to tell, which is the same silent
/// substitution this module exists to prevent.
pub fn find(explicit: Option<&str>, env_var: &str, names: &[&str]) -> Option<PathBuf> {
    let pinned = explicit.map(str::to_string).or_else(|| {
        std::env::var(env_var)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    });
    if let Some(path) = pinned {
        let path = PathBuf::from(path);
        return path.is_file().then_some(path);
    }
    for dir in dirs() {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in names {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Where per-user caches live.
pub fn cache_root() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| std::env::var("XDG_CACHE_HOME").ok())
        .or_else(|| std::env::var("HOME").ok())
        .unwrap_or_else(|| ".".to_string());
    PathBuf::from(base).join("nmscheck")
}

/// What the run had available, and what it therefore could not check.
///
/// Serialised with the report so a missing tool reads as "not checked" rather
/// than as "nothing wrong".
#[derive(Debug, Clone, Default, Serialize)]
pub struct Status {
    /// Path to MBINCompiler, or `None` when it was not found.
    pub mbincompiler: Option<String>,
    /// Path to hgpaktool, or `None` when it was not found.
    pub hgpaktool: Option<String>,
    /// True when the scene check actually ran.
    pub scene_check: bool,
    /// Why it did not run, in words the UI can show as-is.
    pub scene_check_note: Option<String>,
}

impl Status {
    /// Describe a run where the scene check was deliberately not attempted.
    pub fn skipped(reason: &str) -> Self {
        Self {
            scene_check: false,
            scene_check_note: Some(reason.to_string()),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_missing_path_is_not_silently_replaced() {
        assert!(find(Some("Z:\\nope\\thing.exe"), "NMS_NOPE", &["thing.exe"]).is_none());
    }

    #[test]
    fn the_checkout_tools_folder_is_searched_before_the_venv() {
        let dirs = dirs();
        let tools = dirs.iter().position(|d| d.ends_with("tools"));
        let venv = dirs
            .iter()
            .position(|d| d.to_string_lossy().contains("venv"));
        match (tools, venv) {
            (Some(t), Some(v)) => assert!(
                t < v,
                "the redistributable copy in tools/ must win over the venv \
                 launcher, which has this checkout's path baked into it"
            ),
            _ => panic!("expected both tools/ and the venv in the search path"),
        }
    }

    #[test]
    fn a_bundled_dir_is_searched_first() {
        set_bundled_dir(PathBuf::from("Z:\\bundle\\tools"));
        assert_eq!(dirs().first(), Some(&PathBuf::from("Z:\\bundle\\tools")));
        // Leave the static as the rest of the suite expects it.
        if let Ok(mut slot) = BUNDLED.write() {
            *slot = None;
        }
    }

    #[test]
    fn an_installed_build_looks_beside_itself_before_any_checkout() {
        // An installed app has `tools/` next to its executable. The path baked
        // in by CARGO_MANIFEST_DIR points at the machine the build was made on,
        // so it must never be preferred over what actually shipped.
        let dirs = dirs();
        let exe_tools = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|d| d.join("tools")))
            .and_then(|want| dirs.iter().position(|d| *d == want));
        let manifest_tools = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("tools"))
            .and_then(|want| dirs.iter().position(|d| *d == want));
        match (exe_tools, manifest_tools) {
            (Some(e), Some(m)) => assert!(e < m, "shipped tools must win"),
            _ => panic!("expected both the exe-relative and checkout tools dirs"),
        }
    }

    #[test]
    fn an_env_override_pointing_nowhere_does_not_fall_back() {
        // Falling through to some other copy would give the caller a binary
        // they did not ask for, with no way to notice.
        let var = "NMS_TOOLS_TEST_OVERRIDE";
        std::env::set_var(var, r"Z:\nope\thing.exe");
        let found = find(None, var, &["MBINCompiler.exe"]);
        std::env::remove_var(var);
        assert!(found.is_none());
    }

    #[test]
    fn an_empty_env_override_is_ignored_rather_than_treated_as_a_path() {
        let var = "NMS_TOOLS_TEST_EMPTY";
        std::env::set_var(var, "   ");
        let found = find(None, var, &["definitely-not-a-real-tool.exe"]);
        std::env::remove_var(var);
        assert!(found.is_none(), "should search normally and find nothing");
    }

    #[test]
    fn a_skipped_status_carries_its_reason() {
        let status = Status::skipped("hgpaktool not found");
        assert!(!status.scene_check);
        assert_eq!(status.scene_check_note.as_deref(), Some("hgpaktool not found"));
    }
}
