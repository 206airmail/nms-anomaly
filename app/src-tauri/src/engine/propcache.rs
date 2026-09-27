//! Parse each EXML once, ever.
//!
//! Flattening an EXML into `path -> value` is the single most expensive thing
//! this program does. Against a 62-mod library it is 2.1 million properties
//! drawn out of 175 MB of XML, and it happened on every start-up: reading the
//! mods folder alone spent four seconds there, and comparing conflicts against
//! the game's own copies spent nearly two more.
//!
//! None of that work depends on anything but the bytes of the file, and every
//! caller already knows those bytes' hash -- [`super::discovery`] because it
//! just hashed the file, [`super::merge`] and [`super::decompile`] because the
//! decompiler's own cache is keyed on it. So the flattened form is kept beside
//! the hash and the XML is never looked at twice.
//!
//! **Why not JSON.** The obvious `serde_json` round trip is exactly as fast as
//! what is here, and produced 373 MB of cache for 172 MB of source: these keys
//! are long, deeply repetitive property paths, and writing each one out in full
//! costs more than the values do. Front-coding them -- each key stored as
//! "share N bytes with the previous one, then these" -- gives the same 3.6x
//! and 52 MB, which is smaller than the XML it replaces. Measured, both of
//! them, before this was written.
//!
//! A damaged or truncated entry reads back as a miss, never as a document with
//! something missing from it: every length is checked against what is actually
//! there, and anything that does not add up returns `None` so the caller parses
//! the XML as it would have anyway.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;

use super::exml::{self, ExmlDoc};
use super::version::Version;

/// Bumped whenever a cached entry would no longer be what parsing produces.
///
/// It covers the layout below *and* the behaviour of [`exml::parse`]: the hash
/// in the key identifies the file, and nothing identifies the code that read
/// it. Change what flattening produces without touching this and every library
/// already scanned keeps serving the old answer.
const FORMAT: u8 = 1;

const MAGIC: &[u8] = b"NMSPROP";

fn cache_dir() -> PathBuf {
    super::tools::cache_root().join("prop-cache")
}

// ---------------------------------------------------------------------------
// varints, and a reader that cannot run off the end
// ---------------------------------------------------------------------------

fn put_uint(out: &mut Vec<u8>, mut value: usize) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn put_text(out: &mut Vec<u8>, text: &str) {
    put_uint(out, text.len());
    out.extend_from_slice(text.as_bytes());
}

/// `None` and `Some("")` are different answers, so the length carries the
/// distinction: 0 for absent, otherwise the length plus one.
fn put_optional(out: &mut Vec<u8>, text: Option<&str>) {
    match text {
        None => put_uint(out, 0),
        Some(value) => {
            put_uint(out, value.len() + 1);
            out.extend_from_slice(value.as_bytes());
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn uint(&mut self) -> Option<usize> {
        let mut value = 0usize;
        let mut shift = 0u32;
        loop {
            let byte = *self.bytes.get(self.at)?;
            self.at += 1;
            // A length that cannot be represented is a corrupt entry, not a
            // very large file: give up rather than wrap around into a small
            // number and read the wrong bytes.
            value |= ((byte & 0x7f) as usize).checked_shl(shift)?;
            if byte & 0x80 == 0 {
                return Some(value);
            }
            shift += 7;
            if shift > 63 {
                return None;
            }
        }
    }

    fn take(&mut self, len: usize) -> Option<&'a str> {
        let end = self.at.checked_add(len)?;
        let slice = self.bytes.get(self.at..end)?;
        self.at = end;
        std::str::from_utf8(slice).ok()
    }

    fn optional(&mut self) -> Option<Option<String>> {
        match self.uint()? {
            0 => Some(None),
            len => self.take(len - 1).map(|text| Some(text.to_string())),
        }
    }
}

// ---------------------------------------------------------------------------
// the front-coded map
// ---------------------------------------------------------------------------

fn put_entries<'a>(out: &mut Vec<u8>, entries: impl ExactSizeIterator<Item = (&'a str, Option<&'a str>)>) {
    put_uint(out, entries.len());
    let mut previous = String::new();
    for (key, value) in entries {
        // Only whole characters may be shared, or the two halves of a key
        // would not be valid UTF-8 on their own and could not be read back.
        let shared = key
            .as_bytes()
            .iter()
            .zip(previous.as_bytes())
            .take_while(|(a, b)| a == b)
            .count();
        let shared = (0..=shared)
            .rev()
            .find(|n| key.is_char_boundary(*n))
            .unwrap_or(0);
        put_uint(out, shared);
        put_text(out, &key[shared..]);
        put_optional(out, value);
        previous.clear();
        previous.push_str(key);
    }
}

fn take_entries(reader: &mut Reader) -> Option<Vec<(String, Option<String>)>> {
    let count = reader.uint()?;
    // The count comes out of the file, so it cannot be trusted to size an
    // allocation: two bytes of rubbish would ask for gigabytes. One entry is
    // never smaller than three bytes, so that is the honest ceiling.
    let capacity = count.min(reader.bytes.len().saturating_sub(reader.at) / 3 + 1);
    let mut out: Vec<(String, Option<String>)> = Vec::with_capacity(capacity);
    let mut previous = String::new();
    for _ in 0..count {
        let shared = reader.uint()?;
        if shared > previous.len() || !previous.is_char_boundary(shared) {
            return None;
        }
        let rest_len = reader.uint()?;
        let rest = reader.take(rest_len)?;
        let mut key = String::with_capacity(shared + rest.len());
        key.push_str(&previous[..shared]);
        key.push_str(rest);
        let value = reader.optional()?;
        previous.clear();
        previous.push_str(&key);
        out.push((key, value));
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// the document
// ---------------------------------------------------------------------------

fn encode(doc: &ExmlDoc) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(MAGIC);
    out.push(FORMAT);

    put_optional(&mut out, doc.template.as_deref());
    match &doc.mbinc_version {
        None => out.push(0),
        Some(version) => {
            out.push(1);
            for part in [version.major, version.minor, version.patch, version.build] {
                put_uint(&mut out, part as usize);
            }
        }
    }
    put_optional(&mut out, doc.amumss_version.as_deref());
    put_optional(&mut out, doc.error.as_deref());

    put_entries(
        &mut out,
        doc.annotations.iter().map(|(k, v)| (k.as_str(), Some(v.as_str()))),
    );
    put_entries(
        &mut out,
        doc.props.iter().map(|(k, v)| (k.as_str(), v.as_deref())),
    );
    out
}

fn decode(bytes: &[u8]) -> Option<ExmlDoc> {
    if bytes.len() < MAGIC.len() + 1 || &bytes[..MAGIC.len()] != MAGIC {
        return None;
    }
    if bytes[MAGIC.len()] != FORMAT {
        return None;
    }
    let mut reader = Reader {
        bytes,
        at: MAGIC.len() + 1,
    };

    let template = reader.optional()?;
    let mbinc_version = match *reader.bytes.get(reader.at)? {
        0 => {
            reader.at += 1;
            None
        }
        1 => {
            reader.at += 1;
            let mut parts = [0u32; 4];
            for part in parts.iter_mut() {
                *part = u32::try_from(reader.uint()?).ok()?;
            }
            Some(Version::new(parts[0], parts[1], parts[2], parts[3]))
        }
        _ => return None,
    };
    let amumss_version = reader.optional()?;
    let error = reader.optional()?;

    let annotations: IndexMap<String, String> = take_entries(&mut reader)?
        .into_iter()
        .map(|(key, value)| (key, value.unwrap_or_default()))
        .collect();
    let props: IndexMap<String, Option<String>> = take_entries(&mut reader)?.into_iter().collect();

    // Trailing bytes mean this is not the document it claims to be.
    if reader.at != bytes.len() {
        return None;
    }

    Some(ExmlDoc {
        template,
        mbinc_version,
        amumss_version,
        props,
        annotations,
        error,
    })
}

// ---------------------------------------------------------------------------
// what callers use
// ---------------------------------------------------------------------------

/// [`exml::parse`], but done at most once per distinct file.
///
/// `identity` is the content hash of `path`. An empty one means the caller does
/// not know it, and the XML is simply parsed -- a cache keyed on nothing would
/// hand one file's properties to another.
pub fn parse(path: &Path, identity: &str) -> ExmlDoc {
    if identity.is_empty() {
        return exml::parse(path);
    }
    let file = cache_dir().join(format!("{identity}.props"));
    if let Some(doc) = std::fs::read(&file).ok().as_deref().and_then(decode) {
        return doc;
    }

    let doc = exml::parse(path);
    store(&file, &encode(&doc));
    doc
}

/// Keep an entry, or quietly do not. A cache that will not write is a slow
/// run; a run that fails because its cache will not write is a broken one.
fn store(file: &Path, bytes: &[u8]) {
    let Some(dir) = file.parent() else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    // Write under another name and rename over it, so an interrupted run
    // leaves either the previous entry or none -- never half of one, which is
    // the only way a reader could see a document with properties missing.
    let scratch = file.with_extension("writing");
    if std::fs::write(&scratch, bytes).is_ok() {
        let _ = std::fs::rename(&scratch, file);
    } else {
        let _ = std::fs::remove_file(&scratch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(props: &[(&str, Option<&str>)]) -> ExmlDoc {
        ExmlDoc {
            template: Some("cGcTable".into()),
            mbinc_version: Some(Version::new(7, 3, 2, 2)),
            amumss_version: Some("4.1.0".into()),
            props: props
                .iter()
                .map(|(k, v)| (k.to_string(), v.map(str::to_string)))
                .collect(),
            annotations: IndexMap::new(),
            error: None,
        }
    }

    fn roundtrip(original: &ExmlDoc) -> ExmlDoc {
        decode(&encode(original)).expect("its own output must read back")
    }

    #[test]
    fn a_document_comes_back_exactly_as_it_went_in() {
        let original = doc(&[
            ("Table/Row[ALPHA]/Cost", Some("1")),
            ("Table/Row[ALPHA]/Name", Some("Alpha")),
            ("Table/Row[BETA]/Cost", Some("2")),
        ]);
        let back = roundtrip(&original);

        assert_eq!(back.template, original.template);
        assert_eq!(back.mbinc_version, original.mbinc_version);
        assert_eq!(back.amumss_version, original.amumss_version);
        assert_eq!(
            back.props.iter().collect::<Vec<_>>(),
            original.props.iter().collect::<Vec<_>>(),
            "order is part of the document"
        );
    }

    #[test]
    fn an_absent_value_stays_different_from_an_empty_one() {
        // A container has no value; a property set to "" has one. Conflating
        // them would invent an edit, or hide one.
        let original = doc(&[("Table/Row", None), ("Table/Row/Name", Some(""))]);
        let back = roundtrip(&original);
        assert_eq!(back.props["Table/Row"], None);
        assert_eq!(back.props["Table/Row/Name"], Some(String::new()));
    }

    #[test]
    fn annotations_and_an_error_survive_too() {
        let mut original = doc(&[("A", Some("1"))]);
        original.annotations.insert("A".into(), "CHANGED".into());
        original.error = Some("XML parse error: whatever".into());
        original.template = None;
        original.mbinc_version = None;
        original.amumss_version = None;

        let back = roundtrip(&original);
        assert_eq!(back.annotations["A"], "CHANGED");
        assert_eq!(back.error.as_deref(), Some("XML parse error: whatever"));
        assert!(back.template.is_none() && back.mbinc_version.is_none());
    }

    #[test]
    fn keys_that_share_no_prefix_are_still_exact() {
        // Front-coding is only correct if a key that shares nothing with the
        // one before it is written out whole.
        let original = doc(&[("zzz/one", Some("1")), ("aaa/two", Some("2"))]);
        let back = roundtrip(&original);
        assert_eq!(back.props.keys().collect::<Vec<_>>(), vec!["zzz/one", "aaa/two"]);
    }

    #[test]
    fn multi_byte_characters_survive_the_shared_prefix() {
        // Sharing a prefix by bytes could split a character in half, and the
        // two halves are not valid UTF-8 on their own.
        let original = doc(&[
            ("Name/Ünterwegs/A", Some("é")),
            ("Name/Ünterwegs/B", Some("日本語")),
        ]);
        let back = roundtrip(&original);
        assert_eq!(back.props["Name/Ünterwegs/B"], Some("日本語".to_string()));
        assert_eq!(back.props.len(), 2);
    }

    #[test]
    fn an_empty_document_round_trips() {
        let back = roundtrip(&ExmlDoc::default());
        assert!(back.props.is_empty() && back.template.is_none());
    }

    #[test]
    fn damage_reads_as_a_miss_rather_than_as_a_short_document() {
        // Every one of these must return None. A truncated entry that parsed
        // into "a document with fewer properties" would look exactly like a
        // mod that edits less than it does.
        let whole = encode(&doc(&[
            ("Table/Row[ALPHA]/Cost", Some("1")),
            ("Table/Row[BETA]/Cost", Some("2")),
        ]));
        for cut in 0..whole.len() {
            assert!(decode(&whole[..cut]).is_none(), "truncated at {cut}");
        }
        let mut trailing = whole.clone();
        trailing.push(0);
        assert!(decode(&trailing).is_none(), "trailing rubbish");

        let mut wrong_magic = whole.clone();
        wrong_magic[0] = b'X';
        assert!(decode(&wrong_magic).is_none());

        let mut wrong_format = whole.clone();
        wrong_format[MAGIC.len()] = FORMAT.wrapping_add(1);
        assert!(
            decode(&wrong_format).is_none(),
            "an entry from an older format is not read"
        );

        assert!(decode(&[]).is_none());
        assert!(decode(b"NMSPROP").is_none());
    }

    #[test]
    fn rubbish_does_not_ask_for_an_enormous_allocation() {
        // The property count comes out of the file. A few bytes of noise must
        // not turn into a request for gigabytes before it fails.
        let mut blob = Vec::from(MAGIC);
        blob.push(FORMAT);
        blob.extend_from_slice(&[0, 0, 0, 0, 0]); // no template, version, amumss, error, annotations
        blob.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x7f]); // an absurd property count
        assert!(decode(&blob).is_none());
    }

    #[test]
    fn editing_a_file_is_never_served_the_old_properties() {
        // The whole invalidation story, since there is no other: the key is
        // the content, so an updated mod simply asks a question this cache has
        // not been asked before. Nothing has to notice the change.
        let dir = std::env::temp_dir().join("nmscheck-test-propcache-update");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("thing.EXML");

        let write = |body: &str| {
            std::fs::write(&path, format!("<Data template=\"cGcTable\">{body}</Data>")).unwrap();
            let bytes = std::fs::read(&path).unwrap();
            let mut hasher = sha1_smol::Sha1::new();
            hasher.update(&bytes);
            hasher.digest().to_string()
        };

        let before = write("<Property name=\"Cost\" value=\"1\" />");
        assert_eq!(parse(&path, &before).props["Cost"], Some("1".into()));

        // Same path, same caller, new bytes -- and so a different question.
        let after = write("<Property name=\"Cost\" value=\"99\" />");
        assert_ne!(before, after, "the file really did change");
        assert_eq!(parse(&path, &after).props["Cost"], Some("99".into()));

        // And the old answer is still the right answer for the old bytes, so
        // reverting a mod costs nothing either.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cache_is_skipped_when_the_caller_cannot_identify_the_file() {
        // Keying on an empty hash would serve one file's properties for
        // another's. Parsing is the right answer, not a shared entry.
        let dir = std::env::temp_dir().join("nmscheck-test-propcache");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("thing.EXML");
        std::fs::write(
            &path,
            "<Data template=\"cGcTable\"><Property name=\"A\" value=\"1\" /></Data>",
        )
        .unwrap();

        let parsed = parse(&path, "");
        assert_eq!(parsed.props.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
