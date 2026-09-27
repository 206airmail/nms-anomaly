//! Access to the unmodified game assets inside PCBANKS.
//!
//! A sparse EXML patch can be judged on its own: it names a handful of
//! properties and the game merges them. A mod that ships a whole `.SCENE.MBIN`
//! cannot -- every node in the file is now the mod's, and the only way to know
//! which of them the author actually meant to change is to hold the vanilla
//! file next to it.
//!
//! Extraction is cheap enough to do on demand. Against 98 paks totalling 31 GB,
//! pulling a handful of targets takes under three seconds, and results are
//! cached per game build so a repeat run costs nothing.
//!
//! **Paths in the paks are not mod paths.** Mods ship globals at
//! `GLOBALS/GCCAMERAGLOBALS.GLOBAL.MBIN`; inside the paks that file sits at the
//! archive root with no `GLOBALS/` component at all, so a filter built from the
//! mod-relative path silently matches nothing. This module therefore filters on
//! the *basename* and resolves the result by longest matching path suffix,
//! which is correct whether or not the directory prefixes agree.
//!
//! Port of `nmscc/vanilla.py`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use sha1_smol::Sha1;


/// Names the extractor goes by, in preference order.
pub const EXE_NAMES: [&str; 2] = ["hgpaktool.exe", "hgpaktool"];

/// Overrides discovery entirely.
pub const ENV_OVERRIDE: &str = "NMS_HGPAKTOOL";

/// Where the game keeps its archives, relative to the install root.
pub const PCBANKS_SUBPATH: &str = "GAMEDATA/PCBANKS";

/// Names the per-build stamp, which is what makes two caches orderable.
pub const STAMP_FILE: &str = "_build.json";

/// One game build whose vanilla extracts are cached on this machine.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Build {
    /// the digest that names the cache folder
    pub key: String,
    /// when this build was first seen here
    pub first_seen_ms: i64,
    pub paks: usize,
    pub bytes: u64,
    /// false when the time came from the folder's own timestamp rather than
    /// from a stamp written at the time.
    ///
    /// Worth carrying rather than hiding. Caches predating the stamp can still
    /// be ordered by mtime, which is what lets this feature work on the first
    /// update after it ships instead of the second -- but an mtime moves when
    /// anything is written into the folder, so a screen calling that a date
    /// would be overstating what it knows.
    #[serde(default)]
    pub dated: bool,
}

/// Points the whole per-build cache somewhere else.
///
/// Exists so the update comparison can be exercised end to end against a
/// fabricated pair of builds without writing into the real cache, where a fake
/// build would linger in [`builds`] and be reported as a genuine game update
/// forever. Same shape as [`ENV_OVERRIDE`], for the same reason: the thing worth
/// overriding in a test is the thing that reaches outside the process.
pub const CACHE_OVERRIDE: &str = "NMS_VANILLA_CACHE";

/// Where every build's cache lives.
pub fn cache_home() -> PathBuf {
    if let Ok(dir) = std::env::var(CACHE_OVERRIDE) {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    super::tools::cache_root().join("vanilla")
}

/// Every cached build on this machine, oldest first.
///
/// A folder with no stamp is dated by its own modification time, so the caches
/// that existed before stamping was added still take their place in the order.
pub fn builds() -> Vec<Build> {
    let home = cache_home();
    let mut out: Vec<Build> = std::fs::read_dir(&home)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        // `map`, not `filter_map`: every directory yields a build, because an
        // unstamped one still has a modification time to be ordered by.
        .map(|entry| {
            let dir = entry.path();
            let key = entry.file_name().to_string_lossy().to_string();
            if let Some(stamp) = super::read_json::<Build>(&dir.join(STAMP_FILE)) {
                return Build { key, ..stamp };
            }
            let mtime = std::fs::metadata(&dir)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            Build {
                key,
                first_seen_ms: mtime,
                paks: 0,
                bytes: 0,
                dated: false,
            }
        })
        .collect();
    out.sort_by_key(|b| (b.first_seen_ms, b.key.clone()));
    out
}

/// What one [`VanillaSource::fetch`] produced.
#[derive(Debug, Clone, Default)]
pub struct Extraction {
    /// canonical target -> path of the extracted vanilla MBIN
    pub found: HashMap<String, PathBuf>,
    /// targets with no counterpart in PCBANKS, i.e. assets the mod invented
    pub missing: Vec<String>,
    /// True when nothing had to be unpacked because the cache was complete
    pub cached: bool,
    pub error: Option<String>,
}

/// Extracts vanilla assets from a game install, with a per-build cache.
#[derive(Debug, Clone)]
pub struct VanillaSource {
    pub pcbanks: PathBuf,
    pub exe: PathBuf,
    pub cache_dir: PathBuf,
    pub unpacked: usize,
    pub cache_hits: usize,
}

impl VanillaSource {
    /// Find the extractor and the game's archives, or return `None`.
    ///
    /// As with [`super::decompile::Decompiler`], an `explicit` path is honoured
    /// exactly rather than falling back to some other copy.
    pub fn locate(game_root: &Path, explicit: Option<&str>) -> Option<Self> {
        // Built from the constant rather than spelled out again: two copies of
        // the same path is two places to fix when the game moves it.
        let pcbanks = PCBANKS_SUBPATH
            .split('/')
            .fold(game_root.to_path_buf(), |at, part| at.join(part));
        if !pcbanks.is_dir() {
            return None;
        }
        let exe = super::tools::find(explicit, ENV_OVERRIDE, &EXE_NAMES)?;
        let cache_dir = cache_home().join(build_key(&pcbanks));
        let source = Self {
            pcbanks,
            exe,
            cache_dir,
            unpacked: 0,
            cache_hits: 0,
        };
        source.stamp();
        Some(source)
    }

    /// Record which game build this cache folder belongs to, and when it
    /// arrived.
    ///
    /// The folder name is a digest, which tells a person nothing and, worse,
    /// cannot be ordered. [`super::patchdiff`] needs to know which of two
    /// cached builds came *first* to say what a patch changed, and the only
    /// honest answer is one written down at the time. So this is written once,
    /// when a build is first seen, and never touched again -- rewriting
    /// `first_seen_ms` on every launch would quietly redate the baseline and
    /// make the newer build look like the older one.
    fn stamp(&self) {
        let file = self.cache_dir.join(STAMP_FILE);
        if file.is_file() {
            return;
        }
        let paks: Vec<(u64, u64)> = std::fs::read_dir(&self.pcbanks)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .to_uppercase()
                    .ends_with(".PAK")
            })
            .filter_map(|e| e.metadata().ok())
            .map(|m| (1, m.len()))
            .collect();
        let stamp = Build {
            key: self
                .cache_dir
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            first_seen_ms: super::clock::now_ms(),
            paks: paks.len(),
            bytes: paks.iter().map(|(_, b)| b).sum(),
            dated: true,
        };
        let Ok(text) = serde_json::to_string_pretty(&stamp) else {
            return;
        };
        if std::fs::create_dir_all(&self.cache_dir).is_err() {
            return;
        }
        let scratch = self.cache_dir.join("_build.writing");
        if std::fs::write(&scratch, text).is_ok() {
            let _ = std::fs::rename(&scratch, &file);
        } else {
            let _ = std::fs::remove_file(&scratch);
        }
    }


    /// Extract the vanilla counterpart of each canonical `target`.
    ///
    /// Targets already present in the cache are not unpacked again, so the
    /// common case of re-running the checker touches no archives at all.
    pub fn fetch(&mut self, targets: &[String]) -> Extraction {
        let mut result = Extraction::default();

        let mut wanted: Vec<String> = targets.iter().filter(|t| !t.is_empty()).cloned().collect();
        wanted.sort();
        wanted.dedup();
        if wanted.is_empty() {
            result.cached = true;
            return result;
        }

        let absent = self.known_absent();
        let mut pending = Vec::new();
        for target in &wanted {
            let cached = self.cache_path(target);
            if cached.is_file() {
                result.found.insert(target.clone(), cached);
                self.cache_hits += 1;
            } else if absent.contains(target) {
                // Looked for before, on this build, and not there. Asking the
                // archives again can only produce the same answer.
                result.missing.push(target.clone());
                self.cache_hits += 1;
            } else {
                pending.push(target.clone());
            }
        }
        if pending.is_empty() {
            result.cached = true;
            return result;
        }

        let staging = self.cache_dir.join("_staging");
        let _ = std::fs::remove_dir_all(&staging);
        if let Err(err) = std::fs::create_dir_all(&staging) {
            result.error = Some(format!("could not create staging dir: {err}"));
            result.missing.extend(pending);
            return result;
        }

        // One invocation covers every outstanding target: the -f filters are
        // OR'd, and a single pass over the archives costs far less than one
        // pass per file.
        let mut cmd = Command::new(&self.exe);
        cmd.arg("-U").arg("--upper").arg("-O").arg(&staging);
        for target in &pending {
            cmd.arg(format!("-f=*{}", basename(target)));
        }
        cmd.arg(&self.pcbanks);

        let run = cmd.output();
        let ran = match &run {
            Ok(output) => output.status.success(),
            Err(_) => false,
        };
        if let Err(err) = run {
            let _ = std::fs::remove_dir_all(&staging);
            result.error = Some(format!("extraction failed: {err}"));
            result.missing.extend(pending);
            return result;
        }

        let index = index_by_basename(&staging);
        let mut nowhere = Vec::new();
        for target in pending {
            let Some(source) = resolve(&index, &target) else {
                nowhere.push(target.clone());
                result.missing.push(target);
                continue;
            };
            let dest = self.cache_path(&target);
            if let Some(parent) = dest.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::copy(source, &dest).is_err() {
                // The archives *do* carry it; this machine could not store it.
                // Recording that as absence would make a full disk permanent.
                result.missing.push(target);
                continue;
            }
            result.found.insert(target, dest);
            self.unpacked += 1;
        }

        // Only a run that finished can testify that something is not there.
        // A crashed or refused extractor produces the same empty staging
        // folder as a genuinely absent asset, and believing it would teach the
        // cache that the whole game ships nothing.
        if ran && !nowhere.is_empty() {
            self.remember_absent(&nowhere);
        }

        let _ = std::fs::remove_dir_all(&staging);
        result
    }

    /// Targets this build's archives are known not to carry.
    ///
    /// The cache above can only remember files that exist: a mod that invents
    /// an asset has no vanilla counterpart, so nothing is written, so the next
    /// run finds it outstanding and hgpaktool walks all 31 GB again to fail to
    /// find it a second time. Measured on a real library that was four seconds
    /// of every start-up, spent on eighteen files that were never there -- and
    /// it could not shrink, because the misses are exactly the targets that
    /// can never become hits.
    ///
    /// So absence is recorded as deliberately as presence. The list lives
    /// inside the per-build cache folder, which means a game patch discards it
    /// along with everything else. That is the right lifetime: a patch is the
    /// one thing that can turn a missing asset into a present one.
    fn absent_file(&self) -> PathBuf {
        self.cache_dir.join("_absent.json")
    }

    fn known_absent(&self) -> HashSet<String> {
        super::read_json::<Vec<String>>(&self.absent_file())
            .unwrap_or_default()
            .into_iter()
            .collect()
    }

    fn remember_absent(&self, newly: &[String]) {
        let mut all = self.known_absent();
        all.extend(newly.iter().cloned());
        let mut sorted: Vec<&String> = all.iter().collect();
        sorted.sort();
        let Ok(text) = serde_json::to_string_pretty(&sorted) else {
            return;
        };
        if std::fs::create_dir_all(&self.cache_dir).is_err() {
            return;
        }
        // Same rename-over-the-top as everywhere else: a half-written list
        // read as a whole one would hide assets the game really does ship.
        let scratch = self.cache_dir.join("_absent.writing");
        if std::fs::write(&scratch, text).is_ok() {
            let _ = std::fs::rename(&scratch, self.absent_file());
        } else {
            let _ = std::fs::remove_file(&scratch);
        }
    }

    fn cache_path(&self, target: &str) -> PathBuf {
        let mut path = self.cache_dir.clone();
        for part in target.split('/') {
            path.push(part);
        }
        path
    }
}

/// A short digest that changes whenever the game's archives change.
///
/// Names, sizes and modification times of the `.pak` files identify a game
/// build well enough to invalidate the cache after a patch, without reading
/// 31 GB to hash it.
pub fn build_key(pcbanks: &Path) -> String {
    let Ok(entries) = std::fs::read_dir(pcbanks) else {
        return "unknown".to_string();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.to_uppercase().ends_with(".PAK"))
        .collect();
    names.sort();

    let mut hasher = Sha1::new();
    for name in names {
        let Ok(meta) = std::fs::metadata(pcbanks.join(&name)) else {
            continue;
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        hasher.update(format!("{name}:{}:{mtime}|", meta.len()).as_bytes());
    }
    hasher.digest().to_string()[..16].to_string()
}

fn basename(target: &str) -> String {
    target
        .replace('\\', "/")
        .rsplit('/')
        .next()
        .unwrap_or("")
        .to_string()
}

/// Map each extracted file's basename to the full paths carrying it.
fn index_by_basename(root: &Path) -> HashMap<String, Vec<PathBuf>> {
    let mut index: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Some(name) = path.file_name() {
                index
                    .entry(name.to_string_lossy().to_uppercase())
                    .or_default()
                    .push(path);
            }
        }
    }
    index
}

/// Pick the extracted file that best matches `target`.
///
/// Candidates share a basename; the winner is the one whose path agrees with
/// the target over the most trailing components. That handles both the usual
/// case (prefixes identical) and assets the paks store somewhere else.
pub fn resolve<'a>(
    index: &'a HashMap<String, Vec<PathBuf>>,
    target: &str,
) -> Option<&'a PathBuf> {
    let candidates = index.get(&basename(target).to_uppercase())?;
    if candidates.len() == 1 {
        return candidates.first();
    }
    let wanted: Vec<String> = target.split('/').map(|s| s.to_uppercase()).collect();
    let mut best: Option<&PathBuf> = None;
    let mut best_score = -1i32;
    for path in candidates {
        let parts: Vec<String> = path
            .to_string_lossy()
            .replace('\\', "/")
            .split('/')
            .map(|s| s.to_uppercase())
            .collect();
        let mut score = 0i32;
        for (a, b) in wanted.iter().rev().zip(parts.iter().rev()) {
            if a != b {
                break;
            }
            score += 1;
        }
        if score > best_score {
            best = Some(path);
            best_score = score;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(pairs: &[(&str, &[&str])]) -> HashMap<String, Vec<PathBuf>> {
        pairs
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    v.iter().map(PathBuf::from).collect::<Vec<_>>(),
                )
            })
            .collect()
    }

    #[test]
    fn resolve_prefers_the_longest_matching_suffix() {
        let idx = index(&[(
            "FLOOR0.SCENE.MBIN",
            &[
                r"C:\x\MODELS\OTHER\PARTS\FLOOR0.SCENE.MBIN",
                r"C:\x\MODELS\ROOMS\TELEPOROOM\PARTS\FLOOR0.SCENE.MBIN",
            ],
        )]);
        assert_eq!(
            resolve(&idx, "MODELS/ROOMS/TELEPOROOM/PARTS/FLOOR0.SCENE.MBIN"),
            Some(&PathBuf::from(
                r"C:\x\MODELS\ROOMS\TELEPOROOM\PARTS\FLOOR0.SCENE.MBIN"
            ))
        );
    }

    #[test]
    fn resolve_matches_when_the_prefix_disagrees() {
        // Mods ship globals under GLOBALS/; the paks keep them at the root.
        let idx = index(&[(
            "GCCAMERAGLOBALS.GLOBAL.MBIN",
            &[r"C:\x\GCCAMERAGLOBALS.GLOBAL.MBIN"],
        )]);
        assert_eq!(
            resolve(&idx, "GLOBALS/GCCAMERAGLOBALS.GLOBAL.MBIN"),
            Some(&PathBuf::from(r"C:\x\GCCAMERAGLOBALS.GLOBAL.MBIN"))
        );
    }

    #[test]
    fn resolve_returns_none_when_absent() {
        assert!(resolve(&HashMap::new(), "MODELS/NOPE.SCENE.MBIN").is_none());
    }

    fn source_at(dir: &Path) -> VanillaSource {
        VanillaSource {
            pcbanks: dir.to_path_buf(),
            exe: PathBuf::from("does-not-matter"),
            cache_dir: dir.to_path_buf(),
            unpacked: 0,
            cache_hits: 0,
        }
    }

    #[test]
    fn an_asset_the_game_does_not_ship_is_only_looked_for_once() {
        let dir = std::env::temp_dir().join("nmscheck-test-absent");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = source_at(&dir);

        assert!(source.known_absent().is_empty(), "nothing known yet");
        source.remember_absent(&["MODS/INVENTED.MBIN".to_string()]);
        assert!(source.known_absent().contains("MODS/INVENTED.MBIN"));

        // A second sighting must not lose the first.
        source.remember_absent(&["MODS/OTHER.MBIN".to_string()]);
        let known = source.known_absent();
        assert!(known.contains("MODS/INVENTED.MBIN") && known.contains("MODS/OTHER.MBIN"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_absence_list_is_read_as_nothing_known() {
        // Better to walk the archives again than to hide an asset the game
        // really does ship.
        let dir = std::env::temp_dir().join("nmscheck-test-absent-damaged");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = source_at(&dir);

        std::fs::write(source.absent_file(), "{ not a list").unwrap();
        assert!(source.known_absent().is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_key_is_stable_and_survives_a_missing_folder() {
        let dir = std::env::temp_dir();
        assert_eq!(build_key(&dir), build_key(&dir));
        assert_eq!(build_key(&dir.join("definitely-not-here")), "unknown");
    }

    #[test]
    fn an_explicit_missing_extractor_is_not_silently_replaced() {
        let dir = std::env::temp_dir();
        assert!(VanillaSource::locate(&dir, Some("Z:\\nope\\hgpaktool.exe")).is_none());
    }
}
