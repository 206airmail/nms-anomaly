//! Recover the Nexus mod id and version from the archive a folder came from.
//!
//! This is what makes update checking possible on a library that this tool did
//! not install. Vortex records, per deployed file, the archive it came from
//! (`vortex.deployment.json`, read by [`super::hostenv`]), and Nexus bakes the
//! mod id and the version into that archive's name. So for every mod already
//! on disk we can ask the API "what is the current version of mod 3699" without
//! having downloaded it ourselves and without a migration.
//!
//! Nexus has used two naming conventions, and a real library contains both:
//!
//! ```text
//! Accelerated Settlements 3699 6.45.1.0 2026-06-24T22-04Z YjQ5seZ66
//! ^ name                   ^id ^version ^uploaded         ^nonce
//!
//! Better Freighter Entry and Exit 1.8-1180-1-8-1780817964
//! ^ name                          ^id  ^ver ^unix time
//! ```
//!
//! Note the trap in both: the *name* often ends with a version too ("Better
//! Deposit Colors 2.9 2361 2.9 ..."), so a left-to-right scan for "the first
//! number" finds the wrong one. Both parsers therefore anchor on the timestamp
//! at the end, which has a shape nothing else in the string has, and work
//! backwards from it.

/// What a Nexus archive name says about the mod inside it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ArchiveName {
    /// the mod's display name, as Nexus rendered it into the file name
    pub name: String,
    /// the id in the mod's page URL: `nexusmods.com/nomanssky/mods/<id>`
    pub mod_id: u64,
    /// the version string as published, dots restored in the dashed form
    pub version: String,
    /// when that file was uploaded, as the name records it
    pub uploaded: String,
}

/// True for `2026-06-24T22-04Z`, the timestamp in the spaced form.
fn is_stamp(token: &str) -> bool {
    let b = token.as_bytes();
    b.len() == 17
        && b[4] == b'-'
        && b[7] == b'-'
        && b[10] == b'T'
        && b[13] == b'-'
        && b[16] == b'Z'
        && b.iter().enumerate().all(|(i, c)| {
            matches!(i, 4 | 7 | 10 | 13 | 16) || c.is_ascii_digit()
        })
}

/// True for a 9-11 digit unix time, the timestamp in the dashed form.
fn is_unix(token: &str) -> bool {
    (9..=11).contains(&token.len()) && token.bytes().all(|c| c.is_ascii_digit())
}

fn strip_extension(archive: &str) -> &str {
    for ext in [".zip", ".rar", ".7z", ".ZIP", ".RAR", ".7Z"] {
        if let Some(rest) = archive.strip_suffix(ext) {
            return rest;
        }
    }
    archive
}

/// `Accelerated Settlements 3699 6.45.1.0 2026-06-24T22-04Z YjQ5seZ66`
fn parse_spaced(stem: &str) -> Option<ArchiveName> {
    let tokens: Vec<&str> = stem.split(' ').collect();
    // The nonce is last, the stamp before it. Search rather than index from
    // the end: some names have no nonce, and some have a trailing marker.
    let at = tokens.iter().rposition(|t| is_stamp(t))?;
    // Before the stamp: version, and before that the id.
    if at < 2 {
        return None;
    }
    let mod_id: u64 = tokens[at - 2].parse().ok()?;
    Some(ArchiveName {
        name: tokens[..at - 2].join(" ").trim().to_string(),
        mod_id,
        version: tokens[at - 1].to_string(),
        uploaded: tokens[at].to_string(),
    })
}

/// `Better Freighter Entry and Exit 1.8-1180-1-8-1780817964`
fn parse_dashed(stem: &str) -> Option<ArchiveName> {
    let tokens: Vec<&str> = stem.split('-').collect();
    if tokens.len() < 4 || !is_unix(tokens[tokens.len() - 1]) {
        return None;
    }
    // Everything between the id and the unix time is the version, with its
    // dots turned into dashes: `6.45.1.0` ships as `6-45-1-0`.
    //
    // The id is the first all-digit token after the name. The name is taken
    // as short as possible, because a name may itself contain digits but a
    // *token* that is nothing but digits, followed only by version parts, is
    // the id. Scanning left to right finds it at the earliest valid split.
    let last = tokens.len() - 1;
    for split in 1..last {
        let Ok(mod_id) = tokens[split].parse::<u64>() else {
            continue;
        };
        let version: Vec<&str> = tokens[split + 1..last].to_vec();
        if version.is_empty() {
            continue;
        }
        // Version parts are short and alphanumeric; anything else means this
        // split landed inside the name.
        if !version
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 6 && p.chars().all(|c| c.is_ascii_alphanumeric()))
        {
            continue;
        }
        return Some(ArchiveName {
            name: tokens[..split].join("-").trim().to_string(),
            mod_id,
            version: version.join("."),
            uploaded: tokens[last].to_string(),
        });
    }
    None
}

/// Read an archive name, whichever convention Nexus used for it.
pub fn parse(archive: &str) -> Option<ArchiveName> {
    let stem = strip_extension(archive.trim());
    parse_spaced(stem).or_else(|| parse_dashed(stem))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spaced_form_is_read_from_the_stamp_backwards() {
        let got = parse("Accelerated Settlements 3699 6.45.1.0 2026-06-24T22-04Z YjQ5seZ66")
            .unwrap();
        assert_eq!(got.name, "Accelerated Settlements");
        assert_eq!(got.mod_id, 3699);
        assert_eq!(got.version, "6.45.1.0");
        assert_eq!(got.uploaded, "2026-06-24T22-04Z");
    }

    #[test]
    fn a_name_that_ends_in_its_own_version_is_not_mistaken_for_the_id() {
        // "Better Deposit Colors 2.9" carries a version in the display name,
        // so the first number in the string is 2 and the first *token* that
        // parses as a number is "2.9" -- neither is the mod id.
        let got = parse("Better Deposit Colors 2.9 2361 2.9 2026-09-10T18-59Z xDwj9UAeU")
            .unwrap();
        assert_eq!(got.name, "Better Deposit Colors 2.9");
        assert_eq!(got.mod_id, 2361);
        assert_eq!(got.version, "2.9");
    }

    #[test]
    fn a_downloaded_rar_is_read_the_same_as_a_zip() {
        // What the loadout records for a mod installed from a file rather than
        // from a link: the real download, extension and all. `archive_index`
        // hands this string straight to `parse`, so an extension it did not
        // strip would lose the mod its page, its version and its updates.
        let got = parse("IncreasedSClassChance-3141-6-12-1762790084.rar").unwrap();
        assert_eq!(got.name, "IncreasedSClassChance");
        assert_eq!(got.mod_id, 3141);
        assert_eq!(got.version, "6.12");
    }

    #[test]
    fn the_dashed_form_restores_the_dots_in_the_version() {
        let got = parse("Better Freighter Entry and Exit 1.8-1180-1-8-1780817964").unwrap();
        assert_eq!(got.name, "Better Freighter Entry and Exit 1.8");
        assert_eq!(got.mod_id, 1180);
        assert_eq!(got.version, "1.8");
        assert_eq!(got.uploaded, "1780817964");
    }

    #[test]
    fn one_page_can_host_several_files_under_one_id() {
        // Both of these are mod 1201. An update check keyed on the mod id has
        // to expect that, or it will report the same page twice.
        assert_eq!(
            parse("Better Ship Teleport Module Range 6.0-1201-6-0-1751870410")
                .unwrap()
                .mod_id,
            1201
        );
        assert_eq!(
            parse("Better Ship Transfer Range 5.9-1201-5-9-1738187904")
                .unwrap()
                .mod_id,
            1201
        );
    }

    #[test]
    fn an_extension_is_not_part_of_the_name() {
        let got = parse("Astro and Babs Bridge 3680 7.03a 2026-09-17T08-57Z AeW3Tdrur.zip")
            .unwrap();
        assert_eq!(got.mod_id, 3680);
        assert_eq!(got.version, "7.03a");
    }

    #[test]
    fn something_that_did_not_come_from_nexus_is_not_invented() {
        assert!(parse("MyHandmadeMod").is_none());
        assert!(parse("").is_none());
        assert!(parse("some backup 2026").is_none());
    }
}
