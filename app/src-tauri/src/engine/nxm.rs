//! The `nxm://` links the Nexus website hands to a mod manager.
//!
//! Clicking "Mod manager download" on a mod page does not download anything.
//! It asks the operating system to open a URL in a scheme the manager has
//! registered, and that URL carries a short-lived token:
//!
//! ```text
//! nxm://nomanssky/mods/2308/files/48895?key=AbCd...&expires=1790291234&user_id=76276768
//!       ^game      ^mod id  ^file id    ^ the two fields that matter
//! ```
//!
//! The token is the whole point. A free Nexus account cannot ask the API for a
//! download link -- it answers *"You don't have permission to get download
//! links from the API without visiting nexusmods.com"* -- but `key` and
//! `expires` are the proof of that visit, and the same endpoint accepts them.
//! So this parser is what makes installing work without a premium account.
//!
//! Links arrive from outside the program, so nothing here trusts them: the
//! game domain, both ids and both token fields are all validated, and a link
//! for a different game is refused rather than quietly installed.

use serde::Serialize;

/// A download the website has authorised.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Link {
    /// the game as Nexus names it in a URL, e.g. `nomanssky`
    pub game: String,
    pub mod_id: u64,
    pub file_id: u64,
    /// proof that a person clicked the button on the site
    pub key: String,
    /// unix seconds after which `key` stops working
    pub expires: u64,
    pub user_id: Option<u64>,
}

/// Undo the percent-encoding a URL query uses.
fn decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Read a link the website sent us.
///
/// `expect_game` is the domain this program handles; a link for anything else
/// is an error, because the alternative is installing a Skyrim mod into No
/// Man's Sky when two managers are registered for the same scheme.
pub fn parse(url: &str, expect_game: &str) -> Result<Link, String> {
    let trimmed = url.trim();
    let rest = trimmed
        .strip_prefix("nxm://")
        .or_else(|| trimmed.strip_prefix("NXM://"))
        .ok_or_else(|| format!("{trimmed} is not an nxm:// link"))?;

    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, q),
        None => (rest, ""),
    };
    let parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    // nomanssky / mods / <id> / files / <id>
    let (game, mod_id, file_id) = match parts.as_slice() {
        [game, "mods", mod_id, "files", file_id] => (*game, *mod_id, *file_id),
        [_, "collections", ..] => {
            return Err("collection links are not supported yet".into());
        }
        _ => return Err(format!("{trimmed} is not a mod download link")),
    };

    if !game.eq_ignore_ascii_case(expect_game) {
        return Err(format!(
            "that link is for {game}, and this manages {expect_game}"
        ));
    }
    let mod_id: u64 = mod_id
        .parse()
        .map_err(|_| format!("{mod_id} is not a mod id"))?;
    let file_id: u64 = file_id
        .parse()
        .map_err(|_| format!("{file_id} is not a file id"))?;

    let mut key = None;
    let mut expires = None;
    let mut user_id = None;
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        match name {
            "key" => key = Some(decode(value)),
            "expires" => expires = decode(value).parse::<u64>().ok(),
            "user_id" => user_id = decode(value).parse::<u64>().ok(),
            _ => {}
        }
    }

    let key = key.filter(|k| !k.is_empty()).ok_or(
        "that link has no download key. Use the site's \"Mod manager download\" button \
         rather than copying the address.",
    )?;
    let expires = expires.ok_or("that link has no expiry, so it cannot be used")?;

    Ok(Link {
        game: game.to_lowercase(),
        mod_id,
        file_id,
        key,
        expires,
        user_id,
    })
}

impl Link {
    /// True once the website's token is too old for the API to accept.
    pub fn expired(&self, now_unix: u64) -> bool {
        self.expires <= now_unix
    }

    pub fn expired_now(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.expired(now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAME: &str = "nomanssky";

    #[test]
    fn a_real_link_gives_up_its_token() {
        let got = parse(
            "nxm://nomanssky/mods/2308/files/48895?key=tPvTBpYyYw9Cf1&expires=1790291234&user_id=76276768",
            GAME,
        )
        .unwrap();
        assert_eq!(got.mod_id, 2308);
        assert_eq!(got.file_id, 48895);
        assert_eq!(got.key, "tPvTBpYyYw9Cf1");
        assert_eq!(got.expires, 1790291234);
        assert_eq!(got.user_id, Some(76276768));
    }

    #[test]
    fn a_link_for_another_game_is_refused() {
        // Two managers can be registered for nxm:// at once, and the other
        // one's links will land here. Installing a Skyrim mod into No Man's
        // Sky would be a silent disaster.
        let err = parse(
            "nxm://skyrimspecialedition/mods/1/files/2?key=k&expires=9",
            GAME,
        )
        .unwrap_err();
        assert!(err.contains("skyrimspecialedition"), "{err}");
    }

    #[test]
    fn a_link_with_no_token_says_what_to_do_instead() {
        let err = parse("nxm://nomanssky/mods/2308/files/48895", GAME).unwrap_err();
        assert!(err.contains("Mod manager download"), "{err}");
    }

    #[test]
    fn percent_encoding_in_the_key_survives() {
        let got = parse(
            "nxm://nomanssky/mods/1/files/2?key=a%2Bb%2Fc%3D%3D&expires=5",
            GAME,
        )
        .unwrap();
        assert_eq!(got.key, "a+b/c==");
    }

    #[test]
    fn the_scheme_is_matched_whatever_its_case() {
        // Windows hands the registered handler the scheme as the site wrote it.
        assert!(parse("NXM://nomanssky/mods/1/files/2?key=k&expires=5", GAME).is_ok());
        assert!(parse("nxm://NoMansSky/mods/1/files/2?key=k&expires=5", GAME).is_ok());
    }

    #[test]
    fn something_that_is_not_a_download_link_is_not_guessed_at() {
        assert!(parse("https://www.nexusmods.com/nomanssky/mods/2308", GAME).is_err());
        assert!(parse("nxm://nomanssky/mods/2308", GAME).is_err());
        assert!(parse("nxm://nomanssky/mods/abc/files/1?key=k&expires=5", GAME).is_err());
        assert!(parse("", GAME).is_err());
    }

    #[test]
    fn a_collection_link_says_so_rather_than_failing_obscurely() {
        let err = parse("nxm://nomanssky/collections/abc123/revisions/1", GAME).unwrap_err();
        assert!(err.contains("collection"), "{err}");
    }

    #[test]
    fn an_old_token_is_recognised_as_stale() {
        let link = parse("nxm://nomanssky/mods/1/files/2?key=k&expires=1000", GAME).unwrap();
        assert!(link.expired(1001));
        assert!(link.expired(1000), "expiry is not a grace period");
        assert!(!link.expired(999));
    }
}
