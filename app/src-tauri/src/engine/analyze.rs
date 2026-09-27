//! Conflict detection across a scanned mod library.
//!
//! Port of `nmscc/analyze.py`. Two facts shape the whole analysis.
//!
//! **Mods ship two very different kinds of EXML.** Some are *fragments*
//! naming only the handful of properties the mod overrides; others are *full
//! assets*, the entire decompiled table. Both live at the same asset path, so
//! "two mods touch REWARDTABLE" says almost nothing on its own. Telling the
//! two kinds apart reliably needs vanilla game data, which this tool
//! deliberately does not require — so it never guesses.
//!
//! **Property overlap is the signal that does not need vanilla data.**
//! Whatever the file size, if two mods both write `ShipInteractRadius` and
//! disagree, that is a real conflict; if their properties are disjoint, they
//! coexist. Severity follows the overlap, not the file.
//!
//! Attribution — deciding *which* mod authored a difference — is extra
//! confidence on top of a clash, never a precondition for reporting one:
//! AMUMSS markers first, then a majority-of-three vote standing in for
//! vanilla, and with only two unannotated copies no blame is assigned.
//!
//! Iteration order is load-bearing here: the Python's dicts preserve insertion
//! order and its sorts are stable, so `IndexMap` and `sort_by` are used
//! throughout to keep the two engines byte-identical.

use std::collections::{HashMap, HashSet};

use indexmap::{IndexMap, IndexSet};

use super::exml::values_equal;
use super::hostenv::HostInfo;
use super::model::{
    BrokenFile, Conflict, FieldClash, FileKind, LocClash, Mod, ModFile, Report, ScanStats,
    Severity, StaleFinding,
};
use super::paths::declared_matches;
use super::version::Version;

/// How many clashing fields to keep per conflict before truncating.
pub const MAX_CLASHES: usize = 40;

/// A mod this far behind the reference minor version is called out.
pub const STALE_MINOR_GAP: i64 = 1;
pub const SEVERELY_STALE_MINOR_GAP: i64 = 4;

/// Property count above which a copy is treated as a whole-asset replacement
/// rather than a small patch. Real fragments run to a few dozen properties;
/// decompiled game tables run to thousands, so the boundary is wide and the
/// exact value is not load-bearing.
pub const FULL_ASSET_PROPS: usize = 500;

/// Which end of the load order wins a clash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinnerRule {
    First,
    Last,
}

impl WinnerRule {
    pub fn as_str(self) -> &'static str {
        match self {
            WinnerRule::First => "first",
            WinnerRule::Last => "last",
        }
    }
}

/// True when a property map is available for this asset.
///
/// Covers shipped EXML and any compiled asset decompiled along the way, so the
/// two are treated identically from here on.
fn is_comparable(entry: &ModFile) -> bool {
    entry.kind == Some(FileKind::Exml)
        || entry.decompiled
        || !entry.props.is_empty()
        || super::propcache::lookup(&entry.sha1).is_some_and(|d| !d.props.is_empty())
}

type Entry<'a> = (&'a Mod, &'a ModFile);

/// Map every canonical target to the mods that ship a file for it.
fn providers<'a>(mods: &'a [Mod]) -> IndexMap<String, Vec<Entry<'a>>> {
    let mut index: IndexMap<String, Vec<Entry<'a>>> = IndexMap::new();
    for the_mod in mods {
        for entry in the_mod.assets() {
            if let Some(target) = &entry.target {
                index.entry(target.clone()).or_default().push((the_mod, entry));
            }
        }
    }
    index
}

/// The modal value of a path, with ties broken by first appearance.
///
/// Mirrors `Counter.most_common(1)[0]`, whose tie-break is insertion order.
fn modal<'a>(per_mod: &'a IndexMap<String, Option<String>>) -> Option<(&'a str, usize)> {
    let mut counts: IndexMap<&str, usize> = IndexMap::new();
    for value in per_mod.values().flatten() {
        *counts.entry(value.as_str()).or_insert(0) += 1;
    }
    let mut best: Option<(&str, usize)> = None;
    for (value, count) in &counts {
        match best {
            Some((_, best_count)) if *count <= best_count => {}
            _ => best = Some((value, *count)),
        }
    }
    best
}

/// Work out which property paths each mod actually authored.
///
/// Returns mod name -> set of paths, or `None` when attribution is impossible
/// for that mod.
fn attribute(
    entries: &[Entry<'_>],
    values: &IndexMap<String, IndexMap<String, Option<String>>>,
) -> IndexMap<String, Option<IndexSet<String>>> {
    let mut result: IndexMap<String, Option<IndexSet<String>>> = IndexMap::new();
    let comparable = entries.iter().filter(|(_, f)| is_comparable(f)).count();

    for (the_mod, entry) in entries {
        let annotations = super::propcache::annotations_of(entry);
        if !annotations.is_empty() {
            result.insert(
                the_mod.name.clone(),
                Some(annotations.keys().cloned().collect()),
            );
        } else if comparable >= 3 && is_comparable(entry) {
            let mut authored: IndexSet<String> = IndexSet::new();
            for (path, per_mod) in values {
                let Some(mine) = per_mod.get(&the_mod.name) else {
                    continue;
                };
                let Some((modal_value, modal_n)) = modal(per_mod) else {
                    continue;
                };
                // The majority reading stands in for vanilla; only trust it
                // when the majority is genuinely a majority.
                if modal_n > 1 && !values_equal(mine.as_deref(), Some(modal_value)) {
                    authored.insert(path.clone());
                }
            }
            result.insert(the_mod.name.clone(), Some(authored));
        } else {
            result.insert(the_mod.name.clone(), None);
        }
    }
    result
}

/// Outcome of diffing every EXML copy of one target.
struct Comparison {
    clashes: Vec<FieldClash>,
    /// shared properties in total (agreed + clashing)
    overlap: usize,
    /// mod name -> how many properties that copy defines at all
    prop_counts: IndexMap<String, usize>,
    /// mod name -> properties only that copy defines
    unique_counts: IndexMap<String, usize>,
    attribution: IndexMap<String, Option<IndexSet<String>>>,
}

/// Diff the EXML providers of one target.
///
/// Only properties that at least two copies actually define are considered. A
/// property present in a single copy says nothing about conflict: it is simply
/// the one mod that cares about it, whether the copy is a small fragment or a
/// full table.
fn compare_exml(entries: &[Entry<'_>]) -> Comparison {
    let mut values: IndexMap<String, IndexMap<String, Option<String>>> = IndexMap::new();
    let mut prop_counts: IndexMap<String, usize> = IndexMap::new();

    for (the_mod, entry) in entries {
        // Loaded per entry rather than read off the scan: the scan no longer
        // keeps them, and this runs for ONE contested target at a time, so the
        // peak is that target's copies instead of the whole library.
        let props = super::propcache::props_of(entry);
        prop_counts.insert(the_mod.name.clone(), props.len());
        for (path, value) in props.iter() {
            values
                .entry(path.clone())
                .or_default()
                .insert(the_mod.name.clone(), value.clone());
        }
    }

    let attribution = attribute(entries, &values);

    let mut clashes: Vec<FieldClash> = Vec::new();
    let mut unique_counts: IndexMap<String, usize> = IndexMap::new();
    let mut overlap = 0usize;

    for (path, per_mod) in &values {
        if per_mod.len() < 2 {
            if let Some(only) = per_mod.keys().next() {
                *unique_counts.entry(only.clone()).or_insert(0) += 1;
            }
            continue;
        }
        overlap += 1;

        let first = per_mod.values().next().and_then(|v| v.as_deref());
        if per_mod
            .values()
            .all(|v| values_equal(first, v.as_deref()))
        {
            continue;
        }

        let authors: Vec<String> = per_mod
            .keys()
            .filter(|name| {
                attribution
                    .get(*name)
                    .and_then(|a| a.as_ref())
                    // `if attribution.get(name) and path in ...`: an empty set
                    // is falsy in Python, so it attributes nobody.
                    .is_some_and(|paths| !paths.is_empty() && paths.contains(path))
            })
            .cloned()
            .collect();

        clashes.push(FieldClash {
            path: path.clone(),
            values: per_mod.clone(),
            attributed: authors,
        });
    }

    clashes.sort_by(|a, b| {
        b.attributed
            .len()
            .cmp(&a.attributed.len())
            .then_with(|| a.path.cmp(&b.path))
    });

    Comparison {
        clashes,
        overlap,
        prop_counts,
        unique_counts,
        attribution,
    }
}

/// Work out which mod wins a clash.
///
/// When the game has written `GCMODSETTINGS.MXML` its `ModPriority` values are
/// the real ordering and are used directly. Without that file the folder names
/// are sorted alphabetically instead, which matches the order the game assigns
/// priorities in on first run. Which *end* of the order wins is the part that
/// stays an assumption, so it remains configurable.
fn predict_winner(
    names: &[String],
    rule: WinnerRule,
    host: Option<&HostInfo>,
) -> Option<String> {
    if names.is_empty() {
        return None;
    }

    if let Some(host) = host {
        if host.has_real_order() {
            let mut known: Vec<&String> = names
                .iter()
                .filter(|n| host.priority_of(n).is_some())
                .collect();
            if !known.is_empty() {
                // Python's sort is stable, so equal priorities keep their
                // original relative order.
                known.sort_by_key(|n| host.priority_of(n).unwrap_or(i64::MAX));
                return Some(match rule {
                    WinnerRule::Last => known[known.len() - 1].clone(),
                    WinnerRule::First => known[0].clone(),
                });
            }
        }
    }

    let mut ordered: Vec<&String> = names.iter().collect();
    ordered.sort_by_key(|n| n.to_uppercase());
    Some(match rule {
        WinnerRule::Last => ordered[ordered.len() - 1].clone(),
        WinnerRule::First => ordered[0].clone(),
    })
}

fn analyse_target(
    target: &str,
    entries: &[Entry<'_>],
    declared_only: &[String],
    winner_rule: WinnerRule,
    assume_pak: bool,
    host: Option<&HostInfo>,
) -> Option<Conflict> {
    let names: Vec<String> = {
        let unique: std::collections::BTreeSet<String> =
            entries.iter().map(|(m, _)| m.name.clone()).collect();
        unique.into_iter().collect()
    };

    if names.len() < 2 {
        if names.len() == 1 && !declared_only.is_empty() {
            let others = declared_only.join(", ");
            let mut mods = names.clone();
            mods.extend_from_slice(declared_only);
            return Some(Conflict {
                target: target.to_string(),
                severity: Some(Severity::Major),
                kind: "declared-overlap".to_string(),
                summary: format!(
                    "{} ships this asset while {} declare it in an AMUMSS recipe",
                    names[0], others
                ),
                notes: vec![
                    "The recipe has not been built into this library, but \
                     building it would produce a competing copy of the asset."
                        .to_string(),
                ],
                declared_only: declared_only.to_vec(),
                benign: false,
                predicted_winner: predict_winner(&mods, winner_rule, host),
                mods,
                ..Default::default()
            });
        }
        return None;
    }

    let mut notes: Vec<String> = Vec::new();
    let comparable: Vec<Entry<'_>> = entries
        .iter()
        .filter(|(_, f)| is_comparable(f))
        .copied()
        .collect();
    let binaries: Vec<Entry<'_>> = entries
        .iter()
        .filter(|(_, f)| matches!(f.kind, Some(FileKind::Mbin) | Some(FileKind::Dds)))
        .copied()
        .collect();

    let mut clashes: Vec<FieldClash> = Vec::new();
    let mut unique_counts: IndexMap<String, usize> = IndexMap::new();
    let mut benign = true;
    let severity: Severity;
    let kind: String;
    let summary: String;

    if comparable.len() == entries.len() {
        let broken = comparable.iter().filter(|(_, f)| f.parse_error.is_some()).count();
        if broken > 0 {
            notes.push(format!(
                "{broken} file(s) could not be parsed; comparison is partial"
            ));
        }
        let decompiled = comparable.iter().filter(|(_, f)| f.decompiled).count();
        if decompiled > 0 {
            notes.push(format!(
                "{decompiled} compiled asset(s) decompiled with MBINCompiler to \
                 make this comparison possible."
            ));
        }

        let comparison = compare_exml(&comparable);
        clashes = comparison.clashes;
        unique_counts = comparison.unique_counts;

        let mut sorted_names: Vec<&String> = comparison.prop_counts.keys().collect();
        sorted_names.sort();
        let sizes = sorted_names
            .iter()
            .map(|name| format!("{name} {}", comparison.prop_counts[*name]))
            .collect::<Vec<_>>()
            .join(", ");

        if !clashes.is_empty() {
            severity = Severity::Critical;
            kind = "field-disagreement".to_string();
            benign = false;
            summary = format!(
                "{} shared propert(y/ies) set to different values",
                clashes.len()
            );
            let both_sides = clashes.iter().filter(|c| c.attributed.len() >= 2).count();
            if both_sides > 0 {
                notes.push(format!(
                    "{both_sides} of these are confirmed edits on both sides via \
                     AMUMSS change markers."
                ));
            } else if names
                .iter()
                .any(|n| comparison.attribution.get(n).is_some_and(|a| a.is_none()))
            {
                notes.push(
                    "No change markers and too few copies to infer a vanilla \
                     baseline, so which mod authored each difference cannot be \
                     determined."
                        .to_string(),
                );
            }
        } else {
            let counts = &comparison.prop_counts;
            let smallest = counts.values().copied().min().unwrap_or(0);
            let full_copies = counts.values().filter(|c| **c >= FULL_ASSET_PROPS).count();
            // `smallest and overlap >= smallest * 0.5`: a zero-property copy
            // makes the whole condition false in Python.
            let substantial =
                smallest != 0 && (comparison.overlap as f64) >= (smallest as f64) * 0.5;

            if full_copies >= 2 && assume_pak {
                severity = Severity::Major;
                kind = "rival-full-assets".to_string();
                benign = false;
                summary = format!(
                    "{full_copies} copies each define a full-sized asset and agree \
                     wherever they overlap"
                );
                notes.push(
                    "Copies this large are whole-asset replacements rather than \
                     small patches, so packing them separately loses one entirely."
                        .to_string(),
                );
            } else if substantial {
                severity = Severity::Minor;
                kind = "redundant-overlap".to_string();
                summary = format!(
                    "{} shared propert(y/ies), all matching; the smaller copy is \
                     largely contained in the other",
                    comparison.overlap
                );
                notes.push(
                    "The copies agree wherever they overlap, so these mods \
                     duplicate each other here rather than fighting."
                        .to_string(),
                );
            } else {
                severity = if assume_pak { Severity::Major } else { Severity::Minor };
                kind = "disjoint".to_string();
                benign = !assume_pak;
                let shared = if comparison.overlap > 0 {
                    format!("{} shared propert(y/ies), all matching", comparison.overlap)
                } else {
                    "no shared properties".to_string()
                };
                let outcome = if assume_pak {
                    "only one copy loads"
                } else {
                    "these coexist safely"
                };
                summary = format!("Same asset, {shared}; {outcome}");
            }
        }

        notes.push(format!("properties defined per copy: {sizes}"));
    } else if binaries.len() == entries.len() {
        let hashes: HashSet<&str> = binaries.iter().map(|(_, f)| f.sha1.as_str()).collect();
        let textures = binaries.iter().all(|(_, f)| f.kind == Some(FileKind::Dds));
        let noun = if textures { "texture" } else { "compiled asset" };
        if hashes.len() == 1 {
            severity = Severity::Minor;
            kind = "duplicate-binary".to_string();
            summary = format!("Identical {noun} shipped by several mods");
        } else {
            severity = Severity::Major;
            kind = "binary-override".to_string();
            benign = false;
            summary = format!("Different {noun}s for one game file; one wins outright");
            notes.push(if textures {
                "Textures are replaced whole, so only one version is used.".to_string()
            } else {
                "Both copies are compiled MBIN, so their edits cannot be compared. \
                 Decompile with MBINCompiler for a field-level answer."
                    .to_string()
            });
        }
    } else {
        severity = Severity::Major;
        kind = "mixed-format".to_string();
        benign = false;
        summary = "Same asset shipped as both compiled MBIN and decompiled EXML".to_string();
        notes.push(
            "Mixed formats cannot be compared directly. Install MBINCompiler \
             (tools/MBINCompiler.exe) and re-run to compare these properly."
                .to_string(),
        );
    }

    if assume_pak {
        notes.push(
            "Override risk: packed as separate .pak files, only one copy of this \
             asset loads at all, so the losing mod contributes nothing here."
                .to_string(),
        );
    } else if !benign {
        notes.push(
            "The game applies these patches natively and merges edits that do not \
             overlap; where they do overlap, mod order decides the value."
                .to_string(),
        );
    }

    if !declared_only.is_empty() {
        notes.push(format!(
            "Also declared by an unbuilt AMUMSS recipe in: {}",
            declared_only.join(", ")
        ));
    }

    if clashes.len() > MAX_CLASHES {
        notes.push(format!(
            "Showing {MAX_CLASHES} of {} differing fields.",
            clashes.len()
        ));
        clashes.truncate(MAX_CLASHES);
    }

    let predicted_winner = predict_winner(&names, winner_rule, host);
    Some(Conflict {
        target: target.to_string(),
        mods: names,
        severity: Some(severity),
        kind,
        summary,
        clashes,
        unique_counts,
        notes,
        predicted_winner,
        declared_only: declared_only.to_vec(),
        benign,
        // Whether these copies can be combined rather than chosen between
        // needs the vanilla baseline, so `merge::run` fills this in afterwards.
        // Left as `None` here, meaning "not established" rather than "no".
        mergeable: None,
        overlap: Vec::new(),
        edit_counts: IndexMap::new(),
    })
}

fn stale(mods: &[Mod], reference: Option<Version>) -> Vec<StaleFinding> {
    let mut findings: Vec<StaleFinding> = Vec::new();
    let Some(reference) = reference else {
        return findings;
    };

    for the_mod in mods {
        let Some(version) = the_mod.max_version() else {
            continue;
        };
        let gap = (reference.major as i64 - version.major as i64) * 100
            + (reference.minor as i64 - version.minor as i64);
        if gap < STALE_MINOR_GAP {
            continue;
        }
        let stamped: Vec<&ModFile> = the_mod
            .files
            .iter()
            .filter(|f| f.mbinc_version.is_some())
            .collect();
        let examples: Vec<String> = stamped
            .iter()
            .filter(|f| f.mbinc_version.map(|v| v.key()) == Some(version.key()))
            .take(3)
            .map(|f| f.rel_path.clone())
            .collect();

        findings.push(StaleFinding {
            mod_name: the_mod.name.clone(),
            version,
            reference,
            file_count: stamped.len(),
            severity: if gap >= SEVERELY_STALE_MINOR_GAP {
                Severity::Major
            } else {
                Severity::Minor
            },
            examples,
        });
    }

    findings.sort_by_key(|f| f.version.key());
    findings
}

/// Files that failed to parse, ordered so the report is stable.
fn broken(mods: &[Mod]) -> Vec<BrokenFile> {
    let mut found: Vec<BrokenFile> = mods
        .iter()
        .flat_map(|m| {
            m.files.iter().filter_map(move |f| {
                f.parse_error.as_ref().map(|error| BrokenFile {
                    mod_name: m.name.clone(),
                    rel_path: f.rel_path.clone(),
                    error: error.clone(),
                })
            })
        })
        .collect();
    found.sort_by(|a, b| {
        a.mod_name
            .to_uppercase()
            .cmp(&b.mod_name.to_uppercase())
            .then_with(|| a.rel_path.to_uppercase().cmp(&b.rel_path.to_uppercase()))
    });
    found
}

fn loc_clashes(mods: &[Mod]) -> Vec<LocClash> {
    let mut owners: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for the_mod in mods {
        for loc_id in &the_mod.loc_ids {
            owners.entry(loc_id.clone()).or_default().push(the_mod.name.clone());
        }
    }
    owners
        .into_iter()
        .filter(|(_, names)| names.len() > 1)
        .map(|(loc_id, mut names)| {
            names.sort();
            LocClash { loc_id, mods: names }
        })
        .collect()
}

/// Run every check and assemble a [`Report`].
pub fn analyse(
    mods: Vec<Mod>,
    stats: ScanStats,
    roots: Vec<String>,
    reference_version: Option<Version>,
    winner_rule: WinnerRule,
    assume_pak: bool,
    host: Option<&HostInfo>,
) -> Report {
    let index = providers(&mods);

    // Recipes that name a target nobody has built yet.
    let mut declared_index: HashMap<String, Vec<String>> = HashMap::new();
    for the_mod in &mods {
        let own = the_mod.targets();
        for declared in &the_mod.declared_targets {
            for target in index.keys() {
                if declared_matches(declared, target) && !own.contains(target) {
                    declared_index
                        .entry(target.clone())
                        .or_default()
                        .push(the_mod.name.clone());
                }
            }
        }
    }

    let mut conflicts: Vec<Conflict> = Vec::new();
    for (target, entries) in &index {
        let declared: Vec<String> = declared_index
            .get(target)
            .map(|names| {
                let unique: std::collections::BTreeSet<String> =
                    names.iter().cloned().collect();
                unique.into_iter().collect()
            })
            .unwrap_or_default();

        if let Some(conflict) = analyse_target(
            target,
            entries,
            &declared,
            winner_rule,
            assume_pak,
            host,
        ) {
            conflicts.push(conflict);
        }
    }

    conflicts.sort_by(|a, b| {
        a.severity
            .cmp(&b.severity)
            .then_with(|| a.target.cmp(&b.target))
    });

    let reference_version = reference_version.or_else(|| {
        mods.iter()
            .flat_map(|m| m.files.iter().filter_map(|f| f.mbinc_version))
            .max()
    });

    let default_host = HostInfo::default();
    let host = host.unwrap_or(&default_host);

    Report {
        roots,
        // Scene drift needs MBINCompiler and the game's archives, so it and
        // the tool status are filled in by the caller rather than computed
        // here; the analysis itself stays pure and testable.
        drift: Vec::new(),
        tools: Default::default(),
        broken: broken(&mods),
        stale: stale(&mods, reference_version),
        loc_clashes: loc_clashes(&mods),
        unregistered: mods
            .iter()
            .filter(|m| host.has_real_order() && !host.is_known(&m.name))
            .map(|m| m.name.clone())
            .collect(),
        disabled: mods.iter().filter(|m| m.disabled).map(|m| m.name.clone()).collect(),
        mods,
        conflicts,
        stats,
        reference_version,
        winner_rule: winner_rule.as_str().to_string(),
        real_load_order: host.has_real_order(),
        manager: host.manager.clone(),
        disable_all: host.disable_all,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mod_with(name: &str, props: &[(&str, &str)]) -> Mod {
        let mut file = ModFile {
            kind: Some(FileKind::Exml),
            target: Some("GLOBALS/X.MBIN".to_string()),
            rel_path: "GLOBALS/X.EXML".to_string(),
            ..Default::default()
        };
        for (path, value) in props {
            file.props
                .insert(path.to_string(), Some(value.to_string()));
        }
        Mod {
            name: name.to_string(),
            files: vec![file],
            ..Default::default()
        }
    }

    fn run(mods: Vec<Mod>) -> Report {
        analyse(
            mods,
            ScanStats::default(),
            vec!["root".to_string()],
            None,
            WinnerRule::Last,
            false,
            None,
        )
    }

    #[test]
    fn an_unreadable_file_is_reported_as_broken() {
        // The shape that broke "Increased S Class Chance" on game 7.03: the
        // container property is opened and never closed, so the game's parser
        // rejects the file and the mod silently does nothing.
        let mut busted = mod_with("Increased S Class Chance", &[]);
        busted.files[0].parse_error =
            Some("XML parse error: mismatched tag: line 36, column 2".to_string());

        let report = run(vec![busted]);

        assert_eq!(report.broken.len(), 1);
        assert_eq!(report.broken[0].mod_name, "Increased S Class Chance");
        assert!(report.broken[0].error.contains("mismatched tag"));

        // Not a conflict, but it must still demand attention.
        assert_eq!(report.actionable().count(), 0);
        assert!(report.needs_attention());
    }

    #[test]
    fn a_clean_library_reports_nothing_broken() {
        let report = run(vec![mod_with("A", &[("Rate", "1")])]);
        assert!(report.broken.is_empty());
        assert!(!report.needs_attention());
    }

    #[test]
    fn disjoint_properties_are_benign() {
        let report = run(vec![
            mod_with("A", &[("ShipInteractRadius", "200")]),
            mod_with("B", &[("MaxNumSameGroupTech", "6")]),
        ]);
        assert_eq!(report.conflicts.len(), 1);
        let conflict = &report.conflicts[0];
        assert_eq!(conflict.kind, "disjoint");
        assert_eq!(conflict.severity, Some(Severity::Minor));
        assert!(conflict.benign);
        assert_eq!(report.actionable().count(), 0);
    }

    #[test]
    fn same_property_different_values_is_critical() {
        let report = run(vec![
            mod_with("Fast", &[("GroundRunSpeed", "12")]),
            mod_with("Slow", &[("GroundRunSpeed", "4")]),
        ]);
        let conflict = &report.conflicts[0];
        assert_eq!(conflict.severity, Some(Severity::Critical));
        assert_eq!(conflict.kind, "field-disagreement");
        assert!(!conflict.benign);
        assert_eq!(conflict.clashes.len(), 1);
        assert_eq!(conflict.clashes[0].path, "GroundRunSpeed");
    }

    #[test]
    fn float_formatting_alone_is_not_a_clash() {
        let report = run(vec![
            mod_with("A", &[("Rate", "1.0")]),
            mod_with("B", &[("Rate", "1.000000")]),
        ]);
        assert!(report.conflicts[0].benign);
        assert!(report.conflicts[0].clashes.is_empty());
    }

    #[test]
    fn a_single_provider_is_not_a_conflict() {
        let report = run(vec![mod_with("Only", &[("Rate", "1")])]);
        assert!(report.conflicts.is_empty());
    }

    #[test]
    fn majority_of_three_attributes_the_odd_one_out() {
        let report = run(vec![
            mod_with("A", &[("Rate", "1")]),
            mod_with("B", &[("Rate", "1")]),
            mod_with("Odd", &[("Rate", "9")]),
        ]);
        let clash = &report.conflicts[0].clashes[0];
        assert_eq!(clash.attributed, vec!["Odd".to_string()]);
    }

    #[test]
    fn two_unannotated_copies_attribute_nobody() {
        let report = run(vec![
            mod_with("A", &[("Rate", "1")]),
            mod_with("B", &[("Rate", "9")]),
        ]);
        let conflict = &report.conflicts[0];
        assert!(conflict.clashes[0].attributed.is_empty());
        assert!(conflict
            .notes
            .iter()
            .any(|n| n.contains("cannot be determined")));
    }

    #[test]
    fn amumss_markers_attribute_directly() {
        let mut a = mod_with("Annotated", &[("Rate", "9")]);
        a.files[0]
            .annotations
            .insert("Rate".to_string(), "CHANGED".to_string());
        let report = run(vec![a, mod_with("Plain", &[("Rate", "1")])]);
        assert_eq!(
            report.conflicts[0].clashes[0].attributed,
            vec!["Annotated".to_string()]
        );
    }

    #[test]
    fn identical_binaries_are_minor_but_differing_ones_are_major() {
        let make = |name: &str, sha: &str| Mod {
            name: name.to_string(),
            files: vec![ModFile {
                kind: Some(FileKind::Mbin),
                target: Some("GLOBALS/X.MBIN".to_string()),
                sha1: sha.to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };

        let same = run(vec![make("A", "abc"), make("B", "abc")]);
        assert_eq!(same.conflicts[0].kind, "duplicate-binary");
        assert!(same.conflicts[0].benign);

        let differ = run(vec![make("A", "abc"), make("B", "def")]);
        assert_eq!(differ.conflicts[0].kind, "binary-override");
        assert!(!differ.conflicts[0].benign);
    }

    #[test]
    fn mixed_mbin_and_exml_cannot_be_compared() {
        let binary = Mod {
            name: "Compiled".to_string(),
            files: vec![ModFile {
                kind: Some(FileKind::Mbin),
                target: Some("GLOBALS/X.MBIN".to_string()),
                sha1: "abc".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let report = run(vec![binary, mod_with("Readable", &[("Rate", "1")])]);
        assert_eq!(report.conflicts[0].kind, "mixed-format");
        assert!(!report.conflicts[0].benign);
    }

    #[test]
    fn assume_pak_turns_a_benign_overlap_actionable() {
        let mods = vec![
            mod_with("A", &[("ShipInteractRadius", "200")]),
            mod_with("B", &[("MaxNumSameGroupTech", "6")]),
        ];
        let report = analyse(
            mods,
            ScanStats::default(),
            vec!["root".to_string()],
            None,
            WinnerRule::Last,
            true,
            None,
        );
        assert!(!report.conflicts[0].benign);
        assert_eq!(report.conflicts[0].severity, Some(Severity::Major));
    }

    #[test]
    fn winner_defaults_to_last_alphabetically_without_real_order() {
        let report = run(vec![
            mod_with("Alpha", &[("Rate", "1")]),
            mod_with("Zulu", &[("Rate", "9")]),
        ]);
        assert_eq!(
            report.conflicts[0].predicted_winner.as_deref(),
            Some("Zulu")
        );
    }

    #[test]
    fn real_mod_priority_beats_alphabetical_order() {
        let mut host = HostInfo::default();
        host.priorities.insert("ZULU".to_string(), 0);
        host.priorities.insert("ALPHA".to_string(), 1);

        let report = analyse(
            vec![
                mod_with("Alpha", &[("Rate", "1")]),
                mod_with("Zulu", &[("Rate", "9")]),
            ],
            ScanStats::default(),
            vec!["root".to_string()],
            None,
            WinnerRule::Last,
            false,
            Some(&host),
        );
        assert_eq!(
            report.conflicts[0].predicted_winner.as_deref(),
            Some("Alpha"),
            "higher ModPriority wins, not the later name"
        );
    }

    #[test]
    fn winner_rule_first_flips_the_answer() {
        let report = analyse(
            vec![
                mod_with("Alpha", &[("Rate", "1")]),
                mod_with("Zulu", &[("Rate", "9")]),
            ],
            ScanStats::default(),
            vec!["root".to_string()],
            None,
            WinnerRule::First,
            false,
            None,
        );
        assert_eq!(
            report.conflicts[0].predicted_winner.as_deref(),
            Some("Alpha")
        );
    }

    #[test]
    fn clashes_are_truncated_with_a_note() {
        let many: Vec<(String, String)> = (0..MAX_CLASHES + 5)
            .map(|i| (format!("P{i:03}"), i.to_string()))
            .collect();
        let refs: Vec<(&str, &str)> =
            many.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let other: Vec<(&str, &str)> = many.iter().map(|(k, _)| (k.as_str(), "X")).collect();

        let report = run(vec![mod_with("A", &refs), mod_with("B", &other)]);
        let conflict = &report.conflicts[0];
        assert_eq!(conflict.clashes.len(), MAX_CLASHES);
        assert!(conflict.notes.iter().any(|n| n.starts_with("Showing 40 of")));
    }

    #[test]
    fn stale_findings_follow_the_newest_version_seen() {
        let mut old = mod_with("Old", &[("Rate", "1")]);
        old.files[0].mbinc_version = Some(Version::new(6, 1, 0, 1));
        let mut new = mod_with("New", &[("Rate", "1")]);
        new.files[0].mbinc_version = Some(Version::new(7, 3, 0, 1));

        let report = run(vec![old, new]);
        assert_eq!(report.stale.len(), 1);
        assert_eq!(report.stale[0].mod_name, "Old");
        // 7.03 vs 6.01 is a gap of 102 minor versions: severely stale.
        assert_eq!(report.stale[0].severity, Severity::Major);
        assert_eq!(report.reference_version.unwrap().to_string(), "7.03.0.1");
    }

    #[test]
    fn conflicts_sort_worst_first_then_by_target() {
        let mut critical = mod_with("A", &[("Rate", "1")]);
        critical.files[0].target = Some("B_TARGET.MBIN".to_string());
        let mut critical_b = mod_with("B", &[("Rate", "9")]);
        critical_b.files[0].target = Some("B_TARGET.MBIN".to_string());

        let mut minor = mod_with("C", &[("Only", "1")]);
        minor.files[0].target = Some("A_TARGET.MBIN".to_string());
        let mut minor_b = mod_with("D", &[("Other", "2")]);
        minor_b.files[0].target = Some("A_TARGET.MBIN".to_string());

        let report = run(vec![critical, critical_b, minor, minor_b]);
        assert_eq!(report.conflicts[0].severity, Some(Severity::Critical));
        assert_eq!(report.conflicts[1].severity, Some(Severity::Minor));
    }
}
