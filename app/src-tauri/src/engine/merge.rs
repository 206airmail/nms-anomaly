//! Whether a conflict needs a decision, or only needs combining.
//!
//! A conflict says two mods ship the same game file and only one of them will
//! apply. That is true, and it is worth saying — but it is not the whole
//! answer, because it does not distinguish the two cases that matter:
//!
//! * the mods **disagree**: both change the same property to different values,
//!   and a human has to pick one;
//! * the mods **don't overlap**: each edits parts of the file the other leaves
//!   alone, and losing one is pure accident. Combining them loses nothing.
//!
//! The second case is common and completely mechanical, and telling someone to
//! "choose a winner" there is bad advice. A freighter hangar where one mod adds
//! two salvage terminals and another repositions two hover pads is a real
//! conflict — whichever loads second wins outright — and also a perfectly safe
//! merge, because neither touches anything the other does.
//!
//! Deciding which case applies needs the vanilla file, because the question is
//! not "do these two copies differ" but "did they both *change* the same
//! thing". Two copies of one asset differ in every property either author
//! touched; only the vanilla baseline separates an edit from an inheritance.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use super::decompile::Decompiler;
use super::exml::values_equal;
use super::exmltree;
use super::model::{Conflict, Mod};
use super::vanilla::VanillaSource;

/// What comparing one contested asset against vanilla showed.
#[derive(Debug, Clone, Default)]
pub struct Assessment {
    /// mod name -> how many properties that mod changes from vanilla
    pub edits: IndexMap<String, usize>,
    /// property paths that more than one mod changes, i.e. the real decisions
    pub overlap: Vec<String>,
}

impl Assessment {
    /// True when no two mods change the same property.
    pub fn mergeable(&self) -> bool {
        self.overlap.is_empty()
    }
}

/// Which properties this copy changes relative to `vanilla`.
///
/// `whole_file` says whether this copy replaces the asset outright. It decides
/// what silence means, and getting it wrong is catastrophic: a sparse patch
/// names only the handful of properties it edits, so counting everything it
/// omits as a deletion credited one 6,000-property mod with 152,306 edits and
/// made every sparse conflict look unmergeable.
fn edits_against(
    vanilla: &IndexMap<String, Option<String>>,
    copy: &IndexMap<String, Option<String>>,
    whole_file: bool,
) -> HashSet<String> {
    let mut out = HashSet::new();
    for (path, value) in copy {
        match vanilla.get(path) {
            // Present in both and different: an edit.
            Some(base) => {
                if !values_equal(base.as_deref(), value.as_deref()) {
                    out.insert(path.clone());
                }
            }
            // Absent from vanilla: the mod introduces it. That is an edit too,
            // and two mods introducing the same path do have to be reconciled.
            None => {
                out.insert(path.clone());
            }
        }
    }
    // Only a whole-file replacement can delete by omission. A sparse patch
    // that does not mention a property simply leaves it alone.
    if whole_file {
        for path in vanilla.keys() {
            if !copy.contains_key(path) {
                out.insert(path.clone());
            }
        }
    }
    out
}

/// One contesting copy of an asset: who ships it, what it contains, and
/// whether it replaces the asset outright or patches it sparsely.
pub struct Copy<'a> {
    pub name: String,
    pub props: &'a IndexMap<String, Option<String>>,
    pub whole_file: bool,
}

/// Assess one conflict, given every contesting copy's properties.
pub fn assess(vanilla: &IndexMap<String, Option<String>>, copies: &[Copy]) -> Assessment {
    let mut assessment = Assessment::default();
    let mut per_mod: Vec<(String, HashSet<String>)> = Vec::new();

    for Copy { name, props, whole_file } in copies {
        let edits = edits_against(vanilla, props, *whole_file);
        assessment.edits.insert(name.clone(), edits.len());
        per_mod.push((name.clone(), edits));
    }

    let mut seen: HashMap<&str, usize> = HashMap::new();
    for (_name, edits) in &per_mod {
        for path in edits {
            *seen.entry(path.as_str()).or_insert(0) += 1;
        }
    }
    let mut overlap: Vec<String> = seen
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(p, _)| p.to_string())
        .collect();
    overlap.sort();
    assessment.overlap = overlap;
    assessment
}

/// Fill in `mergeable` and `overlap` for every conflict that can be judged.
///
/// Conflicts whose vanilla counterpart cannot be reached — an asset the game
/// does not ship, or a machine without the tools — are left as `None` rather
/// than guessed at, so "unknown" never reads as "safe".
pub fn run(
    conflicts: &mut [Conflict],
    mods: &[Mod],
    decompiler: &mut Decompiler,
    source: &mut VanillaSource,
) {
    let targets: Vec<String> = conflicts
        .iter()
        .filter(|c| c.mods.len() > 1)
        .map(|c| c.target.clone())
        .collect();
    if targets.is_empty() {
        return;
    }
    let extraction = source.fetch(&targets);

    for conflict in conflicts.iter_mut() {
        let Some(vanilla_mbin) = extraction.found.get(&conflict.target) else {
            continue;
        };
        // Hash first rather than letting `decompile_file` do it out of sight:
        // the same hash keys the flattened form, so the game's own copy of a
        // contested asset is read once and then never again.
        let Some(sha1) = Decompiler::content_hash(vanilla_mbin) else {
            continue;
        };
        let Some(xml) = decompiler.decompile(vanilla_mbin, &sha1) else {
            continue;
        };
        let base = super::propcache::parse(&xml, &sha1);
        if base.error.is_some() || base.props.is_empty() {
            continue;
        }

        // Only copies whose properties are readable can be judged; a mod whose
        // asset could not be decompiled must not be silently treated as making
        // no edits, which would make a real overlap look mergeable.
        let mut copies: Vec<Copy> = Vec::new();
        let mut opaque = false;
        for name in &conflict.mods {
            let Some(owner) = mods.iter().find(|m| &m.name == name) else {
                opaque = true;
                continue;
            };
            match owner
                .files
                .iter()
                .find(|f| f.target.as_deref() == Some(conflict.target.as_str()))
            {
                Some(file) if !file.props.is_empty() => copies.push(Copy {
                    name: name.clone(),
                    props: &file.props,
                    // A compiled MBIN is the whole asset; an EXML from AMUMSS
                    // is a sparse patch naming only what it edits.
                    whole_file: file.kind == Some(super::model::FileKind::Mbin),
                }),
                _ => opaque = true,
            }
        }
        if opaque || copies.len() < 2 {
            continue;
        }

        let assessment = assess(&base.props, &copies);
        conflict.mergeable = Some(assessment.mergeable());
        conflict.overlap = assessment.overlap;
        conflict.edit_counts = assessment.edits;
    }
}

/// A merged asset, ready to compile.
#[derive(Debug, Clone)]
pub struct Merged {
    pub target: String,
    /// the EXML text of the combined asset
    pub xml: String,
    /// whose copy the merge was built on top of
    pub host: String,
    /// mod name -> how many of its edits were applied onto the host
    pub applied: IndexMap<String, usize>,
    /// subtrees grafted in whole, because the host did not have them at all
    pub grafted: usize,
    /// edits that could not be placed; a non-empty list means the merge is
    /// incomplete and must not be presented as finished
    pub skipped: Vec<String>,
}

impl Merged {
    /// True when every edit landed.
    ///
    /// A merge that dropped edits is worse than no merge: it looks like both
    /// mods are installed while quietly doing less than either. Callers must
    /// check this before writing the result anywhere the game will load it.
    pub fn complete(&self) -> bool {
        self.skipped.is_empty()
    }

    /// Why the merge is incomplete, in words a user can act on.
    pub fn shortfall(&self) -> Option<String> {
        if self.complete() {
            return None;
        }
        Some(format!(
            "{} edit(s) had nowhere to attach. The merge is built on {}'s copy,              and that copy predates the current game build, so it is missing              rows the other mods patch. Update that mod, or exclude it from the              merge, and try again.",
            self.skipped.len(),
            self.host,
        ))
    }
}

/// The parent path of `path`, or `""` for a top-level node.
fn parent_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(at) => &path[..at],
        None => "",
    }
}

/// Combine several copies of one asset into a single file.
///
/// The host is the copy that already carries the most edits, so the merge
/// applies the fewest changes and stays closest to something a mod author
/// produced. Every other copy's edits are then written onto it: a value where
/// the host has that property, a grafted subtree where it does not.
///
/// Paths are applied shortest first, so a subtree arrives before the edits
/// that live inside it and they always have somewhere to land.
///
/// Only meaningful when [`Assessment::mergeable`] held. Applying overlapping
/// edits would silently keep whichever landed last, which is the very loss
/// this exists to prevent.
pub fn build(
    target: &str,
    vanilla_xml: &str,
    copies: &[(String, String, bool)],
) -> Result<Merged, String> {
    let vanilla = exmltree::parse_str(vanilla_xml)?.flatten();

    let mut parsed: Vec<(String, exmltree::Tree, bool, Vec<String>)> = Vec::new();
    for (name, xml, whole_file) in copies {
        let tree = exmltree::parse_str(xml)?;
        let mut edits: Vec<String> = edits_against(&vanilla, &tree.flatten(), *whole_file)
            .into_iter()
            .collect();
        // Shortest first so an ancestor is grafted before its descendants.
        edits.sort_by_key(|p| (p.matches('/').count(), p.clone()));
        parsed.push((name.clone(), tree, *whole_file, edits));
    }

    // A sparse patch has no document to host a merge, so the host must be a
    // whole-file copy; among those, the one with the most edits needs the
    // least rewriting.
    let host_at = parsed
        .iter()
        .enumerate()
        .filter(|(_, (_, _, whole, _))| *whole)
        .max_by_key(|(_, (_, _, _, edits))| edits.len())
        .map(|(i, _)| i)
        .ok_or_else(|| {
            "every copy is a sparse patch, which the game already merges itself".to_string()
        })?;

    let (host_name, mut host, _, _) = parsed.remove(host_at);
    let mut merged = Merged {
        target: target.to_string(),
        xml: String::new(),
        host: host_name,
        applied: IndexMap::new(),
        grafted: 0,
        skipped: Vec::new(),
    };

    // Indexing walks the whole document, so it happens once here and again
    // only after a graft actually changes the shape. Rebuilding it per edit
    // turned a 153,000-property table into an O(n^2) crawl that ran for over
    // ten minutes.
    let mut index = host.index();

    for (name, donor, _whole, edits) in &parsed {
        let donor_index = donor.index();
        let mut applied = 0usize;

        for path in edits {
            if index.contains_key(path) {
                let value = donor.value_at(&donor_index, path);
                if host.set(&index, path, value.as_deref()) {
                    applied += 1;
                }
                continue;
            }

            // Absent from the host: bring the whole subtree across, but only
            // when its parent is there to receive it.
            let parent = parent_of(path);
            let placeable = parent.is_empty() || index.contains_key(parent);
            let node = donor.take(&donor_index, path);
            match (placeable, node) {
                (true, Some(node)) => {
                    if host.graft(&index, parent, node) {
                        merged.grafted += 1;
                        applied += 1;
                        // The tree grew, so paths and routes have moved.
                        index = host.index();
                    } else {
                        merged.skipped.push(path.clone());
                    }
                }
                _ => merged.skipped.push(path.clone()),
            }
        }
        merged.applied.insert(name.clone(), applied);
    }

    host.renumber();
    merged.xml = exmltree::to_string(&host);
    Ok(merged)
}

/// What a merge of `target` is called, as a mod folder.
///
/// # One folder per merged asset, named from the asset
///
/// Every merge used to land in a single shared `zzz_nmscheck_merged` folder, so
/// a second merge joined the first and neither could be undone without taking
/// the other with it. The name is now built from the whole target path, which
/// makes it unique by construction and, more usefully, *stable*: merging the
/// same asset again resolves to the same folder, so it replaces the old build
/// rather than accumulating beside it.
///
/// The `zzz_` prefix is kept as a belt-and-braces measure. It no longer decides
/// anything between the merge and its own inputs -- those are held out of the
/// game by the loadout now, not by load order -- but a *third* mod touching the
/// same asset is still settled by order, and a merge should win that.
pub fn folder_for(target: &str) -> String {
    let stem = target
        .trim_end_matches(".MBIN")
        .trim_end_matches(".EXML")
        .trim_end_matches(".MXML");
    // Still `nmscheck`, after the program was renamed to Anomaly, and
    // deliberately. This prefix is not a brand, it is a *folder name on disk*:
    // every merge already in the game is called `zzz_nmscheck_…`, the loadout
    // records those names, and `mergeAssetName` on the front end reads them
    // back to label the row. Renaming it would leave the program unable to
    // recognise its own merges, for the sake of a string nobody reads.
    let mut out = String::from("zzz_nmscheck_");
    let mut gap = false;
    for c in stem.chars() {
        if c.is_ascii_alphanumeric() {
            if gap && !out.ends_with('_') {
                out.push('_');
            }
            out.push(c.to_ascii_uppercase());
            gap = false;
        } else {
            gap = true;
        }
    }
    out
}

/// Build the merge for one conflict into `into`.
///
/// `into` is the folder whose contents become the mod's contents -- the same
/// shape a staged mod has, so the result can be deployed by the ordinary path
/// rather than written into the game by hand.
///
/// Returns the path written. Refuses rather than writing a merge that dropped
/// edits: a file that looks like both mods while quietly doing less than
/// either is worse than leaving the conflict alone.
pub fn install(
    conflict: &Conflict,
    mods: &[Mod],
    into: &std::path::Path,
    decompiler: &mut Decompiler,
    source: &mut VanillaSource,
) -> Result<std::path::PathBuf, String> {
    if conflict.mergeable != Some(true) {
        return Err("this conflict has overlapping edits and needs a decision,                     not a merge"
            .to_string());
    }
    let extraction = source.fetch(&[conflict.target.clone()]);
    let vanilla_mbin = extraction
        .found
        .get(&conflict.target)
        .ok_or("the game has no copy of this asset to merge against")?;
    let vanilla_xml = decompiler
        .decompile_file(vanilla_mbin)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .ok_or("could not read the vanilla copy")?;

    let mut copies = Vec::new();
    for name in &conflict.mods {
        let Some(owner) = mods.iter().find(|m| &m.name == name) else {
            continue;
        };
        let Some(file) = owner
            .files
            .iter()
            .find(|f| f.target.as_deref() == Some(conflict.target.as_str()))
        else {
            continue;
        };
        let whole = file.kind == Some(super::model::FileKind::Mbin);
        let path = std::path::Path::new(&file.abs_path);
        let xml = if whole {
            decompiler
                .decompile(path, &file.sha1)
                .and_then(|p| std::fs::read_to_string(p).ok())
        } else {
            std::fs::read_to_string(path).ok()
        };
        if let Some(xml) = xml {
            copies.push((name.clone(), xml, whole));
        }
    }
    if copies.len() < 2 {
        return Err("could not read every copy of this asset".to_string());
    }

    let merged = build(&conflict.target, &vanilla_xml, &copies)?;
    if let Some(reason) = merged.shortfall() {
        return Err(reason);
    }

    let rel: std::path::PathBuf = conflict.target.split('/').collect();
    let dir = into.join(rel.parent().unwrap_or(std::path::Path::new("")));
    let stem = rel
        .file_name()
        .ok_or("the target has no file name")?
        .to_string_lossy()
        .trim_end_matches(".MBIN")
        .to_string();
    decompiler.compile(&merged.xml, &dir, &stem)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_merge_is_named_after_the_whole_asset_path() {
        assert_eq!(
            folder_for("METADATA/REALITY/TABLES/REWARDTABLE.MBIN"),
            "zzz_nmscheck_METADATA_REALITY_TABLES_REWARDTABLE"
        );
    }

    #[test]
    fn two_assets_sharing_a_file_name_do_not_share_a_folder() {
        // The reason the whole path is used rather than the file name: these
        // would otherwise be one folder, and merging the second would silently
        // overwrite the first.
        assert_ne!(
            folder_for("METADATA/REALITY/TABLES/A/THING.MBIN"),
            folder_for("METADATA/REALITY/TABLES/B/THING.MBIN")
        );
    }

    #[test]
    fn merging_the_same_asset_twice_lands_in_the_same_folder() {
        // Stability is the point: a re-merge must replace its own build rather
        // than accumulate beside it.
        assert_eq!(
            folder_for("METADATA/REALITY/TABLES/REWARDTABLE.MBIN"),
            folder_for("METADATA/REALITY/TABLES/REWARDTABLE.MBIN")
        );
    }

    #[test]
    fn a_merge_folder_is_a_usable_directory_name() {
        // Windows rejects most punctuation in a path segment, and the game only
        // ever sees this as a folder.
        for target in [
            "METADATA/REALITY/TABLES/REWARDTABLE.MBIN",
            "MODELS/SPACE/SHIP.SCENE.MBIN",
            "METADATA/SIMULATION/SOLARSYSTEM/REWARDS/NEXUSMISSIONTABLE.EXML",
        ] {
            let name = folder_for(target);
            assert!(name.starts_with("zzz_nmscheck_"), "{name}");
            assert!(
                name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
                "{name}"
            );
            assert!(!name.ends_with('_'), "{name}");
        }
    }

    fn props(pairs: &[(&str, &str)]) -> IndexMap<String, Option<String>> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), Some(v.to_string())))
            .collect()
    }

    fn whole<'a>(name: &str, p: &'a IndexMap<String, Option<String>>) -> Copy<'a> {
        Copy { name: name.to_string(), props: p, whole_file: true }
    }

    fn sparse<'a>(name: &str, p: &'a IndexMap<String, Option<String>>) -> Copy<'a> {
        Copy { name: name.to_string(), props: p, whole_file: false }
    }

    #[test]
    fn untouched_properties_are_not_edits() {
        // Two mods ship a whole file; most of it is inherited unchanged.
        let vanilla = props(&[("a", "1"), ("b", "2"), ("c", "3")]);
        let one = props(&[("a", "9"), ("b", "2"), ("c", "3")]);
        let two = props(&[("a", "1"), ("b", "2"), ("c", "7")]);
        let a = assess(&vanilla, &[whole("one", &one), whole("two", &two)]);
        assert_eq!(a.edits["one"], 1);
        assert_eq!(a.edits["two"], 1);
        assert!(a.mergeable(), "they change different properties");
    }

    #[test]
    fn changing_the_same_property_is_a_real_decision() {
        let vanilla = props(&[("a", "1")]);
        let one = props(&[("a", "9")]);
        let two = props(&[("a", "7")]);
        let a = assess(&vanilla, &[whole("one", &one), whole("two", &two)]);
        assert!(!a.mergeable());
        assert_eq!(a.overlap, vec!["a".to_string()]);
    }

    #[test]
    fn agreeing_on_the_same_new_value_still_needs_reconciling() {
        // Both moved it to the same place. Nothing is lost either way, but the
        // paths do collide, so this is not the clean disjoint case.
        let vanilla = props(&[("a", "1")]);
        let one = props(&[("a", "5")]);
        let two = props(&[("a", "5")]);
        let a = assess(&vanilla, &[whole("one", &one), whole("two", &two)]);
        assert!(!a.mergeable());
    }

    #[test]
    fn float_rendering_noise_is_not_an_edit() {
        // A decompile round trip must not make a mod look like it changed
        // something, or nothing would ever read as mergeable.
        let vanilla = props(&[("x", "65.861860")]);
        let one = props(&[("x", "65.8618546")]);
        let two = props(&[("x", "65.861860")]);
        let a = assess(&vanilla, &[whole("one", &one), whole("two", &two)]);
        assert_eq!(a.edits["one"], 0);
        assert!(a.mergeable());
    }

    #[test]
    fn a_sparse_patch_does_not_delete_what_it_omits() {
        // The whole table, against two patches that each name one row. Before
        // this distinction existed, each patch was credited with deleting
        // everything it did not mention.
        let vanilla = props(&[("a", "1"), ("b", "2"), ("c", "3")]);
        let one = props(&[("a", "9")]);
        let two = props(&[("b", "8")]);
        let a = assess(&vanilla, &[sparse("one", &one), sparse("two", &two)]);
        assert_eq!(a.edits["one"], 1);
        assert_eq!(a.edits["two"], 1);
        assert!(a.mergeable());
    }

    #[test]
    fn a_whole_file_replacement_does_delete_by_omission() {
        let vanilla = props(&[("a", "1"), ("b", "2")]);
        let full = props(&[("a", "1")]);
        let patch = props(&[("a", "1")]);
        let dropped = assess(&vanilla, &[whole("full", &full), sparse("p", &patch)]);
        assert_eq!(dropped.edits["full"], 1, "the whole file no longer has b");
        assert_eq!(dropped.edits["p"], 0, "the patch simply does not mention b");
    }

    #[test]
    fn adding_and_dropping_paths_both_count() {
        let vanilla = props(&[("a", "1"), ("b", "2")]);
        let adds = props(&[("a", "1"), ("b", "2"), ("c", "3")]);
        let drops = props(&[("a", "1")]);
        let a = assess(&vanilla, &[whole("adds", &adds), whole("drops", &drops)]);
        assert_eq!(a.edits["adds"], 1); // introduced c
        assert_eq!(a.edits["drops"], 1); // removed b
        assert!(a.mergeable());
    }
}
