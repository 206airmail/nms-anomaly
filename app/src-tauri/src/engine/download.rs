//! Fetch a file from a CDN to disk, reporting how far it has got.
//!
//! Deliberately small and deliberately not clever. It streams to a `.part`
//! file and renames on success, so an interrupted download can never be
//! mistaken for a complete archive -- the thing that would otherwise send a
//! truncated `.zip` into the installer and produce a mod that is missing half
//! its files.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

/// How far a download has got.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Progress {
    pub bytes: u64,
    /// `None` when the server did not say how big the file is
    pub total: Option<u64>,
}

/// Strip anything that would let a server's filename escape the folder.
///
/// The name comes from a URL, so it is not ours: `../../GAMEDATA/x.zip` would
/// otherwise be written wherever that points.
pub fn safe_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let cleaned: String = base
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' | '\0' => '_',
            c if (c as u32) < 0x20 => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "download.bin".to_string()
    } else {
        trimmed
    }
}

/// The file name a CDN URL implies, with its query string discarded.
pub fn name_from_url(url: &str) -> String {
    let no_query = url.split(['?', '#']).next().unwrap_or(url);
    let raw = no_query.rsplit('/').next().unwrap_or("download.bin");
    safe_name(&percent_decode(raw))
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Bytes that may stand for themselves in the path or query of a URL.
///
/// RFC 3986's `pchar`, plus the delimiters that give a path and query their
/// shape. Everything else has to be escaped to survive a URI parser.
fn stands_alone(byte: u8) -> bool {
    matches!(byte,
        // unreserved
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~'
        // sub-delims, which a query uses to separate its own fields
        | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'='
        // structure: the segment, query and fragment separators
        | b':' | b'@' | b'/' | b'?' | b'#'
    )
}

/// Escape a URL the way its own server should have.
///
/// Nexus hands back a mirror `URI` whose last path segment is the mod's *file
/// name*, and it does not always escape it -- "Better Frigate View 2.3-2574…"
/// arrives with the spaces still in it. `http`'s parser refuses a space
/// outright, so the install died on `http: invalid uri character` before a
/// single byte was requested, under the heading "could not reach the download
/// server". Nothing was unreachable; the address was never posted.
///
/// A `%` that already introduces a valid escape is left exactly as it stands.
/// Re-encoding it would turn `%20` into `%2520` and ask the CDN for a file
/// with a literal `%20` in its name, which is a 404 rather than a download --
/// so this has to be able to tell an escape from a stray percent sign, and it
/// keeps the stray one by escaping it.
///
/// Only the part after the authority is touched. `[` and `]` are legal in an
/// IPv6 host and nowhere else, and a host is the server's own business.
pub fn escape_url(url: &str) -> String {
    let trimmed = url.trim();
    // Where the path begins: after `scheme://host:port`, or at the start when
    // there is no scheme to skip past.
    let from = match trimmed.find("://") {
        Some(at) => {
            let after = at + 3;
            after + trimmed[after..].find('/').unwrap_or(trimmed.len() - after)
        }
        None => 0,
    };

    let (head, rest) = trimmed.split_at(from);
    let bytes = rest.as_bytes();
    let mut out = String::with_capacity(rest.len());
    let mut i = 0;
    while i < bytes.len() {
        let byte = bytes[i];
        let escaped = byte == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit();
        if escaped {
            out.push_str(&rest[i..i + 3]);
            i += 3;
        } else if stands_alone(byte) {
            out.push(byte as char);
            i += 1;
        } else {
            out.push_str(&format!("%{byte:02X}"));
            i += 1;
        }
    }
    format!("{head}{out}")
}

/// Download `url` into `dir`, calling `on_progress` as it goes.
///
/// Returns where the finished file landed. Existing files are not replaced: a
/// second download of the same archive lands beside the first rather than
/// overwriting something that may already be staged and deployed.
pub fn fetch(
    url: &str,
    dir: &Path,
    file_name: Option<&str>,
    mut on_progress: impl FnMut(Progress),
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;

    let name = file_name.map(safe_name).unwrap_or_else(|| name_from_url(url));
    let target = unused_path(dir, &name);
    let part = target.with_extension(format!(
        "{}.part",
        target
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("download")
    ));

    let agent: ureq::Agent = ureq::Agent::config_builder()
        // Long, because this is a whole archive over a CDN, not an API call.
        .timeout_global(Some(Duration::from_secs(60 * 30)))
        .user_agent(concat!("anomaly/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();

    let mut response = agent
        // Escaped here rather than where the mirror is chosen, so that every
        // route to a download gets it: the CDN's own address is the one thing
        // this function is handed and the one thing it cannot vouch for.
        .get(escape_url(url))
        .call()
        .map_err(|e| format!("could not reach the download server: {e}"))?;

    let total = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    let mut body = response.body_mut().as_reader();
    let mut out = std::fs::File::create(&part)
        .map_err(|e| format!("could not write {}: {e}", part.display()))?;

    let mut buffer = vec![0u8; 128 * 1024];
    let mut bytes = 0u64;
    let mut since_report = 0u64;
    on_progress(Progress { bytes, total });

    loop {
        let read = match body.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                drop(out);
                let _ = std::fs::remove_file(&part);
                return Err(format!("the download stopped early: {e}"));
            }
        };
        if let Err(e) = out.write_all(&buffer[..read]) {
            drop(out);
            let _ = std::fs::remove_file(&part);
            return Err(format!("could not write to {}: {e}", part.display()));
        }
        bytes += read as u64;
        since_report += read as u64;
        // Roughly every half megabyte: often enough to look live, rarely
        // enough not to flood the UI with events.
        if since_report >= 512 * 1024 {
            since_report = 0;
            on_progress(Progress { bytes, total });
        }
    }

    out.flush().map_err(|e| e.to_string())?;
    drop(out);

    // A server that promised a length and sent less was cut off. Renaming it
    // to `.zip` would hand a truncated archive to the installer.
    if let Some(expected) = total {
        if bytes != expected {
            let _ = std::fs::remove_file(&part);
            return Err(format!(
                "the download was cut short: {bytes} bytes of {expected}"
            ));
        }
    }

    std::fs::rename(&part, &target)
        .map_err(|e| format!("could not finish {}: {e}", target.display()))?;
    on_progress(Progress { bytes, total });
    Ok(target)
}

/// `name`, or `name (2)`, or `name (3)` -- whichever is free.
fn unused_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let path = Path::new(name);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(name);
    let ext = path.extension().and_then(|e| e.to_str());
    for n in 2..1000 {
        let candidate = match ext {
            Some(ext) => dir.join(format!("{stem} ({n}).{ext}")),
            None => dir.join(format!("{stem} ({n})")),
        };
        if !candidate.exists() {
            return candidate;
        }
    }
    first
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cdn_address_with_a_space_in_it_is_made_fetchable() {
        // The real shape: Nexus puts the mod's file name in the path and does
        // not always escape it. A space is the byte `http` refuses, and it is
        // the one mod file names are full of.
        assert_eq!(
            escape_url("https://cdn.nexus.com/1634/2308/Better Frigate View 2.3-2574.zip?md5=aB-c"),
            "https://cdn.nexus.com/1634/2308/Better%20Frigate%20View%202.3-2574.zip?md5=aB-c"
        );
    }

    #[test]
    fn a_query_keeps_the_punctuation_that_gives_it_shape() {
        // Escaping these would hand the CDN one long parameter instead of
        // three, which fails as surely as not posting the address at all.
        let url = "https://cdn.nexus.com/x.zip?md5=a+b/c=&expires=123&user_id=7#frag";
        assert_eq!(escape_url(url), url);
    }

    #[test]
    fn an_escape_that_is_already_there_is_left_alone() {
        // `%20` re-encoded to `%2520` asks for a file whose name contains a
        // literal "%20", which is a 404 rather than a download.
        assert_eq!(
            escape_url("https://cdn.nexus.com/Better%20Frigate.zip?md5=x"),
            "https://cdn.nexus.com/Better%20Frigate.zip?md5=x"
        );
        // But a percent that introduces nothing is a stray, and stands for
        // itself: `100%` in a file name is not the start of an escape.
        assert_eq!(
            escape_url("https://cdn.nexus.com/100%.zip"),
            "https://cdn.nexus.com/100%25.zip"
        );
        assert_eq!(
            escape_url("https://cdn.nexus.com/a%zz.zip"),
            "https://cdn.nexus.com/a%25zz.zip"
        );
    }

    #[test]
    fn the_host_is_the_servers_own_business() {
        // `[` and `]` are legal in an IPv6 host and illegal in a path, so the
        // authority is skipped rather than escaped along with everything else.
        assert_eq!(
            escape_url("http://[::1]:8080/some file.zip"),
            "http://[::1]:8080/some%20file.zip"
        );
    }

    #[test]
    fn an_address_that_was_already_fine_is_returned_unchanged() {
        let url = "https://supporter-files.nexus-cdn.com/1634/2308/Mod-2308-1-0-1757165929.zip";
        assert_eq!(escape_url(url), url);
    }

    #[test]
    fn a_url_gives_up_its_file_name() {
        assert_eq!(
            name_from_url(
                "https://cf.nexus.com/1634/2308/Unpredictable%20Shelters%201.4.zip?md5=x&expires=9"
            ),
            "Unpredictable Shelters 1.4.zip"
        );
    }

    #[test]
    fn a_server_cannot_name_a_file_out_of_its_folder() {
        // The name comes from a URL, so it is not trusted input.
        assert_eq!(safe_name("../../GAMEDATA/evil.zip"), "evil.zip");
        assert_eq!(safe_name("..\\..\\evil.zip"), "evil.zip");
        assert_eq!(safe_name("/etc/passwd"), "passwd");
        assert!(!safe_name("a:b|c?d*e.zip").contains(':'));
    }

    #[test]
    fn a_name_that_is_nothing_at_all_still_yields_a_file() {
        assert_eq!(safe_name(""), "download.bin");
        assert_eq!(safe_name("..."), "download.bin");
        assert_eq!(safe_name("   "), "download.bin");
    }

    #[test]
    fn a_second_download_lands_beside_the_first() {
        let dir = std::env::temp_dir().join("nmscheck_download_unused");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        assert_eq!(unused_path(&dir, "Mod.zip"), dir.join("Mod.zip"));
        std::fs::write(dir.join("Mod.zip"), "first").unwrap();
        assert_eq!(unused_path(&dir, "Mod.zip"), dir.join("Mod (2).zip"));
        std::fs::write(dir.join("Mod (2).zip"), "second").unwrap();
        assert_eq!(unused_path(&dir, "Mod.zip"), dir.join("Mod (3).zip"));
        // The first one was never touched, which is the point.
        assert_eq!(std::fs::read_to_string(dir.join("Mod.zip")).unwrap(), "first");

        let _ = std::fs::remove_dir_all(&dir);
    }

}
