//! Plain data shared by the scanner and the analyser.
//!
//! Port of the container types in `nmscc/model.py`. Nothing here knows about
//! the UI: the Tauri commands serialise these into the same `--json` shape the
//! Python emits, which is the contract the front end reads.

use indexmap::IndexMap;
use serde::Serialize;

use super::scene::MovedNode;
use super::tools;
use super::version::Version;

/// What a file inside a mod folder actually is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum FileKind {
    /// compiled asset, opaque binary
    Mbin,
    /// AMUMSS-decompiled asset, inspectable XML
    Exml,
    /// AMUMSS build script
    Lua,
    /// AMUMSS localisation table
    Mxml,
    /// texture, opaque binary
    Dds,
    Other,
}

/// How badly a conflict bites, worst first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Severity {
    /// two mods set the same field to different values
    Critical,
    /// same file, one mod's edits get silently discarded
    Major,
    /// same file, byte- or value-identical content
    Minor,
    /// worth knowing, not a conflict
    Info,
}

impl Severity {
    /// The wire form, matching the Python enum's value.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Critical => "CRITICAL",
            Severity::Major => "MAJOR",
            Severity::Minor => "MINOR",
            Severity::Info => "INFO",
        }
    }
}

/// One file inside a mod folder.
#[derive(Debug, Clone, Default)]
pub struct ModFile {
    pub abs_path: String,
    /// relative to the mod root, as on disk
    pub rel_path: String,
    pub kind: Option<FileKind>,
    /// canonical asset key; `None` for non-assets
    pub target: Option<String>,
    pub size: u64,
    pub sha1: String,
    pub mbinc_version: Option<Version>,
    pub amumss_version: Option<String>,
    pub template: Option<String>,
    /// property path -> value, for EXML only
    pub props: IndexMap<String, Option<String>>,
    /// property path -> "CHANGED" / "ADDED", for annotated EXML only
    pub annotations: IndexMap<String, String>,
    pub parse_error: Option<String>,
    /// true when props came from MBINCompiler rather than shipped XML
    pub decompiled: bool,
}

impl ModFile {
    pub fn annotated(&self) -> bool {
        !self.annotations.is_empty()
    }
}

/// A single mod: one immediate subdirectory of a scanned library root.
#[derive(Debug, Clone, Default)]
pub struct Mod {
    pub name: String,
    pub root: String,
    pub files: Vec<ModFile>,
    /// paths declared by AMUMSS Lua scripts but not necessarily built
    pub declared_targets: std::collections::BTreeSet<String>,
    /// localisation ids contributed by LocTable.MXML
    pub loc_ids: std::collections::BTreeSet<String>,
    pub amumss_version: Option<String>,
    /// Nexus archive this folder was deployed from, per the Vortex manifest,
    /// tidied for display
    pub source: Option<String>,
    /// the same archive name untrimmed, which is the only record of the Nexus
    /// mod id and the installed version. Deliberately not serialised: the JSON
    /// is a parity contract with the Python, and this has no Python twin.
    pub archive: Option<String>,
    /// true when GCMODSETTINGS.MXML explicitly disables the mod
    pub disabled: bool,
    /// ModPriority from GCMODSETTINGS.MXML; `None` when the game has not
    /// registered this folder yet. This decides every winner, so the UI shows
    /// mods in this order.
    pub priority: Option<i64>,
}

impl Mod {
    pub fn assets(&self) -> impl Iterator<Item = &ModFile> {
        self.files.iter().filter(|f| f.target.is_some())
    }

    pub fn targets(&self) -> std::collections::BTreeSet<String> {
        self.assets()
            .filter_map(|f| f.target.clone())
            .collect()
    }

    pub fn max_version(&self) -> Option<Version> {
        self.files.iter().filter_map(|f| f.mbinc_version).max()
    }

    pub fn min_version(&self) -> Option<Version> {
        self.files.iter().filter_map(|f| f.mbinc_version).min()
    }
}

/// Counters describing one scan.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ScanStats {
    pub mods: usize,
    pub files: usize,
    pub decompiled: usize,
    pub assets: usize,
    pub exml: usize,
    pub mbin: usize,
    pub lua: usize,
    pub dds: usize,
    pub targets: usize,
    pub parse_errors: usize,
    /// `"mod :: relative path -- message"` for each file that failed to parse
    pub error_details: Vec<String>,
}

/// One property path on which the providers of a target disagree.
#[derive(Debug, Clone, Default)]
pub struct FieldClash {
    pub path: String,
    /// mod name -> value that mod ships (absent mods simply lack the key)
    pub values: IndexMap<String, Option<String>>,
    /// mods that demonstrably authored a change here
    pub attributed: Vec<String>,
}

/// Two or more mods overriding the same game asset.
#[derive(Debug, Clone, Default)]
pub struct Conflict {
    pub target: String,
    pub mods: Vec<String>,
    pub severity: Option<Severity>,
    pub kind: String,
    pub summary: String,
    pub clashes: Vec<FieldClash>,
    /// mod name -> properties only that copy defines
    pub unique_counts: IndexMap<String, usize>,
    pub notes: Vec<String>,
    pub predicted_winner: Option<String>,
    /// mods that only *declare* this target in Lua without shipping a file
    pub declared_only: Vec<String>,
    /// true when the game merges these copies cleanly and nothing is lost, so
    /// the finding is informational rather than something to act on
    pub benign: bool,
    /// `Some(true)` when no two copies change the same property, so the mods
    /// can be combined instead of one being chosen. `None` when the vanilla
    /// baseline was not reachable and the question could not be answered --
    /// never guessed, so "unknown" does not read as "safe".
    pub mergeable: Option<bool>,
    /// property paths more than one mod changes: the part that truly needs a
    /// decision, as opposed to the parts that merely differ
    pub overlap: Vec<String>,
    /// mod name -> how many properties that mod changes from vanilla
    pub edit_counts: IndexMap<String, usize>,
}

/// A mod built against an older game version than the rest of the library.
#[derive(Debug, Clone)]
pub struct StaleFinding {
    pub mod_name: String,
    pub version: Version,
    pub reference: Version,
    pub file_count: usize,
    pub severity: Severity,
    pub examples: Vec<String>,
}

/// A file the game cannot read, so the mod silently does nothing.
///
/// Invalid XML is not a conflict and not a staleness hint -- it is the mod
/// failing outright. The game's loader rejects the file, no error reaches the
/// player, and the feature just never happens. That makes it the most
/// actionable thing this tool can find.
#[derive(Debug, Clone)]
pub struct BrokenFile {
    pub mod_name: String,
    pub rel_path: String,
    pub error: String,
}

/// A wholesale scene replacement that moves nodes it did not mean to.
///
/// A mod shipping a whole `.SCENE.MBIN` owns every node in it, including the
/// ones it never intended to touch. Re-exporting a scene through a 3D tool can
/// rebake a parent's transform into its children: the geometry still draws in
/// the right place, so the mod looks fine, but a locator the engine reads as an
/// anchor has silently moved.
///
/// Nodes *added* or *removed* are the mod doing its job and are only counted.
/// A node present in both copies that has shifted is the finding.
#[derive(Debug, Clone)]
pub struct SceneDrift {
    pub mod_name: String,
    pub rel_path: String,
    pub target: String,
    /// nodes that exist in both copies but sit somewhere else
    pub moved: Vec<MovedNode>,
    pub added: usize,
    pub removed: usize,
    /// what the mod was compared against; currently always "vanilla"
    pub reference: String,
    pub severity: Severity,
}

impl SceneDrift {
    /// Moves that displaced a node without displacing its contents.
    ///
    /// These are the ones worth acting on: the scene still draws correctly, so
    /// nothing looks wrong, but anything reading the node as an anchor now
    /// reads the wrong place.
    pub fn rebaked(&self) -> impl Iterator<Item = &MovedNode> {
        self.moved.iter().filter(|m| m.contents_held)
    }
}

/// Two mods contributing the same localisation id.
#[derive(Debug, Clone)]
pub struct LocClash {
    pub loc_id: String,
    pub mods: Vec<String>,
}

/// Everything one run found.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub roots: Vec<String>,
    pub mods: Vec<Mod>,
    pub conflicts: Vec<Conflict>,
    pub stale: Vec<StaleFinding>,
    pub loc_clashes: Vec<LocClash>,
    pub stats: ScanStats,
    pub reference_version: Option<Version>,
    pub winner_rule: String,
    pub real_load_order: bool,
    pub manager: Option<String>,
    pub disable_all: bool,
    /// mod folders the game has not registered yet
    pub unregistered: Vec<String>,
    /// mod folders the game lists as switched off
    pub disabled: Vec<String>,
    /// files that could not be parsed at all
    pub broken: Vec<BrokenFile>,
    /// wholesale scene replacements that shift nodes they should not
    pub drift: Vec<SceneDrift>,
    /// which external tools were available, and what therefore was not checked
    pub tools: tools::Status,
}

impl Report {
    /// Conflicts where something is actually lost or cannot be verified.
    pub fn actionable(&self) -> impl Iterator<Item = &Conflict> {
        self.conflicts.iter().filter(|c| !c.benign)
    }

    /// Overlaps the game resolves cleanly; nothing to decide.
    pub fn merged(&self) -> impl Iterator<Item = &Conflict> {
        self.conflicts.iter().filter(|c| c.benign)
    }

    /// Scene changes that need a decision, as opposed to a note.
    pub fn serious_drift(&self) -> impl Iterator<Item = &SceneDrift> {
        self.drift.iter().filter(|d| d.severity != Severity::Info)
    }

    /// True when anything in the library needs a decision or a fix.
    ///
    /// Broken files count: a mod whose file the game cannot read is doing
    /// nothing at all, which matters more than any conflict. So does a scene
    /// rebake, which misplaces a game anchor without any file conflicting.
    /// Repositioning that a mod clearly intended is informational and does not
    /// count.
    pub fn needs_attention(&self) -> bool {
        self.actionable().next().is_some()
            || !self.broken.is_empty()
            || self.serious_drift().next().is_some()
    }
}

#[cfg(test)]

mod tests {
    use super::*;

    fn file_with(version: Option<Version>) -> ModFile {
        ModFile {
            mbinc_version: version,
            ..Default::default()
        }
    }

    #[test]
    fn severity_wire_form_matches_python() {
        assert_eq!(Severity::Critical.as_str(), "CRITICAL");
        assert_eq!(Severity::Info.as_str(), "INFO");
    }

    #[test]
    fn severity_orders_worst_first() {
        let mut all = vec![
            Severity::Minor,
            Severity::Critical,
            Severity::Info,
            Severity::Major,
        ];
        all.sort();
        assert_eq!(
            all,
            vec![
                Severity::Critical,
                Severity::Major,
                Severity::Minor,
                Severity::Info
            ]
        );
    }

    #[test]
    fn version_span_ignores_unstamped_files() {
        let mut m = Mod::default();
        m.files.push(file_with(Some(Version::new(6, 34, 0, 3))));
        m.files.push(file_with(None));
        m.files.push(file_with(Some(Version::new(7, 3, 2, 2))));

        assert_eq!(m.max_version().unwrap().to_string(), "7.03.2.2");
        assert_eq!(m.min_version().unwrap().to_string(), "6.34.0.3");
    }

    #[test]
    fn a_mod_with_no_stamps_has_no_version() {
        let mut m = Mod::default();
        m.files.push(file_with(None));
        assert!(m.max_version().is_none());
        assert!(m.min_version().is_none());
    }

    #[test]
    fn targets_deduplicate_across_files() {
        let mut m = Mod::default();
        for _ in 0..2 {
            m.files.push(ModFile {
                target: Some("GLOBALS/X.MBIN".to_string()),
                ..Default::default()
            });
        }
        m.files.push(ModFile {
            target: None,
            ..Default::default()
        });
        assert_eq!(m.targets().len(), 1);
        assert_eq!(m.assets().count(), 2);
    }
}
