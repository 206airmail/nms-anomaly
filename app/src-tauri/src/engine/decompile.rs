//! Optional MBINCompiler bridge for inspecting compiled assets.
//!
//! A mod that ships a compiled `.MBIN` is opaque: it can be hashed but not read,
//! so a clash against a mod shipping readable EXML can only be reported as
//! "cannot be compared". MBINCompiler turns the MBIN back into the same
//! `<Data template=...>` XML the EXML mods ship, which closes that gap.
//!
//! This is deliberately optional. Without the executable the engine still
//! works exactly as before; with it, contested binary assets become comparable
//! and scene replacements can be diffed against the game.
//!
//! Results are cached by content hash, so a repeat run converts nothing. The
//! output is `.MXML`, structurally identical to `.EXML` -- same `Property`
//! tree, same `_id`/`_index` row keys -- so [`super::exml`] and
//! [`super::scene`] both parse it unchanged.
//!
//! Port of `nmscc/decompile.py`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::model::{FileKind, Mod};

/// Names the executable goes by, in preference order.
pub const EXE_NAMES: [&str; 2] = ["MBINCompiler.exe", "MBINCompiler"];

/// Overrides discovery entirely, for a machine that keeps the tool elsewhere.
pub const ENV_OVERRIDE: &str = "NMS_MBINCOMPILER";

// NOTE: there is deliberately no timeout here any more. There was a `TIMEOUT`
// constant claiming one for a long time, and nothing ever applied it --
// `Command::output()` waits as long as the child takes and std has no way to cut
// that short. A constant that describes a protection the code does not have is
// worse than no constant, so the claim is gone rather than the protection
// silently absent. If a pathological asset ever does hang a conversion, this
// needs a real answer (a spawned child polled with `try_wait`), not a number.

/// Wraps an MBINCompiler executable and a conversion cache.
#[derive(Debug, Clone)]
pub struct Decompiler {
    pub exe: PathBuf,
    pub cache_dir: PathBuf,
    pub converted: usize,
    pub cache_hits: usize,
    pub failures: Vec<String>,
}

impl Decompiler {
    /// Find MBINCompiler, or return `None` when it is not installed.
    ///
    /// An `explicit` path is honoured exactly: if it does not exist the result
    /// is `None` rather than a silent fall back to some other copy, so a caller
    /// that named the wrong path is told so instead of quietly getting
    /// different behaviour than it asked for.
    pub fn locate(explicit: Option<&str>, cache_dir: Option<PathBuf>) -> Option<Self> {
        let cache = cache_dir.unwrap_or_else(Self::default_cache_dir);
        super::tools::find(explicit, ENV_OVERRIDE, &EXE_NAMES)
            .map(|exe| Self::new(exe, cache))
    }

    fn new(exe: PathBuf, cache_dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&cache_dir);
        Self {
            exe,
            cache_dir,
            converted: 0,
            cache_hits: 0,
            failures: Vec::new(),
        }
    }


    pub fn default_cache_dir() -> PathBuf {
        super::tools::cache_root().join("mbin-cache")
    }

    /// Compile EXML text back into a `.MBIN`, writing both beside `out_dir`.
    ///
    /// MBINCompiler converts in place and picks the direction from the input,
    /// so the XML is written next to where the binary should land and the
    /// compiler is pointed at it.
    /// MBINCompiler picks the conversion direction from the extension, and
    /// which of these it accepts depends on the asset: scenes round-trip as
    /// `.MXML`, AMUMSS-style data as `.EXML`. Trying both is cheaper than
    /// maintaining a table of which is which.
    pub fn compile(&mut self, xml: &str, out_dir: &Path, stem: &str) -> Result<PathBuf, String> {
        std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
        let mbin = out_dir.join(format!("{stem}.MBIN"));
        let mut last = String::new();

        for ext in ["MXML", "EXML"] {
            let xml_path = out_dir.join(format!("{stem}.{ext}"));
            std::fs::write(&xml_path, xml).map_err(|e| e.to_string())?;

            let result = Command::new(&self.exe)
                .arg("convert")
                .arg(&xml_path)
                .output()
                .map_err(|e| format!("could not run MBINCompiler: {e}"))?;

            if mbin.is_file() {
                return Ok(mbin);
            }
            let message = String::from_utf8_lossy(if result.stderr.is_empty() {
                &result.stdout
            } else {
                &result.stderr
            });
            last = message.trim().chars().take(200).collect();
            // Leave nothing behind for the next attempt to trip over.
            let _ = std::fs::remove_file(&xml_path);
        }
        Err(format!("MBINCompiler produced no .MBIN: {last}"))
    }

    /// Convert one MBIN to XML, hashing the file to key the cache.
    ///
    /// Use this when the caller does not already have the content hash --
    /// vanilla extracts, for instance. [`Self::decompile`] refuses an empty
    /// hash rather than cache under a meaningless key, which is easy to trip
    /// over by passing `""`.
    pub fn decompile_file(&mut self, mbin_path: &Path) -> Option<PathBuf> {
        let sha1 = Self::content_hash(mbin_path)?;
        self.decompile(mbin_path, &sha1)
    }

    /// The hash [`Self::decompile`] keys its cache on.
    ///
    /// Public because a caller that wants to cache something *derived* from a
    /// conversion needs the same identity to key it on, and deriving it a
    /// second way is how two caches end up disagreeing about which file they
    /// are talking about. See [`super::prune`].
    pub fn content_hash(mbin_path: &Path) -> Option<String> {
        let bytes = std::fs::read(mbin_path).ok()?;
        let mut hasher = sha1_smol::Sha1::new();
        hasher.update(&bytes);
        Some(hasher.digest().to_string())
    }

    /// Convert one MBIN to XML and return the path, or `None` on failure.
    ///
    /// Conversion happens on a copy inside the cache directory, so the mod
    /// folder is never written to and the output lands beside the copy without
    /// depending on MBINCompiler output-path flags.
    pub fn decompile(&mut self, mbin_path: &Path, sha1: &str) -> Option<PathBuf> {
        if sha1.is_empty() {
            return None;
        }
        let target = self.cache_dir.join(format!("{sha1}.MXML"));
        if target.is_file() {
            self.cache_hits += 1;
            return Some(target);
        }
        let alternative = self.cache_dir.join(format!("{sha1}.EXML"));
        if alternative.is_file() {
            self.cache_hits += 1;
            return Some(alternative);
        }

        let staged = self.cache_dir.join(format!("{sha1}.MBIN"));
        if let Err(err) = std::fs::copy(mbin_path, &staged) {
            self.failures.push(format!("{}: {err}", mbin_path.display()));
            return None;
        }

        let result = Command::new(&self.exe).arg("convert").arg(&staged).output();
        let _ = std::fs::remove_file(&staged);

        match result {
            Ok(_) => {}
            Err(err) => {
                self.failures.push(format!("{}: {err}", mbin_path.display()));
                return None;
            }
        }

        for candidate in [target, alternative] {
            if candidate.is_file() {
                self.converted += 1;
                return Some(candidate);
            }
        }
        self.failures.push(format!(
            "{}: MBINCompiler produced no output",
            mbin_path.display()
        ));
        None
    }
}



/// Compiled assets that share a target with another mod.
///
/// Only these can produce a conflict, so only these are worth converting: a
/// library holds hundreds of MBINs and each conversion costs about a second.
/// Returns `(mod index, file index)` pairs rather than references, so the
/// caller can mutate the files it names.
pub fn contested_binaries(mods: &[Mod]) -> Vec<(usize, usize)> {
    let mut providers: HashMap<&str, Vec<(usize, usize)>> = HashMap::new();
    for (mi, entry) in mods.iter().enumerate() {
        for (fi, file) in entry.files.iter().enumerate() {
            if let Some(target) = file.target.as_deref() {
                providers.entry(target).or_default().push((mi, fi));
            }
        }
    }

    let mut out = Vec::new();
    for entries in providers.values() {
        let owners: std::collections::HashSet<&str> =
            entries.iter().map(|(mi, _)| mods[*mi].name.as_str()).collect();
        if owners.len() < 2 {
            continue;
        }
        for (mi, fi) in entries {
            let file = &mods[*mi].files[*fi];
            if file.kind == Some(FileKind::Mbin) && file.props.is_empty() {
                out.push((*mi, *fi));
            }
        }
    }
    out.sort_unstable();
    out
}

/// Fill in property maps for contested compiled assets.
///
/// Without this a clash between two `.MBIN` copies can only be reported as
/// "cannot be compared", even with MBINCompiler sitting right there. Returns
/// the number of assets that became comparable.
pub fn enrich(mods: &mut [Mod], decompiler: &mut Decompiler) -> usize {
    let mut enriched = 0;
    for (mi, fi) in contested_binaries(mods) {
        let (path, sha1) = {
            let file = &mods[mi].files[fi];
            (file.abs_path.clone(), file.sha1.clone())
        };
        let Some(xml_path) = decompiler.decompile(Path::new(&path), &sha1) else {
            continue;
        };
        let doc = super::propcache::parse(&xml_path, &sha1);
        if doc.error.is_some() || doc.props.is_empty() {
            continue;
        }
        let file = &mut mods[mi].files[fi];
        file.props = doc.props;
        file.annotations = doc.annotations;
        file.template = doc.template;
        file.decompiled = true;
        if doc.mbinc_version.is_some() {
            file.mbinc_version = doc.mbinc_version;
        }
        enriched += 1;
    }
    enriched
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_explicit_missing_path_is_not_silently_replaced() {
        assert!(Decompiler::locate(Some("Z:\\nope\\MBINCompiler.exe"), None).is_none());
    }

    fn mbin(target: &str, owner: &str) -> Mod {
        Mod {
            name: owner.to_string(),
            files: vec![crate::engine::model::ModFile {
                target: Some(target.to_string()),
                kind: Some(FileKind::Mbin),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn only_binaries_two_mods_both_ship_are_worth_converting() {
        let mods = vec![
            mbin("A.MBIN", "one"),
            mbin("A.MBIN", "two"),
            mbin("B.MBIN", "three"),
        ];
        // B is shipped by a single mod, so nothing can conflict with it.
        assert_eq!(contested_binaries(&mods), vec![(0, 0), (1, 0)]);
    }

    #[test]
    fn one_mod_shipping_a_file_twice_is_not_contested_with_itself() {
        let mut only = mbin("A.MBIN", "one");
        only.files.push(only.files[0].clone());
        assert!(contested_binaries(&[only]).is_empty());
    }

    #[test]
    fn an_empty_hash_is_refused_rather_than_cached_under_it() {
        let dir = std::env::temp_dir().join("nmscheck-test-decompile");
        let mut d = Decompiler {
            exe: PathBuf::from("does-not-matter"),
            cache_dir: dir,
            converted: 0,
            cache_hits: 0,
            failures: Vec::new(),
        };
        assert!(d.decompile(Path::new("x.MBIN"), "").is_none());
    }

}
