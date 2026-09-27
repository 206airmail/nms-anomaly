//! Change a value a mod sets, without changing the mod.
//!
//! Every other verb in this program decides *which* copy of an asset the game
//! reads: switch a mod off, clean a whole-file override down to its edits,
//! combine two mods that do not overlap, pick a winner by load order. None of
//! them can answer the question a user actually arrives with, which is "this
//! mod gives me 60% and I want 45%".
//!
//! Doing that by hand means finding the file inside the mod, decompiling it if
//! it is an `.MBIN`, finding the one property among a hundred thousand, editing
//! it, recompiling, and remembering to do the whole thing again the next time
//! the mod updates. This module is that, as a list of text boxes.
//!
//! # What is offered, and why it is exactly that
//!
//! Only the properties the mod *changes from vanilla* — the set
//! [`super::prune::changed_leaves`] computes, shared with cleaning so the two
//! features can never disagree. That bound is not a simplification, it is the
//! honest one:
//!
//! * a property the mod ships at the game's own value is not something the mod
//!   does, so changing it here would be inventing a mod edit and calling it
//!   one the author made;
//! * and it is the set cleaning keeps. Offering anything outside it would mean
//!   an edit that silently disappeared the next time the mod was cleaned.
//!
//! Each property therefore has three values worth knowing, and all three are
//! reported: what the installed game has (`vanilla`), what the mod's author
//! shipped (`author`), and what you set (`yours`). The first two are facts
//! about files on disk, re-read every time this is opened; only the third is
//! stored.
//!
//! # The edits are data, not a file
//!
//! What is remembered is `path -> value`, in `edits.json`, and the build is
//! produced from it. That is the whole reason this is worth having over
//! "decompile it yourself and keep a note": a mod that updates is rebuilt with
//! your values re-applied by path, and the one property that the update *moved*
//! is reported by name instead of quietly reverting.
//!
//! It is also what makes the overlay compose with the other builds. Cleaning a
//! mod rebuilds it from the copy its author shipped, so an edit written into
//! the cleaned build would be thrown away — see the rebuild in `run_edit`,
//! which every other build runs through afterwards.
//!
//! # The build has its own folder, and that is not an accident
//!
//! [`super::loadout::reconcile`] decides "already correct" from the file names
//! deployed and the `source` they came from. An edit changes a file's contents
//! and nothing else, so editing a build *in place* reports unchanged and leaves
//! the old bytes in the game — the same trap mending fell into, recorded on
//! `Entry::built_from`. Deploying is by hardlink, so an edit that happened to
//! preserve the inode would sometimes appear in the game anyway and sometimes
//! not, depending on whether MBINCompiler wrote a fresh file. A separate folder
//! makes `source` different, which makes the relink certain.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::decompile::Decompiler;
use super::discovery;
use super::exmltree::{self, Tree};
use super::model::FileKind;
use super::prune;
use super::vanilla::VanillaSource;

/// Suffix on the build folder holding your values. See the module note.
pub const SUFFIX: &str = "__edited";

// ---------------------------------------------------------------------------
// what a value is
// ---------------------------------------------------------------------------

/// What kind of value sits at a property, which decides the input drawn for it.
///
/// Inferred from the author's value rather than looked up, because the type is
/// only recorded in MBINCompiler's own templates and nothing here reads those.
/// Inference is safe in the one direction that matters: a value that parses as
/// a number is offered a number box and rejected if you type a word into it,
/// and anything else is a plain text box that accepts whatever the author's
/// value was. Being *wrong* means offering a text box for a float, which costs
/// a validation message; it never means writing a value the game cannot read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    /// `True` / `False`
    Bool,
    /// a whole number
    Int,
    /// `40.000000`
    Float,
    /// an enum name, a resource id, a path — anything else
    Text,
}

/// Which sort of value this is, judged from the text of it. See [`Sort`].
pub fn sort_of(value: &str) -> Sort {
    Sort::of(value)
}

impl Sort {
    fn of(value: &str) -> Sort {
        let text = value.trim();
        if text.eq_ignore_ascii_case("true") || text.eq_ignore_ascii_case("false") {
            return Sort::Bool;
        }
        // A leading `0x` parses as neither, and must not: it is an id, and
        // offering arithmetic on it would be nonsense.
        if text.len() > 1 && text[1..].contains('x') {
            return Sort::Text;
        }
        if text.parse::<i64>().is_ok() {
            return Sort::Int;
        }
        if text.parse::<f64>().is_ok() {
            return Sort::Float;
        }
        Sort::Text
    }
}

/// Is `value` something the game could read at a property of this sort?
///
/// Checked here, where the value is recorded, rather than at the point the file
/// is written: a refusal is only useful next to the box it came from, and a
/// build that fails halfway through has already replaced some of the mod.
pub fn check(sort: Sort, value: &str) -> Result<(), String> {
    let text = value.trim();
    if text.is_empty() {
        return Err("a value cannot be empty — clear the box to go back to the \
                    author's value instead"
            .into());
    }
    match sort {
        Sort::Bool => {
            if text.eq_ignore_ascii_case("true") || text.eq_ignore_ascii_case("false") {
                Ok(())
            } else {
                Err(format!("{text} is not True or False"))
            }
        }
        Sort::Int => text
            .parse::<i64>()
            .map(|_| ())
            .map_err(|_| format!("{text} is not a whole number")),
        Sort::Float => text
            .parse::<f64>()
            .map(|_| ())
            .map_err(|_| format!("{text} is not a number")),
        Sort::Text => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// what is remembered
// ---------------------------------------------------------------------------

/// One value you set, and the author's value it replaced.
///
/// `was` is not decoration. Two things need it and neither can be done without
/// it: going back to the author's value, which cannot be read off a build that
/// no longer holds it; and noticing that an *update* changed the author's value
/// under your edit, which is the one case where keeping your number silently is
/// the wrong answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub value: String,
    /// the mod's own value at this path when the change was made
    pub was: String,
}

/// Every value you have set, across every mod.
///
/// Keyed by the loadout entry's owner, then by `<folder>/<path inside it>` —
/// the file as the *author* ships it. Not by canonical asset target: one mod
/// may put two folders in the game and both may carry the same asset, and not
/// by the built file's path either, because cleaning moves an `.MBIN` to an
/// `.EXML` and the record has to outlive that.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Book {
    #[serde(default)]
    pub mods: BTreeMap<String, BTreeMap<String, BTreeMap<String, Change>>>,
}

/// One file's worth of values, as the book holds them.
pub type FileEdits = BTreeMap<String, Change>;
/// One mod's worth, keyed by file.
pub type ModEdits = BTreeMap<String, FileEdits>;

impl Book {
    /// Read, treating anything unreadable as "nothing has been edited".
    ///
    /// The same tolerance the loadout has, for the same reason: this file is in
    /// a folder a user can open, and a BOM from Notepad must not read as an
    /// empty set of edits with a rebuild behind it.
    pub fn read(path: &Path) -> Book {
        super::read_json(path).unwrap_or_default()
    }

    pub fn write(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("could not make {}: {e}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| format!("could not write {}: {e}", path.display()))
    }

    pub fn of(&self, owner: &str) -> Option<&ModEdits> {
        self.mods.get(owner)
    }

    /// How many values you have set on this mod.
    pub fn count(&self, owner: &str) -> usize {
        self.of(owner)
            .map(|files| files.values().map(BTreeMap::len).sum())
            .unwrap_or(0)
    }

    pub fn set(&mut self, owner: &str, file: &str, path: &str, change: Change) {
        self.mods
            .entry(owner.to_string())
            .or_default()
            .entry(file.to_string())
            .or_default()
            .insert(path.to_string(), change);
    }

    /// Stop holding a value at one property. Returns what was held, if any.
    ///
    /// Empty files and empty mods are pruned as they empty, so `of` answering
    /// `None` is the same question as "has this mod been edited at all".
    pub fn clear(&mut self, owner: &str, file: &str, path: &str) -> Option<Change> {
        let files = self.mods.get_mut(owner)?;
        let was = files.get_mut(file).and_then(|paths| paths.remove(path));
        files.retain(|_, paths| !paths.is_empty());
        if files.is_empty() {
            self.mods.remove(owner);
        }
        was
    }

    /// Drop every value set on one mod. True when there was something to drop.
    pub fn forget(&mut self, owner: &str) -> bool {
        self.mods.remove(owner).is_some()
    }
}

// ---------------------------------------------------------------------------
// what is shown
// ---------------------------------------------------------------------------

/// One property a mod changes, with everywhere its value comes from.
#[derive(Debug, Clone, Serialize)]
pub struct Field {
    /// the property path, e.g. `Table[LAUNCHER]/Cost`
    pub path: String,
    /// what the installed game has here. `None` when the game's copy has no
    /// such property, i.e. the mod introduces it.
    pub vanilla: Option<String>,
    /// what the mod's author ships here
    pub author: String,
    /// what you set, when you have set something
    pub yours: Option<String>,
    pub sort: Sort,
    /// The author's value has changed since you set yours.
    ///
    /// The mod updated, and the number your edit was a reaction to is not the
    /// number it ships now. Your value still applies — reverting it silently
    /// would be worse — but this is the one case where the author has
    /// effectively disagreed with you since, and it is worth saying so.
    pub author_moved: Option<String>,
}

/// Every property of one asset that its mod changes.
#[derive(Debug, Clone, Serialize)]
pub struct Asset {
    /// the folder the game loads this from
    pub folder: String,
    /// `<folder>/<path inside it>`, as the book keys it and the UI sends it back
    pub file: String,
    /// where the file sits inside that folder
    pub rel: String,
    /// the canonical asset key, for the heading
    pub target: String,
    /// true when this copy replaces the asset outright — a compiled `.MBIN`
    pub whole_file: bool,
    /// false when the game ships no copy of this asset: the mod adds it, so
    /// every property in it is an introduction and none has a vanilla value
    pub in_game: bool,
    pub fields: Vec<Field>,
    /// why nothing here can be edited; `None` when it can
    pub refused: Option<String>,
}

impl Asset {
    /// How many of this asset's values you have set.
    pub fn touched(&self) -> usize {
        self.fields.iter().filter(|f| f.yours.is_some()).count()
    }
}

/// One held value the mod can no longer carry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stale {
    pub file: String,
    pub path: String,
    /// the value still held for it
    pub value: String,
    /// why it can no longer be carried, in words the UI can show
    pub why: String,
}

/// Everything the editor needs about one mod.
#[derive(Debug, Clone, Serialize)]
pub struct Survey {
    pub owner: String,
    pub assets: Vec<Asset>,
    /// Held values the mod cannot carry any more.
    ///
    /// An update can take a file away or move a property, and when it does the
    /// value set on it stops being applicable *and* stops being visible — which
    /// would leave it in `edits.json` for ever, invisibly, with a `lost` line on
    /// every rebuild that nobody is looking at. Named here so the UI can offer
    /// to forget it.
    pub stale: Vec<Stale>,
}

/// Read one mod, and say what can be changed in it.
///
/// `origin` is the copy its author shipped — never the build the game is
/// reading. The author's value is one of the three facts this reports, and a
/// cleaned or already-edited build is not where that lives.
///
/// `members` are the folders the mod puts in the game, which is what `origin`
/// holds at its top level. One staged mod is often several of these.
pub fn survey(
    owner: &str,
    origin: &Path,
    members: &[String],
    decompiler: &mut Decompiler,
    source: &mut VanillaSource,
    held: Option<&ModEdits>,
) -> Survey {
    // Every asset across every member folder, with the file key it is stored
    // under, before anything is extracted: the extraction is one call for the
    // whole mod and each one shells out to hgpaktool.
    struct Subject {
        folder: String,
        file: String,
        rel: String,
        target: String,
        abs: PathBuf,
        sha1: String,
        whole_file: bool,
    }

    let mut subjects: Vec<Subject> = Vec::new();
    for member in members {
        let scanned = discovery::scan_mod(&origin.join(member), member, false);
        for file in &scanned.files {
            let Some(target) = file.target.as_deref() else {
                continue;
            };
            let whole_file = match file.kind {
                Some(FileKind::Mbin) => true,
                Some(FileKind::Exml) => false,
                // A texture or a build script has no properties to offer. Not
                // a refusal worth reporting: nobody opened the editor to change
                // a `.DDS`.
                _ => continue,
            };
            subjects.push(Subject {
                folder: member.clone(),
                file: file_key(member, &file.rel_path),
                rel: file.rel_path.clone(),
                target: target.to_string(),
                abs: PathBuf::from(&file.abs_path),
                sha1: file.sha1.clone(),
                whole_file,
            });
        }
    }

    let targets: Vec<String> = subjects.iter().map(|s| s.target.clone()).collect();
    let extraction = source.fetch(&targets);

    let mut assets: Vec<Asset> = Vec::new();
    for subject in &subjects {
        let yours = held.and_then(|files| files.get(&subject.file));
        let mut asset = Asset {
            folder: subject.folder.clone(),
            file: subject.file.clone(),
            rel: subject.rel.clone(),
            target: subject.target.clone(),
            whole_file: subject.whole_file,
            in_game: extraction.found.contains_key(&subject.target),
            fields: Vec::new(),
            refused: None,
        };

        // The mod's own copy, as XML. A compiled asset has to go through
        // MBINCompiler to be read at all; an `.EXML` already is the XML.
        let mod_xml = if subject.whole_file {
            decompiler
                .decompile(&subject.abs, &subject.sha1)
                .and_then(|path| std::fs::read_to_string(path).ok())
        } else {
            std::fs::read_to_string(&subject.abs).ok()
        };
        let Some(mod_xml) = mod_xml else {
            asset.refused = Some(if subject.whole_file {
                "this file could not be decompiled, so what it changes cannot be read".into()
            } else {
                "this file could not be read".into()
            });
            assets.push(asset);
            continue;
        };
        let tree = match exmltree::parse_str(&mod_xml) {
            Ok(tree) => tree,
            Err(why) => {
                asset.refused = Some(format!("this file could not be parsed: {why}"));
                assets.push(asset);
                continue;
            }
        };
        let props = tree.flatten();

        // The game's own copy. Absent means the mod invented this asset, which
        // is not a refusal: every property in it is the mod's, so all of them
        // are editable and none of them has a vanilla value to show.
        let vanilla = extraction
            .found
            .get(&subject.target)
            .and_then(|path| decompiler.decompile_file(path))
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|xml| exmltree::parse_str(&xml).ok())
            .map(|tree| tree.flatten());
        if asset.in_game && vanilla.is_none() {
            asset.refused =
                Some("the game's own copy of this asset could not be read to compare against"
                    .into());
            assets.push(asset);
            continue;
        }
        let empty = indexmap::IndexMap::new();
        let baseline = vanilla.as_ref().unwrap_or(&empty);

        // What the mod changes, plus anything a value is already held for.
        //
        // The second half is not tidiness. A held value is applied by *setting
        // the path*, which works whether or not the mod still disagrees with the
        // game there -- so if an update reverted a property to the game's own
        // value, `changed_leaves` stops naming it while the user's value goes on
        // loading. Showing only the changed set would leave that edit live and
        // invisible, which is the worst of the three possible answers.
        let mut offer = prune::changed_leaves(baseline, &tree, &props);
        if let Some(paths) = yours {
            let named: HashSet<&str> = offer.iter().map(String::as_str).collect();
            let extra: Vec<String> = paths
                .keys()
                .filter(|path| !named.contains(path.as_str()) && props.contains_key(*path))
                .cloned()
                .collect();
            offer.extend(extra);
        }

        for path in offer {
            // A leaf always has a value: `flatten` gives a property with no
            // `value` attribute `None`, and that is a container. Skipped rather
            // than shown empty, because there is nothing a box could hold.
            let Some(author) = props.get(&path).and_then(Option::clone) else {
                continue;
            };
            let change = yours.and_then(|paths| paths.get(&path));
            asset.fields.push(Field {
                sort: Sort::of(&author),
                vanilla: baseline.get(&path).and_then(Option::clone),
                author_moved: change
                    .filter(|held| held.was != author)
                    .map(|held| held.was.clone()),
                yours: change.map(|held| held.value.clone()),
                author,
                path,
            });
        }
        assets.push(asset);
    }

    // Held values the mod can no longer carry at all. See `Survey::stale`.
    let mut stale: Vec<Stale> = Vec::new();
    if let Some(files) = held {
        let reachable: HashSet<(&str, &str)> = assets
            .iter()
            .flat_map(|a| a.fields.iter().map(|f| (a.file.as_str(), f.path.as_str())))
            .collect();
        let readable: HashSet<&str> = assets
            .iter()
            .filter(|a| a.refused.is_none())
            .map(|a| a.file.as_str())
            .collect();
        for (file, paths) in files {
            // An asset that could not be read is not a lost value: the next run
            // with the tools present will find it. Only say "gone" when we
            // actually looked.
            if !readable.contains(file.as_str()) {
                continue;
            }
            for (path, change) in paths {
                if reachable.contains(&(file.as_str(), path.as_str())) {
                    continue;
                }
                stale.push(Stale {
                    file: file.clone(),
                    path: path.clone(),
                    value: change.value.clone(),
                    why: "this property is not in the mod any more".into(),
                });
            }
        }
        // And values held against a file the mod no longer ships at all.
        let known: HashSet<&str> = subjects.iter().map(|s| s.file.as_str()).collect();
        for (file, paths) in files {
            if known.contains(file.as_str()) {
                continue;
            }
            for (path, change) in paths {
                stale.push(Stale {
                    file: file.clone(),
                    path: path.clone(),
                    value: change.value.clone(),
                    why: "this file is not in the mod any more".into(),
                });
            }
        }
    }

    // Most edited first, then by folder: the reason the editor was opened is
    // almost always a value already set.
    assets.sort_by(|a, b| {
        b.touched()
            .cmp(&a.touched())
            .then_with(|| a.file.cmp(&b.file))
    });

    Survey {
        owner: owner.to_string(),
        assets,
        stale,
    }
}

/// How the book names one file inside a mod. Always forward slashes: the key is
/// written to disk and read back on a path that may have been scanned either
/// way round.
pub fn file_key(folder: &str, rel: &str) -> String {
    format!("{folder}/{}", rel.replace('\\', "/"))
}

// ---------------------------------------------------------------------------
// writing the build
// ---------------------------------------------------------------------------

/// What applying an overlay to a build produced.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Applied {
    /// a line per file rewritten
    pub done: Vec<String>,
    /// values that could not be applied, one message each.
    ///
    /// Reported rather than fatal, and reported loudly. A mod that updated may
    /// have moved or removed the property an edit named; refusing the whole
    /// build over it would leave the mod unusable, and applying the rest in
    /// silence is exactly the "quietly does less than it said" failure the rest
    /// of this engine is built to avoid.
    pub lost: Vec<String>,
}

/// Write `edits` into the build at `dest`, which must already be a copy of the
/// mod.
///
/// `dest` is addressed the same way [`super::prune::clean_into`] addresses it:
/// `<folder>/<rel>` inside it, because that is where the staged mod keeps the
/// file and where `deploy` mirrors it into the game from. The one wrinkle is
/// that a *cleaned* build has moved the file: an `.MBIN` override became an
/// `.EXML` patch at the same stem. Both are tried, and the patch is preferred,
/// because a build that has been cleaned no longer holds the `.MBIN` at all.
pub fn apply_into(
    dest: &Path,
    edits: &ModEdits,
    decompiler: &mut Decompiler,
) -> Result<Applied, String> {
    let mut out = Applied::default();
    for (file, paths) in edits {
        if paths.is_empty() {
            continue;
        }
        let Some(at) = locate(dest, file) else {
            out.lost.push(format!(
                "{file} is not in this build, so the {} value(s) set on it were not applied",
                paths.len()
            ));
            continue;
        };
        // MBINCompiler decides direction from the extension, so what is on
        // disk in the build says whether this has to be recompiled.
        let compiled = at
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("MBIN"));

        let tree = if compiled {
            decompiler
                .decompile_file(&at)
                .and_then(|path| std::fs::read_to_string(path).ok())
                .ok_or_else(|| format!("{file} could not be decompiled to change"))
                .and_then(|xml| exmltree::parse_str(&xml))
        } else {
            exmltree::parse(&at)
        };
        let mut tree = tree.map_err(|why| format!("{file}: {why}"))?;

        let index = tree.index();
        let mut set = 0usize;
        for (path, change) in paths {
            if tree.set(&index, path, Some(&change.value)) {
                set += 1;
            } else {
                out.lost.push(format!(
                    "{file} no longer has {path}, so your value of {} was not applied",
                    change.value
                ));
            }
        }
        if set == 0 {
            continue;
        }

        write_back(&at, &tree, compiled, decompiler)?;
        out.done.push(format!(
            "{}: {set} value(s) set",
            at.file_name().unwrap_or_default().to_string_lossy()
        ));
    }
    Ok(out)
}

/// Where a file the book names sits inside a build. See [`apply_into`].
fn locate(dest: &Path, file: &str) -> Option<PathBuf> {
    let at: PathBuf = file.split('/').filter(|p| !p.is_empty()).collect();
    let shipped = dest.join(&at);
    // A cleaned build put a patch where the override was, and removed the
    // override. Checked first: on an uncleaned build the patch does not exist,
    // and on a cleaned one the override does not.
    let patch = dest.join(at.with_extension("EXML"));
    if patch.is_file() {
        return Some(patch);
    }
    shipped.is_file().then_some(shipped)
}

/// Put the edited tree back where it came from, in the form it came in.
///
/// A compiled asset is recompiled. MBINCompiler converts in place and leaves
/// its input beside its output, so it is run in a scratch folder and only the
/// `.MBIN` is brought back: a stray loose `.EXML` left in a mod folder is read
/// by the game at some paths, and is a second copy of the asset at all of them,
/// which would make this mod refuse to be cleaned ever again.
fn write_back(
    at: &Path,
    tree: &Tree,
    compiled: bool,
    decompiler: &mut Decompiler,
) -> Result<(), String> {
    let xml = exmltree::to_string(tree);
    if !compiled {
        return std::fs::write(at, xml)
            .map_err(|e| format!("could not write {}: {e}", at.display()));
    }

    let name = at
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or("the file being edited has no name")?;
    let stem = name.trim_end_matches(".MBIN").trim_end_matches(".mbin");
    let scratch = at
        .parent()
        .ok_or("the file being edited has no folder")?
        .join(".anomaly-compile");
    let _ = std::fs::remove_dir_all(&scratch);

    let built = decompiler.compile(&xml, &scratch, stem);
    let result = built.and_then(|mbin| {
        std::fs::copy(&mbin, at)
            .map(|_| ())
            .map_err(|e| format!("could not put the rebuilt {name} back: {e}"))
    });
    // Whatever happened, take the scratch folder away: it holds the XML
    // MBINCompiler was pointed at, and leaving that inside the mod is the
    // hazard the doc above describes.
    let _ = std::fs::remove_dir_all(&scratch);
    result
}

/// Copy a whole mod folder. The base of an edited build is always a copy.
pub fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("could not make {}: {e}", to.display()))?;
    let entries =
        std::fs::read_dir(from).map_err(|e| format!("could not read {}: {e}", from.display()))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let into = to.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, &into)?;
        } else {
            std::fs::copy(&path, &into)
                .map_err(|e| format!("could not copy {}: {e}", path.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Data template="GcFoo">
  <Property name="Table">
    <Property name="Row" _id="LAUNCHER">
      <Property name="Cost" value="40.000000" />
      <Property name="Name" value="Launcher" />
    </Property>
  </Property>
</Data>"#;

    /// A decompiler that is never run.
    ///
    /// Every test here edits an `.EXML`, which is read and written as text, so
    /// the executable is not reached. Constructed rather than located so the
    /// tests pass on a machine with no MBINCompiler installed.
    fn unused_decompiler() -> Decompiler {
        Decompiler {
            exe: PathBuf::from("mbincompiler-not-needed"),
            cache_dir: std::env::temp_dir().join("anomaly_edit_cache"),
            converted: 0,
            cache_hits: 0,
            failures: Vec::new(),
        }
    }

    fn dir(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("anomaly_edit_{tag}"));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn sorts_are_inferred_from_the_authors_value() {
        assert_eq!(Sort::of("40.000000"), Sort::Float);
        assert_eq!(Sort::of("7"), Sort::Int);
        assert_eq!(Sort::of("-7"), Sort::Int);
        assert_eq!(Sort::of("True"), Sort::Bool);
        assert_eq!(Sort::of("false"), Sort::Bool);
        assert_eq!(Sort::of("GcRealitySubstanceCategory"), Sort::Text);
        // An id is text, however much it looks like a number.
        assert_eq!(Sort::of("0x1F"), Sort::Text);
    }

    #[test]
    fn a_value_is_checked_against_its_sort() {
        assert!(check(Sort::Float, "45.5").is_ok());
        assert!(check(Sort::Float, "lots").is_err());
        assert!(check(Sort::Int, "45.5").is_err());
        assert!(check(Sort::Bool, "TRUE").is_ok());
        assert!(check(Sort::Bool, "yes").is_err());
        assert!(check(Sort::Text, "anything").is_ok());
        // Empty is never a value; clearing the box means the author's value.
        assert!(check(Sort::Text, "  ").is_err());
    }

    #[test]
    fn an_exml_is_edited_in_place() {
        let root = dir("exml");
        let folder = root.join("MyMod/METADATA");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("FOO.EXML"), DOC).unwrap();

        let mut edits = ModEdits::new();
        edits.insert(
            "MyMod/METADATA/FOO.EXML".into(),
            [(
                "Table/Row[LAUNCHER]/Cost".to_string(),
                Change { value: "45.000000".into(), was: "40.000000".into() },
            )]
            .into_iter()
            .collect(),
        );

        let mut decompiler = unused_decompiler();
        let applied = apply_into(&root, &edits, &mut decompiler).unwrap();
        assert!(applied.lost.is_empty(), "{:?}", applied.lost);
        assert_eq!(applied.done.len(), 1);

        let now = std::fs::read_to_string(folder.join("FOO.EXML")).unwrap();
        assert!(now.contains("45.000000"), "{now}");
        assert!(!now.contains("40.000000"), "{now}");
        // Everything the mod did besides that value is still there.
        assert!(now.contains(r#"value="Launcher""#), "{now}");
    }

    #[test]
    fn a_value_the_mod_no_longer_has_is_reported_not_dropped_silently() {
        let root = dir("moved");
        let folder = root.join("MyMod/METADATA");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("FOO.EXML"), DOC).unwrap();

        let mut edits = ModEdits::new();
        edits.insert(
            "MyMod/METADATA/FOO.EXML".into(),
            [(
                "Table/Row[RIFLE]/Cost".to_string(),
                Change { value: "1.0".into(), was: "2.0".into() },
            )]
            .into_iter()
            .collect(),
        );

        let mut decompiler = unused_decompiler();
        let applied = apply_into(&root, &edits, &mut decompiler).unwrap();
        assert_eq!(applied.done.len(), 0);
        assert_eq!(applied.lost.len(), 1);
        assert!(applied.lost[0].contains("Table/Row[RIFLE]/Cost"));
    }

    #[test]
    fn a_cleaned_build_is_found_at_its_patch_path() {
        let root = dir("cleaned");
        let folder = root.join("MyMod/METADATA");
        std::fs::create_dir_all(&folder).unwrap();
        // What cleaning leaves: the patch, and no `.MBIN`.
        std::fs::write(folder.join("FOO.EXML"), DOC).unwrap();

        // The book names the file the author shipped, which was compiled.
        assert_eq!(
            locate(&root, "MyMod/METADATA/FOO.MBIN"),
            Some(folder.join("FOO.EXML"))
        );
    }

    #[test]
    fn a_file_key_is_always_forward_slashed() {
        assert_eq!(
            file_key("MyMod", "METADATA\\FOO.MBIN"),
            "MyMod/METADATA/FOO.MBIN"
        );
    }

    /// The case this whole `stale`/`offer` business exists for.
    ///
    /// A held value is applied by *setting the path*, which works whether or not
    /// the mod still disagrees with the game there. So when an update reverts a
    /// property to the game's own value, the changed set stops naming it while
    /// the user's value goes on loading -- and an edit that is live and invisible
    /// is worse than one reported as lost. It must still be offered.
    #[test]
    fn a_held_value_is_offered_even_where_the_mod_no_longer_differs() {
        let mod_xml = r#"<Data template="GcFoo">
  <Property name="Cost" value="40.000000" />
  <Property name="Name" value="Launcher" />
</Data>"#;
        // The game agrees with the mod about Cost: nothing is changed there.
        let vanilla_xml = r#"<Data template="GcFoo">
  <Property name="Cost" value="40.000000" />
  <Property name="Name" value="Rifle" />
</Data>"#;

        let tree = exmltree::parse_str(mod_xml).unwrap();
        let props = tree.flatten();
        let vanilla = exmltree::parse_str(vanilla_xml).unwrap().flatten();

        let changed = prune::changed_leaves(&vanilla, &tree, &props);
        assert_eq!(changed, vec!["Name".to_string()], "Cost is not a change");

        // But a value held at `Cost` is still applied, and the tree still has
        // the path -- so the editor has to show it.
        let mut paths = FileEdits::new();
        paths.insert(
            "Cost".into(),
            Change { value: "99.000000".into(), was: "60.000000".into() },
        );
        assert!(props.contains_key("Cost"));
        assert!(!changed.contains(&"Cost".to_string()));
        // Which is exactly the condition `survey` adds on: held, not changed,
        // present in the file.
        let extra: Vec<&String> = paths
            .keys()
            .filter(|p| !changed.contains(p) && props.contains_key(*p))
            .collect();
        assert_eq!(extra, vec![&"Cost".to_string()]);
    }

    #[test]
    fn clearing_the_last_value_forgets_the_mod() {
        let mut book = Book::default();
        book.set(
            "M",
            "M/A.EXML",
            "Foo",
            Change { value: "1".into(), was: "2".into() },
        );
        assert_eq!(book.count("M"), 1);
        assert!(book.clear("M", "M/A.EXML", "Foo").is_some());
        assert_eq!(book.count("M"), 0);
        // Not merely empty: absent, so "has this been edited" is one question.
        assert!(book.of("M").is_none());
    }
}
