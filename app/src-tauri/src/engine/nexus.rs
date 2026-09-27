//! Ask Nexus what the current version of each installed mod is.
//!
//! The hard part is not the HTTP. It is deciding *which file on a mod's page*
//! is the one sitting in the mods folder, because a page can host several
//! unrelated mods (id 1201 hosts both "Better Ship Transfer Range" and "Better
//! Ship Teleport Module Range") and a version string is not always a number --
//! one mod in the measured library publishes version "Cosmos".
//!
//! So this does not compare versions. It matches the installed archive to an
//! exact file record, then walks Nexus's own `file_updates` chain, which is a
//! list of `old_file_id -> new_file_id` links the author creates when they
//! upload a replacement. The end of the chain is the successor, whatever it is
//! called. That was exact for all 61 identifiable mods in the measured library.
//!
//! # The stale-record trap
//!
//! Vortex records, per deployed file, the *staging folder* it came from -- and
//! when you reinstall an update over an existing mod entry, the staging folder
//! keeps its original name. The measured library had exactly one: the folder
//! `Unpredictable Shelters 1.4` holds 1.4 (its Lua declares
//! `Unpredictable Shelters 1.4.pak`) while Vortex still calls its source
//! `Unpredictable Shelters 1.3-2308-...`. Believing the record would have
//! offered an update the user already had.
//!
//! Two anchors were tried and rejected before settling on corroboration:
//! deployed file mtime matched the right Nexus file for only 5 of 61 within
//! two minutes and actively mispaired 6.0 with 5.9; and the folder name holds
//! a version for only 41 of 61, with `Universal Dot Crosshairs - Cosmos
//! Experimental v8` naming an internal variant rather than a version. So the
//! folder name and the Lua are used only to *suppress* a claimed update, never
//! to assert one -- see [`Check::corroborate`].

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::model::Mod;
use super::nexusname;

/// The game as Nexus names it in a URL.
pub const GAME: &str = "nomanssky";

const BASE: &str = "https://api.nexusmods.com/v1";
const AGENT: &str = concat!("nmscheck/", env!("CARGO_PKG_VERSION"));

/// One file on a mod's Nexus page.
#[derive(Debug, Clone, Deserialize)]
pub struct NexusFile {
    pub file_id: u64,
    pub name: String,
    pub version: String,
    pub file_name: String,
    pub uploaded_timestamp: i64,
    /// `MAIN`, `OPTIONAL`, `UPDATE`, `ARCHIVED`, or absent once the author
    /// retires a file. Only the first three are things a user should install.
    pub category_name: Option<String>,
}

impl NexusFile {
    /// True when Nexus still offers this file as something to install.
    fn offered(&self) -> bool {
        matches!(
            self.category_name.as_deref(),
            Some("MAIN") | Some("OPTIONAL") | Some("UPDATE")
        )
    }
}

/// A link the author made when replacing one file with another.
#[derive(Debug, Clone, Deserialize)]
pub struct FileUpdate {
    pub old_file_id: u64,
    pub new_file_id: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FilesResponse {
    pub files: Vec<NexusFile>,
    #[serde(default)]
    pub file_updates: Vec<FileUpdate>,
}

/// What the API says about a whole mod page.
///
/// Enough to show the mod inside this app. The real page cannot be embedded:
/// Nexus sends `X-Frame-Options: SAMEORIGIN` and puts Cloudflare in front, so
/// an iframe gets a challenge screen rather than a mod. Rendering from this is
/// also faster, works with the window offline, and carries no trackers.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModPage {
    pub mod_id: Option<u64>,
    /// Absent for a mod that is hidden, under moderation or deleted. Measured
    /// on the live listings: `available` is false for exactly those, and true
    /// for exactly the ones that carry a name, so [`ModPage::listable`] tests
    /// the flag rather than guessing from a missing field.
    pub name: Option<String>,
    pub version: Option<String>,
    pub summary: Option<String>,
    /// the long description, in Nexus's BBCode with `<br />` for line breaks
    pub description: Option<String>,
    pub picture_url: Option<String>,
    pub author: Option<String>,
    pub uploaded_by: Option<String>,
    pub uploaded_users_profile_url: Option<String>,
    pub endorsement_count: Option<u64>,
    pub mod_downloads: Option<u64>,
    pub mod_unique_downloads: Option<u64>,
    pub updated_time: Option<String>,
    pub created_time: Option<String>,
    pub status: Option<String>,
    #[serde(default)]
    pub contains_adult_content: bool,
    #[serde(default)]
    pub available: bool,
}

/// How an installed mod stands against its Nexus page.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Standing {
    /// the installed file is the end of its update chain and still offered
    Current,
    /// the author has published a successor
    Outdated {
        latest_version: String,
        latest_name: String,
        latest_file_id: u64,
        /// where a person would go to get it
        page: String,
    },
    /// a successor exists, but the folder already contains it -- Vortex's
    /// record of which archive this came from is simply out of date
    RecordStale { actual_version: String },
    /// the installed file is the newest in its chain but Nexus no longer
    /// offers it: the author pulled or archived it without a replacement
    Withdrawn { installed_version: String },
    /// nothing could be asked, and why
    Unknown { reason: String },
}

/// One mod's update verdict.
#[derive(Debug, Clone, Serialize)]
pub struct Check {
    /// the mod folder in GAMEDATA\MODS
    pub owner: String,
    pub mod_id: Option<u64>,
    /// version of the file Vortex recorded, which the stale case disproves
    pub recorded_version: Option<String>,
    #[serde(flatten)]
    pub standing: Standing,
}

/// A page URL a person can open.
fn page_url(mod_id: u64) -> String {
    format!("https://www.nexusmods.com/{GAME}/mods/{mod_id}")
}

/// The curated lists the site publishes for a game.
///
/// There is no keyword search in this API. Searching is done by opening the
/// site's own search in the browser window, which is always current and needs
/// no reimplementation -- see `browser` in the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Listing {
    Trending,
    LatestAdded,
    LatestUpdated,
}

impl Listing {
    fn path(self) -> &'static str {
        match self {
            Listing::Trending => "trending",
            Listing::LatestAdded => "latest_added",
            Listing::LatestUpdated => "latest_updated",
        }
    }
}

impl ModPage {
    /// True when this is a mod a person could actually go and install.
    ///
    /// The listings include mods that are hidden, awaiting moderation or in
    /// the wastebin, and those arrive with almost every field missing -- no
    /// name, no summary, no picture. Showing them would put blank cards in
    /// the middle of the page.
    pub fn listable(&self) -> bool {
        self.available && self.name.is_some() && self.mod_id.is_some()
    }

    /// Where a person would go to read about it.
    pub fn url(&self) -> Option<String> {
        self.mod_id.map(page_url)
    }
}

/// The Nexus API, with the user's personal key.
pub struct Api {
    key: String,
    agent: ureq::Agent,
}

/// What the rate-limit headers said on the last call.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Budget {
    pub hourly_remaining: i64,
    pub daily_remaining: i64,
}

impl Api {
    pub fn new(key: impl Into<String>) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .user_agent(AGENT)
            .build();
        Api {
            key: key.into(),
            agent: config.into(),
        }
    }

    fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<(T, Budget), String> {
        let mut resp = self
            .agent
            .get(&format!("{BASE}/{path}"))
            .header("apikey", &self.key)
            .call()
            .map_err(|e| match e {
                // A download request carries a second credential -- the `key`
                // and `expires` out of the `nxm://` link -- and Nexus answers
                // 400 when that pair is wrong or used up. Saying "could not
                // reach Nexus" there is untrue and sends people to look at
                // their connection instead of pressing the button again.
                ureq::Error::StatusCode(400) if path.contains("download_link") => {
                    "Nexus would not accept that download link. It is single use and \
                     short lived, so press the download button on the mod page again."
                        .to_string()
                }
                ureq::Error::StatusCode(400) => "Nexus rejected that request.".to_string(),
                ureq::Error::StatusCode(401) => "Nexus rejected the API key.".to_string(),
                ureq::Error::StatusCode(403) if path.contains("download_link") => {
                    "Nexus will only hand out a download link on the back of a click on \
                     its own website. Open the mod page here and use its download button."
                        .to_string()
                }
                ureq::Error::StatusCode(403) => {
                    "Nexus refused the request (403). The key may lack access.".to_string()
                }
                ureq::Error::StatusCode(404) => "no such mod on Nexus".to_string(),
                ureq::Error::StatusCode(429) => {
                    "Nexus rate limit reached; try again later.".to_string()
                }
                other => format!("could not reach Nexus: {other}"),
            })?;

        let header = |name: &str| -> i64 {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(-1)
        };
        let budget = Budget {
            hourly_remaining: header("x-rl-hourly-remaining"),
            daily_remaining: header("x-rl-daily-remaining"),
        };

        let body = resp
            .body_mut()
            .read_json::<T>()
            .map_err(|e| format!("Nexus sent something unreadable: {e}"))?;
        Ok((body, budget))
    }

    /// Every file ever published on a mod's page, plus the successor links.
    pub fn files(&self, mod_id: u64) -> Result<(FilesResponse, Budget), String> {
        self.get(&format!("games/{GAME}/mods/{mod_id}/files.json"))
    }

    /// The page itself. Only needed to explain a mod that has been taken down.
    pub fn page(&self, mod_id: u64) -> Result<(ModPage, Budget), String> {
        self.get(&format!("games/{GAME}/mods/{mod_id}.json"))
    }

    /// One of the site's own curated lists, with the unshowable ones dropped.
    pub fn browse(&self, list: Listing) -> Result<(Vec<ModPage>, Budget), String> {
        let (mods, budget) =
            self.get::<Vec<ModPage>>(&format!("games/{GAME}/mods/{}.json", list.path()))?;
        Ok((mods.into_iter().filter(ModPage::listable).collect(), budget))
    }

    /// Confirms the key and names the account it belongs to.
    pub fn whoami(&self) -> Result<Account, String> {
        self.get::<Account>("users/validate.json").map(|(a, _)| a)
    }

    /// Where a file can actually be fetched from.
    ///
    /// `token` is the `key` and `expires` pair out of an [`super::nxm::Link`].
    /// Without it the API refuses anyone who is not premium, saying so in
    /// as many words; with it a free account is served, because the pair is
    /// proof that a person clicked the button on the site. So this is only
    /// ever called on the back of a real `nxm://` handoff.
    pub fn download_link(
        &self,
        mod_id: u64,
        file_id: u64,
        token: Option<(&str, u64)>,
    ) -> Result<Vec<Mirror>, String> {
        let mut path = format!("games/{GAME}/mods/{mod_id}/files/{file_id}/download_link.json");
        if let Some((key, expires)) = token {
            // The key is base64-ish and can hold `+` and `=`, which a query
            // string would otherwise read as a space and a separator.
            path.push_str(&format!("?key={}&expires={expires}", escape(key)));
        }
        let (mirrors, _) = self.get::<Vec<Mirror>>(&path)?;
        if mirrors.is_empty() {
            return Err("Nexus offered no download for that file".into());
        }
        Ok(mirrors)
    }
}

/// Percent-encode everything that is not unreserved in a query value.
fn escape(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// One CDN the file can be fetched from.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Mirror {
    /// the server's full name, e.g. "Nexus CDN"
    pub name: Option<String>,
    pub short_name: Option<String>,
    #[serde(rename = "URI")]
    pub uri: String,
}

/// Who a key belongs to, and what it is allowed to do.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Account {
    pub name: String,
    /// Non-premium keys can read everything here but cannot ask the API for a
    /// download link, so an in-app "download it for me" needs either premium
    /// or an `nxm://` handoff from the website.
    #[serde(default)]
    pub is_premium: bool,
}

/// Strip the archive extension Nexus file names carry and Vortex's do not.
fn stem(name: &str) -> &str {
    for ext in [".zip", ".rar", ".7z", ".ZIP", ".RAR", ".7Z"] {
        if let Some(rest) = name.strip_suffix(ext) {
            return rest;
        }
    }
    name
}

/// Find the exact file record the installed archive came from.
///
/// The archive name Vortex keeps *is* the Nexus file name for every mod in the
/// measured library, so this is an equality test, not a guess. The fallback on
/// version exists for the day Vortex renames something, and refuses to answer
/// when it is ambiguous.
fn locate<'a>(archive: &str, parsed_version: &str, files: &'a [NexusFile]) -> Option<&'a NexusFile> {
    let want = stem(archive.trim());
    if let Some(hit) = files.iter().find(|f| stem(&f.file_name) == want) {
        return Some(hit);
    }
    let mut by_version = files.iter().filter(|f| f.version == parsed_version);
    let first = by_version.next()?;
    // Ambiguous is not a match: two files sharing a version on one page means
    // we cannot say which is installed, and a wrong guess invents an update.
    by_version.next().is_none().then_some(first)
}

/// Walk `old -> new` to the end of the chain.
///
/// A cycle means the author's links contradict each other and no file in the
/// loop is "the newest". Returning `start` is the conservative reading -- it
/// reports the mod as current rather than offering an update we cannot name.
fn newest(start: u64, updates: &[FileUpdate]) -> u64 {
    let next: BTreeMap<u64, u64> = updates
        .iter()
        .map(|u| (u.old_file_id, u.new_file_id))
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    let mut at = start;
    while let Some(&to) = next.get(&at) {
        if !seen.insert(at) {
            return start;
        }
        at = to;
    }
    at
}

impl Check {
    /// Does the mod folder itself already contain the version being offered?
    ///
    /// Only ever turns an `Outdated` into a `RecordStale`. It cannot create an
    /// update, because the evidence it reads is too weak to assert one: see
    /// the module docs for the two anchors this replaced.
    fn corroborate(standing: Standing, root: &Path, offered: &str) -> Standing {
        if matches!(standing, Standing::Outdated { .. }) && folder_shows(root, offered) {
            return Standing::RecordStale {
                actual_version: offered.to_string(),
            };
        }
        standing
    }
}

/// True when the folder's own name, or an AMUMSS Lua beside it, names `version`.
///
/// A version has to sit on its own to count. `1.4` must not match inside the
/// later `1.40`, nor inside `v11.4`, but it does have to match in
/// `Shelters 1.4.pak` -- so the test is not "is the neighbour a dot", it is
/// "is the neighbour another digit of the same number".
fn version_named_in(hay: &str, version: &str) -> bool {
    if version.is_empty() {
        return false;
    }
    // A number runs on through `.` only when a digit follows it, so `1.4`
    // continues in `1.40` and in `1.4.1`, but ends in `1.4.pak`.
    let runs_on = |mut rest: std::str::Chars| match rest.next() {
        Some(c) if c.is_ascii_alphanumeric() => true,
        Some('.') => matches!(rest.next(), Some(c) if c.is_ascii_digit()),
        _ => false,
    };

    let mut from = 0;
    while let Some(rel) = hay[from..].find(version) {
        let at = from + rel;
        // Reading backwards, the same rule applies mirrored.
        let mut back = hay[..at].chars().rev();
        let preceded = match back.next() {
            Some(c) if c.is_ascii_alphanumeric() => true,
            Some('.') => matches!(back.next(), Some(c) if c.is_ascii_digit()),
            _ => false,
        };
        if !preceded && !runs_on(hay[at + version.len()..].chars()) {
            return true;
        }
        from = at + 1;
    }
    false
}

/// True when the folder's own name, or an AMUMSS Lua beside it, names `version`.
fn folder_shows(root: &Path, version: &str) -> bool {
    let Some(folder) = root.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if version_named_in(folder, version) {
        return true;
    }

    // AMUMSS scripts sit beside the folder and share its name, and they declare
    // the pak they build, which carries the version:
    //     ["MOD_FILENAME"] = "Unpredictable Shelters 1.4.pak",
    //
    // The name is built by appending, not by `with_extension`: a folder called
    // `Unpredictable Shelters 1.4` has ".4" for an extension as far as `Path`
    // is concerned, and replacing it would look for `... 1.lua`.
    let Some(parent) = root.parent() else {
        return false;
    };
    let Ok(text) = std::fs::read_to_string(parent.join(format!("{folder}.lua"))) else {
        return false;
    };
    text.lines()
        .filter(|l| l.contains("MOD_FILENAME"))
        .any(|l| version_named_in(l, version))
}

/// Check every mod that can be identified, one request per Nexus page.
///
/// Mods sharing a page cost one request between them. `progress` is called
/// after each page so a UI can show movement on a library of this size.
pub fn check_all(
    api: &Api,
    mods: &[Mod],
    // Where each mod's archive is recorded. After a cutover the manifest is
    // gone and only the staging tree knows, so this is read the same way the
    // mod's name is -- see `library::archive_of`.
    staged: &super::library::Staged,
    mut progress: impl FnMut(usize, usize),
) -> (Vec<Check>, Budget) {
    let mut wanted: Vec<(usize, u64, String, String)> = Vec::new();
    let mut out: Vec<Check> = Vec::new();
    for (i, m) in mods.iter().enumerate() {
        let archive = super::library::archive_of(m, staged).map(str::to_string);
        match archive.as_deref().and_then(nexusname::parse) {
            Some(found) => {
                wanted.push((i, found.mod_id, archive.clone().unwrap_or_default(), found.version))
            }
            None => out.push(Check {
                owner: m.name.clone(),
                mod_id: None,
                recorded_version: None,
                standing: Standing::Unknown {
                    reason: "no Nexus archive is recorded for this folder".into(),
                },
            }),
        }
    }

    let total = wanted
        .iter()
        .map(|w| w.1)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let mut pages: BTreeMap<u64, Result<FilesResponse, String>> = BTreeMap::new();
    let mut budget = Budget::default();
    for (_, mod_id, _, _) in &wanted {
        if pages.contains_key(mod_id) {
            continue;
        }
        let fetched = match api.files(*mod_id) {
            Ok((body, b)) => {
                budget = b;
                Ok(body)
            }
            Err(e) => Err(e),
        };
        pages.insert(*mod_id, fetched);
        progress(pages.len(), total);
    }

    for (i, mod_id, archive, version) in wanted {
        let m = &mods[i];
        let standing = match pages.get(&mod_id) {
            Some(Err(e)) => Standing::Unknown { reason: e.clone() },
            None => Standing::Unknown {
                reason: "not checked".into(),
            },
            Some(Ok(body)) => match locate(&archive, &version, &body.files) {
                None => Standing::Unknown {
                    reason: "the installed file is not listed on its Nexus page".into(),
                },
                Some(installed) => {
                    let tip = newest(installed.file_id, &body.file_updates);
                    if tip == installed.file_id {
                        if installed.offered() {
                            Standing::Current
                        } else {
                            Standing::Withdrawn {
                                installed_version: installed.version.clone(),
                            }
                        }
                    } else {
                        match body.files.iter().find(|f| f.file_id == tip) {
                            Some(latest) if latest.offered() => Check::corroborate(
                                Standing::Outdated {
                                    latest_version: latest.version.clone(),
                                    latest_name: latest.name.clone(),
                                    latest_file_id: latest.file_id,
                                    page: page_url(mod_id),
                                },
                                Path::new(&m.root),
                                &latest.version,
                            ),
                            // superseded by something the author then retired
                            _ => Standing::Current,
                        }
                    }
                }
            },
        };
        out.push(Check {
            owner: m.name.clone(),
            mod_id: Some(mod_id),
            recorded_version: Some(version),
            standing,
        });
    }

    out.sort_by(|a, b| a.owner.cmp(&b.owner));
    (out, budget)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: u64, version: &str, name: &str, category: Option<&str>) -> NexusFile {
        NexusFile {
            file_id: id,
            name: name.to_string(),
            version: version.to_string(),
            file_name: format!("{name}.zip"),
            uploaded_timestamp: id as i64,
            category_name: category.map(str::to_string),
        }
    }

    /// A scratch folder that cleans up after itself.
    struct Dir(std::path::PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let path = std::env::temp_dir().join(format!("nmscheck_nexus_{tag}"));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("scratch dir");
            Dir(path)
        }

        fn folder(&self, name: &str) -> std::path::PathBuf {
            let path = self.0.join(name);
            std::fs::create_dir_all(&path).expect("mod folder");
            path
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_chain_is_followed_to_its_end() {
        let updates = vec![
            FileUpdate { old_file_id: 1, new_file_id: 2 },
            FileUpdate { old_file_id: 2, new_file_id: 3 },
        ];
        assert_eq!(newest(1, &updates), 3);
        assert_eq!(newest(3, &updates), 3);
    }

    #[test]
    fn a_circular_chain_claims_nothing() {
        // No file in a loop is the newest, so the caller is told the installed
        // one is the end of the line rather than offered a coin-flip.
        let updates = vec![
            FileUpdate { old_file_id: 1, new_file_id: 2 },
            FileUpdate { old_file_id: 2, new_file_id: 1 },
        ];
        assert_eq!(newest(1, &updates), 1);
        assert_eq!(newest(2, &updates), 2);
    }

    #[test]
    fn two_mods_on_one_page_are_told_apart_by_file_name() {
        // Page 1201 really does host both of these.
        let files = vec![
            file(38570, "5.9", "Better Ship Transfer Range 5.9-1201-5-9-1738187904", Some("MAIN")),
            file(41897, "6.0", "Better Ship Teleport Module Range 6.0-1201-6-0-1751870410", Some("MAIN")),
        ];
        let hit = locate("Better Ship Transfer Range 5.9-1201-5-9-1738187904", "5.9", &files).unwrap();
        assert_eq!(hit.file_id, 38570);
    }

    #[test]
    fn an_extension_does_not_stop_a_match() {
        let files = vec![file(
            7,
            "2",
            "All Dot Crosshairs 4511 2 2026-09-17T18-53Z pQZbnWODh",
            Some("MAIN"),
        )];
        assert_eq!(
            locate("All Dot Crosshairs 4511 2 2026-09-17T18-53Z pQZbnWODh.zip", "2", &files)
                .unwrap()
                .file_id,
            7
        );
    }

    #[test]
    fn an_ambiguous_version_is_not_guessed() {
        // Nothing matches by name, and two files share the version, so there
        // is no answer -- inventing one would invent an update.
        let files = vec![
            file(1, "3.0", "Something Else", Some("MAIN")),
            file(2, "3.0", "Another Thing", Some("MAIN")),
        ];
        assert!(locate("Renamed By Vortex", "3.0", &files).is_none());
    }

    #[test]
    fn a_version_must_stand_on_its_own_to_count() {
        // A later release must not be read as the one we are looking for...
        assert!(!version_named_in("Some Mod 1.40", "1.4"));
        assert!(!version_named_in("Some Mod 1.4.1", "1.4"));
        assert!(!version_named_in("Some Mod v11.4", "1.4"));
        assert!(!version_named_in("Some Mod 21.4", "1.4"));
        // ...but a real occurrence must, including before a file extension,
        // which is how the AMUMSS scripts write it.
        assert!(version_named_in("Some Mod 1.40", "1.40"));
        assert!(version_named_in("Unpredictable Shelters 1.4", "1.4"));
        assert!(version_named_in("\"Unpredictable Shelters 1.4.pak\",", "1.4"));
        // Non-numeric versions are real: one mod publishes version "Cosmos".
        assert!(version_named_in("TERRALYSIS Path Of Orion Cosmos", "Cosmos"));
        assert!(!version_named_in("", "1.4"));
        assert!(!version_named_in("Some Mod 1.4", ""));
    }

    #[test]
    fn a_version_in_the_folder_name_does_not_hide_the_script() {
        // `Path::with_extension` would turn `Unpredictable Shelters 1.4` into
        // `Unpredictable Shelters 1.lua`, and the evidence would be missed.
        let dir = Dir::new("dotted");
        let root = dir.folder("Unpredictable Shelters 1.4");
        std::fs::write(
            dir.0.join("Unpredictable Shelters 1.4.lua"),
            "[\"MOD_FILENAME\"] = \"Unpredictable Shelters 1.5.pak\",",
        )
        .unwrap();
        // found in the folder name
        assert!(folder_shows(&root, "1.4"));
        // and in the script, which `with_extension` would never have opened
        assert!(folder_shows(&root, "1.5"));
        assert!(!folder_shows(&root, "1.6"));
    }

    #[test]
    fn a_folder_that_already_holds_the_update_downgrades_it_to_a_stale_record() {
        let dir = Dir::new("stale");
        let root = dir.folder("Unpredictable Shelters 1.4");
        let offered = Standing::Outdated {
            latest_version: "1.4".into(),
            latest_name: "Unpredictable Shelters 1.4".into(),
            latest_file_id: 48895,
            page: page_url(2308),
        };
        assert_eq!(
            Check::corroborate(offered, &root, "1.4"),
            Standing::RecordStale { actual_version: "1.4".into() }
        );
    }

    #[test]
    fn a_real_update_survives_corroboration() {
        let dir = Dir::new("real");
        let root = dir.folder("Unpredictable Shelters 1.3");
        let offered = Standing::Outdated {
            latest_version: "1.4".into(),
            latest_name: "Unpredictable Shelters 1.4".into(),
            latest_file_id: 48895,
            page: page_url(2308),
        };
        assert_eq!(Check::corroborate(offered.clone(), &root, "1.4"), offered);
    }

    #[test]
    fn an_amumss_script_beside_the_folder_counts_as_evidence() {
        let dir = Dir::new("lua");
        let root = dir.folder("Shelters");
        std::fs::write(
            dir.0.join("Shelters.lua"),
            "NMS_MOD_DEFINITION_CONTAINER =\n{\n[\"MOD_FILENAME\"] = \"Unpredictable Shelters 1.4.pak\",\n}",
        )
        .unwrap();
        assert!(folder_shows(&root, "1.4"));
        assert!(!folder_shows(&root, "1.3"));
    }

    #[test]
    fn only_the_categories_nexus_still_offers_count_as_installable() {
        assert!(file(2, "2.0", "Here", Some("MAIN")).offered());
        assert!(!file(1, "2.0", "Gone", Some("ARCHIVED")).offered());
        assert!(!file(3, "2.0", "Nameless", None).offered());
    }
}
