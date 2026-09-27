//! Named sets of mods, to switch between.
//!
//! A preset is *not* a second copy of the library. It is a list of which
//! installed mods should be switched on, so switching presets costs nothing on
//! disk beyond relinking what actually changed: everything stays staged, and
//! [`super::loadout::reconcile`] already takes away what is no longer wanted
//! and puts back what is. "Visuals only" and "everything" can share one copy
//! of every mod they have in common.
//!
//! # A preset names mods, not files
//!
//! It records owners, so a mod that is cleaned, updated or replaced by a merge
//! after the preset was saved still belongs to it. The alternative -- recording
//! the files -- would make every preset go stale the moment a mod changed.
//!
//! # Switching cannot silently drop a mod
//!
//! A preset can name a mod that is no longer installed, because presets
//! outlive mods. Applying one reports those by name ([`Applied::missing`])
//! rather than skipping them quietly, so "my visuals preset lost a mod" is
//! something the user is told about at the moment it happens.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::loadout::Loadout;

/// One named selection of installed mods.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Preset {
    pub name: String,
    /// the mods switched on. Anything installed and not listed is switched off.
    #[serde(default)]
    pub enabled: Vec<String>,
    /// free text, for "the one that works with the multiplayer group"
    #[serde(default)]
    pub note: Option<String>,
}

/// Every preset, and which one is applied.
///
/// A view onto [`super::settings::Settings`], which is where presets actually
/// live -- they are something the user chose, so they belong with the rest of
/// what the user chose rather than in a file of their own. This shape exists
/// because it is what the screen reads, and because "the list, and which one
/// is on" is one answer rather than two.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Presets {
    #[serde(default)]
    pub presets: Vec<Preset>,
    #[serde(default)]
    pub active: Option<String>,
}

impl Presets {
    pub fn of(settings: &super::settings::Settings) -> Presets {
        Presets {
            presets: settings.presets.clone(),
            active: settings.active_preset.clone(),
        }
    }

    pub fn get(&self, name: &str) -> Option<&Preset> {
        self.presets.iter().find(|p| p.name == name)
    }
}

/// What applying a preset changed, before anything is deployed.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Applied {
    pub switched_on: Vec<String>,
    pub switched_off: Vec<String>,
    /// mods the preset names that are not installed any more
    pub missing: Vec<String>,
}

/// Make a preset out of whatever is switched on right now.
pub fn capture(loadout: &Loadout, name: &str, note: Option<String>) -> Result<Preset, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("a preset needs a name".into());
    }
    let mut enabled: Vec<String> = loadout
        .entries
        .iter()
        .filter(|e| e.enabled)
        .map(|e| e.owner.clone())
        .collect();
    enabled.sort();
    Ok(Preset {
        name: name.to_string(),
        enabled,
        note,
    })
}

/// Switch the loadout over to a preset, without deploying anything yet.
///
/// Nothing is written to the game here. The caller reconciles afterwards,
/// which is what makes this safe to show as a preview first.
pub fn apply(loadout: &mut Loadout, preset: &Preset) -> Applied {
    let want: BTreeSet<&str> = preset.enabled.iter().map(String::as_str).collect();
    let installed: BTreeSet<&str> = loadout.entries.iter().map(|e| e.owner.as_str()).collect();

    let mut done = Applied {
        missing: want
            .difference(&installed)
            .map(|s| (*s).to_string())
            .collect(),
        ..Applied::default()
    };

    for entry in &mut loadout.entries {
        let should = want.contains(entry.owner.as_str());
        if should == entry.enabled {
            continue;
        }
        if should {
            done.switched_on.push(entry.owner.clone());
        } else {
            done.switched_off.push(entry.owner.clone());
        }
        entry.enabled = should;
    }

    done.switched_on.sort();
    done.switched_off.sort();
    done
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::loadout::{Entry, Variant};

    fn book(mods: &[(&str, bool)]) -> Loadout {
        let mut out = Loadout::default();
        for (owner, enabled) in mods {
            out.put(Entry {
                owner: (*owner).into(),
                source: format!("/s/{owner}"),
                origin: None,
                archive: None,
                variant: Variant::Original,
                replaces: Vec::new(),
                deployed: Vec::new(),
                built_from: None,
                enabled: *enabled,
                edited: false,
            });
        }
        out
    }

    #[test]
    fn capturing_takes_what_is_switched_on_and_nothing_else() {
        let loadout = book(&[("A", true), ("B", false), ("C", true)]);
        let preset = capture(&loadout, "Visuals", None).unwrap();
        assert_eq!(preset.enabled, vec!["A", "C"]);
    }

    #[test]
    fn a_preset_needs_a_name() {
        let loadout = book(&[("A", true)]);
        assert!(capture(&loadout, "   ", None).is_err());
    }

    #[test]
    fn applying_switches_both_ways() {
        let mut loadout = book(&[("A", true), ("B", false), ("C", true)]);
        let preset = Preset {
            name: "Visuals".into(),
            enabled: vec!["B".into(), "C".into()],
            note: None,
        };
        let done = apply(&mut loadout, &preset);

        assert_eq!(done.switched_on, vec!["B"]);
        assert_eq!(done.switched_off, vec!["A"]);
        assert!(done.missing.is_empty());
        assert!(!loadout.get("A").unwrap().enabled);
        assert!(loadout.get("B").unwrap().enabled);
        assert!(loadout.get("C").unwrap().enabled, "already on, left alone");
    }

    #[test]
    fn a_preset_naming_an_uninstalled_mod_says_so_instead_of_skipping_it() {
        // Presets outlive mods. Losing one quietly is how a user ends up
        // wondering why their saved setup is not the setup they saved.
        let mut loadout = book(&[("A", false)]);
        let preset = Preset {
            name: "Old".into(),
            enabled: vec!["A".into(), "Uninstalled Mod".into()],
            note: None,
        };
        let done = apply(&mut loadout, &preset);

        assert_eq!(done.missing, vec!["Uninstalled Mod"]);
        assert_eq!(done.switched_on, vec!["A"], "the rest still applied");
    }

    #[test]
    fn applying_the_same_preset_twice_changes_nothing_the_second_time() {
        let mut loadout = book(&[("A", true), ("B", false)]);
        let preset = Preset {
            name: "P".into(),
            enabled: vec!["B".into()],
            note: None,
        };
        apply(&mut loadout, &preset);
        let again = apply(&mut loadout, &preset);
        assert!(again.switched_on.is_empty());
        assert!(again.switched_off.is_empty());
    }

    #[test]
    fn an_empty_preset_switches_everything_off_without_uninstalling() {
        let mut loadout = book(&[("A", true), ("B", true)]);
        let preset = Preset {
            name: "Vanilla".into(),
            enabled: Vec::new(),
            note: None,
        };
        let done = apply(&mut loadout, &preset);

        assert_eq!(done.switched_off, vec!["A", "B"]);
        // Still installed, still staged -- switching back on costs a relink.
        assert_eq!(loadout.entries.len(), 2);
    }

    #[test]
    fn deleting_the_active_preset_leaves_none_active() {
        let mut settings = crate::engine::settings::Settings::fresh();
        settings.put_preset(Preset {
            name: "P".into(),
            enabled: vec!["A".into()],
            note: None,
        });
        settings.active_preset = Some("P".into());
        settings.forget_preset("P");

        assert!(settings.preset("P").is_none());
        assert_eq!(settings.active_preset, None);
    }

    #[test]
    fn saving_over_a_preset_replaces_it_rather_than_adding_a_twin() {
        let mut settings = crate::engine::settings::Settings::fresh();
        for enabled in [vec!["A".to_string()], vec!["B".to_string()]] {
            settings.put_preset(Preset {
                name: "P".into(),
                enabled,
                note: None,
            });
        }
        assert_eq!(settings.presets.len(), 1);
        assert_eq!(settings.preset("P").unwrap().enabled, vec!["B"]);
    }

    #[test]
    fn the_view_the_screen_reads_carries_both_the_list_and_which_is_on() {
        let mut settings = crate::engine::settings::Settings::fresh();
        settings.put_preset(Preset {
            name: "Visuals".into(),
            enabled: vec!["A".into()],
            note: Some("for screenshots".into()),
        });
        settings.active_preset = Some("Visuals".into());

        let shown = Presets::of(&settings);
        assert_eq!(shown.active.as_deref(), Some("Visuals"));
        assert_eq!(
            shown.get("Visuals").unwrap().note.as_deref(),
            Some("for screenshots")
        );
    }
}
