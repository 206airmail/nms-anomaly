//! What the game actually did with the library, read back off a session log.
//!
//! Every other part of this engine reasons about mods as files on disk and
//! *predicts* what the game will make of them. The hook reports what it did.
//! This module joins the two, and it exists to replace an assumption with a
//! measurement.
//!
//! # What was assumed
//!
//! `GCMODSETTINGS.MXML` gives every mod a `ModPriority`, so the ordering is
//! read rather than guessed. What nobody had observed was how the game *uses*
//! it. [`super::analyze`] sorts by priority and takes the highest, on the
//! stated reasoning that the highest is applied last and a later patch
//! overwrites an earlier one. `--winner first` exists because that was a
//! coin-flip.
//!
//! # What is now measured
//!
//! The hook logs one line per mod file the game opens, in the order it opens
//! them. Grouped by asset, that is the sequence the game reads a contested
//! asset's copies in:
//!
//! ```text
//! GLOBALS/GCGAMEPLAYGLOBALS.GLOBAL.MBIN
//!     Supercharge Multiplier       [62]
//!  -> Quick Scan with Range Boost  [56]
//!  -> Larger Upgrade Stacks        [39]
//!  -> Better Ship Transfer Range   [18]
//!  -> Auto Translate Words         [12]
//!  -> More Freighter Battles 50%   [8]
//! ```
//!
//! Measured on the reference library, every contested asset ran like that:
//! **strictly descending `ModPriority`**. The game walks assets, and within one
//! asset it reads the providers highest priority first. Sixteen assets, no
//! exceptions, so this is not coincidence -- and it is the opposite of the order
//! the tool's own documentation claimed.
//!
//! # What is still open, stated plainly
//!
//! Knowing the read order does *not* by itself name the winner, and this module
//! must not pretend otherwise. It leaves exactly one question, and both answers
//! are consistent with what the hook can see:
//!
//! | if the loader | then the survivor is | which means |
//! |---|---|---|
//! | lets a later read overwrite an earlier one | the copy read **last** -- the *lowest* priority | [`WinnerRule::First`] |
//! | keeps the first value written for a property | the copy read **first** -- the *highest* priority | [`WinnerRule::Last`] |
//!
//! The hook watches file opens, not the table in memory, so it cannot tell
//! these apart. What it has done is turn a vague question ("which end of the
//! order wins?") into a sharp one ("does the loader overwrite, or skip?"), and
//! narrow the candidates to two named mods per asset instead of a whole list.
//!
//! The tool's default, [`WinnerRule::Last`], is the one the second row
//! supports, and that row is also the design the name *ModPriority* implies:
//! reading highest-first and keeping the first write is how a priority system is
//! built. So the conclusion the tool reaches is probably right and the reasoning
//! printed beside it was wrong. [`Measured::basis`] says exactly that, in the
//! words the screen shows, rather than letting a green tick imply the question
//! is closed.
//!
//! # Why the log and not the sidecar
//!
//! [`super::sessionlog::Brief`] already records which files each mod got
//! opened, but it keeps them in a `BTreeSet` per folder, so the one fact this
//! module needs -- the *sequence* -- is exactly the fact it throws away.
//! Reading the text log costs a parse of a file already on disk, and buys
//! something a format change would not: it works retroactively, on every
//! session ever recorded, including those from the standalone tool this app
//! absorbed.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};

use super::analyze::WinnerRule;
use super::hostenv::HostInfo;
use super::model::Report;
use super::paths;

/// One mod file the game opened, in the order it opened them.
#[derive(Debug, Clone, Serialize)]
pub struct Load {
    /// the clock reading the line carried
    pub at: String,
    /// mod folder, as the game asked for it -- which is upper-cased
    pub folder: String,
    /// path within the mod folder, as reported
    pub rel: String,
    /// canonical asset key, so the `.EXML` and `.MBIN` of one asset agree
    pub target: String,
}

/// The order the game read the copies of one asset.
#[derive(Debug, Clone, Serialize)]
pub struct Applied {
    pub target: String,
    /// mod folders, in the order the game opened them.
    ///
    /// Spelled as they are on disk when the folder could be recognised, and as
    /// the game reported them when it could not -- a mod installed since the
    /// session ran has no on-disk spelling to find, and what the game said is
    /// then the best name there is.
    pub order: Vec<String>,
    /// `ModPriority` for each entry of `order`, where the game had registered
    /// the mod. `None` leaves the pair untestable rather than guessed.
    pub priorities: Vec<Option<i64>>,
}

impl Applied {
    /// The copy read first: the winner if the loader keeps the first write.
    pub fn read_first(&self) -> Option<&String> {
        self.order.first()
    }

    /// The copy read last: the winner if a later read overwrites an earlier one.
    pub fn read_last(&self) -> Option<&String> {
        self.order.last()
    }

    /// True when the priorities of this sequence can settle the direction.
    ///
    /// Two conditions, both necessary. Every mod has to be registered, or a gap
    /// could hide the very inversion being looked for. And the priorities have
    /// to differ: a run of equal values is consistent with both directions at
    /// once, so counting it as agreement would pad the evidence with a sequence
    /// that cannot disagree.
    pub fn testable(&self) -> bool {
        if self.order.len() < 2 || self.priorities.iter().any(Option::is_none) {
            return false;
        }
        let known: Vec<i64> = self.priorities.iter().flatten().copied().collect();
        known.iter().any(|p| *p != known[0])
    }

    /// Which way priority runs across this sequence, when it can be told.
    fn direction(&self) -> Option<Direction> {
        if !self.testable() {
            return None;
        }
        let p: Vec<i64> = self.priorities.iter().flatten().copied().collect();
        let rising = p.windows(2).all(|w| w[0] <= w[1]);
        let falling = p.windows(2).all(|w| w[0] >= w[1]);
        Some(match (rising, falling) {
            (true, false) => Direction::Ascending,
            (false, true) => Direction::Descending,
            // Neither monotonic: for this asset the game is not ordering by
            // ModPriority at all. Reported, never averaged away.
            _ => Direction::Mixed,
        })
    }
}

/// Which way `ModPriority` ran in the order the game read files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// priority rises as reading proceeds: the highest is read last
    Ascending,
    /// priority falls: the highest is read first. This is what the reference
    /// library measured.
    Descending,
    /// at least one asset was read in an order no priority rule describes
    Mixed,
    /// nothing in this session could settle it
    Untestable,
}

impl Direction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Direction::Ascending => "ascending",
            Direction::Descending => "descending",
            Direction::Mixed => "mixed",
            Direction::Untestable => "untestable",
        }
    }

    /// The winner rule this direction implies under one loader model.
    ///
    /// `overwrite` is the open question: true if a copy read later overwrites
    /// one read earlier, false if the first value written for a property is the
    /// one that survives. See the module doc -- the hook cannot see which it is,
    /// so a caller has to name the model it is asking about rather than be
    /// handed one answer that hides the choice.
    pub fn implies(&self, overwrite: bool) -> Option<WinnerRule> {
        let highest_read_last = match self {
            Direction::Ascending => true,
            Direction::Descending => false,
            Direction::Mixed | Direction::Untestable => return None,
        };
        // The highest priority survives when it was read last and the last read
        // wins, or when it was read first and the first write sticks.
        // `WinnerRule::Last` is this engine's name for "highest ModPriority
        // survives".
        Some(if highest_read_last == overwrite {
            WinnerRule::Last
        } else {
            WinnerRule::First
        })
    }
}

/// One of the two conclusions the measurement leaves open.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reading {
    /// the loader model, in words
    pub model: String,
    /// the winner rule it implies, as [`WinnerRule::as_str`] spells it
    pub rule: String,
    /// true when this is the rule the analysis currently uses
    pub is_current: bool,
}

/// A sequence that did not run the way the rest of them did.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Exception {
    pub target: String,
    pub order: Vec<String>,
    pub priorities: Vec<i64>,
    /// what this one sequence showed, against the majority
    pub direction: String,
}

/// What one session settled about load order.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Measured {
    /// the log this was read from
    pub log: String,
    pub measured_ms: i64,
    pub direction: Direction,
    /// the conclusions still consistent with the measurement -- two of them,
    /// because the direction alone cannot pick one
    pub readings: Vec<Reading>,
    /// assets read from more than one mod
    pub contested: usize,
    /// of those, how many could settle the direction
    pub testable: usize,
    /// sequences running the majority direction
    pub agreed: usize,
    /// sequences that did not, named rather than outvoted
    pub exceptions: Vec<Exception>,
    /// mod files the game opened at all, as a check on the parse
    pub loads: usize,
}

impl Measured {
    /// True when every testable sequence in this session agreed on a direction.
    ///
    /// One asset agreeing is thin evidence; it is also all some libraries will
    /// ever offer, so the bar is "something testable and nothing against it"
    /// rather than a count.
    ///
    /// `Mixed` can never satisfy this even with nothing to contradict it, and
    /// the empty-exception check alone would have let it: a lone asset read in
    /// no priority order at all *is* the majority direction, so it disagrees
    /// with nothing and would have read as settled. What it settles is that
    /// there is no rule.
    ///
    /// Note what this does not claim either way -- the direction is measured,
    /// the winner is not. See [`Measured::basis`].
    pub fn consistent(&self) -> bool {
        self.testable > 0
            && self.exceptions.is_empty()
            && matches!(
                self.direction,
                Direction::Ascending | Direction::Descending
            )
    }

    /// The paragraph the UI puts under the verdict.
    ///
    /// Written to leave the remaining question visible. The load order is
    /// measured; the step from order to surviving value is not, and a screen
    /// that said "confirmed" would be claiming the second.
    pub fn basis(&self) -> String {
        match self.direction {
            Direction::Untestable => {
                "No asset in this session was read from two mods with different priorities, \
                 so the load order could not be measured."
                    .to_string()
            }
            Direction::Mixed => format!(
                "The game did not read these assets in ModPriority order: {} of {} testable \
                 assets ran against the rest. Load order is not the whole story here, and the \
                 predicted winners cannot be trusted until this is understood.",
                self.exceptions.len(),
                self.testable
            ),
            Direction::Ascending | Direction::Descending => {
                let end = if self.direction == Direction::Descending {
                    "highest"
                } else {
                    "lowest"
                };
                let mut text = format!(
                    "Measured across {} contested asset(s): the game read each asset's copies \
                     in {} ModPriority order, {end} priority first",
                    self.testable,
                    self.direction.as_str(),
                );
                if self.exceptions.is_empty() {
                    text.push_str(", with no exceptions. ");
                } else {
                    text.push_str(&format!(
                        ", except for {} asset(s) listed below. ",
                        self.exceptions.len()
                    ));
                }
                text.push_str(
                    "That is the order, not the winner. Which copy survives still depends on \
                     whether the game lets a later read overwrite an earlier one or keeps the \
                     first value written, and a hook watching file opens cannot see the \
                     difference. Both readings are below.",
                );
                text
            }
        }
    }
}

/// Both conclusions the direction leaves open, with the current rule flagged.
fn readings_for(direction: Direction, current: WinnerRule) -> Vec<Reading> {
    [
        ("a copy read later overwrites one read earlier", true),
        ("the first value written for a property survives", false),
    ]
    .into_iter()
    .filter_map(|(model, overwrite)| {
        let rule = direction.implies(overwrite)?;
        Some(Reading {
            model: model.to_string(),
            rule: rule.as_str().to_string(),
            is_current: rule == current,
        })
    })
    .collect()
}

/// The analysis's predicted winner against the two the order allows.
#[derive(Debug, Clone, Serialize)]
pub struct Checked {
    pub target: String,
    pub predicted: Option<String>,
    /// the copy read first, which wins if the first write survives
    pub read_first: Option<String>,
    /// the copy read last, which wins if a later read overwrites
    pub read_last: Option<String>,
    /// true when the prediction is one of the two the observed order allows
    pub plausible: bool,
    /// true when the mods the analysis expected are the ones the game read
    pub same_mods: bool,
    pub order: Vec<String>,
}

/// Read the `modfile loaded` lines out of a session log, in order.
///
/// Lines are matched on the log category rather than on the message text: the
/// crash report quotes recently-touched paths back, and reading those as loads
/// would invent an order out of the game's last gasp.
pub fn loads(log_text: &str) -> Vec<Load> {
    let mut out = Vec::new();
    for line in log_text.lines() {
        let Some((at, cat, msg)) = split_line(line) else {
            continue;
        };
        if cat != "modfile" {
            continue;
        }
        let Some(path) = msg.strip_prefix("loaded ") else {
            continue;
        };
        let Some((folder, rel)) = super::sessionlog::mod_path(path) else {
            continue;
        };
        out.push(Load {
            at: at.to_string(),
            folder,
            target: paths::normalize_target(&rel),
            rel,
        });
    }
    out
}

/// Split one written log line into its clock, category and message.
///
/// The shape is fixed by `Recorder::record`: `HH:MM:SS.mmm LEVEL [cat      ]
/// message`, the category padded to a constant width. Parsed rather than
/// pattern-matched so a widened pad or a new level does not silently drop every
/// line.
fn split_line(line: &str) -> Option<(&str, &str, &str)> {
    let open = line.find('[')?;
    let close = line[open..].find(']')? + open;
    let at = line[..open].split_whitespace().next()?;
    let cat = line[open + 1..close].trim();
    let msg = line[close + 1..].trim_start();
    Some((at, cat, msg))
}

/// Group loads into the per-asset sequences the game read them in.
///
/// `known` is the mod folders as they are spelled on disk. The game asks for its
/// paths upper-cased -- `MODS\TERRALYSIS_PATH_OF_ORION\` for a folder called
/// `TERRALYSIS_Path_Of_Orion` -- so without this every name coming out of here
/// would fail to match the library and the whole join would silently produce
/// nothing. The same trap [`super::sessionlog::Recorder`] keeps a `spelling` map
/// for.
///
/// Only assets are considered, and only those read from more than one mod. Both
/// filters are load-bearing:
///
/// * A single copy has no order to observe.
/// * `LocTable.MXML` is the AMUMSS localisation table, not a game asset -- it
///   carries no target and the engine handles it separately as `loc_clashes`.
///   Left in, it is read from several mods and so looks contested, and on the
///   reference library it was the *only* sequence running against the measured
///   direction. One unfiltered non-asset was therefore enough to turn a
///   unanimous result into "no rule found", which is how a filtering mistake
///   becomes a wrong conclusion.
pub fn applied(loads: &[Load], known: &[String], host: &HostInfo) -> Vec<Applied> {
    let spelling: HashMap<String, &String> =
        known.iter().map(|name| (name.to_uppercase(), name)).collect();

    // BTreeMap so the output is ordered by asset and a run is reproducible; the
    // sequence *within* each asset is insertion order, which is the whole point
    // and must not be sorted.
    let mut by_target: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for load in loads {
        if !paths::is_asset(&load.rel) {
            continue;
        }
        let folder = spelling
            .get(&load.folder.to_uppercase())
            .map(|name| (*name).clone())
            .unwrap_or_else(|| load.folder.clone());
        let seen = by_target.entry(load.target.as_str()).or_default();
        // One mod can open its own copy more than once; the asset is applied
        // when it is first read, and a re-read does not move it down the order.
        if !seen.contains(&folder) {
            seen.push(folder);
        }
    }

    by_target
        .into_iter()
        .filter(|(_, order)| order.len() > 1)
        .map(|(target, order)| Applied {
            target: target.to_string(),
            priorities: order.iter().map(|name| host.priority_of(name)).collect(),
            order,
        })
        .collect()
}

/// Settle the direction of load order from one session.
///
/// The majority direction decides, and the minority is *named* rather than
/// averaged away: a single sequence running the other way is the one observation
/// that would have been worth the whole exercise, and [`Measured::consistent`]
/// goes false the moment there is one.
pub fn measure(
    log: &str,
    applied: &[Applied],
    loads: usize,
    current: WinnerRule,
    now_ms: i64,
) -> Measured {
    let judged: Vec<(&Applied, Direction)> = applied
        .iter()
        .filter_map(|a| a.direction().map(|d| (a, d)))
        .collect();

    let mut tally: BTreeMap<&'static str, usize> = BTreeMap::new();
    for (_, dir) in &judged {
        *tally.entry(dir.as_str()).or_insert(0) += 1;
    }
    let direction = match tally.iter().max_by_key(|(_, n)| **n).map(|(k, _)| *k) {
        None => Direction::Untestable,
        Some("ascending") => Direction::Ascending,
        Some("descending") => Direction::Descending,
        _ => Direction::Mixed,
    };

    let mut exceptions: Vec<Exception> = judged
        .iter()
        .filter(|(_, dir)| *dir != direction)
        .map(|(a, dir)| Exception {
            target: a.target.clone(),
            order: a.order.clone(),
            priorities: a.priorities.iter().flatten().copied().collect(),
            direction: dir.as_str().to_string(),
        })
        .collect();
    exceptions.sort_by(|a, b| a.target.cmp(&b.target));

    Measured {
        log: log.to_string(),
        measured_ms: now_ms,
        readings: readings_for(direction, current),
        direction,
        contested: applied.len(),
        testable: judged.len(),
        agreed: judged.len() - exceptions.len(),
        exceptions,
        loads,
    }
}

/// Check the analysis's predicted winners against the order the game read in.
///
/// Only targets the report calls contested are checked. A target the game read
/// from two mods that the *analysis* does not list is not a disagreement: the
/// report drops overlaps it has judged benign, and this is not the place to
/// reopen that.
pub fn check(report: &Report, applied: &[Applied]) -> Vec<Checked> {
    let observed: HashMap<&str, &Applied> =
        applied.iter().map(|a| (a.target.as_str(), a)).collect();

    report
        .conflicts
        .iter()
        .filter_map(|conflict| {
            let seen = observed.get(conflict.target.as_str())?;
            let ends = [seen.read_first(), seen.read_last()];
            let plausible = conflict.predicted_winner.as_ref().is_some_and(|winner| {
                ends.iter()
                    .flatten()
                    .any(|end| end.eq_ignore_ascii_case(winner))
            });
            // Whether the two halves are even talking about the same mods. A
            // prediction that fails because the game read a different set of
            // mods is a different problem from one where the rule is wrong, and
            // merging the two would hide both.
            let expected: BTreeSet<String> =
                conflict.mods.iter().map(|m| m.to_uppercase()).collect();
            let got: BTreeSet<String> = seen.order.iter().map(|m| m.to_uppercase()).collect();
            Some(Checked {
                target: conflict.target.clone(),
                predicted: conflict.predicted_winner.clone(),
                read_first: seen.read_first().cloned(),
                read_last: seen.read_last().cloned(),
                plausible,
                same_mods: expected == got,
                order: seen.order.clone(),
            })
        })
        .collect()
}

/// Everything one session settled, ready for the screen.
#[derive(Debug, Clone, Serialize)]
pub struct Observation {
    pub measured: Measured,
    pub basis: String,
    pub consistent: bool,
    pub applied: Vec<Applied>,
    pub checked: Vec<Checked>,
    /// predicted winners the observed order cannot produce under either model
    pub implausible: usize,
    /// contested assets where the analysis and the game disagree about *which*
    /// mods are involved
    pub mismatched: usize,
}

/// Read a session log and say everything it settles.
pub fn observe(
    log: &str,
    text: &str,
    known: &[String],
    host: &HostInfo,
    report: Option<&Report>,
    current: WinnerRule,
    now_ms: i64,
) -> Observation {
    let loaded = loads(text);
    let sequences = applied(&loaded, known, host);
    let measured = measure(log, &sequences, loaded.len(), current, now_ms);
    let checked = report.map(|r| check(r, &sequences)).unwrap_or_default();
    Observation {
        basis: measured.basis(),
        consistent: measured.consistent(),
        implausible: checked
            .iter()
            .filter(|c| c.predicted.is_some() && !c.plausible)
            .count(),
        mismatched: checked.iter().filter(|c| !c.same_mods).count(),
        measured,
        applied: sequences,
        checked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;

    fn host(pairs: &[(&str, i64)]) -> HostInfo {
        let mut priorities = IndexMap::new();
        for (name, p) in pairs {
            priorities.insert(name.to_uppercase(), *p);
        }
        HostInfo {
            priorities,
            ..Default::default()
        }
    }

    /// Build a log the way the recorder writes one.
    ///
    /// Spelled out through a helper rather than as a literal because these are
    /// Windows paths: a test full of escaped backslashes is a test nobody can
    /// read, and one whose escaping is wrong fails for the wrong reason. The
    /// folder and path are upper-cased on the way in, because that is how the
    /// game asks for them and matching the real shape is the point.
    fn log_of(loaded: &[(&str, &str)]) -> String {
        loaded
            .iter()
            .enumerate()
            .map(|(n, (folder, rel))| {
                format!(
                    "21:12:48.{:03} INFO  [modfile  ] loaded D:\\SteamLibrary\\steamapps\\common\\No Man's Sky\\GAMEDATA\\MODS\\{}\\{}\n",
                    n,
                    folder.to_uppercase(),
                    rel.to_uppercase().replace('/', "\\"),
                )
            })
            .collect()
    }

    /// Three mods patching one asset and a fourth patching its own, which is
    /// the shape of the real session log this was written against.
    fn real() -> String {
        log_of(&[
            ("No Bloom Effect 5.1", "GLOBALS/GCDEBUGOPTIONS.GLOBAL.EXML"),
            ("No Warp Flash 3.8", "GLOBALS/GCSPACESHIPGLOBALS.GLOBAL.EXML"),
            (
                "No Metrics Lines 3.5",
                "GLOBALS/GCSPACESHIPGLOBALS.GLOBAL.EXML",
            ),
            ("Flat Landing 4.8", "GLOBALS/GCSPACESHIPGLOBALS.GLOBAL.EXML"),
        ])
    }

    const SPACESHIP: &str = "GLOBALS/GCSPACESHIPGLOBALS.GLOBAL.MBIN";

    /// The priorities the real library has for those three, which run down.
    fn descending() -> HostInfo {
        host(&[
            ("NO WARP FLASH 3.8", 50),
            ("NO METRICS LINES 3.5", 47),
            ("FLAT LANDING 4.8", 32),
        ])
    }

    fn measured_from(text: &str, host: &HostInfo) -> Measured {
        let loaded = loads(text);
        let seq = applied(&loaded, &[], host);
        measure("x.log", &seq, loaded.len(), WinnerRule::Last, 0)
    }

    #[test]
    fn reads_the_real_log_shape() {
        let found = loads(&real());
        assert_eq!(found.len(), 4);
        assert_eq!(found[0].folder, "NO BLOOM EFFECT 5.1");
        assert_eq!(found[0].at, "21:12:48.000");
        assert_eq!(found[0].target, "GLOBALS/GCDEBUGOPTIONS.GLOBAL.MBIN");
        // Order is the whole product of this parse.
        assert_eq!(found[1].folder, "NO WARP FLASH 3.8");
        assert_eq!(found[3].folder, "FLAT LANDING 4.8");
    }

    #[test]
    fn a_crash_report_quoting_a_mod_path_is_not_a_load() {
        // The crash and module categories list recently-touched files. Reading
        // those as loads would invent an order out of the game's last gasp.
        let text = real().replace("[modfile  ]", "[crash    ]");
        assert!(loads(&text).is_empty());
    }

    #[test]
    fn only_contested_assets_have_an_order_to_observe() {
        let known = vec![
            "No Warp Flash 3.8".to_string(),
            "No Metrics Lines 3.5".to_string(),
            "Flat Landing 4.8".to_string(),
            "No Bloom Effect 5.1".to_string(),
        ];
        let seq = applied(&loads(&real()), &known, &host(&[]));
        assert_eq!(seq.len(), 1, "GCDEBUGOPTIONS had a single provider");
        assert_eq!(seq[0].target, SPACESHIP);
        // Spelled as the disk has them, not as the game shouted them.
        assert_eq!(
            seq[0].order,
            vec!["No Warp Flash 3.8", "No Metrics Lines 3.5", "Flat Landing 4.8"]
        );
    }

    #[test]
    fn the_loctable_is_not_an_asset_and_cannot_break_the_measurement() {
        // Measured on the real library this was the only sequence running
        // against the direction, and it is not a game asset at all. One
        // unfiltered non-asset turned a unanimous result into "no rule found".
        let text = format!(
            "{}{}",
            real(),
            log_of(&[
                ("Savegame By Hotkey", "LocTable.MXML"),
                ("alchemist_GPS", "LocTable.MXML"),
                ("RefinerWikiSlots", "LocTable.MXML"),
            ])
        );
        let m = measured_from(
            &text,
            &host(&[
                ("NO WARP FLASH 3.8", 50),
                ("NO METRICS LINES 3.5", 47),
                ("FLAT LANDING 4.8", 32),
                ("SAVEGAME BY HOTKEY", 6),
                ("ALCHEMIST_GPS", 11),
                ("REFINERWIKISLOTS", 58),
            ]),
        );
        assert_eq!(m.contested, 1, "only the real asset is contested");
        assert_eq!(m.direction, Direction::Descending);
        assert!(m.consistent());
    }

    #[test]
    fn an_unknown_folder_keeps_the_name_the_game_gave_it() {
        // A mod installed since the session ran has no on-disk spelling to
        // find, and dropping the load would lose a real observation.
        let seq = applied(&loads(&real()), &[], &host(&[]));
        assert_eq!(seq[0].order[0], "NO WARP FLASH 3.8");
    }

    #[test]
    fn the_reference_library_reads_highest_priority_first() {
        // The measurement this module was built to make. Descending, which is
        // the opposite of what the tool's documentation claimed.
        let m = measured_from(&real(), &descending());
        assert_eq!(m.direction, Direction::Descending);
        assert!(m.consistent());
        assert_eq!(m.testable, 1);
        assert!(m.basis().contains("highest priority first"), "{}", m.basis());
    }

    #[test]
    fn the_direction_alone_never_names_a_single_winner() {
        // The honest core of this module: both loader models stay open, so the
        // measurement offers two readings and flags which one is in use.
        let m = measured_from(&real(), &descending());
        assert_eq!(m.readings.len(), 2);
        let rules: Vec<&str> = m.readings.iter().map(|r| r.rule.as_str()).collect();
        assert!(
            rules.contains(&"first") && rules.contains(&"last"),
            "{rules:?}"
        );
        assert_eq!(m.readings.iter().filter(|r| r.is_current).count(), 1);
        assert!(m.basis().contains("not the winner"), "{}", m.basis());
    }

    #[test]
    fn reading_highest_first_and_keeping_the_first_write_means_highest_wins() {
        // The combination that makes the tool's default correct, and the one the
        // name ModPriority implies.
        assert_eq!(Direction::Descending.implies(false), Some(WinnerRule::Last));
        // Overwriting instead would make the lowest priority survive.
        assert_eq!(Direction::Descending.implies(true), Some(WinnerRule::First));
        // And the mirror image, so the mapping is not accidentally one-sided.
        assert_eq!(Direction::Ascending.implies(true), Some(WinnerRule::Last));
        assert_eq!(Direction::Ascending.implies(false), Some(WinnerRule::First));
        assert_eq!(Direction::Mixed.implies(true), None);
        assert_eq!(Direction::Untestable.implies(false), None);
    }

    #[test]
    fn a_rising_order_would_be_reported_as_rising() {
        let m = measured_from(
            &real(),
            &host(&[
                ("NO WARP FLASH 3.8", 3),
                ("NO METRICS LINES 3.5", 7),
                ("FLAT LANDING 4.8", 9),
            ]),
        );
        assert_eq!(m.direction, Direction::Ascending);
        assert!(m.basis().contains("lowest priority first"), "{}", m.basis());
    }

    #[test]
    fn equal_priorities_settle_nothing() {
        // Consistent with both directions at once, so counting it as agreement
        // would pad the evidence with a sequence that cannot disagree.
        let m = measured_from(
            &real(),
            &host(&[
                ("NO WARP FLASH 3.8", 5),
                ("NO METRICS LINES 3.5", 5),
                ("FLAT LANDING 4.8", 5),
            ]),
        );
        assert_eq!(m.direction, Direction::Untestable);
        assert!(!m.consistent());
        assert_eq!(m.testable, 0);
        assert!(m.readings.is_empty());
        assert!(m.basis().contains("could not be measured"));
    }

    #[test]
    fn an_unregistered_mod_leaves_the_sequence_untestable() {
        // A gap could hide the very inversion being looked for.
        let seq = applied(
            &loads(&real()),
            &[],
            &host(&[("NO WARP FLASH 3.8", 3), ("FLAT LANDING 4.8", 9)]),
        );
        assert!(!seq[0].testable());
        assert_eq!(
            measured_from(&real(), &host(&[("NO WARP FLASH 3.8", 3)])).direction,
            Direction::Untestable
        );
    }

    #[test]
    fn one_asset_running_against_the_rest_is_named_not_outvoted() {
        let text = format!(
            "{}{}",
            real(),
            log_of(&[
                ("A", "GLOBALS/Q.GLOBAL.EXML"),
                ("B", "GLOBALS/Q.GLOBAL.EXML"),
            ])
        );
        let m = measured_from(
            &text,
            &host(&[
                ("NO WARP FLASH 3.8", 50),
                ("NO METRICS LINES 3.5", 47),
                ("FLAT LANDING 4.8", 32),
                // Rising, against the other sequence.
                ("A", 1),
                ("B", 9),
            ]),
        );
        assert_eq!(m.testable, 2);
        assert_eq!(m.exceptions.len(), 1);
        assert_eq!(m.exceptions[0].target, "GLOBALS/Q.GLOBAL.MBIN");
        assert_eq!(m.exceptions[0].direction, "ascending");
        assert_eq!(m.agreed, 1);
        // The direction still reads as the majority, but nothing is settled.
        assert_eq!(m.direction, Direction::Descending);
        assert!(!m.consistent());
        assert!(m.basis().contains("except for 1"), "{}", m.basis());
    }

    #[test]
    fn an_asset_following_no_order_is_an_exception() {
        let text = log_of(&[
            ("A", "GLOBALS/Q.GLOBAL.EXML"),
            ("B", "GLOBALS/Q.GLOBAL.EXML"),
            ("C", "GLOBALS/Q.GLOBAL.EXML"),
        ]);
        let m = measured_from(&text, &host(&[("A", 5), ("B", 1), ("C", 8)]));
        assert_eq!(m.direction, Direction::Mixed);
        assert!(m.readings.is_empty(), "no rule can be read off no order");
        assert!(!m.consistent());
        assert!(m.basis().contains("not the whole story"));
    }

    #[test]
    fn a_mod_rereading_its_own_copy_does_not_move_down_the_order() {
        let text = format!(
            "{}{}",
            real(),
            log_of(&[(
                "No Warp Flash 3.8",
                "GLOBALS/GCSPACESHIPGLOBALS.GLOBAL.EXML"
            )])
        );
        let seq = applied(&loads(&text), &[], &host(&[]));
        assert_eq!(seq[0].order.len(), 3);
        assert_eq!(seq[0].order[0], "NO WARP FLASH 3.8");
    }

    #[test]
    fn mbin_and_exml_of_one_asset_are_one_target() {
        let text = log_of(&[
            ("A", "GLOBALS/Z.GLOBAL.EXML"),
            ("B", "GLOBALS/Z.GLOBAL.MBIN"),
        ]);
        let seq = applied(&loads(&text), &[], &host(&[]));
        assert_eq!(seq.len(), 1);
        assert_eq!(seq[0].order, vec!["A", "B"]);
    }

    #[test]
    fn a_prediction_at_either_end_of_the_order_is_plausible() {
        use super::super::model::Conflict;
        let seq = applied(&loads(&real()), &[], &descending());
        let conflict = |winner: &str| Conflict {
            target: SPACESHIP.to_string(),
            mods: vec![
                "NO WARP FLASH 3.8".to_string(),
                "NO METRICS LINES 3.5".to_string(),
                "FLAT LANDING 4.8".to_string(),
            ],
            predicted_winner: Some(winner.to_string()),
            ..Default::default()
        };

        for end in ["NO WARP FLASH 3.8", "FLAT LANDING 4.8"] {
            let report = Report {
                conflicts: vec![conflict(end)],
                ..Default::default()
            };
            let got = check(&report, &seq);
            assert!(got[0].plausible, "{end} is an end of the order");
            assert!(got[0].same_mods);
        }

        // The middle copy cannot win under either loader model, so a prediction
        // naming it is a finding rather than a tick.
        let report = Report {
            conflicts: vec![conflict("NO METRICS LINES 3.5")],
            ..Default::default()
        };
        let got = check(&report, &seq);
        assert!(!got[0].plausible);
        assert_eq!(got[0].read_first.as_deref(), Some("NO WARP FLASH 3.8"));
        assert_eq!(got[0].read_last.as_deref(), Some("FLAT LANDING 4.8"));
    }
}
