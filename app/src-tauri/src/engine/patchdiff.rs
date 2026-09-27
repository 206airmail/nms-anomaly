//! What a game update changed underneath the mods.
//!
//! The version stamp in a mod says which build it was compiled for, and the
//! library is full of mods whose stamp is old and whose behaviour is perfect.
//! That is why the outdated list is hidden by default: "built for 6.45" is not
//! a finding, it is a date.
//!
//! The finding worth having is the one underneath it. When Hello Games change a
//! value in a table, a mod carrying its own copy of that table hands the change
//! straight back -- silently, because nothing errors and the mod still does
//! what it advertises. `SalvageRights` shipped the whole 1.2 MB `REWARDTABLE`
//! to change two percentages, and on a newer build it was also reverting the
//! Nexus mission reward and the expedition Phase 4 ship costs. Fifteen
//! properties nobody asked about.
//!
//! This module finds that class of harm, for every mod, from two facts the
//! program already has on disk.
//!
//! # Where the old game comes from
//!
//! [`super::vanilla`] extracts vanilla assets into a cache keyed by game build,
//! so the copies from the build that was installed *before* the update are
//! still there after it. Nothing has to be predicted or downloaded: the patch
//! diff is the old cache against the new one, and the paks themselves are never
//! read twice.
//!
//! Two consequences, both worth saying out loud rather than papering over:
//!
//! * The comparison covers the assets that were cached before the update, and
//!   no others -- once the old paks are gone there is no recovering an asset
//!   nobody looked at. [`Impact::uncomparable`] names what fell in that gap,
//!   and [`prepare`] exists so it can be made empty in advance.
//! * A machine with one cached build cannot do this at all yet. That is not a
//!   failure state, it is the first run, and [`Impact::verdict`] says which one
//!   it is looking at.
//!
//! # The discriminator, which matters more than the diff
//!
//! A raw "this property differs from vanilla" is the wrong signal, because that
//! is the definition of a mod. What separates harm from intent is *which*
//! vanilla value the mod agrees with:
//!
//! | the mod's value | reading |
//! |---|---|
//! | equals the **old** vanilla value | it is carrying a stale baseline, and the patch is being reverted by accident -- [`Judged::reverts`] |
//! | equals the **new** vanilla value | it already agrees with the patched game; nothing to say |
//! | its own, agreeing with neither | the author meant to set this, and the ground moved under them -- [`Judged::overridden`] |
//!
//! Only the first is a bug, and telling it from the third is the whole job.
//! Reporting the third as harm would bury every real finding under the entire
//! purpose of every mod in the library.
//!
//! Two more kinds come out of the same comparison. A property the patch
//! *removed* leaves the mod's edit with nothing to land on
//! ([`Judged::dead`]), and a property the patch *added* is one a whole-file
//! copy silently deletes by not containing it ([`Judged::drops`]) -- the other
//! half of the `SalvageRights` shape, and the reason `whole_file` is a
//! parameter here exactly as it is in [`super::prune`].

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use indexmap::IndexMap;
use serde::Serialize;

use super::decompile::Decompiler;
use super::exml;
use super::model::{FileKind, Mod, Severity};
use super::vanilla::{self, Build};

/// property path -> value, the shape everything here compares in.
type Props = IndexMap<String, Option<String>>;

/// What one property did across the patch, and what the mod makes of it.
#[derive(Debug, Clone, Serialize)]
pub struct Moved {
    pub path: String,
    /// the value the game shipped before the update
    pub before: Option<String>,
    /// the value it ships now
    pub after: Option<String>,
    /// the value the mod writes over it
    pub mod_value: Option<String>,
}

/// The whole judgement on one mod file.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Judged {
    /// the mod still carries the pre-patch value, so it undoes the patch
    pub reverts: Vec<Moved>,
    /// the patch removed the property; the mod's edit lands nowhere now
    pub dead: Vec<Moved>,
    /// the patch added a property this whole-file copy does not contain, so
    /// loading the copy deletes it
    pub drops: Vec<Moved>,
    /// the mod sets its own value here and the game's moved underneath it
    pub overridden: Vec<Moved>,
}

impl Judged {
    /// Everything that needs a decision, worst first.
    pub fn harm(&self) -> usize {
        self.reverts.len() + self.dead.len() + self.drops.len()
    }

    pub fn quiet(&self) -> bool {
        self.harm() == 0 && self.overridden.is_empty()
    }
}

/// Which paths in `props` are leaves.
///
/// Containers are excluded from every comparison here for the reason
/// [`super::prune::changed_leaves`] excludes them: a container differs whenever
/// anything beneath it does, so counting one would drag in its whole subtree and
/// report a single changed float as a hundred findings.
///
/// Derived from the flattened map rather than from a [`super::exmltree::Tree`],
/// because this module holds three flattened maps and only ever parsed two
/// trees -- and "has a child" is the same question either way.
fn leaves(props: &Props) -> HashSet<&String> {
    let parents: HashSet<&str> = props
        .keys()
        .filter_map(|p| p.rfind('/').map(|at| &p[..at]))
        .collect();
    props.keys().filter(|p| !parents.contains(p.as_str())).collect()
}

/// Judge one mod file against the game before and after an update.
///
/// Pure, and the only place the rules live. `whole_file` decides whether
/// silence means deletion, the same distinction [`super::prune::prune`] and
/// [`super::merge`] draw: a compiled `.MBIN` is the entire table, so a property
/// it omits is one the mod removes, whereas a sparse `.EXML` omits nearly
/// everything by definition and omission means only "not mentioned".
pub fn judge(old: &Props, new: &Props, mod_props: &Props, whole_file: bool) -> Judged {
    let mut out = Judged::default();
    let mine = leaves(mod_props);

    for path in mod_props.keys() {
        if !mine.contains(path) {
            continue;
        }
        let value = mod_props.get(path).cloned().flatten();
        let was = old.get(path);
        let now = new.get(path);

        match (was, now) {
            // The patch took the property away. Whatever the mod wanted here,
            // there is no longer anything for it to land on.
            (Some(before), None) => out.dead.push(Moved {
                path: path.clone(),
                before: before.clone(),
                after: None,
                mod_value: value,
            }),
            (Some(before), Some(after)) => {
                if exml::values_equal(before.as_deref(), after.as_deref()) {
                    // The game did not touch this. Whether the mod changes it
                    // is the ordinary conflict question, not a patch question.
                    continue;
                }
                let moved = Moved {
                    path: path.clone(),
                    before: before.clone(),
                    after: after.clone(),
                    mod_value: value.clone(),
                };
                if exml::values_equal(value.as_deref(), before.as_deref()) {
                    // The discriminator. The mod holds the pre-patch value, so
                    // loading it undoes the update -- and since the mod agreed
                    // with the game before, this was never something it meant
                    // to set.
                    out.reverts.push(moved);
                } else if exml::values_equal(value.as_deref(), after.as_deref()) {
                    // Already agrees with the patched game.
                    continue;
                } else {
                    out.overridden.push(moved);
                }
            }
            // Absent from the old game too: the mod invented this property, or
            // the game never had it. Either way the update did not do it.
            (None, _) => continue,
        }
    }

    if whole_file {
        // A property the patch introduced, which this copy of the table does
        // not contain. Loading the whole file therefore deletes it. Only
        // askable of a whole-file copy: a patch omits almost everything.
        let theirs = leaves(new);
        for path in new.keys() {
            if !theirs.contains(path) || mod_props.contains_key(path) || old.contains_key(path) {
                continue;
            }
            out.drops.push(Moved {
                path: path.clone(),
                before: None,
                after: new.get(path).cloned().flatten(),
                mod_value: None,
            });
        }
    }

    out
}

/// One mod file, judged, with the words the screen uses.
#[derive(Debug, Clone, Serialize)]
pub struct FileImpact {
    pub owner: String,
    pub rel_path: String,
    pub target: String,
    pub whole_file: bool,
    pub severity: String,
    pub summary: String,
    #[serde(flatten)]
    pub judged: Judged,
}

/// How bad one file's judgement is.
///
/// A whole-file copy reverting a patch is the worst thing in this module: it
/// reverts by *existing*, needs no conflict to do harm, and the mod's author
/// cannot have intended any of it. A sparse patch doing the same is a rung down
/// only because it takes back fewer things, not because it is innocent.
fn weigh(judged: &Judged, whole_file: bool) -> Severity {
    if judged.harm() == 0 {
        return Severity::Info;
    }
    if whole_file && (!judged.reverts.is_empty() || !judged.drops.is_empty()) {
        return Severity::Critical;
    }
    Severity::Major
}

fn describe(owner: &str, judged: &Judged, whole_file: bool) -> String {
    let mut parts = Vec::new();
    if !judged.reverts.is_empty() {
        parts.push(format!(
            "takes back {} value(s) this update changed",
            judged.reverts.len()
        ));
    }
    if !judged.drops.is_empty() {
        parts.push(format!(
            "deletes {} value(s) the update added",
            judged.drops.len()
        ));
    }
    if !judged.dead.is_empty() {
        parts.push(format!(
            "edits {} property(ies) the update removed, so those edits do nothing now",
            judged.dead.len()
        ));
    }
    if parts.is_empty() {
        if judged.overridden.is_empty() {
            return format!("{owner} is unaffected by this update.");
        }
        return format!(
            "{owner} deliberately sets {} value(s) this update also changed. Nothing is \
             being reverted by accident, but the author chose those numbers against the \
             older game.",
            judged.overridden.len()
        );
    }
    let how = if whole_file {
        " It ships the whole file, so this happens whether or not anything conflicts with it."
    } else {
        ""
    };
    format!("{owner} {}.{how}", parts.join(", "))
}

/// Everything one update did to one library.
#[derive(Debug, Clone, Serialize)]
pub struct Impact {
    /// the build the library was last checked against
    pub from: Option<Build>,
    /// the build installed now
    pub to: Option<Build>,
    /// assets compared in both builds
    pub compared: usize,
    /// assets the patch changed at all
    pub touched: usize,
    /// targets a mod ships that the older cache has no copy of, so the update
    /// cannot be judged for them
    pub uncomparable: Vec<String>,
    pub files: Vec<FileImpact>,
    /// mods examined and found unaffected, which is the answer the hidden
    /// outdated list never gave
    pub unaffected: Vec<String>,
    pub verdict: String,
    /// why the survey could not run, when it could not
    pub blocked: Option<String>,
}

impl Impact {
    pub fn harm(&self) -> usize {
        self.files.iter().map(|f| f.judged.harm()).sum()
    }
}

/// Explain an empty result, so "no findings" cannot be confused with "not run".
fn verdict_for(impact: &Impact) -> String {
    if let Some(why) = &impact.blocked {
        return why.clone();
    }
    let harm = impact.harm();
    if harm == 0 {
        return format!(
            "This update changed {} of the {} assets your mods touch, and none of your mods \
             takes any of it back.",
            impact.touched, impact.compared
        );
    }
    format!(
        "{harm} value(s) across {} mod file(s) are being undone by mods built against the \
         older game.",
        impact.files.iter().filter(|f| f.judged.harm() > 0).count()
    )
}

/// The two builds a survey runs between: the newest cached, and the one before.
///
/// Returns `None` with a reason when there is nothing to compare, which is the
/// ordinary state of a fresh install and must not read as a clean bill of
/// health.
pub fn bracket(builds: &[Build]) -> Result<(Build, Build), String> {
    match builds.len() {
        0 => Err("No vanilla game data has been extracted yet, so there is no baseline to \
                  compare an update against."
            .to_string()),
        1 => Err(format!(
            "Only one game build has been recorded here ({}). The comparison becomes \
             available after the next game update, which is when there are two.",
            &builds[0].key[..builds[0].key.len().min(8)]
        )),
        n => Ok((builds[n - 2].clone(), builds[n - 1].clone())),
    }
}

/// Flatten a cached vanilla `.MBIN` into properties.
fn read_vanilla(dir: &Path, target: &str, decompiler: &mut Decompiler) -> Option<Props> {
    let mut path = dir.to_path_buf();
    for part in target.split('/') {
        path.push(part);
    }
    if !path.is_file() {
        return None;
    }
    let sha1 = Decompiler::content_hash(&path)?;
    let mxml = decompiler.decompile(&path, &sha1)?;
    let text = std::fs::read_to_string(mxml).ok()?;
    Some(super::exmltree::parse_str(&text).ok()?.flatten())
}

/// The mod's own view of the asset, decompiling only when it has to.
fn read_mod_file(
    file: &super::model::ModFile,
    decompiler: &mut Decompiler,
) -> Option<Props> {
    // Discovery already flattened every `.EXML`, so the usual case costs
    // nothing. A `.MBIN` has to go through MBINCompiler to be read at all.
    let flattened = super::propcache::props_of(file);
    if !flattened.is_empty() {
        return Some(flattened.into_owned());
    }
    if file.kind != Some(FileKind::Mbin) {
        return None;
    }
    let mxml = decompiler.decompile(Path::new(&file.abs_path), &file.sha1)?;
    let text = std::fs::read_to_string(mxml).ok()?;
    Some(super::exmltree::parse_str(&text).ok()?.flatten())
}

/// Compare two cached builds and judge every mod against the difference.
pub fn survey(mods: &[Mod], from: &Build, to: &Build, decompiler: &mut Decompiler) -> Impact {
    let home = vanilla::cache_home();
    let (old_dir, new_dir) = (home.join(&from.key), home.join(&to.key));

    let mut impact = Impact {
        from: Some(from.clone()),
        to: Some(to.clone()),
        compared: 0,
        touched: 0,
        uncomparable: Vec::new(),
        files: Vec::new(),
        unaffected: Vec::new(),
        verdict: String::new(),
        blocked: None,
    };

    // Which mods ship which target, so each asset is read from the archives'
    // cache once however many mods touch it.
    let mut wanted: BTreeMap<&str, Vec<(&Mod, &super::model::ModFile)>> = BTreeMap::new();
    for owner in mods {
        for file in owner.assets() {
            if let Some(target) = file.target.as_deref() {
                wanted.entry(target).or_default().push((owner, file));
            }
        }
    }

    let mut affected: HashSet<&str> = HashSet::new();
    let mut examined: HashSet<&str> = HashSet::new();

    for (target, holders) in wanted {
        let Some(old) = read_vanilla(&old_dir, target, decompiler) else {
            // No copy from before the update. The old paks are gone, so this
            // is not recoverable -- only preventable next time, which is what
            // `prepare` is for.
            impact.uncomparable.push(target.to_string());
            continue;
        };
        let Some(new) = read_vanilla(&new_dir, target, decompiler) else {
            impact.uncomparable.push(target.to_string());
            continue;
        };
        impact.compared += 1;

        // Did the patch touch this asset at all? Asked once per asset rather
        // than per mod, and it is the cheap gate: most assets are untouched by
        // any given update, and an untouched asset cannot produce a finding.
        let changed = new
            .iter()
            .any(|(path, value)| match old.get(path) {
                Some(before) => !exml::values_equal(before.as_deref(), value.as_deref()),
                None => true,
            })
            || old.keys().any(|path| !new.contains_key(path));
        if !changed {
            for (owner, _) in &holders {
                examined.insert(owner.name.as_str());
            }
            continue;
        }
        impact.touched += 1;

        for (owner, file) in holders {
            examined.insert(owner.name.as_str());
            let Some(props) = read_mod_file(file, decompiler) else {
                continue;
            };
            let whole_file = file.kind == Some(FileKind::Mbin);
            let judged = judge(&old, &new, &props, whole_file);
            if judged.quiet() {
                continue;
            }
            affected.insert(owner.name.as_str());
            impact.files.push(FileImpact {
                owner: owner.name.clone(),
                rel_path: file.rel_path.clone(),
                target: target.to_string(),
                whole_file,
                severity: weigh(&judged, whole_file).as_str().to_string(),
                summary: describe(&owner.name, &judged, whole_file),
                judged,
            });
        }
    }

    // Worst first, then by how much of it there is, so the list reads in the
    // order somebody would work through it.
    impact.files.sort_by(|a, b| {
        (&a.severity, std::cmp::Reverse(a.judged.harm()), &a.owner)
            .cmp(&(&b.severity, std::cmp::Reverse(b.judged.harm()), &b.owner))
    });

    // Saying which mods were checked and found clean is the point of the
    // exercise as much as the findings are: it is the answer the version-stamp
    // list could never give, and it is why that list is hidden.
    let mut unaffected: Vec<String> = examined
        .into_iter()
        .filter(|name| !affected.contains(name))
        .map(str::to_string)
        .collect();
    unaffected.sort();
    impact.unaffected = unaffected;
    impact.uncomparable.sort();
    impact.uncomparable.dedup();
    impact.verdict = verdict_for(&impact);
    impact
}

/// Cache vanilla copies of every asset the library touches, for next time.
///
/// The survey can only compare assets that were extracted before the update,
/// and ordinary running extracts only what it needs -- whole-file overrides and
/// contested patches, per [`super::prune::plan`]. Everything else is a gap that
/// cannot be filled retroactively, because the old archives are replaced by the
/// update that makes the question interesting.
///
/// So this is the one deliberately eager extraction in the program, and it is a
/// user action rather than part of any scan. That distinction is not cosmetic:
/// `VanillaSource::fetch` walking 31 GB for targets that were never there was
/// most of a 76-second start-up once, and the fix was to stop doing work nobody
/// asked for on the launch path.
pub fn prepare(mods: &[Mod], source: &mut vanilla::VanillaSource) -> Prepared {
    let mut targets: Vec<String> = mods
        .iter()
        .flat_map(|m| m.assets())
        .filter_map(|f| f.target.clone())
        // A texture has no properties to diff, so caching one buys nothing.
        .filter(|t| !t.to_uppercase().ends_with(".DDS"))
        .collect();
    targets.sort();
    targets.dedup();

    let asked = targets.len();
    let result = source.fetch(&targets);
    Prepared {
        asked,
        cached: result.found.len(),
        absent: result.missing.len(),
        unpacked: source.unpacked,
        error: result.error.clone(),
    }
}

/// What [`prepare`] managed to lay down.
#[derive(Debug, Clone, Serialize)]
pub struct Prepared {
    pub asked: usize,
    pub cached: usize,
    /// targets the game does not ship, i.e. assets the mods invented
    pub absent: usize,
    pub unpacked: usize,
    pub error: Option<String>,
}

/// Convenience for the command layer: pick the bracket and run it.
pub fn latest(mods: &[Mod], decompiler: &mut Decompiler) -> Impact {
    let builds = vanilla::builds();
    match bracket(&builds) {
        Ok((from, to)) => survey(mods, &from, &to, decompiler),
        Err(why) => {
            // Only `to` is known when there is no bracket. Filling `from` with
            // the same build to have something there would draw a comparison
            // between a build and itself, which is exactly the false reassurance
            // `blocked` exists to prevent.
            let mut impact = Impact {
                from: None,
                to: builds.last().cloned(),
                compared: 0,
                touched: 0,
                uncomparable: Vec::new(),
                files: Vec::new(),
                unaffected: Vec::new(),
                verdict: String::new(),
                blocked: Some(why),
            };
            impact.verdict = verdict_for(&impact);
            impact
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(pairs: &[(&str, &str)]) -> Props {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Some(v.to_string())))
            .collect()
    }

    fn build(key: &str, ms: i64) -> Build {
        Build {
            key: key.to_string(),
            first_seen_ms: ms,
            paks: 1,
            bytes: 1,
            dated: true,
        }
    }

    // The shape this module exists for, in miniature: the mod meant to change
    // one value and is carrying an old copy of everything else.
    const OLD: &[(&str, &str)] = &[
        ("T/R_SCRAPHEAP/Chance", "40.000000"),
        ("T/R_GT_NEW_EASY_N/Reward", "5"),
    ];
    const NEW: &[(&str, &str)] = &[
        ("T/R_SCRAPHEAP/Chance", "40.000000"),
        ("T/R_GT_NEW_EASY_N/Reward", "9"),
    ];

    #[test]
    fn a_stale_copy_reverting_a_patch_is_the_finding() {
        // The mod changes Chance on purpose and holds Reward at the pre-patch
        // value it never meant to touch.
        let mine = props(&[
            ("T/R_SCRAPHEAP/Chance", "100.000000"),
            ("T/R_GT_NEW_EASY_N/Reward", "5"),
        ]);
        let j = judge(&props(OLD), &props(NEW), &mine, true);
        assert_eq!(j.reverts.len(), 1);
        assert_eq!(j.reverts[0].path, "T/R_GT_NEW_EASY_N/Reward");
        assert_eq!(j.reverts[0].before.as_deref(), Some("5"));
        assert_eq!(j.reverts[0].after.as_deref(), Some("9"));
        // The intended edit is not a finding: the game never changed Chance.
        assert!(j.overridden.is_empty());
        assert_eq!(weigh(&j, true), Severity::Critical);
    }

    #[test]
    fn a_deliberate_value_is_not_a_revert() {
        // The author set their own number on a property the patch also moved.
        // Real, worth knowing, and emphatically not the same bug -- reporting
        // it as harm would bury every finding under what mods are *for*.
        let mine = props(&[("T/R_GT_NEW_EASY_N/Reward", "99")]);
        let j = judge(&props(OLD), &props(NEW), &mine, false);
        assert!(j.reverts.is_empty());
        assert_eq!(j.overridden.len(), 1);
        assert_eq!(j.harm(), 0);
        assert_eq!(weigh(&j, false), Severity::Info);
    }

    #[test]
    fn a_mod_already_agreeing_with_the_patch_is_silent() {
        let mine = props(&[("T/R_GT_NEW_EASY_N/Reward", "9")]);
        assert!(judge(&props(OLD), &props(NEW), &mine, false).quiet());
    }

    #[test]
    fn an_untouched_property_says_nothing_however_the_mod_sets_it() {
        // Disagreeing with vanilla is the definition of a mod; only the update
        // is this module's business.
        let mine = props(&[("T/R_SCRAPHEAP/Chance", "100.000000")]);
        assert!(judge(&props(OLD), &props(NEW), &mine, false).quiet());
    }

    #[test]
    fn float_formatting_does_not_manufacture_a_finding() {
        let old = props(&[("A/x", "1.0")]);
        let new = props(&[("A/x", "1.000000")]);
        let mine = props(&[("A/x", "1.00")]);
        assert!(judge(&old, &new, &mine, true).quiet());
    }

    #[test]
    fn a_removed_property_leaves_the_edit_with_nothing_to_land_on() {
        let old = props(&[("A/gone", "1")]);
        let new = props(&[("A/other", "2")]);
        let mine = props(&[("A/gone", "7")]);
        let j = judge(&old, &new, &mine, false);
        assert_eq!(j.dead.len(), 1);
        assert_eq!(j.dead[0].path, "A/gone");
        assert_eq!(weigh(&j, false), Severity::Major);
    }

    #[test]
    fn a_whole_file_copy_deletes_what_the_patch_added() {
        let old = props(&[("A/x", "1")]);
        let new = props(&[("A/x", "1"), ("A/brand_new", "3")]);
        let mine = props(&[("A/x", "2")]);
        let j = judge(&old, &new, &mine, true);
        assert_eq!(j.drops.len(), 1);
        assert_eq!(j.drops[0].path, "A/brand_new");
        assert_eq!(j.drops[0].after.as_deref(), Some("3"));
    }

    #[test]
    fn a_sparse_patch_is_not_accused_of_deleting_what_it_never_mentioned() {
        // The distinction `whole_file` exists for. A patch omits nearly
        // everything by definition, so asking this of one would accuse every
        // mod in the library of deleting the entire game.
        let old = props(&[("A/x", "1")]);
        let new = props(&[("A/x", "1"), ("A/brand_new", "3")]);
        let mine = props(&[("A/x", "2")]);
        assert!(judge(&old, &new, &mine, false).drops.is_empty());
    }

    #[test]
    fn containers_are_not_counted_as_changes() {
        // `A/row` has a child, so it is a container. Counting it would report
        // one changed float twice.
        let old = props(&[("A/row", ""), ("A/row/value", "1")]);
        let new = props(&[("A/row", ""), ("A/row/value", "2")]);
        let mine = props(&[("A/row", ""), ("A/row/value", "1")]);
        let j = judge(&old, &new, &mine, true);
        assert_eq!(j.reverts.len(), 1);
        assert_eq!(j.reverts[0].path, "A/row/value");
    }

    #[test]
    fn a_property_the_mod_invented_is_not_the_updates_doing() {
        let mine = props(&[("A/mine_alone", "5")]);
        assert!(judge(&props(OLD), &props(NEW), &mine, false).quiet());
    }

    #[test]
    fn one_cached_build_is_reported_as_not_yet_rather_than_as_clean() {
        let err = bracket(&[build("aaaaaaaaaaaaaaaa", 1)]).unwrap_err();
        assert!(err.contains("next game update"), "{err}");
        assert!(bracket(&[]).is_err());
    }

    #[test]
    fn the_bracket_is_the_newest_two_builds_in_order() {
        let builds = vec![build("old", 100), build("mid", 200), build("new", 300)];
        let (from, to) = bracket(&builds).unwrap();
        assert_eq!(from.key, "mid");
        assert_eq!(to.key, "new");
    }

    #[test]
    fn a_blocked_survey_never_reads_as_a_clean_bill_of_health() {
        let impact = Impact {
            from: None,
            to: None,
            compared: 0,
            touched: 0,
            uncomparable: Vec::new(),
            files: Vec::new(),
            unaffected: Vec::new(),
            verdict: String::new(),
            blocked: Some("only one build".to_string()),
        };
        assert_eq!(verdict_for(&impact), "only one build");
    }

    #[test]
    fn the_summary_names_what_it_found() {
        let mine = props(&[
            ("T/R_SCRAPHEAP/Chance", "100.000000"),
            ("T/R_GT_NEW_EASY_N/Reward", "5"),
        ]);
        let j = judge(&props(OLD), &props(NEW), &mine, true);
        let text = describe("SalvageRights", &j, true);
        assert!(text.contains("takes back 1"), "{text}");
        assert!(text.contains("whole file"), "{text}");
    }
}
