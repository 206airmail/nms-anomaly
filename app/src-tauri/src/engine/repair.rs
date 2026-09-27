//! Mending a mod file the game refuses to read.
//!
//! A malformed `.EXML` is the worst kind of broken, because it is *quiet*: the
//! game gives up on the file and carries on, so the mod simply does nothing
//! and the player is left wondering why. The measured case is `Increased S
//! Class Chance`, whose `INVENTORYTABLE.EXML` opens
//! `<Property name="ClassProbabilityData">` and never closes it -- 33 lines of
//! correct edits that the game throws away over one missing tag.
//!
//! # Only repairs that cannot be wrong
//!
//! This will not attempt a clever reconstruction. It fixes one fault, the one
//! where a file simply *stops* with elements left open, and only when what is
//! there up to that point is well formed. That matters because a repair is
//! applied to a file the user cannot read: if the diagnosis is uncertain, a
//! plausible-looking fix is worse than no fix, since it turns "this mod does
//! nothing" into "this mod does something I did not intend".
//!
//! So [`diagnose`] refuses anything it cannot account for exactly -- a genuine
//! mismatch like `<Property></Data>` in the middle of a file is reported as
//! not repairable, rather than guessed at.
//!
//! # Indentation is evidence, not decoration
//!
//! In the measured file every element after the unclosed one is indented
//! *inside* it, which is what makes appending the close tag the author's
//! evident intent rather than a coin flip. [`Fault::confident`] says whether
//! that evidence is present, so the UI can offer a one-click fix when it is
//! and ask for a look first when it is not.
//!
//! # A repair never edits the original
//!
//! [`repair`] returns text. Writing it is [`super::loadout`]'s business, as a
//! `derived` variant, so the mod as its author shipped it is still there and
//! undoing is switching back rather than restoring from a backup.

use std::path::{Path, PathBuf};

use serde::Serialize;

/// What is wrong with a file, when it is something this module can mend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fault {
    /// the elements left open at the end of the file, outermost first
    pub unclosed: Vec<String>,
    /// the line the file ends on
    pub ends_at: usize,
    /// true when every element after the unclosed one is indented inside it,
    /// so closing it at the end is plainly what the author meant
    pub confident: bool,
    /// what to tell the user, in their terms
    pub summary: String,
}

/// One tag as the scanner sees it.
#[derive(Debug, PartialEq, Eq)]
enum Tag {
    Open {
        name: String,
        line: usize,
        /// how far in this sits, with tabs expanded -- for judging intent
        width: usize,
        /// the literal leading whitespace, so a repair matches the file's own
        /// style instead of converting its tabs to spaces
        indent: String,
    },
    Close {
        name: String,
        line: usize,
    },
    /// `<Property ... />`, and also the declaration and comments.
    ///
    /// Carries its position because it is evidence too: a self-closed element
    /// sitting at the *same* depth as an unclosed one says they were meant to
    /// be siblings, which is the case where a repair should not be presented
    /// as obvious.
    SelfContained { line: usize, width: usize },
}

/// How far a line is indented, counting a tab as reaching the next stop.
///
/// The measured file indents its root's children with two *spaces* and
/// everything below with *tabs*, so counting characters would make the nested
/// elements look no deeper than the element they sit inside, and the evidence
/// that they belong to it would be thrown away.
fn width_of(indent: &str) -> usize {
    const TAB: usize = 4;
    indent.chars().fold(0, |at, c| match c {
        '\t' => at / TAB * TAB + TAB,
        _ => at + 1,
    })
}

/// An element that is opened and never closed, and where that became evident.
#[derive(Debug, Clone)]
struct Gap {
    name: String,
    opened_line: usize,
    width: usize,
    indent: String,
    /// the line of the close tag that revealed it, which is where the missing
    /// tag has to go. `None` when the file simply ended.
    before_line: Option<usize>,
}

/// Pair every tag up, collecting the elements that are never closed.
///
/// `Err` means the file is broken in a way that would have to be guessed at: a
/// close tag matching nothing that is open. That is deliberately not the same
/// as "unclosed", because the two need different answers -- one we can mend
/// exactly, the other we must not touch.
fn gaps(tags: &[Tag]) -> Result<Vec<Gap>, ()> {
    let mut open: Vec<(String, usize, usize, String)> = Vec::new();
    let mut found: Vec<Gap> = Vec::new();

    for tag in tags {
        match tag {
            Tag::Open {
                name,
                line,
                width,
                indent,
            } => open.push((name.clone(), *line, *width, indent.clone())),
            Tag::Close { name, line } => {
                // Does this close anything that is actually open? If it closes
                // an *ancestor*, everything between was never closed -- which
                // is the fault we mend. If it closes nothing, we stop.
                let Some(at) = open.iter().rposition(|(n, _, _, _)| n == name) else {
                    return Err(());
                };
                for (skipped, opened_line, width, indent) in open.drain(at + 1..) {
                    found.push(Gap {
                        name: skipped,
                        opened_line,
                        width,
                        indent,
                        before_line: Some(*line),
                    });
                }
                open.pop();
            }
            Tag::SelfContained { .. } => {}
        }
    }

    // Whatever is still open when the file runs out was never closed either.
    for (name, opened_line, width, indent) in open {
        found.push(Gap {
            name,
            opened_line,
            width,
            indent,
            before_line: None,
        });
    }

    found.sort_by_key(|g| g.opened_line);
    Ok(found)
}

/// Walk the tags without a full XML parse.
///
/// A real parser is no use here: the file does not parse, which is the whole
/// problem. This only needs enough structure to pair up tags and to see how
/// they are indented.
fn tags(xml: &str) -> Result<Vec<Tag>, String> {
    let bytes: Vec<char> = xml.chars().collect();
    let mut out = Vec::new();
    let mut line = 1usize;
    let mut i = 0usize;
    let mut line_start = 0usize;

    while i < bytes.len() {
        match bytes[i] {
            '\n' => {
                line += 1;
                i += 1;
                line_start = i;
            }
            '<' => {
                let open_line = line;
                let indent: String = bytes[line_start..i]
                    .iter()
                    .take_while(|c| c.is_whitespace())
                    .collect();
                // Find the matching '>', counting quotes so a '>' inside an
                // attribute value does not end the tag early.
                let mut j = i + 1;
                let mut quote: Option<char> = None;
                while j < bytes.len() {
                    match bytes[j] {
                        c if Some(c) == quote => quote = None,
                        '"' | '\'' if quote.is_none() => quote = Some(bytes[j]),
                        '>' if quote.is_none() => break,
                        '\n' => line += 1,
                        _ => {}
                    }
                    j += 1;
                }
                if j >= bytes.len() {
                    return Err(format!("a tag opened on line {open_line} is never finished"));
                }
                let body: String = bytes[i + 1..j].iter().collect();
                let trimmed = body.trim();

                if trimmed.starts_with('?') || trimmed.starts_with('!') {
                    out.push(Tag::SelfContained {
                        line: open_line,
                        width: width_of(&indent),
                    });
                } else if let Some(name) = trimmed.strip_prefix('/') {
                    out.push(Tag::Close {
                        name: name.trim().to_string(),
                        line: open_line,
                    });
                } else if trimmed.ends_with('/') {
                    out.push(Tag::SelfContained {
                        line: open_line,
                        width: width_of(&indent),
                    });
                } else {
                    let name = trimmed
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .to_string();
                    out.push(Tag::Open {
                        name,
                        line: open_line,
                        width: width_of(&indent),
                        indent,
                    });
                }
                i = j + 1;
            }
            _ => i += 1,
        }
    }
    Ok(out)
}

/// Say what is wrong, or `None` when this is not a fault we can mend.
///
/// Returns `None` both for a healthy file and for one broken in a way that
/// would have to be guessed at -- the caller cannot tell those apart from here
/// and does not need to, because in both cases there is nothing safe to offer.
pub fn diagnose(xml: &str) -> Option<Fault> {
    let tags = tags(xml).ok()?;
    let gaps = gaps(&tags).ok()?;
    if gaps.is_empty() {
        return None; // nothing left open, so this is not the fault we mend
    }

    // Is every element opened after the gap indented *inside* it? If so,
    // closing it is what the author's own layout says they meant.
    let first = &gaps[0];
    let confident = tags.iter().all(|tag| match tag {
        Tag::Open { line, width, .. } | Tag::SelfContained { line, width } => {
            *line <= first.opened_line || *width > first.width
        }
        Tag::Close { .. } => true,
    });

    let unclosed: Vec<String> = gaps
        .iter()
        .map(|g| format!("<{}> opened on line {}", g.name, g.opened_line))
        .collect();

    let summary = if gaps.len() == 1 {
        format!(
            "one element is never closed: {}. The game gives up on the file there and \
             ignores every edit in it.",
            unclosed[0]
        )
    } else {
        format!(
            "{} elements are never closed. The game gives up on the file there and \
             ignores every edit in it.",
            gaps.len()
        )
    };

    Some(Fault {
        unclosed,
        ends_at: xml.lines().count(),
        confident,
        summary,
    })
}

/// Produce the mended text. Does not write anything.
///
/// The close tags go in immediately before the final element's own close tag,
/// indented to match what they are closing, so the result reads the way the
/// author would have written it.
pub fn repair(xml: &str) -> Result<String, String> {
    let tags_seen = tags(xml)?;
    let gaps = gaps(&tags_seen).map_err(|()| {
        "this file closes a tag that was never opened, so mending it would be guesswork"
            .to_string()
    })?;
    if gaps.is_empty() {
        return Err("nothing here needs mending".into());
    }

    // A gap the file simply ran out on has nothing to anchor a repair to: we
    // would be inventing an end for the document, not restoring a missing tag.
    if let Some(loose) = gaps.iter().find(|g| g.before_line.is_none()) {
        return Err(format!(
            "<{}> on line {} is still open when the file ends, so there is no way to tell \
             where it was meant to close",
            loose.name, loose.opened_line
        ));
    }

    let newline = if xml.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = xml.split(newline).map(str::to_string).collect();

    // Work from the bottom up so that inserting does not shift the lines of
    // the gaps still to be mended.
    let mut by_line: std::collections::BTreeMap<usize, Vec<&Gap>> = std::collections::BTreeMap::new();
    for gap in &gaps {
        by_line.entry(gap.before_line.unwrap()).or_default().push(gap);
    }

    for (line_no, mut here) in by_line.into_iter().rev() {
        // Innermost closes first, which is the reverse of the order they were
        // opened in.
        here.sort_by_key(|g| std::cmp::Reverse(g.opened_line));
        let at = line_no.saturating_sub(1).min(lines.len());
        for (offset, gap) in here.into_iter().enumerate() {
            lines.insert(at + offset, format!("{}</{}>", gap.indent, gap.name));
        }
    }

    Ok(lines.join(newline))
}

/// One file this program would mend, and what is wrong with it.
#[derive(Debug, Clone, Serialize)]
pub struct Mendable {
    /// path relative to the mod's own folder
    pub rel_path: String,
    pub fault: Fault,
}

/// What mending a whole mod did, or would do.
#[derive(Debug, Clone, Serialize)]
pub struct Mended {
    pub owner: String,
    /// the files that were mended
    pub fixed: Vec<Mendable>,
    /// files that are broken but not safely mendable, with the reason
    pub refused: Vec<(String, String)>,
    /// where the mended build was written
    pub dest: String,
}

fn walk(dir: &Path, prefix: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = prefix.join(entry.file_name());
        if path.is_dir() {
            walk(&path, &rel, out);
        } else {
            out.push(rel);
        }
    }
}

fn is_xmlish(rel: &Path) -> bool {
    rel.extension()
        .map(|e| {
            let e = e.to_string_lossy().to_uppercase();
            e == "EXML" || e == "MXML"
        })
        .unwrap_or(false)
}

/// Look at a staged mod and say what could be mended, writing nothing.
pub fn inspect(source: &Path) -> Mended {
    let mut files = Vec::new();
    walk(source, Path::new(""), &mut files);

    let mut fixed = Vec::new();
    let mut refused = Vec::new();
    for rel in files.iter().filter(|r| is_xmlish(r)) {
        let Ok(text) = std::fs::read_to_string(source.join(rel)) else {
            continue;
        };
        // Only offer to touch a file the game actually cannot read. A file
        // that parses is not ours to rewrite, whatever its layout.
        if super::exml::parse_str(&text).is_ok() {
            continue;
        }
        let shown = rel.display().to_string();
        match (diagnose(&text), repair(&text)) {
            (Some(fault), Ok(_)) => fixed.push(Mendable {
                rel_path: shown,
                fault,
            }),
            (_, Err(why)) => refused.push((shown, why)),
            (None, Ok(_)) => {}
        }
    }

    Mended {
        owner: source
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        fixed,
        refused,
        dest: String::new(),
    }
}

/// Write a mended build of `source` into `dest`, leaving `source` untouched.
///
/// The whole mod is copied, not just the broken file, because the result has
/// to be a complete mod that can be deployed on its own -- that is what makes
/// switching back to the original a switch rather than a restore.
pub fn mend_into(source: &Path, dest: &Path) -> Result<Mended, String> {
    let found = inspect(source);
    if found.fixed.is_empty() {
        return Err(if found.refused.is_empty() {
            "nothing in this mod needs mending".to_string()
        } else {
            format!(
                "this mod is broken in a way that cannot be mended safely: {}",
                found.refused[0].1
            )
        });
    }

    if dest.exists() {
        std::fs::remove_dir_all(dest)
            .map_err(|e| format!("could not clear {}: {e}", dest.display()))?;
    }

    let mut files = Vec::new();
    walk(source, Path::new(""), &mut files);
    let mending: std::collections::BTreeSet<&str> =
        found.fixed.iter().map(|f| f.rel_path.as_str()).collect();

    for rel in &files {
        let to = dest.join(rel);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        if mending.contains(rel.display().to_string().as_str()) {
            let text = std::fs::read_to_string(source.join(rel)).map_err(|e| e.to_string())?;
            let fixed = repair(&text)?;
            std::fs::write(&to, fixed)
                .map_err(|e| format!("could not write {}: {e}", to.display()))?;
        } else {
            std::fs::copy(source.join(rel), &to)
                .map_err(|e| format!("could not copy {}: {e}", rel.display()))?;
        }
    }

    Ok(Mended {
        dest: dest.display().to_string(),
        ..found
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real file, reduced but with the fault and the layout intact.
    const REAL: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Data template="GcInventoryTable">
  <Property name="ClassProbabilityData">
		<Property name="Poor" value="GcInventoryClassProbabilities">
			<Property name="ClassProbabilities">
				<Property name="C" value="25.000000" />
				<Property name="S" value="25.000000" />
			</Property>
		</Property>
		<Property name="Wealthy" value="GcInventoryClassProbabilities">
			<Property name="ClassProbabilities">
				<Property name="S" value="50.000000" />
			</Property>
		</Property>
</Data>"#;

    #[test]
    fn the_real_faulty_file_is_diagnosed_exactly() {
        let fault = diagnose(REAL).expect("should be repairable");
        assert_eq!(fault.unclosed.len(), 1);
        assert!(
            fault.unclosed[0].contains("ClassProbabilityData") || fault.unclosed[0].contains("<Property>"),
            "{:?}",
            fault.unclosed
        );
        assert!(
            fault.confident,
            "everything after it is indented inside it, so the intent is plain"
        );
    }

    #[test]
    fn repairing_the_real_file_makes_it_parse() {
        let fixed = repair(REAL).unwrap();
        assert!(
            crate::engine::exml::parse_str(&fixed).is_ok(),
            "the repaired file still does not parse:\n{fixed}"
        );
        // And it is now genuinely balanced, not merely accepted.
        assert!(diagnose(&fixed).is_none(), "still reports a fault");
    }

    #[test]
    fn the_repair_keeps_every_value_the_author_wrote() {
        let fixed = repair(REAL).unwrap();
        for kept in ["25.000000", "50.000000", "ClassProbabilityData", "Wealthy"] {
            assert!(fixed.contains(kept), "{kept} was lost");
        }
        // Exactly one line added: the tag that was missing.
        assert_eq!(fixed.lines().count(), REAL.lines().count() + 1);
    }

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let path = std::env::temp_dir().join(format!("nmscheck_repair_{tag}"));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            Dir(path)
        }
        fn file(&self, rel: &str, body: &str) {
            let path = self.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn mending_a_mod_leaves_the_original_exactly_as_it_was() {
        // The point of writing a second build: undoing is switching back, and
        // switching back cannot fail because nothing was ever overwritten.
        let dir = Dir::new("mend");
        dir.file("staging/M/METADATA/T.EXML", REAL);
        dir.file("staging/M/TEXTURES/A.DDS", "not xml");
        let source = dir.0.join("staging/M");
        let dest = dir.0.join("derived/M");

        let done = mend_into(&source, &dest).unwrap();
        assert_eq!(done.fixed.len(), 1);
        assert_eq!(done.fixed[0].rel_path, "METADATA\\T.EXML".replace('\\', std::path::MAIN_SEPARATOR_STR));

        // The mended build is whole: the untouched file came along too.
        assert!(dest.join("TEXTURES/A.DDS").is_file());
        let fixed = std::fs::read_to_string(dest.join("METADATA/T.EXML")).unwrap();
        assert!(crate::engine::exml::parse_str(&fixed).is_ok());

        // And the original is byte-for-byte what the author shipped.
        assert_eq!(
            std::fs::read_to_string(source.join("METADATA/T.EXML")).unwrap(),
            REAL
        );
    }

    #[test]
    fn a_mod_with_nothing_wrong_is_not_given_a_second_build() {
        let dir = Dir::new("healthy");
        dir.file(
            "staging/M/METADATA/T.EXML",
            "<Data template=\"T\">\n  <Property name=\"A\" value=\"1\" />\n</Data>",
        );
        let err = mend_into(&dir.0.join("staging/M"), &dir.0.join("derived/M")).unwrap_err();
        assert!(err.contains("nothing in this mod needs mending"), "{err}");
        assert!(!dir.0.join("derived/M").exists(), "wrote a build anyway");
    }

    #[test]
    fn a_file_that_parses_is_never_rewritten_however_it_is_laid_out() {
        let dir = Dir::new("leavealone");
        // Ugly but valid. Not ours to tidy.
        dir.file("staging/M/A.EXML", "<Data template=\"T\"><Property name=\"A\" value=\"1\"/></Data>");
        assert!(inspect(&dir.0.join("staging/M")).fixed.is_empty());
    }

    #[test]
    fn a_mod_broken_beyond_repair_says_so_rather_than_writing_a_guess() {
        let dir = Dir::new("hopeless");
        dir.file(
            "staging/M/A.EXML",
            "<Data template=\"T\">\n  <Property name=\"A\">\n  </Other>\n</Data>",
        );
        let found = inspect(&dir.0.join("staging/M"));
        assert!(found.fixed.is_empty());
        assert_eq!(found.refused.len(), 1);
        assert!(mend_into(&dir.0.join("staging/M"), &dir.0.join("derived/M")).is_err());
    }

    #[test]
    fn a_healthy_file_is_not_offered_a_fix() {
        let good = r#"<?xml version="1.0" encoding="utf-8"?>
<Data template="GcInventoryTable">
  <Property name="A" value="1" />
</Data>"#;
        assert_eq!(diagnose(good), None);
    }

    #[test]
    fn a_mismatched_tag_in_the_middle_is_refused_rather_than_guessed_at() {
        // This is broken in a way that could be mended several different ways.
        // Offering a confident-looking fix would turn "does nothing" into
        // "does something the author never wrote".
        let muddled = r#"<Data template="T">
  <Property name="A">
    <Property name="B" value="1" />
  </Other>
</Data>"#;
        assert_eq!(diagnose(muddled), None);
        assert!(repair(muddled).is_err());
    }

    #[test]
    fn a_stray_closing_tag_is_refused() {
        let stray = r#"<Data template="T">
  </Property>
</Data>"#;
        assert_eq!(diagnose(stray), None);
    }

    #[test]
    fn an_attribute_containing_an_angle_bracket_does_not_end_the_tag() {
        let tricky = r#"<Data template="T">
  <Property name="Expr" value="a > b" />
  <Property name="Open">
</Data>"#;
        let fault = diagnose(tricky).expect("the unclosed Open is still the fault");
        assert_eq!(fault.unclosed.len(), 1);
        assert!(fault.unclosed[0].contains("line 3"), "{:?}", fault.unclosed);
    }

    #[test]
    fn several_unclosed_elements_are_all_closed_innermost_first() {
        let deep = r#"<Data template="T">
  <Property name="Outer">
    <Property name="Inner">
      <Property name="V" value="1" />
</Data>"#;
        let fault = diagnose(deep).unwrap();
        assert_eq!(fault.unclosed.len(), 2);

        let fixed = repair(deep).unwrap();
        assert!(crate::engine::exml::parse_str(&fixed).is_ok(), "{fixed}");
        assert!(diagnose(&fixed).is_none());
    }

    #[test]
    fn a_file_whose_root_never_closes_is_not_mended_quietly() {
        // Nothing to anchor the repair to, so say so rather than inventing an
        // end for the document.
        let headless = r#"<Data template="T">
  <Property name="A" value="1" />"#;
        assert!(repair(headless).is_err());
    }

    #[test]
    fn windows_line_endings_survive_the_repair() {
        let crlf = "<Data template=\"T\">\r\n  <Property name=\"A\">\r\n</Data>";
        let fixed = repair(crlf).unwrap();
        assert!(fixed.contains("\r\n"), "line endings were rewritten");
        assert!(!fixed.contains("\n\n"), "a stray bare newline crept in");
        assert!(diagnose(&fixed).is_none());
    }

    #[test]
    fn a_fix_that_contradicts_the_layout_is_flagged_as_less_certain() {
        // Here the later element is *not* indented inside the unclosed one, so
        // whether it belongs inside it is a judgement the user should make.
        let flat = r#"<Data template="T">
  <Property name="Open">
  <Property name="Sibling" value="1" />
</Data>"#;
        let fault = diagnose(flat).unwrap();
        assert!(!fault.confident, "should not claim the intent is obvious");
    }
}
