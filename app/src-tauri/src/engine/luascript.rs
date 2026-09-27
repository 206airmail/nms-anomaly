//! Extraction of declared targets from AMUMSS build scripts.
//!
//! An AMUMSS `.lua` recipe names every asset it edits via `MBIN_FILE_SOURCE`.
//! Reading those declarations catches a class of conflict the built files
//! miss: a mod whose recipe is present but whose output has not been generated
//! still intends to rewrite that asset, and two recipes aiming at one asset
//! collide the moment either is built.
//!
//! Declarations appear in several shapes, all handled here: a plain quoted
//! path, a bare filename with no directory component, and a braced list of
//! paths. Bare names are kept bare; [`super::paths::declared_matches`]
//! resolves them by basename, which is what AMUMSS does against the unpacked
//! game data.
//!
//! Port of `nmscc/luascript.py`.

use std::collections::BTreeSet;
use std::path::Path;

use super::paths::normalize_declared;

/// Strip whole-line Lua comments.
///
/// A commented-out recipe line must not contribute declarations. Mirrors the
/// Python `(?m)^\s*--.*$`: only lines whose first non-space characters are
/// `--` are removed, so a `--` inside a string on a live line is left alone.
fn strip_comment_lines(source: &str) -> String {
    source
        .lines()
        .map(|line| if line.trim_start().starts_with("--") { "" } else { line })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `"..."` string inside `blob`.
fn quoted_strings(blob: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = blob;
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        match after.find('"') {
            Some(close) => {
                if close > 0 {
                    out.push(&after[..close]);
                }
                rest = &after[close + 1..];
            }
            None => break,
        }
    }
    out
}

/// Find each `["MBIN_FILE_SOURCE"] = <value>` and return the value blob.
///
/// The value is either a quoted string or a `{...}` list. Mirrors the Python
/// regex, including its tolerance of whitespace around the brackets and the
/// equals sign.
fn declaration_blobs(source: &str) -> Vec<&str> {
    const KEY: &str = "\"MBIN_FILE_SOURCE\"";
    let upper = source.to_uppercase();
    let mut blobs = Vec::new();
    let mut from = 0usize;

    while let Some(offset) = upper[from..].find(KEY) {
        let key_start = from + offset;
        let mut cursor = key_start + KEY.len();
        from = cursor;

        // The key must be bracketed: `[ "MBIN_FILE_SOURCE" ]`.
        let before = source[..key_start].trim_end();
        if !before.ends_with('[') {
            continue;
        }

        let rest = &source[cursor..];
        let trimmed = rest.trim_start();
        if !trimmed.starts_with(']') {
            continue;
        }
        cursor += rest.len() - trimmed.len() + 1;

        let rest = &source[cursor..];
        let trimmed = rest.trim_start();
        if !trimmed.starts_with('=') {
            continue;
        }
        cursor += rest.len() - trimmed.len() + 1;

        let rest = &source[cursor..];
        let trimmed = rest.trim_start();
        let value_start = cursor + (rest.len() - trimmed.len());

        if trimmed.starts_with('{') {
            // `[^}]*}` then an optional second `}`: the Python deliberately
            // stops at the first closing brace rather than balancing them.
            if let Some(close) = trimmed.find('}') {
                let mut end = value_start + close + 1;
                let after = &source[end..];
                let after_trimmed = after.trim_start();
                if after_trimmed.starts_with('}') {
                    end += after.len() - after_trimmed.len() + 1;
                }
                blobs.push(&source[value_start..end]);
                from = end;
            }
        } else if trimmed.starts_with('"') {
            if let Some(close) = trimmed[1..].find('"') {
                let end = value_start + 1 + close + 1;
                blobs.push(&source[value_start..end]);
                from = end;
            }
        }
    }
    blobs
}

/// The canonical targets declared by the Lua script at `path`.
pub fn declared_targets(path: &Path) -> BTreeSet<String> {
    let Ok(bytes) = std::fs::read(path) else {
        return BTreeSet::new();
    };
    let source = String::from_utf8_lossy(&bytes).into_owned();
    let source = strip_comment_lines(&source);

    let mut targets = BTreeSet::new();
    for blob in declaration_blobs(&source) {
        for raw in quoted_strings(blob) {
            let raw = raw.trim();
            // Recipes sometimes use wildcards; those cannot be resolved here.
            if raw.is_empty() || raw.contains('*') {
                continue;
            }
            let upper = raw.to_uppercase();
            if !(upper.ends_with(".MBIN") || upper.ends_with(".EXML")) {
                continue;
            }
            targets.insert(normalize_declared(raw));
        }
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn declared(source: &str) -> BTreeSet<String> {
        let path = std::env::temp_dir().join(format!(
            "nmscheck-lua-{}-{}.lua",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, source).unwrap();
        let out = declared_targets(&path);
        std::fs::remove_file(&path).ok();
        out
    }

    #[test]
    fn plain_quoted_declaration() {
        let out = declared(r#"["MBIN_FILE_SOURCE"] = "GLOBALS\\GCGAMEPLAYGLOBALS.GLOBAL.MBIN""#);
        assert!(out.contains("GLOBALS/GCGAMEPLAYGLOBALS.GLOBAL.MBIN"), "{out:?}");
    }

    #[test]
    fn braced_list_declaration() {
        let out = declared(
            r#"["MBIN_FILE_SOURCE"] = {"A/ONE.MBIN", "B/TWO.EXML"}"#,
        );
        assert!(out.contains("A/ONE.MBIN"));
        // .EXML declarations canonicalise to the .MBIN the game loads.
        assert!(out.contains("B/TWO.MBIN"));
    }

    #[test]
    fn bare_filename_stays_bare() {
        let out = declared(r#"["MBIN_FILE_SOURCE"] = "GCGAMEPLAYGLOBALS.GLOBAL.MBIN""#);
        assert!(out.contains("GCGAMEPLAYGLOBALS.GLOBAL.MBIN"));
    }

    #[test]
    fn commented_out_lines_declare_nothing() {
        let out = declared("-- [\"MBIN_FILE_SOURCE\"] = \"A/ONE.MBIN\"\n");
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn wildcards_are_skipped() {
        let out = declared(r#"["MBIN_FILE_SOURCE"] = "MODELS/*.MBIN""#);
        assert!(out.is_empty());
    }

    #[test]
    fn non_asset_extensions_are_skipped() {
        let out = declared(r#"["MBIN_FILE_SOURCE"] = "NOTES.txt""#);
        assert!(out.is_empty());
    }

    #[test]
    fn several_declarations_in_one_file() {
        let out = declared(
            "[\"MBIN_FILE_SOURCE\"] = \"A/ONE.MBIN\"\n\
             something else\n\
             [\"MBIN_FILE_SOURCE\"] = {\"B/TWO.MBIN\"}\n",
        );
        assert_eq!(out.len(), 2, "{out:?}");
    }

    #[test]
    fn case_insensitive_key() {
        let out = declared(r#"["mbin_file_source"] = "A/ONE.MBIN""#);
        assert!(out.contains("A/ONE.MBIN"));
    }

    #[test]
    fn a_missing_file_yields_nothing() {
        assert!(declared_targets(Path::new("no-such-recipe.lua")).is_empty());
    }
}
