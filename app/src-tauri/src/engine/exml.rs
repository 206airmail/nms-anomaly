//! Parser for AMUMSS-decompiled `.EXML` assets.
//!
//! An EXML file is a tree of `<Property name="..." value="..."/>` nodes. This
//! flattens that tree into `path -> value` pairs so two mods shipping the same
//! asset can be compared field by field instead of merely "both touch
//! REWARDTABLE".
//!
//! Port of `nmscc/exml.py`. Three things have to match the Python exactly or
//! the analysis silently changes meaning:
//!
//! **Identity-keyed paths.** Naively numbering repeated siblings makes a mod
//! that inserts one row at the top of a table look like it rewrote every row
//! after it. `_id`/`_index` attributes are authoritative, then an `Id`/`Name`
//! child, and only then position.
//!
//! **Change annotations.** AMUMSS marks rewritten lines with a trailing
//! `!# CHANGED`, as bare character data rather than a comment:
//! `<Property name="R" value="0.57" /> !# CHANGED`. Where present this is
//! direct evidence of which mod authored an edit.
//!
//! **Comment handling.** Python's ElementTree discards comments *and
//! concatenates the character data on either side into one `text`/`tail`
//! string*. roxmltree keeps comments as nodes and leaves the text either side
//! separate, so [`text_of`] and [`tail_of`] rejoin them deliberately. Without
//! that, a marker following a comment would be missed here but found by the
//! Python, and the two engines would disagree about who authored a change.

use std::collections::HashMap;
use std::path::Path;

use indexmap::IndexMap;

use super::version::Version;

/// Child property names that identify a row.
///
/// Matched in *document* order, not in this list's order: the first child
/// whose name appears here wins, which is what the Python does.
pub const ID_KEYS: [&str; 11] = [
    "ID",
    "NAME",
    "FILENAME",
    "TABLEID",
    "MISSIONID",
    "KEY",
    "LABELID",
    "PRODUCTID",
    "SUBSTANCEID",
    "TECHNOLOGYID",
    "EVENT",
];

const MARKERS: [&str; 3] = ["CHANGED", "ADDED", "REMOVED"];

#[derive(Debug, Default, Clone)]
pub struct ExmlDoc {
    pub template: Option<String>,
    pub mbinc_version: Option<Version>,
    pub amumss_version: Option<String>,
    /// Flattened `path -> value`, in document order.
    pub props: IndexMap<String, Option<String>>,
    /// `path -> CHANGED | ADDED | REMOVED`
    pub annotations: IndexMap<String, String>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// small scanners, hand-written instead of pulling in a regex engine
// ---------------------------------------------------------------------------

/// First line of a string, or "" for nothing.
fn first_line(text: &str) -> &str {
    text.split('\n').next().unwrap_or("")
}

/// Find an AMUMSS `!# CHANGED`-style marker, case-insensitively.
///
/// Mirrors `!#\s*(CHANGED|ADDED|REMOVED)`: the first occurrence anywhere in
/// the string wins, and the returned keyword is upper-cased.
fn find_marker(line: &str) -> Option<String> {
    let upper = line.to_uppercase();
    let mut from = 0usize;
    while let Some(offset) = upper[from..].find("!#") {
        let after = from + offset + 2;
        let rest = upper[after..].trim_start();
        for marker in MARKERS {
            if rest.starts_with(marker) {
                return Some(marker.to_string());
            }
        }
        from = after;
    }
    None
}

/// Case-insensitive substring search, returning a byte offset into `haystack`.
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    let hay = haystack.to_uppercase();
    let nee = needle.to_uppercase();
    hay.find(&nee)
}

/// `MBINCompiler version (6.34.0.3)` -> `6.34.0.3`
fn scan_mbinc(head: &str) -> Option<&str> {
    let at = find_ci(head, "MBINCompiler version (")?;
    let start = at + "MBINCompiler version (".len();
    let rest = &head[start..];
    let end = rest.find(')')?;
    let value = &rest[..end];
    if value.is_empty() || !value.chars().all(|c| c.is_ascii_digit() || c == '.') {
        return None;
    }
    Some(value)
}

/// `EXML Created by AMUMSS v.5.6.5.0w` -> `5.6.5.0w`
fn scan_amumss(head: &str) -> Option<&str> {
    let at = find_ci(head, "EXML Created by AMUMSS v")?;
    let mut start = at + "EXML Created by AMUMSS v".len();
    if head[start..].starts_with('.') {
        start += 1;
    }
    let rest = &head[start..];
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.'))
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// Strip `<!-- ... -->`, including the ill-formed ones [`repair`] exists for.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("<!--") {
        out.push_str(&rest[..open]);
        match rest[open..].find("-->") {
            Some(close) => rest = &rest[open + close + 3..],
            None => return out, // unterminated: drop the remainder, as the regex does
        }
    }
    out.push_str(rest);
    out
}

/// True when `&` at `rest` begins a real character or entity reference.
fn is_reference(rest: &str) -> bool {
    let body = &rest[1..];
    if let Some(hex) = body.strip_prefix("#x").or_else(|| body.strip_prefix("#X")) {
        let digits: String = hex.chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        return !digits.is_empty() && hex[digits.len()..].starts_with(';');
    }
    if let Some(dec) = body.strip_prefix('#') {
        let digits: String = dec.chars().take_while(|c| c.is_ascii_digit()).collect();
        return !digits.is_empty() && dec[digits.len()..].starts_with(';');
    }
    let mut chars = body.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    let name: String = body.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
    body[name.len()..].starts_with(';')
}

/// Make hand-edited EXML parseable.
///
/// Mod authors comment blocks out and annotate them
/// (`<!--<Property .../> --perfect-->`), but a double hyphen inside a comment
/// is illegal XML and kills a strict parser. Dropping comments wholesale is
/// right here: a commented-out property is disabled, so it must not count as a
/// change. Bare ampersands in mod names are escaped for the same reason.
fn repair(text: &str) -> String {
    let stripped = strip_comments(text);
    let mut out = String::with_capacity(stripped.len());
    let mut idx = 0;
    let bytes = stripped.as_bytes();
    while idx < bytes.len() {
        if bytes[idx] == b'&' && !is_reference(&stripped[idx..]) {
            out.push_str("&amp;");
            idx += 1;
        } else {
            let ch = stripped[idx..].chars().next().unwrap();
            out.push(ch);
            idx += ch.len_utf8();
        }
    }
    out
}

// ---------------------------------------------------------------------------
// ElementTree-compatible text extraction
// ---------------------------------------------------------------------------

/// ElementTree's `.text`: character data after the start tag, up to the first
/// *element* child. Comments are skipped without ending the run, so text on
/// both sides of one is joined.
fn text_of(node: roxmltree::Node) -> String {
    let mut out = String::new();
    for child in node.children() {
        if child.is_element() {
            break;
        }
        if let Some(t) = child.text() {
            if child.is_text() {
                out.push_str(t);
            }
        }
    }
    out
}

/// ElementTree's `.tail`: character data after this element's end tag, up to
/// the next element sibling. Comments are skipped, not treated as a boundary.
fn tail_of(node: roxmltree::Node) -> String {
    let mut out = String::new();
    let mut cursor = node.next_sibling();
    while let Some(sib) = cursor {
        if sib.is_element() {
            break;
        }
        if sib.is_text() {
            if let Some(t) = sib.text() {
                out.push_str(t);
            }
        }
        cursor = sib.next_sibling();
    }
    out
}

/// The AMUMSS marker attached to `node`, if any.
///
/// A container tag carries its marker in `text` (between the opening tag and
/// the first child); a self-closing tag carries it in `tail`. Only the first
/// line of each is considered, so a marker belonging to a later sibling is
/// never mistakenly claimed.
fn annotation(node: roxmltree::Node) -> Option<String> {
    find_marker(first_line(&text_of(node)))
        .or_else(|| find_marker(first_line(&tail_of(node))))
}

/// Stable key for a table row, taken from its Id/Name-ish child.
fn identity(node: roxmltree::Node) -> Option<String> {
    for child in node.children().filter(|c| c.is_element()) {
        let name = child.attribute("name").unwrap_or("").to_uppercase();
        if ID_KEYS.contains(&name.as_str()) {
            if let Some(value) = child.attribute("value") {
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

/// Path segment for one property, keyed as stably as the file allows.
///
/// A sparse patch names the row it edits with an `_id` (or `_index`) attribute
/// — that is how the game itself matches the patch onto the real table — so
/// those attributes are authoritative and are honoured even when the element
/// has no siblings. Ignoring them and numbering positionally instead makes the
/// first row of one patch collide with the unrelated first row of another,
/// which manufactures conflicts that do not exist.
///
/// **Identity beats position.** `_id` and a `Name`/`Id` child both say *which*
/// row this is; `_index` only says *where* it currently sits. A mod that
/// inserts a row shifts every `_index` after it, so keying on position makes
/// every later row look edited. One scene mod adding two nodes to a freighter
/// hangar made 210 untouched nodes read as contested that way. Positional
/// numbering is therefore the last resort, not the second.
fn segment(name: &str, node: roxmltree::Node, position: usize, repeated: bool) -> String {
    if let Some(id) = node.attribute("_id") {
        if !id.is_empty() {
            return format!("{name}[{id}]");
        }
    }
    if let Some(ident) = identity(node) {
        return format!("{name}[{ident}]");
    }
    if let Some(index) = node.attribute("_index") {
        return format!("{name}[{index}]");
    }
    if !repeated {
        return name.to_string();
    }
    format!("{name}[{position}]")
}

fn walk(node: roxmltree::Node, prefix: &str, doc: &mut ExmlDoc) {
    // Group children by their `name`, preserving first-seen order.
    let mut groups: IndexMap<String, Vec<roxmltree::Node>> = IndexMap::new();
    for child in node.children().filter(|c| c.is_element()) {
        if !child.has_tag_name("Property") {
            continue;
        }
        let name = child.attribute("name").unwrap_or("?").to_string();
        groups.entry(name).or_default().push(child);
    }

    for (name, kids) in groups {
        let repeated = kids.len() > 1;
        let mut seen: HashMap<String, usize> = HashMap::new();
        for (position, kid) in kids.into_iter().enumerate() {
            let mut seg = segment(&name, kid, position, repeated);
            // Duplicate keys do occur; disambiguate without losing the key.
            let count = seen.entry(seg.clone()).or_insert(0);
            let n = *count;
            *count += 1;
            if n > 0 {
                seg = format!("{seg}#{n}");
            }

            let path = if prefix.is_empty() {
                seg
            } else {
                format!("{prefix}/{seg}")
            };
            doc.props
                .insert(path.clone(), kid.attribute("value").map(str::to_string));
            if let Some(marker) = annotation(kid) {
                doc.annotations.insert(path.clone(), marker);
            }
            walk(kid, &path, doc);
        }
    }
}

// ---------------------------------------------------------------------------
// public surface
// ---------------------------------------------------------------------------

fn read_text(path: &Path) -> Result<String, std::io::Error> {
    let bytes = std::fs::read(path)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    // A UTF-8 BOM is legal in the file but not in the string handed to a
    // parser; Python's reader strips it, so this does too.
    Ok(text.strip_prefix('\u{feff}').map(str::to_string).unwrap_or(text))
}

/// Pull the MBINCompiler and AMUMSS stamps out of the leading comments.
pub fn read_stamps(path: &Path) -> (Option<Version>, Option<String>) {
    let Ok(text) = read_text(path) else {
        return (None, None);
    };
    // Python reads 1024 *characters* of the decoded stream.
    let head: String = text.chars().take(1024).collect();
    (
        scan_mbinc(&head).and_then(Version::parse),
        scan_amumss(&head).map(str::to_string),
    )
}

/// Flatten the EXML at `path`.
pub fn parse(path: &Path) -> ExmlDoc {
    let mut doc = ExmlDoc::default();
    let (version, amumss) = read_stamps(path);
    doc.mbinc_version = version;
    doc.amumss_version = amumss;

    let text = match read_text(path) {
        Ok(text) => text,
        Err(exc) => {
            doc.error = Some(exc.to_string());
            return doc;
        }
    };

    // Strict parse first; retry on a repaired copy before giving up.
    let repaired;
    let parsed = match roxmltree::Document::parse(&text) {
        Ok(document) => Ok(document),
        Err(_) => {
            repaired = repair(&text);
            roxmltree::Document::parse(&repaired)
        }
    };

    match parsed {
        Ok(document) => {
            let root = document.root_element();
            doc.template = root.attribute("template").map(str::to_string);
            walk(root, "", &mut doc);
        }
        Err(exc) => doc.error = Some(format!("XML parse error: {exc}")),
    }
    doc
}

/// How far apart two float32 values may sit and still count as the same value.
///
/// The game stores these as float32, and a file that has been through a
/// decompile/recompile round trip comes back with a slightly different decimal
/// rendering of the same number: vanilla writes `65.861860` where a re-exported
/// copy writes `65.8618546`.
///
/// Measured on real pairs, the two populations do not overlap anywhere near
/// each other. Round-trip noise ran to **8 ulps** at worst; the smallest
/// genuine edit seen -- a probability moved from `0.35` to `0.36` -- was
/// **335,545 ulps**. Anything in this range is a rendering artefact, and the
/// margin to a real edit is four orders of magnitude.
const FLOAT_TOLERANCE_ULPS: i32 = 64;

/// True when two float32 values are within [`FLOAT_TOLERANCE_ULPS`].
fn within_ulps(a: f32, b: f32) -> bool {
    if a == b {
        return true; // also settles +0.0 against -0.0
    }
    if a.is_nan() || b.is_nan() || a.is_infinite() || b.is_infinite() {
        return false;
    }
    let (ia, ib) = (a.to_bits() as i32, b.to_bits() as i32);
    // Across zero the bit patterns are not comparable; anything that close is
    // already caught by the equality above.
    if (ia < 0) != (ib < 0) {
        return false;
    }
    ia.saturating_sub(ib).saturating_abs() <= FLOAT_TOLERANCE_ULPS
}

/// True when the text is written as a float rather than a whole number.
///
/// Integers are compared exactly. Two counts one apart can land within a few
/// float32 ulps of each other once they pass a million, and silently treating
/// distinct integers as equal is a worse failure than reporting a trivial
/// difference.
fn looks_like_float(text: &str) -> bool {
    text.contains('.') || text.contains('e') || text.contains('E')
}

/// Compare two property values, tolerating float formatting differences.
///
/// `"1.0"` and `"1.000000"` describe the same game value; so, less obviously,
/// do `"65.861860"` and `"65.8618546"`. Reporting either as a conflict would
/// bury the real findings.
pub fn values_equal(left: Option<&str>, right: Option<&str>) -> bool {
    if left == right {
        return true;
    }
    let (Some(l), Some(r)) = (left, right) else {
        return false;
    };
    let (l, r) = (l.trim(), r.trim());
    let (Ok(a), Ok(b)) = (l.parse::<f64>(), r.parse::<f64>()) else {
        return false;
    };
    if a == b {
        return true;
    }
    looks_like_float(l) && looks_like_float(r) && within_ulps(a as f32, b as f32)
}

/// Parse from a string rather than from a file.
///
/// Used wherever a document is already in hand -- a decompiled vanilla copy,
/// or a patch being built -- and by [`super::exmltree`], which checks its own
/// path building against this one on the same document.
pub fn parse_str(xml: &str) -> Result<ExmlDoc, String> {
    let mut doc = ExmlDoc::default();
    let parsed = roxmltree::Document::parse(xml).map_err(|err| err.to_string())?;
    let root = parsed.root_element();
    doc.template = root.attribute("template").map(str::to_string);
    walk(root, "", &mut doc);
    Ok(doc)
}

/// The same, for tests that are entitled to assume their fixture parses.
#[cfg(test)]
pub fn parse_str_for_test(xml: &str) -> ExmlDoc {
    parse_str(xml).expect("valid xml")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sparse patch and the table it edits must share a path.
    ///
    /// Keying a lone row positionally gave it a bare name while the full table
    /// numbered its rows, so the two never met and a real disagreement was
    /// invisible.
    #[test]
    fn a_one_row_patch_compares_against_the_full_table() {
        let row = |id: &str, cost: i32| {
            format!(
                "<Property name=\"Row\" value=\"E\">\
                 <Property name=\"Id\" value=\"{id}\" />\
                 <Property name=\"Cost\" value=\"{cost}\" />\
                 </Property>"
            )
        };
        let doc = |body: String| {
            parse_str(&format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
                 <Data template=\"T\">{body}</Data>"
            ))
            .props
        };
        let patch = doc(row("BETA", 99));
        let full = doc(format!("{}{}", row("ALPHA", 0), row("BETA", 1)));

        assert_eq!(patch.get("Row[BETA]/Cost"), Some(&Some("99".to_string())));
        assert_eq!(full.get("Row[BETA]/Cost"), Some(&Some("1".to_string())));
    }

    /// `_index` is a position, not an identity.
    ///
    /// A scene mod adding two nodes to a freighter hangar made 210 untouched
    /// nodes read as contested, purely because every later `_index` moved.
    #[test]
    fn inserting_a_row_does_not_shift_the_rows_after_it() {
        let scene = |names: &[&str]| {
            let rows: String = names
                .iter()
                .enumerate()
                .map(|(i, n)| {
                    format!(
                        "<Property name=\"Children\" value=\"TkSceneNodeData\" _index=\"{i}\">\
                         <Property name=\"Name\" value=\"{n}\" />\
                         <Property name=\"TransX\" value=\"{i}\" />\
                         </Property>"
                    )
                })
                .collect();
            parse_str(&format!(
                "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
                 <Data template=\"cTkSceneNodeData\">{rows}</Data>"
            ))
            .props
        };
        let before = scene(&["Pad", "Dock"]);
        let after = scene(&["NewTerminal", "Pad", "Dock"]);

        // Dock moved from _index 1 to 2; keyed by name it is still the same row.
        assert!(before.contains_key("Children[Dock]"));
        assert!(after.contains_key("Children[Dock]"));
        assert!(after.contains_key("Children[NewTerminal]"));
        assert!(!before.contains_key("Children[NewTerminal]"));
    }

    fn parse_str(xml: &str) -> ExmlDoc {
        let path = std::env::temp_dir().join(format!(
            "nmscheck-exml-{}-{}.EXML",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&path, xml).unwrap();
        let doc = parse(&path);
        std::fs::remove_file(&path).ok();
        doc
    }

    #[test]
    fn flattens_nested_properties() {
        let doc = parse_str(
            r#"<Data template="GcX">
                 <Property name="A" value="1" />
                 <Property name="B"><Property name="C" value="2" /></Property>
               </Data>"#,
        );
        assert_eq!(doc.template.as_deref(), Some("GcX"));
        assert_eq!(doc.props.get("A"), Some(&Some("1".to_string())));
        assert_eq!(doc.props.get("B/C"), Some(&Some("2".to_string())));
    }

    #[test]
    fn explicit_id_beats_position() {
        let doc = parse_str(
            r#"<Data>
                 <Property name="Table" value="GcT" _id="LAUNCHER">
                   <Property name="Cost" value="5" />
                 </Property>
               </Data>"#,
        );
        assert!(doc.props.contains_key("Table[LAUNCHER]/Cost"));
    }

    #[test]
    fn explicit_index_is_honoured_without_siblings() {
        let doc = parse_str(
            r#"<Data><Property name="Slot" value="GcS" _index="3" /></Data>"#,
        );
        assert!(doc.props.contains_key("Slot[3]"));
    }

    #[test]
    fn repeated_siblings_key_on_identity_not_order() {
        let doc = parse_str(
            r#"<Data>
                 <Property name="Row"><Property name="Id" value="ALPHA" /></Property>
                 <Property name="Row"><Property name="Id" value="BETA" /></Property>
               </Data>"#,
        );
        assert!(doc.props.contains_key("Row[ALPHA]/Id"));
        assert!(doc.props.contains_key("Row[BETA]/Id"));
    }

    #[test]
    fn repeated_siblings_without_identity_fall_back_to_position() {
        let doc = parse_str(
            r#"<Data>
                 <Property name="V" value="1" />
                 <Property name="V" value="2" />
               </Data>"#,
        );
        assert_eq!(doc.props.get("V[0]"), Some(&Some("1".to_string())));
        assert_eq!(doc.props.get("V[1]"), Some(&Some("2".to_string())));
    }

    #[test]
    fn a_lone_property_keeps_a_bare_name() {
        let doc = parse_str(r#"<Data><Property name="Only" value="1" /></Data>"#);
        assert!(doc.props.contains_key("Only"));
    }

    #[test]
    fn amumss_marker_in_a_tail_is_captured() {
        let doc = parse_str(
            "<Data>\n  <Property name=\"R\" value=\"0.57\" /> !# CHANGED\n</Data>",
        );
        assert_eq!(doc.annotations.get("R"), Some(&"CHANGED".to_string()));
    }

    #[test]
    fn amumss_marker_survives_an_intervening_comment() {
        // ElementTree drops the comment and joins the text either side, so a
        // marker after a comment is still found. roxmltree does not do this on
        // its own -- this is the case that proves tail_of() rejoins them.
        let doc = parse_str(
            "<Data>\n  <Property name=\"R\" value=\"0.57\" /> <!--x--> !# CHANGED\n</Data>",
        );
        assert_eq!(doc.annotations.get("R"), Some(&"CHANGED".to_string()));
    }

    #[test]
    fn a_marker_on_the_next_line_is_not_claimed() {
        let doc = parse_str(
            "<Data>\n  <Property name=\"R\" value=\"1\" />\n!# CHANGED\n  <Property name=\"S\" value=\"2\" />\n</Data>",
        );
        assert!(doc.annotations.is_empty());
    }

    #[test]
    fn container_tags_carry_their_marker_in_text() {
        let doc = parse_str(
            "<Data>\n  <Property name=\"B\"> !# ADDED\n    <Property name=\"C\" value=\"2\" />\n  </Property>\n</Data>",
        );
        assert_eq!(doc.annotations.get("B"), Some(&"ADDED".to_string()));
    }

    #[test]
    fn stamps_are_read_from_the_leading_comments() {
        let doc = parse_str(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
             <!--File created using MBINCompiler version (6.34.0.3)-->\n\
             <!--EXML Created by AMUMSS v.5.6.5.0w-->\n\
             <Data><Property name=\"A\" value=\"1\" /></Data>",
        );
        assert_eq!(doc.mbinc_version.unwrap().to_string(), "6.34.0.3");
        assert_eq!(doc.amumss_version.as_deref(), Some("5.6.5.0w"));
    }

    #[test]
    fn illegal_double_hyphen_comment_is_repaired_not_fatal() {
        let doc = parse_str(
            "<Data><!--<Property name=\"X\" value=\"9\"/> --perfect--><Property name=\"A\" value=\"1\" /></Data>",
        );
        assert!(doc.error.is_none(), "{:?}", doc.error);
        assert!(doc.props.contains_key("A"));
        // The commented-out property is disabled, so it must not appear.
        assert!(!doc.props.contains_key("X"));
    }

    #[test]
    fn bare_ampersand_is_repaired() {
        let doc = parse_str(r#"<Data><Property name="N" value="Tom & Jerry" /></Data>"#);
        assert!(doc.error.is_none(), "{:?}", doc.error);
        assert_eq!(doc.props.get("N"), Some(&Some("Tom & Jerry".to_string())));
    }

    #[test]
    fn real_entities_are_left_alone() {
        assert!(is_reference("&amp;"));
        assert!(is_reference("&#65;"));
        assert!(is_reference("&#x41;"));
        assert!(!is_reference("& "));
        assert!(!is_reference("&amp"));
    }

    #[test]
    fn float_formatting_is_not_a_conflict() {
        assert!(values_equal(Some("1.0"), Some("1.000000")));
        assert!(values_equal(Some("True"), Some("True")));
        assert!(!values_equal(Some("1.0"), Some("2.0")));
        assert!(!values_equal(Some("1.0"), None));
        assert!(values_equal(None, None));
    }

    #[test]
    fn a_decompile_round_trip_does_not_manufacture_a_conflict() {
        // Real pairs: vanilla's rendering against a re-exported copy of the
        // same float32. All within a handful of ulps.
        assert!(values_equal(Some("65.861860"), Some("65.8618546")));
        assert!(values_equal(Some("-23.3400269"), Some("-23.340023")));
        assert!(values_equal(Some("59.092850"), Some("59.0928459")));
        assert!(values_equal(Some("1.739448"), Some("1.739447")));
    }

    #[test]
    fn a_real_edit_is_still_a_conflict() {
        // The smallest genuine edit measured was 0.35 -> 0.36, some 335,545
        // ulps away -- four orders of magnitude clear of the noise.
        assert!(!values_equal(Some("0.35"), Some("0.36")));
        assert!(!values_equal(Some("100.000000"), Some("125.000000")));
        assert!(!values_equal(Some("1.000000"), Some("1.300000")));
        assert!(!values_equal(Some("0.0"), Some("0.001")));
    }

    #[test]
    fn whole_numbers_are_compared_exactly() {
        // Past a million, consecutive integers sit a few float32 ulps apart.
        // Quietly equating them would lose a real difference.
        assert!(!values_equal(Some("1000000"), Some("1000001")));
        assert!(!values_equal(Some("48"), Some("49")));
        // A float written against an integer still compares by value.
        assert!(values_equal(Some("1"), Some("1.000000")));
    }

    #[test]
    fn opposite_signs_are_not_equal_however_small() {
        assert!(!values_equal(Some("-0.001"), Some("0.001")));
        // Signed zero is still zero.
        assert!(values_equal(Some("-0.0"), Some("0.0")));
    }
}
