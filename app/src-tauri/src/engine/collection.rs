//! A mod list one player can hand to another.
//!
//! A preset ([`super::preset`]) names *folders in this install*, which is
//! exactly right while it stays on this machine and useless the moment it
//! leaves: the folder a mod deploys under is the top folder inside whatever
//! archive its author happened to zip, so the same mod can sit under
//! `MOD.PM_PortalGlyphs` here and under something else there. Sent as-is, a
//! preset would arrive naming nothing the recipient has.
//!
//! So a collection records, per mod, everything that could identify it, and
//! [`plan`] matches on whichever of them the receiving library can answer:
//!
//! ```text
//! owner     the folder name              exact when both installed the same archive
//! mod_id    the Nexus page               a page can host several mods -- see below
//! name      the title a person reads     the fallback, and the one shown when nothing matches
//! ```
//!
//! # A Nexus page is not a mod
//!
//! Page 1371 hosts four different mods and page 1201 hosts two, so pairing on
//! `mod_id` alone will confidently match "Better Ship Transfer Range" to
//! "Better Ship Teleport Module Range". [`plan`] therefore uses a page as
//! evidence only when it names exactly one mod on *both* sides; otherwise it
//! falls through to the title. That is the same rule [`super::library`] already
//! applies when it decides whether a page title may be used as a mod's name.
//!
//! # What a collection is not
//!
//! It carries no paths, no settings and no API key. It is a list of mods and
//! nothing else, so there is nothing in one that a person would need to read
//! through before sending it.
//!
//! # Importing does not install
//!
//! [`plan`] is read-only and reports three groups -- what the recipient has,
//! what they are missing, and which of *their* enabled mods the list does not
//! mention. That last group is the one a naive import loses: applying a list
//! switches off everything not on it, and a mod going quiet without being
//! named is the failure this program exists to prevent.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::nexus;
use super::preset::Preset;

/// Marks a file as one of ours, so a JSON file that is something else entirely
/// is refused by name rather than by a deserialisation error about a field.
pub const FORMAT: &str = "anomaly.modlist";

/// What this file was called before the program was renamed.
///
/// Read but never written. The tag is a *file format*, not a brand: a list
/// exported last week, or one a friend was sent, is the same list whatever the
/// program that wrote it was called that day, and refusing it over a name
/// change would be this program breaking its own files for no reason.
pub const FORMAT_WAS: &str = "nmscheck.modlist";

/// Bumped only for a change that an older build could not read correctly.
/// Unknown *fields* are ignored, so adding one does not need a new version.
pub const VERSION: u32 = 1;

/// A shareable mod list.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Collection {
    pub format: String,
    pub version: u32,
    pub name: String,
    /// free text from whoever exported it: "the co-op group's setup"
    #[serde(default)]
    pub note: Option<String>,
    /// when it was exported, so a recipient can tell a year-old list from a
    /// fresh one without opening it
    #[serde(default)]
    pub exported: Option<String>,
    /// the game version it was exported against, when that is known
    #[serde(default)]
    pub game_version: Option<String>,
    pub mods: Vec<Listed>,
}

/// One mod on a list, described every way we can describe it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Listed {
    /// the folder it deployed under on the machine that exported it
    pub owner: String,
    /// the title a person would recognise
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub mod_id: Option<u64>,
    #[serde(default)]
    pub version: Option<String>,
    /// the archive name, which carries the id, the version and the upload
    /// time. Kept whole because it is the one string that can be matched
    /// against a download the recipient may already have.
    #[serde(default)]
    pub archive: Option<String>,
}

impl Listed {
    /// The title to show, which is never empty: the name, else the folder.
    pub fn shown(&self) -> &str {
        match self.name.as_deref() {
            Some(name) if !name.trim().is_empty() => name,
            _ => &self.owner,
        }
    }

    /// Where a person would go to get this, when we know which page.
    pub fn page(&self) -> Option<String> {
        self.mod_id
            .map(|id| format!("https://www.nexusmods.com/{}/mods/{id}", nexus::GAME))
    }
}

/// One mod the receiving library already has.
#[derive(Debug, Clone)]
pub struct Held {
    pub owner: String,
    pub name: String,
    pub mod_id: Option<u64>,
    pub version: Option<String>,
    /// whether it is switched on right now, which decides what `extra` means
    pub enabled: bool,
}

/// Build a list out of the mods named by `owners`, in the order given.
pub fn compose(
    name: &str,
    note: Option<String>,
    game_version: Option<String>,
    exported: Option<String>,
    owners: &[String],
    held: &[Held],
    archives: &BTreeMap<String, String>,
) -> Result<Collection, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a mod list needs a name".into());
    }
    let by_owner: BTreeMap<&str, &Held> = held.iter().map(|h| (h.owner.as_str(), h)).collect();

    let mods: Vec<Listed> = owners
        .iter()
        .map(|owner| {
            let known = by_owner.get(owner.as_str());
            Listed {
                owner: owner.clone(),
                name: known.map(|h| h.name.clone()),
                mod_id: known.and_then(|h| h.mod_id),
                version: known.and_then(|h| h.version.clone()),
                archive: archives.get(owner).cloned(),
            }
        })
        .collect();

    if mods.is_empty() {
        return Err("there are no mods to put on the list".into());
    }

    Ok(Collection {
        format: FORMAT.to_string(),
        version: VERSION,
        name: name.to_string(),
        note: note.filter(|n| !n.trim().is_empty()),
        exported,
        game_version,
        mods,
    })
}

/// Read a list someone sent, refusing anything that is not one.
///
/// The two refusals are worded differently on purpose. "Not a mod list" is a
/// wrong-file mistake the user fixes by picking another file; "made by a newer
/// version" is a mistake they fix by updating, and reading it anyway would
/// mean quietly dropping whatever the newer version added.
pub fn read(path: &Path) -> Result<Collection, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("could not read {}: {err}", path.display()))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);

    let found: Collection = serde_json::from_str(text).map_err(|_| {
        format!(
            "{} is not a mod list made by this program",
            path.file_name().unwrap_or(path.as_os_str()).to_string_lossy()
        )
    })?;
    check(found)
}

/// The same validation, for a list that arrived already parsed.
pub fn check(found: Collection) -> Result<Collection, String> {
    if found.format != FORMAT && found.format != FORMAT_WAS {
        return Err("that file is not a mod list made by this program".into());
    }
    if found.version > VERSION {
        return Err(format!(
            "that mod list was made by a newer version of this program (list version {}, this build reads {VERSION}) -- update, and it will open",
            found.version
        ));
    }
    Ok(found)
}

pub fn write(list: &Collection, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let text = serde_json::to_string_pretty(list).map_err(|err| err.to_string())?;
    std::fs::write(path, text).map_err(|err| format!("could not save the mod list: {err}"))
}

/// Every list that has been imported, kept so it can be opened again.
///
/// ---------------------------------------------------------------------------
/// Why the preset is not enough
/// ---------------------------------------------------------------------------
///
/// Importing turns a list into a preset, and a preset is a list of *local
/// folder names*. That is the right shape for switching mods on and off and
/// the wrong shape for everything else the list knows: each mod's Nexus page,
/// its title, the version they were running. Those are exactly the details you
/// need for the mods you do **not** have yet -- and a preset cannot carry
/// them, because a mod nobody has installed has no local folder to name.
///
/// So the list was shown once, in the sheet the import happened in, and then
/// dropped. Working through a sixty-mod list means going to Nexus sixty times,
/// and after the first trip the sheet was gone: the file had to be imported
/// again from disk to get at the second mod's link. Keeping the list is what
/// makes an import something you come back to rather than something you do
/// once.
///
/// One file, read and written whole. A list is a few KB and there are a
/// handful of them, so an index plus a file each would be two things to keep
/// agreeing about for no gain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Saved {
    /// the preset this list was imported as, which is the name it is found by
    pub preset: String,
    pub list: Collection,
}

/// Read the imported lists. A missing or damaged store reads as none.
///
/// Deliberately not an error. This is a convenience over a preset that has
/// already been saved, so a store that cannot be read costs the user the
/// *links* and nothing else -- and failing the screen it is drawn on would
/// cost them the presets too.
pub fn saved_all(path: &Path) -> Vec<Saved> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    serde_json::from_str(text).unwrap_or_default()
}

/// The names of the presets that have a list behind them.
pub fn saved_names(path: &Path) -> Vec<String> {
    saved_all(path).into_iter().map(|one| one.preset).collect()
}

/// One imported list, by the preset it was imported as.
pub fn saved_get(path: &Path, preset: &str) -> Option<Collection> {
    saved_all(path)
        .into_iter()
        .find(|one| one.preset == preset)
        .map(|one| one.list)
}

/// Keep `list` under `preset`, replacing any list already kept under it.
///
/// Re-importing is the ordinary way to refresh a preset as the missing mods
/// arrive, so this replaces rather than accumulates.
pub fn saved_put(path: &Path, preset: &str, list: &Collection) -> Result<(), String> {
    let mut all = saved_all(path);
    all.retain(|one| one.preset != preset);
    all.push(Saved {
        preset: preset.to_string(),
        list: list.clone(),
    });
    saved_write(path, &all)
}

/// Forget the list kept under `preset`, which is what deleting it means.
pub fn saved_drop(path: &Path, preset: &str) -> Result<(), String> {
    let mut all = saved_all(path);
    let before = all.len();
    all.retain(|one| one.preset != preset);
    if all.len() == before {
        return Ok(());
    }
    saved_write(path, &all)
}

fn saved_write(path: &Path, all: &[Saved]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    let text = serde_json::to_string_pretty(all).map_err(|err| err.to_string())?;
    std::fs::write(path, text).map_err(|err| format!("could not save the mod lists: {err}"))
}

/// How a listed mod was recognised, so the screen can say so rather than
/// presenting a guess as a fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum How {
    /// same folder name -- both installed the same archive
    Folder,
    /// same Nexus page, and that page names one mod on each side
    Page,
    /// same title
    Title,
}

/// A listed mod paired with the local mod it turned out to be.
#[derive(Debug, Clone, Serialize)]
pub struct Have {
    pub listed: Listed,
    /// the folder name *here*, which is what a preset has to record
    pub owner: String,
    pub name: String,
    pub how: How,
    pub version: Option<String>,
    /// true when both sides name a version and they are not the same one
    pub differs: bool,
    pub enabled: bool,
}

/// What importing a list would mean for this library.
#[derive(Debug, Clone, Serialize)]
pub struct Plan {
    pub name: String,
    pub note: Option<String>,
    pub exported: Option<String>,
    pub game_version: Option<String>,
    pub have: Vec<Have>,
    pub missing: Vec<Listed>,
    /// mods switched on here that the list does not mention.
    ///
    /// Applying the list switches these off. Naming them is the point: a mod
    /// going quiet without being named is the failure this program exists to
    /// prevent.
    pub extra: Vec<String>,
}

impl Plan {
    /// The preset this list becomes here.
    ///
    /// Two kinds of name go in. For a mod that is installed, the *local* folder
    /// name, because a preset is applied against this library and the sender's
    /// folder name may be nothing like it. For a mod that is not installed, the
    /// sender's name anyway -- a list is usually imported before its mods have
    /// been downloaded, and the folder a mod deploys under comes from inside
    /// the archive, so the same download lands under the same name on both
    /// machines. That turns the preset into something that completes itself as
    /// the missing mods arrive, instead of a list that had to be re-imported
    /// afterwards to be worth anything.
    ///
    /// A name that never resolves is not lost silently: [`super::preset::apply`]
    /// reports it under `missing` every time the preset is switched to.
    pub fn preset(&self, name: &str) -> Result<Preset, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a preset needs a name".into());
        }
        let mut enabled: Vec<String> = self
            .have
            .iter()
            .map(|h| h.owner.clone())
            .chain(self.missing.iter().map(|m| m.owner.clone()))
            .collect();
        enabled.sort();
        enabled.dedup();
        Ok(Preset {
            name: name.to_string(),
            enabled,
            note: self.note.clone(),
        })
    }
}

/// Work out what a list means here, without changing anything.
pub fn plan(list: &Collection, held: &[Held]) -> Plan {
    // Case-insensitively, because the game reports its own mod folders
    // upper-cased and a list exported from a reading of the game would
    // otherwise match nothing.
    let by_folder: BTreeMap<String, &Held> = held
        .iter()
        .map(|h| (h.owner.to_uppercase(), h))
        .collect();

    // A page is evidence only when it names one mod on each side. Page 1371
    // hosts four different mods; pairing on the id alone would match whichever
    // was seen first.
    let here_by_page = one_each(held.iter().filter_map(|h| h.mod_id.map(|id| (id, h))));
    let there_pages: BTreeMap<u64, usize> =
        count(list.mods.iter().filter_map(|m| m.mod_id));

    let by_title = one_each(held.iter().map(|h| (h.name.to_uppercase(), h)));
    let there_titles: BTreeMap<String, usize> =
        count(list.mods.iter().map(|m| m.shown().to_uppercase()));

    let mut have: Vec<Have> = Vec::new();
    let mut missing: Vec<Listed> = Vec::new();
    let mut claimed: std::collections::BTreeSet<String> = Default::default();

    for listed in &list.mods {
        let found = by_folder
            .get(&listed.owner.to_uppercase())
            .map(|h| (*h, How::Folder))
            .or_else(|| {
                let id = listed.mod_id?;
                (there_pages.get(&id).copied().unwrap_or(0) == 1)
                    .then(|| here_by_page.get(&id).copied())
                    .flatten()
                    .map(|h| (h, How::Page))
            })
            .or_else(|| {
                let title = listed.shown().to_uppercase();
                (there_titles.get(&title).copied().unwrap_or(0) == 1)
                    .then(|| by_title.get(&title).copied())
                    .flatten()
                    .map(|h| (h, How::Title))
            })
            // One local mod cannot stand in for two listed ones. Without this,
            // a list naming two variants of a mod where the recipient has one
            // would report both as present and the preset would name it twice.
            .filter(|(h, _)| !claimed.contains(&h.owner));

        match found {
            Some((h, how)) => {
                claimed.insert(h.owner.clone());
                have.push(Have {
                    differs: match (listed.version.as_deref(), h.version.as_deref()) {
                        (Some(theirs), Some(ours)) => theirs.trim() != ours.trim(),
                        _ => false,
                    },
                    listed: listed.clone(),
                    owner: h.owner.clone(),
                    name: h.name.clone(),
                    how,
                    version: h.version.clone(),
                    enabled: h.enabled,
                });
            }
            None => missing.push(listed.clone()),
        }
    }

    let mut extra: Vec<String> = held
        .iter()
        .filter(|h| h.enabled && !claimed.contains(&h.owner))
        .map(|h| h.name.clone())
        .collect();
    extra.sort();

    Plan {
        name: list.name.clone(),
        note: list.note.clone(),
        exported: list.exported.clone(),
        game_version: list.game_version.clone(),
        have,
        missing,
        extra,
    }
}

fn count<K: Ord>(items: impl IntoIterator<Item = K>) -> BTreeMap<K, usize> {
    let mut out = BTreeMap::new();
    for key in items {
        *out.entry(key).or_insert(0) += 1;
    }
    out
}

/// Keep only the keys that name exactly one thing.
fn one_each<'a, K: Ord>(
    items: impl IntoIterator<Item = (K, &'a Held)>,
) -> BTreeMap<K, &'a Held> {
    let mut out: BTreeMap<K, Option<&Held>> = BTreeMap::new();
    for (key, held) in items {
        out.entry(key)
            // Seen before: it names two mods, so it names neither.
            .and_modify(|slot| *slot = None)
            .or_insert(Some(held));
    }
    out.into_iter()
        .filter_map(|(key, held)| held.map(|h| (key, h)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(owner: &str, name: &str, mod_id: Option<u64>, version: Option<&str>) -> Held {
        Held {
            owner: owner.into(),
            name: name.into(),
            mod_id,
            version: version.map(str::to_string),
            enabled: true,
        }
    }

    fn listed(owner: &str, name: &str, mod_id: Option<u64>, version: Option<&str>) -> Listed {
        Listed {
            owner: owner.into(),
            name: Some(name.into()),
            mod_id,
            version: version.map(str::to_string),
            archive: None,
        }
    }

    fn list(mods: Vec<Listed>) -> Collection {
        Collection {
            format: FORMAT.into(),
            version: VERSION,
            name: "Co-op setup".into(),
            note: None,
            exported: None,
            game_version: None,
            mods,
        }
    }

    #[test]
    fn the_same_folder_is_the_same_mod() {
        let here = [held("MOD.Glyphs", "Smooth Glyphs", Some(3071), Some("1.1"))];
        let plan = plan(&list(vec![listed("MOD.Glyphs", "Smooth Glyphs", Some(3071), Some("1.1"))]), &here);

        assert_eq!(plan.have.len(), 1);
        assert_eq!(plan.have[0].how, How::Folder);
        assert!(!plan.have[0].differs);
        assert!(plan.missing.is_empty());
        assert!(plan.extra.is_empty());
    }

    #[test]
    fn the_game_upper_casing_a_folder_does_not_hide_a_match() {
        // The game reports its mod folders upper-cased, so a list exported from
        // a reading of the game carries them that way.
        let here = [held("MOD.Glyphs", "Smooth Glyphs", None, None)];
        let plan = plan(&list(vec![listed("MOD.GLYPHS", "Smooth Glyphs", None, None)]), &here);
        assert_eq!(plan.have.len(), 1, "{:?}", plan.missing);
        assert_eq!(plan.have[0].owner, "MOD.Glyphs", "the LOCAL folder name");
    }

    #[test]
    fn a_different_folder_for_the_same_page_still_matches() {
        // The deployed folder is whatever was inside the archive, so two people
        // running the same mod can have different folder names for it.
        let here = [held("Glyphs", "Smooth Glyphs", Some(3071), None)];
        let plan = plan(&list(vec![listed("MOD.PM_PortalGlyphs", "Smooth Glyphs", Some(3071), None)]), &here);

        assert_eq!(plan.have.len(), 1);
        assert_eq!(plan.have[0].how, How::Page);
        assert_eq!(plan.have[0].owner, "Glyphs");
    }

    #[test]
    fn a_page_hosting_two_mods_is_not_evidence_of_anything() {
        // Page 1201 hosts both "Better Ship Transfer Range" and "Better Ship
        // Teleport Module Range". Matching on the id alone pairs whichever was
        // seen first, confidently and wrongly.
        let here = [
            held("TransferRange", "Better Ship Transfer Range", Some(1201), None),
            held("TeleportRange", "Better Ship Teleport Module Range", Some(1201), None),
        ];
        let plan = plan(
            &list(vec![listed("Whatever", "Better Ship Teleport Module Range", Some(1201), None)]),
            &here,
        );

        assert_eq!(plan.have.len(), 1);
        assert_eq!(plan.have[0].how, How::Title, "the title told them apart");
        assert_eq!(plan.have[0].owner, "TeleportRange");
    }

    #[test]
    fn a_title_matches_when_nothing_better_does() {
        let here = [held("A_Folder", "Accelerated Settlements", None, None)];
        let plan = plan(&list(vec![listed("Another Folder", "accelerated settlements", None, None)]), &here);
        assert_eq!(plan.have.len(), 1);
        assert_eq!(plan.have[0].how, How::Title);
    }

    #[test]
    fn one_installed_mod_cannot_stand_in_for_two_listed_ones() {
        let here = [held("Glyphs", "Smooth Glyphs", Some(3071), None)];
        let plan = plan(
            &list(vec![
                listed("Glyphs", "Smooth Glyphs", Some(3071), None),
                listed("GlyphsHD", "Smooth Glyphs HD", Some(3071), None),
            ]),
            &here,
        );

        assert_eq!(plan.have.len(), 1);
        assert_eq!(plan.missing.len(), 1);
        assert_eq!(plan.missing[0].owner, "GlyphsHD");
    }

    #[test]
    fn a_mod_the_list_does_not_mention_is_named_rather_than_dropped() {
        // Applying the list switches this off. A mod going quiet without being
        // named is the failure this program exists to prevent.
        let mut off = held("Switched Off", "Something Off", None, None);
        off.enabled = false;
        let here = [
            held("Glyphs", "Smooth Glyphs", Some(3071), None),
            held("Mine", "My Own Mod", None, None),
            off,
        ];
        let plan = plan(&list(vec![listed("Glyphs", "Smooth Glyphs", Some(3071), None)]), &here);

        assert_eq!(plan.extra, vec!["My Own Mod"]);
        assert!(
            !plan.extra.contains(&"Something Off".to_string()),
            "already off, so applying the list takes nothing away"
        );
    }

    #[test]
    fn a_version_that_does_not_match_is_flagged_and_not_treated_as_missing() {
        let here = [held("Glyphs", "Smooth Glyphs", Some(3071), Some("1.0"))];
        let plan = plan(&list(vec![listed("Glyphs", "Smooth Glyphs", Some(3071), Some("1.1"))]), &here);

        assert_eq!(plan.have.len(), 1);
        assert!(plan.have[0].differs);
        assert_eq!(plan.have[0].version.as_deref(), Some("1.0"), "ours, not theirs");
    }

    #[test]
    fn a_version_nobody_published_is_not_a_disagreement() {
        let here = [held("Glyphs", "Smooth Glyphs", None, None)];
        let plan = plan(&list(vec![listed("Glyphs", "Smooth Glyphs", None, Some("1.1"))]), &here);
        assert!(!plan.have[0].differs, "one side saying nothing is not a conflict");
    }

    #[test]
    fn the_preset_names_local_folders_not_the_senders() {
        let here = [held("Glyphs", "Smooth Glyphs", Some(3071), None)];
        let plan = plan(&list(vec![listed("MOD.PM_PortalGlyphs", "Smooth Glyphs", Some(3071), None)]), &here);
        let preset = plan.preset("Co-op setup").unwrap();

        assert_eq!(preset.enabled, vec!["Glyphs"]);
        assert!(preset.enabled.iter().all(|o| o != "MOD.PM_PortalGlyphs"));
    }

    #[test]
    fn a_mod_not_installed_yet_is_still_on_the_preset() {
        // A list is usually imported *before* its mods have been downloaded.
        // Leaving these out would make the preset worthless until the whole
        // list was imported a second time.
        let here = [held("Glyphs", "Smooth Glyphs", Some(3071), None)];
        let plan = plan(
            &list(vec![
                listed("Glyphs", "Smooth Glyphs", Some(3071), None),
                listed("NotYet", "Something Else", Some(9999), None),
            ]),
            &here,
        );
        let preset = plan.preset("Co-op").unwrap();

        assert_eq!(preset.enabled, vec!["Glyphs", "NotYet"]);
        assert_eq!(plan.missing.len(), 1, "still reported as not installed");
    }

    #[test]
    fn a_preset_made_from_a_list_still_needs_a_name() {
        let plan = plan(&list(vec![listed("A", "A", None, None)]), &[]);
        assert!(plan.preset("  ").is_err());
    }

    #[test]
    fn composing_records_every_way_of_naming_a_mod() {
        let here = [held("Glyphs", "Smooth Glyphs", Some(3071), Some("1.1"))];
        let archives = BTreeMap::from([(
            "Glyphs".to_string(),
            "Smooth glyphs-3071-1-1-1739735732".to_string(),
        )]);
        let made = compose(
            "Co-op",
            Some("  ".into()),
            Some("7.03".into()),
            Some("2026-09-26".into()),
            &["Glyphs".to_string()],
            &here,
            &archives,
        )
        .unwrap();

        assert_eq!(made.format, FORMAT);
        assert_eq!(made.mods[0].mod_id, Some(3071));
        assert_eq!(made.mods[0].name.as_deref(), Some("Smooth Glyphs"));
        assert_eq!(made.mods[0].version.as_deref(), Some("1.1"));
        assert!(made.mods[0].archive.is_some());
        assert_eq!(made.note, None, "a note of only spaces is no note");
    }

    /// A rename of the program must not orphan the files it has already
    /// written. New lists carry the new tag; old ones still open.
    #[test]
    fn a_list_written_under_the_old_name_still_opens() {
        let mut made = Collection {
            format: FORMAT_WAS.into(),
            version: VERSION,
            name: "Old".into(),
            note: None,
            game_version: None,
            exported: None,
            mods: vec![listed("Glyphs", "Smooth Glyphs", Some(3071), Some("1.1"))],
        };
        assert!(check(made.clone()).is_ok(), "a list from before the rename");

        made.format = FORMAT.into();
        assert!(check(made.clone()).is_ok(), "and one from after it");

        made.format = "something.else".into();
        assert!(check(made).is_err(), "but not a JSON file that is not ours");
    }

    #[test]
    fn a_list_needs_a_name_and_at_least_one_mod() {
        let here = [held("Glyphs", "Smooth Glyphs", None, None)];
        let archives = BTreeMap::new();
        assert!(compose(" ", None, None, None, &["Glyphs".into()], &here, &archives).is_err());
        assert!(compose("Co-op", None, None, None, &[], &here, &archives).is_err());
    }

    #[test]
    fn a_mod_with_nothing_known_about_it_is_still_listed_by_folder() {
        // Better a list the recipient has to match by hand than a list that
        // silently leaves a mod out.
        let archives = BTreeMap::new();
        let made = compose("Co-op", None, None, None, &["Mystery".into()], &[], &archives).unwrap();
        assert_eq!(made.mods.len(), 1);
        assert_eq!(made.mods[0].owner, "Mystery");
        assert_eq!(made.mods[0].shown(), "Mystery");
    }

    #[test]
    fn an_imported_list_is_kept_and_can_be_opened_again() {
        // The bug this exists for: the list was shown once, in the sheet the
        // import happened in, and then dropped. Getting at the second mod's
        // Nexus page meant importing the file again.
        let dir = std::env::temp_dir().join("anomaly_saved_lists");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("lists.json");

        assert!(saved_all(&path).is_empty(), "nothing kept yet");
        assert_eq!(saved_get(&path, "Co-op"), None);

        let made = list(vec![listed("Glyphs", "Smooth Glyphs", Some(3071), Some("1.1"))]);
        saved_put(&path, "Co-op", &made).unwrap();

        assert_eq!(saved_names(&path), vec!["Co-op".to_string()]);
        assert_eq!(saved_get(&path, "Co-op").unwrap(), made);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn importing_the_same_list_again_replaces_it_rather_than_stacking_up() {
        // Re-importing is how a preset is refreshed as the missing mods
        // arrive, so it must not leave two lists under one name.
        let dir = std::env::temp_dir().join("anomaly_saved_replace");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("lists.json");

        saved_put(&path, "Co-op", &list(vec![listed("A", "A", None, None)])).unwrap();
        let grown = list(vec![
            listed("A", "A", None, None),
            listed("B", "B", None, None),
        ]);
        saved_put(&path, "Co-op", &grown).unwrap();

        assert_eq!(saved_all(&path).len(), 1, "one entry, not two");
        assert_eq!(saved_get(&path, "Co-op").unwrap().mods.len(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn deleting_the_preset_forgets_its_list_and_leaves_the_others() {
        let dir = std::env::temp_dir().join("anomaly_saved_drop");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("lists.json");

        saved_put(&path, "Co-op", &list(vec![listed("A", "A", None, None)])).unwrap();
        saved_put(&path, "Visuals", &list(vec![listed("B", "B", None, None)])).unwrap();

        saved_drop(&path, "Co-op").unwrap();
        assert_eq!(saved_names(&path), vec!["Visuals".to_string()]);

        // Dropping one that was never kept is not an error: a preset saved by
        // hand has no list behind it, and deleting it is ordinary.
        saved_drop(&path, "Co-op").unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_store_that_cannot_be_read_costs_the_links_and_nothing_else() {
        // It is a convenience over presets that are already saved elsewhere,
        // so a damaged file reads as "no lists kept" rather than failing the
        // screen it is drawn on -- which would take the presets with it.
        let dir = std::env::temp_dir().join("anomaly_saved_damaged");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lists.json");

        std::fs::write(&path, "{ this is not json").unwrap();
        assert!(saved_all(&path).is_empty());
        assert_eq!(saved_get(&path, "Co-op"), None);

        // And it is repaired by the next import rather than staying broken.
        saved_put(&path, "Co-op", &list(vec![listed("A", "A", None, None)])).unwrap();
        assert_eq!(saved_names(&path), vec!["Co-op".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_json_file_that_is_not_a_mod_list_is_refused_by_name() {
        let dir = std::env::temp_dir().join("nmscheck_collection_read");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"mods_dir":"D:\\Game\\MODS"}"#).unwrap();
        let said = read(&path).unwrap_err();
        assert!(said.contains("not a mod list"), "{said}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_list_from_a_newer_build_says_so_rather_than_dropping_what_it_cannot_read() {
        let mut ahead = list(vec![listed("A", "A", None, None)]);
        ahead.version = VERSION + 1;
        let said = check(ahead).unwrap_err();
        assert!(said.contains("newer version"), "{said}");
    }

    #[test]
    fn a_list_survives_a_round_trip_through_disk() {
        let dir = std::env::temp_dir().join("nmscheck_collection_roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("co-op.nmslist.json");

        let made = list(vec![listed("Glyphs", "Smooth Glyphs", Some(3071), Some("1.1"))]);
        write(&made, &path).unwrap();
        assert_eq!(read(&path).unwrap(), made);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_list_saved_by_a_windows_tool_still_opens() {
        // Same trap as the loadout and the settings: Notepad and PowerShell
        // both write a BOM, and a shared file is exactly the kind of thing
        // someone opens in Notepad to look at before sending it on.
        let dir = std::env::temp_dir().join("nmscheck_collection_bom");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("co-op.json");

        let made = list(vec![listed("Glyphs", "Smooth Glyphs", None, None)]);
        let text = serde_json::to_string(&made).unwrap();
        std::fs::write(&path, format!("\u{feff}{text}")).unwrap();

        assert_eq!(read(&path).unwrap().mods.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_page_url_is_offered_for_every_mod_we_know_the_page_of() {
        let one = listed("A", "A", Some(3071), None);
        assert_eq!(
            one.page().as_deref(),
            Some("https://www.nexusmods.com/nomanssky/mods/3071")
        );
        assert!(listed("B", "B", None, None).page().is_none());
    }
}
