//! Canonicalisation of in-game asset paths.
//!
//! No Man's Sky resolves PCBANKS paths case-insensitively, and a mod may ship
//! the same logical asset either as a compiled `.MBIN` or as a decompiled
//! `.EXML`. Everything is reduced to one canonical *target* string so that
//! `Globals/GCGAMEPLAYGLOBALS.GLOBAL.EXML` and
//! `GLOBALS/GCGAMEPLAYGLOBALS.GLOBAL.MBIN` collapse onto the same key and the
//! conflict between them becomes visible.
//!
//! Port of `nmscc/paths.py`. The Python remains the reference implementation;
//! these functions must agree with it exactly.

/// Extensions that represent an overridable game asset. `.DDS` is here because
/// texture mods replace game textures by path just like data assets.
pub const ASSET_EXTS: [&str; 3] = [".MBIN", ".EXML", ".DDS"];

/// Collapse `a/b/../c` and `a/./b` the way `posixpath.normpath` does.
///
/// Only the cases that can appear in a mod-relative path are handled: there is
/// no root to escape to, so a leading `..` is simply kept, matching Python.
fn normpath(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if matches!(parts.last(), Some(&last) if last != "..") {
                    parts.pop();
                } else {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    }
}

/// The canonical target key for a mod-relative asset path.
///
/// Separators are unified, case is folded upward, and the `.EXML` source
/// extension is rewritten to the `.MBIN` the game actually loads.
pub fn normalize_target(rel_path: &str) -> String {
    let upper = rel_path.replace('\\', "/");
    let upper = upper.trim_matches('/').to_uppercase();
    let p = normpath(&upper);
    match p.strip_suffix(".EXML") {
        Some(stem) => format!("{stem}.MBIN"),
        None => p,
    }
}

/// True when `filename` is an asset that can override a game file.
pub fn is_asset(filename: &str) -> bool {
    let upper = filename.to_uppercase();
    ASSET_EXTS.iter().any(|ext| upper.ends_with(ext))
}

/// Final path component of a canonical target.
pub fn target_basename(target: &str) -> &str {
    target.rsplit('/').next().unwrap_or(target)
}

/// Canonicalise a path declared inside an AMUMSS Lua script.
///
/// Scripts are inconsistent: either separator, either extension, and often a
/// bare filename that AMUMSS resolves against unpacked game data. Bare names
/// stay bare so [`declared_matches`] can fall back to basename comparison.
pub fn normalize_declared(decl: &str) -> String {
    normalize_target(decl)
}

/// True when a Lua-declared path refers to `target`.
///
/// A declaration carrying no directory component matches on basename alone.
pub fn declared_matches(declared: &str, target: &str) -> bool {
    if !declared.contains('/') {
        target_basename(target) == declared
    } else {
        declared == target
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exml_and_mbin_collapse_to_one_target() {
        assert_eq!(
            normalize_target(r"Globals\GCGAMEPLAYGLOBALS.GLOBAL.EXML"),
            normalize_target("GLOBALS/GCGAMEPLAYGLOBALS.GLOBAL.MBIN"),
        );
    }

    #[test]
    fn separators_and_case_are_unified() {
        assert_eq!(
            normalize_target(r"metadata\reality\tables\rewardtable.exml"),
            "METADATA/REALITY/TABLES/REWARDTABLE.MBIN",
        );
    }

    #[test]
    fn leading_and_trailing_slashes_are_dropped() {
        assert_eq!(normalize_target("/GLOBALS/X.MBIN/"), "GLOBALS/X.MBIN");
    }

    #[test]
    fn dot_segments_collapse() {
        assert_eq!(normalize_target("A/./B/../C.MBIN"), "A/C.MBIN");
    }

    #[test]
    fn dds_is_an_asset_but_lua_is_not() {
        assert!(is_asset("thing.DDS"));
        assert!(is_asset("thing.mbin"));
        assert!(!is_asset("recipe.lua"));
    }

    #[test]
    fn bare_declaration_matches_on_basename() {
        assert!(declared_matches(
            "GCGAMEPLAYGLOBALS.GLOBAL.MBIN",
            "GLOBALS/GCGAMEPLAYGLOBALS.GLOBAL.MBIN",
        ));
        assert!(!declared_matches(
            "OTHER.MBIN",
            "GLOBALS/GCGAMEPLAYGLOBALS.GLOBAL.MBIN",
        ));
    }

    #[test]
    fn qualified_declaration_must_match_in_full() {
        assert!(declared_matches(
            "GLOBALS/GCGAMEPLAYGLOBALS.GLOBAL.MBIN",
            "GLOBALS/GCGAMEPLAYGLOBALS.GLOBAL.MBIN",
        ));
        assert!(!declared_matches(
            "OTHER/GCGAMEPLAYGLOBALS.GLOBAL.MBIN",
            "GLOBALS/GCGAMEPLAYGLOBALS.GLOBAL.MBIN",
        ));
    }
}
