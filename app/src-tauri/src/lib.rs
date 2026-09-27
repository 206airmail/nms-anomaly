pub mod adblock;
pub mod browser;
pub mod engine;
pub mod watch;

use std::path::{Path, PathBuf};

use engine::analyze::WinnerRule;
use engine::gamefind::{self, Install};
use engine::decompile::{self, Decompiler};
use engine::vanilla::VanillaSource;
use engine::namecache::Names;
use engine::hook;
use engine::{
    analyze, deploy, drift, edit, erase, library, loadout, merge, nexus, nxm, pipeline, preset,
    prune, repair, report, savewatch, scancache, sessionlog, settings, tools,
};

use tauri::{Emitter, Manager};

/// Every No Man's Sky install found, best source first.
///
/// The UI shows the first one and offers the rest, rather than scanning them
/// together: a mod present in two installs would otherwise be reported as
/// conflicting with its own other copy.
#[tauri::command]
fn find_installs() -> Vec<Install> {
    gamefind::find_installs()
}

/// The install that would be scanned, or `None` when the game is not found.
#[tauri::command]
fn find_install() -> Option<Install> {
    gamefind::find_install()
}

/// Scan a mod library and return the analysis as the JSON the UI consumes.
///
/// `mods_dir` defaults to the detected install's MODS folder, which is what the
/// user gets by pressing Rescan with nothing chosen. `game_root` pins which
/// install the vanilla comparison reads from, for the case where the folder
/// being scanned belongs to a different copy of the game than the detected one.
///
/// The work is blocking and runs for several seconds on a large library --
/// decompiling assets and extracting vanilla scenes both shell out -- so it
/// runs off the UI thread.
#[tauri::command]
async fn analyse_library(
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || run_analysis(mods_dir, game_root))
        .await
        .map_err(|err| format!("analysis task failed: {err}"))?
}

fn run_analysis(
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> Result<serde_json::Value, String> {
    let (root, vanilla_root) = library_roots(mods_dir, game_root)?;

    // The one place that always scans: pressing "Rescan library" has to mean
    // it. Everything else reuses what this leaves behind.
    let scan = scancache::refresh(&root)
        .map_err(|err| format!("could not read {}: {err}", root.display()))?;
    let (mods, stats, host) = (&scan.mods, scan.stats.clone(), &scan.host);
    if mods.is_empty() {
        return Err(format!("no mods found in {}", root.display()));
    }

    // Only the mods the game has switched on, matching what it will load.
    let mut active: Vec<_> = mods.iter().filter(|m| !m.disabled).cloned().collect();
    let disabled: Vec<String> = mods
        .iter()
        .filter(|m| m.disabled)
        .map(|m| m.name.clone())
        .collect();
    if active.is_empty() {
        return Err("every mod in this folder is switched off".into());
    }

    // A clash between two compiled assets can only be reported as "cannot be
    // compared" until they are decompiled, so do that before analysing rather
    // than telling the user to go and run MBINCompiler themselves.
    let mut stats = stats;
    let mut decompiler = Decompiler::locate(None, None);
    if let Some(d) = decompiler.as_mut() {
        stats.decompiled = decompile::enrich(&mut active, d);
    }

    let mut built = analyze::analyse(
        active.clone(),
        stats,
        vec![std::path::absolute(&root)
            .unwrap_or(root)
            .to_string_lossy()
            .into_owned()],
        None,
        WinnerRule::Last,
        false,
        Some(&host),
    );
    built.disabled = disabled;

    // The scene check needs both tools and the game's own archives. When any
    // of that is missing it reports *why* rather than returning quietly, so
    // the UI never shows an unchecked library as a clean one.
    let (findings, status) = drift::run(&active, vanilla_root.as_deref(), None, None);
    built.drift = findings;
    built.tools = status;

    // "One of these wins" is true but incomplete: two mods editing different
    // parts of one file can simply be combined. Answering that needs vanilla,
    // so it runs only when the same tools the scene check needs are present.
    if let (Some(d), Some(root)) = (decompiler.as_mut(), vanilla_root.as_deref()) {
        if let Some(mut src) = VanillaSource::locate(root, None) {
            merge::run(&mut built.conflicts, &active, d, &mut src);
        }
    }

    Ok(report::to_json(&built))
}

/// Combine the mods contesting one asset into a single merged mod.
///
/// Writes into its own folder beside the originals, which are left untouched
/// so the merge can be undone by deleting it. Refuses when the copies have
/// overlapping edits, or when any edit could not be placed.
///
/// `only` narrows the merge to a subset of the mods contesting the asset. The
/// UI sends it when the mods left out are better *cleaned* than merged: the
/// merge then stands in for exactly the mods named and the others stay in the
/// game, where cleaning them is the other half of the fix.
#[tauri::command]
async fn merge_conflict(
    app: tauri::AppHandle,
    target: String,
    only: Option<Vec<String>>,
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> Result<String, String> {
    let spots = places(mods_dir.clone(), game_root.clone())?;
    let book_path = loadout_file(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        run_merge(target, only, mods_dir, game_root, &spots, &book_path)
    })
    .await
    .map_err(|err| format!("merge task failed: {err}"))?
}

/// Build a merge, stage it, and let the loadout put it in the game.
///
/// # Why this is not just "write a folder into MODS"
///
/// It used to be, and that left the merge outside everything else this program
/// knows how to do. It could not be deactivated or deleted, it had no entry to
/// hang a version or an update check on, and -- the part that actually bit --
/// nothing held its *inputs* out of the game. All three loaded together and the
/// merge won only because its folder name sorted last, which is the very
/// failure the loadout's `replaces` was written to end.
///
/// So a merge now goes where every other build of ours goes: into `derived/`,
/// with a loadout entry naming the mods it stands in for. `reconcile` then takes
/// those inputs out of the game and puts the merge in, and deactivating or
/// deleting the merge gives them straight back.
fn run_merge(
    target: String,
    only: Option<Vec<String>>,
    mods_dir: Option<String>,
    game_root: Option<String>,
    spots: &pipeline::Places,
    book_path: &Path,
) -> Result<String, String> {
    let (root, vanilla_root) = library_roots(mods_dir, game_root)?;
    let vanilla_root = vanilla_root.ok_or("no game install found to merge against")?;

    let scan = scancache::get(&root)
        .map_err(|err| format!("could not read {}: {err}", root.display()))?;
    let (stats, host) = (scan.stats.clone(), &scan.host);
    let mut active = scan.active();

    let mut decompiler =
        Decompiler::locate(None, None).ok_or("MBINCompiler was not found")?;
    let mut source = VanillaSource::locate(&vanilla_root, None)
        .ok_or("hgpaktool was not found, or the game has no PCBANKS folder")?;
    decompile::enrich(&mut active, &mut decompiler);

    let mut built = analyze::analyse(
        active.clone(),
        stats,
        vec![root.to_string_lossy().into_owned()],
        None,
        WinnerRule::Last,
        false,
        Some(&host),
    );

    let at = built
        .conflicts
        .iter()
        .position(|c| c.target == target)
        .ok_or("that conflict is no longer present; rescan and try again")?;

    // Only the asset being merged needs judging. `merge::run` extracts and
    // decompiles a vanilla copy per target, so running it across the whole
    // report here did fifteen assets' work to answer one question -- and the
    // answer for the other fourteen was thrown away on the next line.
    merge::run(
        std::slice::from_mut(&mut built.conflicts[at]),
        &active,
        &mut decompiler,
        &mut source,
    );

    let mut conflict = built.conflicts.swap_remove(at);
    if let Some(only) = only {
        // Keep the analysis's own order and drop anything not asked for, so the
        // merge stands in for exactly the mods named and no others.
        let kept: Vec<String> = conflict
            .mods
            .iter()
            .filter(|name| only.iter().any(|want| want == *name))
            .cloned()
            .collect();
        if kept.len() < 2 {
            return Err(
                "fewer than two of those mods still ship this asset; rescan and try again".into(),
            );
        }
        conflict.mods = kept;
    }

    let done = pipeline::record_merge(
        &conflict,
        &active,
        spots,
        book_path,
        &mut decompiler,
        &mut source,
    )?;
    scancache::invalidate();

    if let Some(trouble) = done.changes.problems.first() {
        return Err(trouble.clone());
    }
    Ok(done.written)
}

/// The loadout entry responsible for the mod folder called `name`.
///
/// Not simply `book.get(name)`: one staged mod can deploy several top-level
/// folders, and the scan sees each of those as a mod in its own right. The
/// entry is the one that put that folder in the game, whatever it is called.
fn entry_deploying<'a>(
    book: &'a loadout::Loadout,
    name: &str,
) -> Option<&'a loadout::Entry> {
    book.get(name).or_else(|| {
        book.entries
            .iter()
            .find(|e| e.deployed.iter().any(|shown| shown == name))
    })
}

/// Everything a whole-file override could be reduced to, and what it cannot.
///
/// Read-only: it decompiles and compares but writes nothing, so the UI can
/// show the user what cleaning would cost them before they agree to it.
///
/// The already-cleaned entries are read off the loadout rather than the mods
/// folder, because a cleaned mod ships a patch where its override used to be --
/// so the scan sees nothing to clean and the mod would disappear from this list
/// entirely, taking its undo with it.
#[tauri::command]
async fn clean_preview(
    app: tauri::AppHandle,
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> Result<Vec<prune::Plan>, String> {
    let book = loadout_file(&app)
        .map(|path| loadout::Loadout::read(&path))
        .unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        let ours = merge_folders(&book);
        let (mods, mut decompiler, mut source) = clean_context(mods_dir, game_root, &ours)?;
        let mut plans = prune::plan(&mods, &mut decompiler, &mut source);
        for entry in book.entries.iter().filter(|e| e.variant == loadout::Variant::Cleaned) {
            let origin = entry.origin_path();
            let built = std::path::PathBuf::from(&entry.source);
            for name in &entry.deployed {
                plans.extend(prune::already_cleaned(&origin, &built, name));
            }
        }
        Ok(plans)
    })
    .await
    .map_err(|err| format!("clean preview failed: {err}"))?
}

/// What cleaning one mod did, or why it did nothing.
#[derive(serde::Serialize)]
struct Cleaned {
    owner: String,
    /// a line per asset reduced
    done: Vec<String>,
    /// why this one was left alone; `None` when it was cleaned
    failed: Option<String>,
}

/// Build a cleaned copy of each mod and switch the game over to all of them.
///
/// The mods are not edited. A cleaned *build* of each is written into
/// `derived/` and the loadout is pointed at it, exactly as mending does -- so
/// the copy its author shipped is still in staging and [`clean_undo`] is a
/// switch back rather than a restore. See [`prune::clean_into`] for why it is
/// worth the extra folder.
///
/// # Why this takes a list
///
/// The same reason [`set_mods_enabled`] does, twice over. The expensive work --
/// scanning the library, locating the tools, planning every override against
/// the game's own copy -- is *per library*, not per mod, so doing twenty mods
/// one command at a time repeats all of it twenty times. And every one of those
/// commands ends in its own [`loadout::reconcile`], which relinks the mods
/// folder and leaves it briefly inconsistent in between.
///
/// So the plan is made once, every build is written, and the game folder is
/// brought into line once at the end.
///
/// A mod that cannot be cleaned is reported against its own name and the rest
/// carry on: one refusal in twenty should not cost the other nineteen.
fn run_clean(
    owners: Vec<String>,
    mods_dir: Option<String>,
    game_root: Option<String>,
    spots: &pipeline::Places,
    book_path: &Path,
    edits_path: &Path,
) -> Result<Vec<Cleaned>, String> {
    let mut book = loadout::Loadout::read(book_path);
    let edits = edit::Book::read(edits_path);
    let ours = merge_folders(&book);
    let (mods, mut decompiler, mut source) = clean_context(mods_dir, game_root, &ours)?;
    let plans = prune::plan(&mods, &mut decompiler, &mut source);

    let mut out: Vec<Cleaned> = Vec::new();
    let mut built = 0usize;
    for owner in &owners {
        let mut refuse = |why: String| {
            out.push(Cleaned { owner: owner.clone(), done: Vec::new(), failed: Some(why) })
        };

        let Some(was) = entry_deploying(&book, owner).cloned() else {
            refuse(format!(
                "{owner} was not installed by this program, so there is no untouched copy of \
                 it to build a cleaned one from. Reinstall it here first."
            ));
            continue;
        };

        // Everything this entry put in the game, not just the folder that was
        // clicked: one staged mod can deploy several, and a build that cleaned
        // only one of them would be a different mod from the one installed.
        let mine: Vec<&prune::Plan> = plans
            .iter()
            .filter(|p| was.deployed.iter().any(|shown| shown == &p.owner))
            .collect();
        if mine.is_empty() {
            refuse(format!("{owner} has nothing that could be cleaned"));
            continue;
        }
        if !mine.iter().any(|p| p.can_clean()) {
            let why = mine
                .iter()
                .find_map(|p| p.refused.clone())
                .unwrap_or_else(|| "see the reason against each file".into());
            refuse(format!("nothing in {owner} could be cleaned: {why}"));
            continue;
        }

        // Always built from the mod as its author shipped it, never from
        // whatever build is deployed now -- cleaning an already-cleaned build
        // would find its own patches and reduce them to nothing.
        let dest = spots.derived.join(&was.owner);
        match prune::clean_into(&was.origin_path(), &dest, &mine) {
            Ok(mut done) => {
                let key = was.owner.clone();
                book.put(loadout::Entry {
                    source: dest.display().to_string(),
                    variant: loadout::Variant::Cleaned,
                    // Kept, so reconcile knows what to take away first: the
                    // author's build is deployed under these names right now.
                    deployed: was.deployed.clone(),
                    ..was
                });

                // The clean just rebuilt this mod from the copy its author
                // shipped, which is the one place the user's own values are
                // *not*. Without this the values would be silently reverted by
                // an unrelated verb -- exactly the failure the note on
                // `prune::clean_into` records about reconcile undoing cleans.
                if edits.count(&key) > 0 {
                    match rebuild_edited(&mut book, &edits, &key, spots, &mut decompiler) {
                        Ok(applied) => {
                            done.extend(applied.done);
                            done.extend(applied.lost);
                        }
                        // The clean itself stands; only the values did not go
                        // back on, and saying so is the whole point.
                        Err(why) => done.push(format!("your values were not re-applied: {why}")),
                    }
                }

                built += 1;
                out.push(Cleaned { owner: owner.clone(), done, failed: None });
            }
            Err(why) => refuse(why),
        }
    }

    if built == 0 {
        // Nothing was written, so there is nothing to reconcile and no reason
        // to rewrite the loadout. One mod asked for is one error to raise;
        // several is a list the caller can show against each name.
        return match out.len() {
            1 => Err(out.remove(0).failed.unwrap_or_else(|| "nothing was cleaned".into())),
            _ => Ok(out),
        };
    }

    // Once, at the end, for every build written above.
    let changes = loadout::reconcile(&mut book, &spots.mods_dir, false);
    book.write(book_path)?;
    scancache::invalidate();
    if let Some(trouble) = changes.problems.first() {
        return Err(trouble.clone());
    }
    Ok(out)
}

/// Clean one mod. See [`run_clean`].
#[tauri::command]
async fn clean_apply(
    app: tauri::AppHandle,
    owner: String,
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> Result<Vec<String>, String> {
    let spots = places(mods_dir.clone(), game_root.clone())?;
    let book_path = loadout_file(&app)?;
    let edits_path = edits_file(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let done = run_clean(vec![owner], mods_dir, game_root, &spots, &book_path, &edits_path)?;
        Ok(done.into_iter().flat_map(|c| c.done).collect())
    })
    .await
    .map_err(|err| format!("clean failed: {err}"))?
}

/// Clean several mods, reconciling the game folder once at the end.
#[tauri::command]
async fn clean_apply_many(
    app: tauri::AppHandle,
    owners: Vec<String>,
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> Result<Vec<Cleaned>, String> {
    if owners.is_empty() {
        return Err("no mods were chosen".into());
    }
    let spots = places(mods_dir.clone(), game_root.clone())?;
    let book_path = loadout_file(&app)?;
    let edits_path = edits_file(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        run_clean(owners, mods_dir, game_root, &spots, &book_path, &edits_path)
    })
    .await
    .map_err(|err| format!("clean failed: {err}"))?
}

/// Put a mod back to the build its author shipped.
///
/// The one operation behind both "put the originals back" after a clean and
/// "undo the repair" after a mend: both are the same switch, because both
/// variants are builds sitting beside an untouched staged copy.
/// Going back to the author's build forgets the values the user set, and does
/// not merely stop applying them.
///
/// The alternative was to keep them, unapplied, for a later change of mind.
/// That would leave `edits.json` holding values nothing carries -- and
/// [`run_clean`] re-applies whatever it finds there, so the next clean of that
/// mod would put them silently back. A value that reappears without being asked
/// for is worse than one the user has to type again.
fn forget_edits(edits_path: &Path, owner: &str) {
    let mut edits = edit::Book::read(edits_path);
    if edits.forget(owner) {
        let _ = edits.write(edits_path);
    }
}

fn back_to_original(
    book_path: &Path,
    edits_path: &Path,
    owner: &str,
    mods_dir: &Path,
) -> Result<loadout::Changes, String> {
    let mut book = loadout::Loadout::read(book_path);
    let was = entry_deploying(&book, owner)
        .cloned()
        .ok_or_else(|| format!("{owner} is not in the mod list"))?;
    // `edited` counts as not-original. Without it, a mod carrying hand-set
    // values on an otherwise untouched build would be told it was already the
    // author's copy and left with the user's values in the game.
    if was.variant == loadout::Variant::Original && !was.edited {
        return Err(format!("{owner} is already the build its author shipped"));
    }
    forget_edits(edits_path, &was.owner);
    book.put(loadout::Entry {
        source: was.origin_path().display().to_string(),
        variant: loadout::Variant::Original,
        edited: false,
        deployed: was.deployed.clone(),
        ..was.clone()
    });
    let changes = loadout::reconcile(&mut book, mods_dir, false);
    book.write(book_path)?;
    scancache::invalidate();
    Ok(changes)
}

/// Put back the mod as its author shipped it, undoing a clean.
#[tauri::command]
async fn clean_undo(
    app: tauri::AppHandle,
    owner: String,
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> Result<usize, String> {
    let _ = game_root;
    let root = mods_root(mods_dir)?;
    let book_path = loadout_file(&app)?;
    let edits_path = edits_file(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let changes = back_to_original(&book_path, &edits_path, &owner, &root)?;
        if let Some(trouble) = changes.problems.first() {
            return Err(trouble.clone());
        }
        Ok(changes.deployed.len())
    })
    .await
    .map_err(|err| format!("undo failed: {err}"))?
}


// ---------------------------------------------------------------------------
// Changing a value the mod sets
//
// The other build verbs decide *which* copy of an asset the game reads. This
// one changes what is in it, and it is the only verb whose result the user
// typed. See `engine::edit` for the shape of the record and why the build sits
// in a folder of its own.
// ---------------------------------------------------------------------------

/// Where the edited copy of a mod is built.
fn edited_build(spots: &pipeline::Places, owner: &str) -> PathBuf {
    spots.derived.join(format!("{owner}{}", edit::SUFFIX))
}

/// The build an edited copy is made from: whatever the mod would run without
/// the user's values.
///
/// For an untouched mod that is the copy its author shipped. For a cleaned or
/// mended one it is that build -- named after the entry rather than read off
/// `source`, because `source` already points at the edited copy whenever one
/// exists, and building an edited copy from an edited copy would apply the
/// overlay to itself and lose the author's values for ever.
fn edit_base(spots: &pipeline::Places, entry: &loadout::Entry) -> PathBuf {
    match entry.variant {
        loadout::Variant::Original => entry.origin_path(),
        _ => spots.derived.join(&entry.owner),
    }
}

/// Stop applying the user's values, without forgetting them.
///
/// Points the entry back at the build underneath and takes the edited copy
/// away. The values stay in `edits.json` only when the caller leaves them
/// there; every caller here that means "undo" clears them first, because a
/// recorded value that nothing applies would come back by surprise the next
/// time the mod was cleaned.
fn drop_edited_build(
    book: &mut loadout::Loadout,
    spots: &pipeline::Places,
    entry: &loadout::Entry,
) {
    let built = edited_build(spots, &entry.owner);
    book.put(loadout::Entry {
        source: edit_base(spots, entry).display().to_string(),
        edited: false,
        deployed: entry.deployed.clone(),
        ..entry.clone()
    });
    // Only ever our own folder in `derived/`, by construction: the name is
    // built from the suffix this module owns.
    if built.starts_with(&spots.derived) {
        let _ = std::fs::remove_dir_all(&built);
    }
}

/// Rebuild the edited copy of one mod from the values recorded against it.
///
/// Called whenever either half of the recipe moves: when the user changes a
/// value, and when the build underneath is replaced -- cleaning rebuilds from
/// the author's copy, so an edit written into the old cleaned build would be
/// thrown away silently.
///
/// Writes the loadout entry but does not reconcile or save; the caller does
/// both, once, because it usually has other work to land in the same pass.
fn rebuild_edited(
    book: &mut loadout::Loadout,
    edits: &edit::Book,
    owner: &str,
    spots: &pipeline::Places,
    decompiler: &mut Decompiler,
) -> Result<edit::Applied, String> {
    let entry = entry_deploying(book, owner)
        .cloned()
        .ok_or_else(|| format!("{owner} is not in the mod list"))?;
    if entry.variant == loadout::Variant::Merged {
        return Err(format!(
            "{owner} is a build this program made out of several mods, not a mod with an \
             author's copy behind it. Change the value on one of the mods that went into it \
             and the merge will be rebuilt from them."
        ));
    }

    let Some(held) = edits.of(&entry.owner).filter(|files| !files.is_empty()) else {
        // Nothing is set any more: the mod runs the build underneath.
        drop_edited_build(book, spots, &entry);
        return Ok(edit::Applied::default());
    };

    let base = edit_base(spots, &entry);
    if !base.is_dir() {
        return Err(format!(
            "the build {owner} would be edited from is not at {}, so there is nothing to \
             copy. Reinstall it here, or put it back to the build its author shipped first.",
            base.display()
        ));
    }
    let dest = edited_build(spots, &entry.owner);
    if base == dest {
        return Err(format!("{owner} cannot be edited from its own edited copy"));
    }

    if dest.exists() {
        std::fs::remove_dir_all(&dest)
            .map_err(|e| format!("could not clear {}: {e}", dest.display()))?;
    }
    edit::copy_tree(&base, &dest)?;

    let applied = edit::apply_into(&dest, held, decompiler)?;
    if applied.done.is_empty() {
        // Every value was lost -- the mod has moved on past all of them. The
        // copy is identical to the build underneath, so switching to it would
        // claim an edit that is not there. Go back, and say why.
        drop_edited_build(book, spots, &entry);
        return Err(if applied.lost.is_empty() {
            format!("nothing in {owner} was changed")
        } else {
            applied.lost.join("\n")
        });
    }

    book.put(loadout::Entry {
        source: dest.display().to_string(),
        edited: true,
        // Kept, so reconcile knows what to take away first: the unedited build
        // is deployed under these names right now.
        deployed: entry.deployed.clone(),
        ..entry
    });
    Ok(applied)
}

/// Every property one mod changes, with the game's value and yours beside it.
///
/// Read-only. Deliberately per mod and on demand, never on the start-up path:
/// this looks at *every* asset the mod ships, including the sparse patches
/// `prune::plan` skips, and one mod's worth of that is a second or two against
/// a warm cache.
#[tauri::command]
async fn edit_survey(
    app: tauri::AppHandle,
    owner: String,
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> Result<edit::Survey, String> {
    let book = loadout::Loadout::read(&loadout_file(&app)?);
    let edits = edit::Book::read(&edits_file(&app)?);
    let (_, vanilla_root) = library_roots(mods_dir, game_root)?;
    let vanilla_root = vanilla_root.ok_or("no game install found to compare against")?;

    let entry = entry_deploying(&book, &owner).cloned().ok_or_else(|| {
        format!(
            "{owner} was not installed by this program, so there is no untouched copy of it \
             to read the author's values from. Reinstall it here first."
        )
    })?;
    if entry.variant == loadout::Variant::Merged {
        return Err(format!(
            "{owner} is a build this program made out of several mods. Open one of the mods \
             that went into it to change a value."
        ));
    }

    tauri::async_runtime::spawn_blocking(move || {
        let mut decompiler = Decompiler::locate(None, None).ok_or("MBINCompiler was not found")?;
        let mut source = VanillaSource::locate(&vanilla_root, None)
            .ok_or("hgpaktool was not found, or the game has no PCBANKS folder")?;
        let origin = entry.origin_path();
        // The folders this mod puts in the game, which is what the staged copy
        // holds at its top level. Falling back to the entry's own name covers
        // an entry recorded before anything was deployed from it.
        let members = if entry.deployed.is_empty() {
            vec![entry.owner.clone()]
        } else {
            entry.deployed.clone()
        };
        Ok(edit::survey(
            &entry.owner,
            &origin,
            &members,
            &mut decompiler,
            &mut source,
            edits.of(&entry.owner),
        ))
    })
    .await
    .map_err(|err| format!("reading the mod failed: {err}"))?
}

/// One value the user wants set, as the editor sends it.
#[derive(serde::Deserialize)]
struct Wanted {
    /// the file, as `edit::Asset::file` gave it
    file: String,
    path: String,
    /// the new value, or `None` to go back to the author's
    value: Option<String>,
    /// the author's value the box was showing, which is what going back means
    /// and what a later mod update is compared against
    author: String,
}

/// Set values on one mod, rebuild it, and switch the game over.
///
/// Takes every change at once rather than one per keystroke: a rebuild copies
/// the mod, may run MBINCompiler over an asset, and relinks the mods folder.
#[tauri::command]
async fn edit_apply(
    app: tauri::AppHandle,
    owner: String,
    changes: Vec<Wanted>,
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> Result<edit::Applied, String> {
    if changes.is_empty() {
        return Err("nothing was changed".into());
    }
    let spots = places(mods_dir, game_root)?;
    let book_path = loadout_file(&app)?;
    let edits_path = edits_file(&app)?;

    tauri::async_runtime::spawn_blocking(move || {
        let mut book = loadout::Loadout::read(&book_path);
        let mut edits = edit::Book::read(&edits_path);
        // The entry's owner, not the folder that was clicked: one staged mod
        // can deploy several folders and the values belong to the mod.
        let key = entry_deploying(&book, &owner)
            .map(|e| e.owner.clone())
            .ok_or_else(|| format!("{owner} is not in the mod list"))?;

        for change in &changes {
            match &change.value {
                Some(value) => {
                    // Refused here, where the value came from, rather than half
                    // way through writing the build. The sort is taken from the
                    // author's value: see `edit::Sort`.
                    edit::check(edit::sort_of(&change.author), value)
                        .map_err(|why| format!("{}: {why}", change.path))?;
                    // A value typed back to what the author ships is not an
                    // edit. Recording it as one would put the mod on an edited
                    // build that changes nothing.
                    if engine::exml::values_equal(Some(value), Some(&change.author)) {
                        edits.clear(&key, &change.file, &change.path);
                    } else {
                        edits.set(
                            &key,
                            &change.file,
                            &change.path,
                            edit::Change {
                                value: value.trim().to_string(),
                                was: change.author.clone(),
                            },
                        );
                    }
                }
                None => {
                    edits.clear(&key, &change.file, &change.path);
                }
            }
        }

        let mut decompiler = Decompiler::locate(None, None).ok_or("MBINCompiler was not found")?;
        let applied = rebuild_edited(&mut book, &edits, &key, &spots, &mut decompiler)?;

        // Written only once the build it describes exists. The other way round,
        // a failed build would leave a record of values nothing carries, and
        // the next clean would apply them without the user asking.
        edits.write(&edits_path)?;
        let changes = loadout::reconcile(&mut book, &spots.mods_dir, false);
        book.write(&book_path)?;
        scancache::invalidate();
        if let Some(trouble) = changes.problems.first() {
            return Err(trouble.clone());
        }
        Ok(applied)
    })
    .await
    .map_err(|err| format!("changing the values failed: {err}"))?
}

/// Forget every value set on one mod and put its own values back.
///
/// Keeps whichever build was underneath -- a cleaned mod stays cleaned. Going
/// all the way back to the author's copy is [`clean_undo`], which this does not
/// duplicate.
#[tauri::command]
async fn edit_undo(
    app: tauri::AppHandle,
    owner: String,
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> Result<usize, String> {
    let spots = places(mods_dir, game_root)?;
    let book_path = loadout_file(&app)?;
    let edits_path = edits_file(&app)?;

    tauri::async_runtime::spawn_blocking(move || {
        let mut book = loadout::Loadout::read(&book_path);
        let mut edits = edit::Book::read(&edits_path);
        let entry = entry_deploying(&book, &owner)
            .cloned()
            .ok_or_else(|| format!("{owner} is not in the mod list"))?;
        let held = edits.count(&entry.owner);
        if held == 0 && !entry.edited {
            return Err(format!("no values have been set on {owner}"));
        }
        edits.forget(&entry.owner);
        drop_edited_build(&mut book, &spots, &entry);

        edits.write(&edits_path)?;
        let changes = loadout::reconcile(&mut book, &spots.mods_dir, false);
        book.write(&book_path)?;
        scancache::invalidate();
        if let Some(trouble) = changes.problems.first() {
            return Err(trouble.clone());
        }
        Ok(held)
    })
    .await
    .map_err(|err| format!("undo failed: {err}"))?
}

/// Forget held values the mod can no longer carry.
///
/// An update can take a file away or move a property, and the value set on it
/// then stops being applicable *and* stops being visible -- which would leave it
/// in `edits.json` for ever, with a `lost` line on every rebuild that nobody is
/// looking at. `edit::Survey::stale` names them; this is the answer to it.
#[tauri::command]
async fn edit_forget_values(
    app: tauri::AppHandle,
    owner: String,
    values: Vec<edit::Stale>,
) -> Result<usize, String> {
    let book = loadout::Loadout::read(&loadout_file(&app)?);
    let edits_path = edits_file(&app)?;
    let key = entry_deploying(&book, &owner)
        .map(|e| e.owner.clone())
        .unwrap_or(owner);

    let mut edits = edit::Book::read(&edits_path);
    let mut gone = 0usize;
    for value in &values {
        if edits.clear(&key, &value.file, &value.path).is_some() {
            gone += 1;
        }
    }
    if gone > 0 {
        edits.write(&edits_path)?;
    }
    // No rebuild. These are values the mod cannot carry, so the build already
    // does not carry them and dropping the record changes nothing the game
    // reads -- which is why this one verb does not touch the mods folder.
    Ok(gone)
}

/// The Nexus personal API key, which lives in the settings file.
///
/// It used to have a file of its own, on the reasoning that a credential must
/// not end up in something a user would paste into a bug report. Nothing here
/// exports settings and every path in them is specific to one machine, so that
/// was guarding against a thing that does not happen -- while costing a second
/// place to look and, worse, a second copy of the key to revoke. What *is*
/// shared is a mod list, and that has its own format in [`engine::collection`]
/// which carries no settings at all.
///
/// [`settings::Settings::read`] folds in the old `nexus.key` the first time it
/// sees one, so nobody has to paste their key again.
fn stored_key(app: &tauri::AppHandle) -> Option<String> {
    settings::Settings::read(&settings_file(app).ok()?).nexus_key
}

/// Save a key after checking that Nexus accepts it.
///
/// Verifying first means a typo is rejected at the point the user can still
/// see what they pasted, rather than looking like "no updates found" later.
#[tauri::command]
async fn nexus_set_key(app: tauri::AppHandle, key: String) -> Result<nexus::Account, String> {
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err("no key given".into());
    }
    let path = settings_file(&app)?;
    let (account, key) =
        tauri::async_runtime::spawn_blocking(move || nexus::Api::new(&key).whoami().map(|a| (a, key)))
            .await
            .map_err(|err| err.to_string())??;

    // Read back rather than writing a held copy: connecting can take a moment
    // against the network, and anything else that saved settings meanwhile
    // must not be undone by it.
    let mut stored = settings::Settings::read(&path);
    stored.nexus_key = Some(key);
    stored.write(&path)?;
    Ok(account)
}

/// Who the stored key belongs to, or `None` when there is no key yet.
#[tauri::command]
async fn nexus_account(app: tauri::AppHandle) -> Result<Option<nexus::Account>, String> {
    let Some(key) = stored_key(&app) else {
        return Ok(None);
    };
    tauri::async_runtime::spawn_blocking(move || nexus::Api::new(key).whoami().map(Some))
        .await
        .map_err(|err| err.to_string())?
}

#[tauri::command]
fn nexus_forget_key(app: tauri::AppHandle) -> Result<(), String> {
    let path = settings_file(&app)?;
    let mut stored = settings::Settings::read(&path);
    stored.nexus_key = None;
    stored.write(&path)
}

/// What Nexus says about every installed mod.
#[derive(serde::Serialize)]
struct UpdateReport {
    checks: Vec<nexus::Check>,
    budget: nexus::Budget,
}

/// How far the update check has got, so sixty requests can show movement.
#[derive(Clone, serde::Serialize)]
struct CheckProgress {
    done: usize,
    total: usize,
}

/// Ask Nexus what is current, for the whole library or for a few mods.
///
/// `owners` narrows it to those mod folders, which costs one request per Nexus
/// page rather than sixty. That is what makes it reasonable to re-ask about a
/// single mod whenever our own record of it changes — a mod installed a moment
/// ago has no answer at all, and one whose archive we could not read when we
/// last asked has a *wrong* one that no amount of waiting will correct, because
/// "no Nexus archive is recorded for this folder" is a statement about this
/// program's records rather than about Nexus.
#[tauri::command]
async fn check_updates(
    app: tauri::AppHandle,
    mods_dir: Option<String>,
    owners: Option<Vec<String>>,
) -> Result<UpdateReport, String> {
    let key = stored_key(&app).ok_or("no Nexus API key has been set")?;
    let staged = archive_index(&app, mods_dir.clone(), None);
    let book = loadout_file(&app)
        .map(|path| loadout::Loadout::read(&path))
        .unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        let root = mods_root(mods_dir)?;
        let scan = scancache::get(&root)
            .map_err(|err| format!("could not read {}: {err}", root.display()))?;
        let mut active = scan.active();

        // A merge is ours. It has no Nexus page and never will have one, so
        // asking about it can only produce "no Nexus archive is recorded for
        // this folder" — a permanent non-answer sitting in a list of answers,
        // and, now that a non-answer is no longer remembered, one that `fill`
        // would re-ask about after every single scan.
        let ours: std::collections::BTreeSet<&str> = book
            .entries
            .iter()
            .filter(|e| e.variant == loadout::Variant::Merged)
            .flat_map(|e| e.deployed.iter().map(String::as_str))
            .collect();
        active.retain(|m| !ours.contains(m.name.as_str()));

        // And the mods a merge stands in for are still installed — they are
        // held out of the game, so no scan can see them, and until now they
        // were the one part of the library nobody ever asked Nexus about.
        // They are also the part where an update matters *most*: a merge has
        // its inputs' edits baked in at the version it was built from, so a new
        // version of one of them reaches the game only once the merge is
        // rebuilt, and nothing would have said so. Their archives are already
        // known, because `archive_index` reads the loadout rather than the game
        // folder; `root` is the staged copy, which is where `corroborate` finds
        // the folder name and the AMUMSS script it reads.
        let deployed: std::collections::BTreeSet<String> =
            active.iter().map(|m| m.name.clone()).collect();
        for held in book.superseded() {
            if deployed.contains(&held) {
                continue;
            }
            let Some(entry) = book.get(&held) else { continue };
            active.push(engine::model::Mod {
                name: held.clone(),
                root: entry.origin_path().join(&held).display().to_string(),
                ..Default::default()
            });
        }

        if let Some(wanted) = owners {
            active.retain(|m| wanted.iter().any(|name| name == &m.name));
        }
        // `check_all` has always reported its progress and this dropped it on
        // the floor, so a check of sixty mod pages showed a spinner and no sign
        // of whether it was moving.
        let (checks, budget) =
            nexus::check_all(&nexus::Api::new(key), &active, &staged, |done, total| {
                let _ = app.emit("update-progress", CheckProgress { done, total });
            });
        Ok(UpdateReport { checks, budget })
    })
    .await
    .map_err(|err| err.to_string())?
}

/// The mods folder to work in, and the game install to compare against.
///
/// Both are resolved the same way everywhere: the argument, then the user's
/// setting, then detection. This used to exist as four near-identical copies --
/// in the analysis, the merge, the clean context and [`places`] -- which is
/// three chances for one of them to settle on a different folder than the rest
/// and report on a library the buttons then act on somewhere else.
///
/// The game root comes back as `None` rather than an error when nothing is
/// found: scanning a mods folder is still worth doing without the game's own
/// archives. Callers that need a vanilla baseline say so at their own point of
/// use, where they can explain what it is for.
fn library_roots(
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> Result<(PathBuf, Option<PathBuf>), String> {
    let (mods_dir, game_root) = with_settings(mods_dir, game_root);
    let detected = gamefind::find_install();
    let root = match mods_dir {
        Some(path) => PathBuf::from(path),
        None => match &detected {
            Some(found) if found.has_mods_dir => PathBuf::from(&found.mods_dir),
            Some(_) => return Err("the game was found but has no MODS folder".into()),
            None => {
                return Err(
                    "no No Man's Sky install found -- set the mods folder in Settings if the \
                     game is somewhere unusual"
                        .into(),
                )
            }
        },
    };
    let game = game_root
        .map(PathBuf::from)
        .or_else(|| detected.as_ref().map(|i| PathBuf::from(&i.root)));
    Ok((root, game))
}

/// The mod folder to work in: the argument, else the setting, else detection.
fn mods_root(mods_dir: Option<String>) -> Result<PathBuf, String> {
    library_roots(mods_dir, None).map(|(root, _)| root)
}

/// The folder holding everything this program knows: settings, the loadout,
/// resolved mod names, session logs and save copies.
///
/// Deliberately *not* `app_data_dir()`. Tauri builds that one out of the bundle
/// identifier, so it landed at `%APPDATA%\com.nsbro.anomaly` -- a reverse-DNS
/// string is the right shape for an identifier and the wrong one for a folder a
/// person may have to open, read, back up or hand to a bug report. This is a
/// name, and the game it belongs to is in it, because `%APPDATA%\Anomaly` says
/// nothing about which anomaly.
///
/// Everything here is about *this machine's* view of things -- which mods are
/// deployed, who we are at Nexus, what the game said last run -- so none of it
/// belongs in the game folder, where a mod backup would sweep it up.
pub fn app_data(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path()
        .data_dir()
        .map(|dir| dir.join(APP_DATA_DIR))
        .map_err(|err| format!("no place to keep this program's own files: {err}"))
}

/// The one place the app-data folder is spelled. See [`app_data`].
pub const APP_DATA_DIR: &str = "NMS Anomaly";

/// Where the resolved Nexus mod names are kept.
fn names_file(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app_data(app).map(|dir| dir.join("nexus-names.json"))
}

/// Mod folder -> the archive it came from, for the whole library.
///
/// The archive name is the only record of a mod's Nexus id, its version and its
/// title, and there are three places it can be, which is why this exists rather
/// than one field being read:
///
/// 1. **Our own loadout**, which records the staged folder each deployed mod
///    came from. The authority for anything this program installed or adopted,
///    and the only one that knows about a mod that is switched *off* -- such a
///    mod has no files in the game at all.
/// 2. **The staging tree's shape**, one folder per archive holding the folders
///    it deploys. Covers a staged mod the loadout has not recorded.
/// 3. **The downloads folder**, matched by name, for a mod staged under a folder
///    that does not resemble its archive at all.
/// 4. **Vortex's manifest**, read per-mod by `engine::hostenv` into
///    `Mod.archive` and preferred over all of them by `library::archive_of`,
///    because a manager that is still deploying is the authority on what it
///    deployed.
///
/// # The trap this exists to avoid
///
/// A mod that this program has **cleaned, mended or merged** no longer points at
/// the folder its author shipped: `source` moves to the derived build, which is
/// our own output and carries no Nexus name. Reading `source` alone therefore
/// costs a fixed mod its page, its version *and* its update checking -- silently,
/// and only for the mods the user has fixed. `origin` is kept for exactly this,
/// so it is tried first and `source` is the fallback rather than the other way
/// round.
///
/// Empty is a fine answer, not an error: a library nothing has staged still has
/// its folder names.
fn archive_index(
    app: &tauri::AppHandle,
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> library::Staged {
    let mut out = library::Staged::new();
    let spots = places(mods_dir, game_root).ok();

    // Downloads, for the mods whose staged folder name says nothing.
    let by_slug = spots
        .as_ref()
        .map(|s| library::archives_by_slug(&s.archives))
        .unwrap_or_default();

    if let Ok(path) = loadout_file(app) {
        for entry in loadout::Loadout::read(&path).entries {
            // The download itself, then the folder the author's build is staged
            // under, then whatever is deployed now -- which for a fixed mod is a
            // build of ours and names nobody.
            let named = entry
                .archive
                .as_deref()
                .map(library::archive_stem)
                .map(str::to_string)
                .into_iter()
                .chain(
                    entry
                        .origin
                        .as_deref()
                        .map(|o| library::archive_stem(o).to_string()),
                )
                .chain(std::iter::once(
                    library::archive_stem(&entry.source).to_string(),
                ))
                .find(|name| engine::nexusname::parse(name).is_some())
                .or_else(|| {
                    library::archive_for_slug(
                        &entry.owner,
                        entry.origin.as_deref().map(library::archive_stem),
                        &by_slug,
                    )
                });
            if let Some(name) = named {
                out.insert(entry.owner.clone(), name);
            }
        }
    }

    if let Some(spots) = spots.as_ref() {
        for (owner, archive) in library::staged_archives(&spots.staging) {
            out.entry(owner).or_insert(archive);
        }
    }
    out
}

/// The remembered names, or an empty set when none have been resolved yet.
///
/// Never fails: a library that cannot read its name cache still has folder
/// names, and showing those for one run is not worth refusing to list mods.
fn stored_names(app: &tauri::AppHandle) -> Names {
    names_file(app).map(|path| Names::load(&path)).unwrap_or_default()
}

/// Everything installed, with whatever we know about where it came from.
#[tauri::command]
async fn library_list(
    app: tauri::AppHandle,
    mods_dir: Option<String>,
) -> Result<Vec<library::Identity>, String> {
    let names = stored_names(&app);
    let staged = archive_index(&app, mods_dir.clone(), None);
    tauri::async_runtime::spawn_blocking(move || {
        let root = mods_root(mods_dir)?;
        let scan = scancache::get(&root)
            .map_err(|err| format!("could not read {}: {err}", root.display()))?;
        Ok(library::identities(&root, &scan.mods, &names, &staged))
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Folder -> the name to show for it, for every mod in the library.
///
/// Offline and instant: this only reads the archive names already on disk and
/// whatever [`resolve_names`] has resolved before. The UI asks for this on
/// start-up so no screen ever has to render a folder name while waiting for
/// the network.
#[tauri::command]
async fn mod_names(
    app: tauri::AppHandle,
    mods_dir: Option<String>,
) -> Result<library::Staged, String> {
    let names = stored_names(&app);
    let staged = archive_index(&app, mods_dir.clone(), None);
    // Switched-off mods included. They have no files in the game, so no scan can
    // see them -- and they are exactly the ones a person has to find in the list
    // in order to switch them back on.
    let book = loadout_file(&app)
        .map(|path| loadout::Loadout::read(&path))
        .unwrap_or_default();
    tauri::async_runtime::spawn_blocking(move || {
        let root = mods_root(mods_dir)?;
        let scan = scancache::get(&root)
            .map_err(|err| format!("could not read {}: {err}", root.display()))?;

        let mut owners: Vec<(String, Option<String>)> = scan
            .mods
            .iter()
            .map(|m| {
                (
                    m.name.clone(),
                    library::archive_of(m, &staged).map(str::to_string),
                )
            })
            .collect();
        let seen: std::collections::BTreeSet<&str> =
            scan.mods.iter().map(|m| m.name.as_str()).collect();
        for entry in &book.entries {
            if !seen.contains(entry.owner.as_str()) {
                owners.push((
                    entry.owner.clone(),
                    Some(library::archive_stem(&entry.source).to_string()),
                ));
            }
        }
        let mut shown = library::names_for(owners, &names);
        // A merge is named for what it stands in for. Done after the rest, so
        // the parents' own names are already settled and the merge reads in the
        // same words the list uses for them.
        for entry in &book.entries {
            if entry.variant == loadout::Variant::Merged {
                let label = library::merged_label(&entry.replaces, &shown);
                shown.insert(entry.owner.clone(), label);
            }
        }
        Ok(shown)
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Ask Nexus for the real title of every mod whose page we have never read.
///
/// One request per *page*, and only for pages missing from the cache -- so the
/// first run on a sixty-mod library spends sixty of two thousand an hour and
/// every run after it spends none. Returns the same map as [`mod_names`], so
/// the caller can simply replace what it is holding.
///
/// A page that cannot be read is skipped rather than failing the batch: one
/// hidden mod should not cost the other fifty-nine their names. The whole
/// command errors only when there is no key to ask with, which is a thing the
/// user can act on.
#[tauri::command]
async fn resolve_names(
    app: tauri::AppHandle,
    mods_dir: Option<String>,
) -> Result<library::Staged, String> {
    let key = stored_key(&app).ok_or("no Nexus API key has been set")?;
    let path = names_file(&app)?;
    let staged = archive_index(&app, mods_dir.clone(), None);
    tauri::async_runtime::spawn_blocking(move || {
        let root = mods_root(mods_dir)?;
        let scan = scancache::get(&root)
            .map_err(|err| format!("could not read {}: {err}", root.display()))?;
        let mut names = Names::load(&path);

        let wanted = names.missing(library::mod_ids(&scan.mods, &staged));
        if !wanted.is_empty() {
            let api = nexus::Api::new(key);
            for mod_id in wanted {
                if let Ok((page, _)) = api.page(mod_id) {
                    if let Some(name) = page.name.as_deref() {
                        names.put(mod_id, name);
                    }
                }
            }
            // Written even on a partial batch: the names we did get should not
            // be asked for again just because a later one failed.
            let _ = names.save(&path);
        }

        Ok(library::display_names(&scan.mods, &names, &staged))
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Throw the resolved names away, so the next resolve asks Nexus again.
#[tauri::command]
async fn forget_names(app: tauri::AppHandle) -> Result<(), String> {
    let path = names_file(&app)?;
    let mut names = Names::load(&path);
    names.forget();
    names.save(&path).map_err(|err| format!("could not clear the mod names: {err}"))
}

/// One mod's Nexus page, for showing inside the app.
#[tauri::command]
async fn nexus_mod(app: tauri::AppHandle, mod_id: u64) -> Result<nexus::ModPage, String> {
    let key = stored_key(&app).ok_or("no Nexus API key has been set")?;
    tauri::async_runtime::spawn_blocking(move || {
        nexus::Api::new(key).page(mod_id).map(|(page, _)| page)
    })
    .await
    .map_err(|err| err.to_string())?
}

/// The three durable layers, and where the loadout is recorded.
fn places(mods_dir: Option<String>, game_root: Option<String>) -> Result<pipeline::Places, String> {
    let chosen = settings_now();
    let (root, found) = library_roots(mods_dir, game_root)?;
    let game = found
        // Falling back to the mods folder's grandparent keeps staging on the
        // game's volume even when detection failed, which is what the
        // hardlink needs.
        .or_else(|| root.parent().and_then(|p| p.parent()).map(Path::to_path_buf))
        .ok_or("no game install found")?;

    let mut spots = pipeline::Places::beside(&game, &root);
    // A chosen staging or archive folder replaces the derived one. Kept to
    // these two fields so that choosing where downloads live cannot move the
    // mods folder out from under the game.
    if let Some(path) = chosen.staging_dir {
        spots.staging = PathBuf::from(path);
    }
    if let Some(path) = chosen.archives_dir {
        spots.archives = PathBuf::from(path);
    }
    Ok(spots)
}

fn loadout_file(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app_data(app).map(|dir| dir.join("loadout.json"))
}

/// Where the values the user set by hand are kept.
///
/// A file of its own beside the loadout, not a field inside it. The loadout is
/// one line per mod and is read on every start-up and rewritten on every
/// switch; the edits are a few hundred property paths that only the editor and
/// a rebuild ever look at. Folding them in would put the larger, rarely-read
/// half of the record on the hot path — the opposite of the consolidation that
/// absorbed `nexus.key` and `presets.json` into settings, and for the same
/// reason: those were read whenever settings were, and these are not.
fn edits_file(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app_data(app).map(|dir| dir.join("edits.json"))
}

fn settings_file(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app_data(app).map(|dir| dir.join("settings.json"))
}

/// Where the settings file is, recorded once at startup.
///
/// Most commands resolve paths without an `AppHandle` in reach -- they run on
/// a blocking thread, or are plain helpers like [`mods_root`] and [`places`].
/// Threading a handle through all of them just to find one file would touch
/// every command for no gain, so the location is noted once while the app is
/// being set up and read from wherever it is needed after that.
static SETTINGS_PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// What the user has chosen, or the defaults before they have chosen anything.
pub fn settings_now() -> settings::Settings {
    SETTINGS_PATH
        .get()
        .map(|path| settings::Settings::read(path))
        .unwrap_or_default()
}

/// Let the user's chosen paths stand in wherever a command was not given one.
///
/// An explicit argument still wins, then the setting, then detection. That
/// ordering is what makes a manually set mod folder take effect across the
/// whole program while leaving one-off overrides working.
fn with_settings(
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> (Option<String>, Option<String>) {
    let chosen = settings_now();
    (mods_dir.or(chosen.mods_dir), game_root.or(chosen.game_root))
}

/// What the user has chosen, with the API key left out.
///
/// The screen has no use for the key -- connecting and disconnecting go
/// through their own commands, which report the account instead -- so it is
/// not handed out. [`settings_save`] puts it back.
#[tauri::command]
fn settings_read(app: tauri::AppHandle) -> Result<settings::Settings, String> {
    Ok(settings::Settings::read(&settings_file(&app)?).without_secrets())
}

/// Every path the program will use, with overrides applied and gaps named.
#[tauri::command]
fn settings_resolve() -> Result<settings::Resolved, String> {
    Ok(settings::resolve(&settings_now()))
}

/// Save settings and report what they resolve to, so the screen can show the
/// consequence of a change at the moment it is made rather than at next use.
///
/// Only the fields the form owns are taken. The key and the saved mod lists
/// are read back off disk, because the form was never given them and writing
/// back what it holds would disconnect the account and delete every preset as
/// a side effect of ticking a checkbox.
#[tauri::command]
fn settings_save(
    app: tauri::AppHandle,
    settings: settings::Settings,
) -> Result<settings::Resolved, String> {
    let path = settings_file(&app)?;
    let stored = settings::Settings::read(&path);
    settings.keeping(&stored).write(&path)?;
    Ok(settings::resolve(&settings::Settings::read(&path)))
}

/// Start the game, using the configured or detected executable.
#[tauri::command]
fn launch_game() -> Result<String, String> {
    let found = settings::resolve(&settings_now());
    let exe = found
        .game_exe
        .as_path()
        .ok_or("no game program found -- set it in Settings")?;
    if !exe.is_file() {
        return Err(format!("{} is not there", exe.display()));
    }
    std::process::Command::new(&exe)
        // From the game's own folder, which is where it expects to be started.
        .current_dir(exe.parent().unwrap_or(&exe))
        .spawn()
        .map_err(|err| format!("could not start the game: {err}"))?;
    Ok(exe.display().to_string())
}

// ---------------------------------------------------------------------------
// Recording what the game says while it runs
// ---------------------------------------------------------------------------

/// The folder the hook belongs in, for the install commands.
fn hook_home() -> Option<PathBuf> {
    watch::bin_dir(&settings::resolve(&settings_now()))
}

/// What is installed in the game right now, and what can be done about it.
#[tauri::command]
fn hook_state() -> hook::State {
    hook::state(hook_home().as_deref())
}

/// Put the recorder's DLL beside the game, so the next run is recorded.
///
/// `force` is the answer to a question the screen has already put to the user:
/// another tool owns the same slot, replace it? The displaced file is kept and
/// [`hook_uninstall`] puts it back.
#[tauri::command]
fn hook_install(force: bool) -> Result<hook::State, String> {
    let bin = hook_home().ok_or("the game's folder has not been found -- set it in Settings")?;
    hook::install(&bin, force)
}

/// Take the recorder back out of the game.
#[tauri::command]
fn hook_uninstall() -> Result<hook::State, String> {
    let bin = hook_home().ok_or("the game's folder has not been found -- set it in Settings")?;
    hook::uninstall(&bin)
}

/// What the recorder is doing this second.
#[tauri::command]
fn watch_state(watcher: tauri::State<watch::Watcher>) -> watch::State {
    watcher.state()
}

/// The events of the session in progress, for a screen opened part-way through.
#[tauri::command]
fn watch_tail(watcher: tauri::State<watch::Watcher>) -> Vec<sessionlog::Live> {
    watcher.tail()
}

/// Every recorded session, newest first.
#[tauri::command]
fn sessions_list(app: tauri::AppHandle) -> Result<Vec<sessionlog::Brief>, String> {
    Ok(sessionlog::list(&watch::sessions_dir(&app)?))
}

/// A session log path, confirmed to be one of ours.
///
/// The path arrives from the front end, and one of the things asked of it is
/// "delete this". A path is therefore only ever accepted when it names a `.log`
/// sitting directly in our own sessions folder -- not because the front end is
/// expected to lie, but because a bug there should not be able to reach a file
/// that is not ours.
fn a_session(app: &tauri::AppHandle, log: &str) -> Result<PathBuf, String> {
    let dir = watch::sessions_dir(app)?;
    let path = PathBuf::from(log);
    let same = |a: Option<&Path>, b: &Path| {
        a.map(|a| a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase())
            .unwrap_or(false)
    };
    if !same(path.parent(), &dir) || path.extension().and_then(|e| e.to_str()) != Some("log") {
        return Err(format!("{log} is not one of this program's session logs"));
    }
    Ok(path)
}

/// The text of one session log, for the viewer.
#[tauri::command]
async fn session_text(app: tauri::AppHandle, log: String) -> Result<String, String> {
    /// A session with every file open logged can run to megabytes, and the end
    /// is where the summary and the crash are. More than this in the webview at
    /// once is slower to draw than it is useful.
    const MOST: usize = 2 << 20;
    let path = a_session(&app, &log)?;
    tauri::async_runtime::spawn_blocking(move || sessionlog::read_log(&path, MOST))
        .await
        .map_err(|err| format!("reading the log failed: {err}"))?
}

/// Delete one session log, and give back the list that is left.
#[tauri::command]
fn session_forget(app: tauri::AppHandle, log: String) -> Result<Vec<sessionlog::Brief>, String> {
    let path = a_session(&app, &log)?;
    sessionlog::forget(&path)?;
    sessions_list(app)
}


// ---------------------------------------------------------------------------
// What the game itself did with the load order
// ---------------------------------------------------------------------------

/// Measure load order from one recorded session.
///
/// This is the one command that reads back rather than predicts: it takes the
/// order the game opened each contested asset's copies in and lines it up
/// against `ModPriority`. See [`engine::observed`] for what that settles and,
/// just as importantly, what it does not.
///
/// The whole log is read rather than the tail [`session_text`] shows, because
/// the mod files are opened in the first second of a run and the tail is the
/// part that would be missing them.
#[tauri::command]
async fn observe_session(
    app: tauri::AppHandle,
    log: String,
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> Result<engine::observed::Observation, String> {
    let path = a_session(&app, &log)?;
    tauri::async_runtime::spawn_blocking(move || {
        let text = std::fs::read_to_string(&path)
            .map_err(|err| format!("could not read {}: {err}", path.display()))?;
        Ok(observe_text(&log, &text, mods_dir, game_root))
    })
    .await
    .map_err(|err| format!("reading the session failed: {err}"))?
}

/// The join itself, with the library it is being judged against.
///
/// A missing library is not an error here. The measurement that matters -- which
/// direction `ModPriority` ran in -- needs only the log and the game's own
/// settings file, so a session recorded against an install that has since moved
/// still answers the question. Only the per-asset winner check needs the scan,
/// and it is simply left empty.
fn observe_text(
    log: &str,
    text: &str,
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> engine::observed::Observation {
    let roots = library_roots(mods_dir, game_root).ok();
    let host = roots
        .as_ref()
        .and_then(|(_, game)| game.as_deref())
        .map(engine::hostenv::read_mod_settings)
        .unwrap_or_default();

    let scan = roots
        .as_ref()
        .and_then(|(root, _)| scancache::get(root).ok());
    let known: Vec<String> = scan
        .as_ref()
        .map(|s| s.mods.iter().map(|m| m.name.clone()).collect())
        .unwrap_or_default();

    // The analysis is only needed for the predicted-winner column, and it is
    // taken from whatever the last scan left behind rather than run here: this
    // command is a read of a file that already exists and should not turn into
    // a several-second library scan because a screen was opened.
    let built = scan.as_ref().map(|s| {
        let active = s.active();
        analyze::analyse(
            active,
            s.stats.clone(),
            Vec::new(),
            None,
            WinnerRule::Last,
            false,
            Some(&s.host),
        )
    });

    engine::observed::observe(
        log,
        text,
        &known,
        &host,
        built.as_ref(),
        WinnerRule::Last,
        engine::clock::now_ms(),
    )
}

// ---------------------------------------------------------------------------
// What a game update changed underneath the mods
// ---------------------------------------------------------------------------

/// Every game build whose vanilla data is cached here, oldest first.
///
/// Shown so the update comparison can say what it is comparing, and so a fresh
/// install reads as "one build recorded" rather than as a clean library.
#[tauri::command]
fn vanilla_builds() -> Vec<engine::vanilla::Build> {
    engine::vanilla::builds()
}

/// What the latest game update did to the mods, if there is one to compare.
///
/// Deliberately its own command and not part of `analyse_library`. It decompiles
/// two copies of every asset the library touches, which is minutes of work on a
/// first run, and the answer only changes when the game does. Folding it into
/// the scan would put it on the launch path, which is the mistake that once cost
/// 76 seconds of start-up.
#[tauri::command]
async fn update_impact(
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> Result<engine::patchdiff::Impact, String> {
    tauri::async_runtime::spawn_blocking(move || run_update_impact(mods_dir, game_root))
        .await
        .map_err(|err| format!("update comparison failed: {err}"))?
}

fn run_update_impact(
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> Result<engine::patchdiff::Impact, String> {
    let (root, _) = library_roots(mods_dir, game_root)?;
    let scan = scancache::get(&root)
        .map_err(|err| format!("could not read {}: {err}", root.display()))?;
    let mut decompiler = Decompiler::locate(None, None).ok_or(
        "MBINCompiler was not found, and the game's own files cannot be read without it",
    )?;
    Ok(engine::patchdiff::latest(&scan.active(), &mut decompiler))
}

/// Cache vanilla copies of everything the library touches, for the next update.
///
/// The comparison above can only judge assets that were extracted *before* the
/// update; ordinary running extracts only the ones it needs. This fills the gap
/// in advance, and it has to be asked for -- see [`engine::patchdiff::prepare`]
/// for why it is not done automatically.
#[tauri::command]
async fn vanilla_prepare(
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> Result<engine::patchdiff::Prepared, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (root, game) = library_roots(mods_dir, game_root)?;
        let game = game.ok_or("no game install found to extract vanilla data from")?;
        let scan = scancache::get(&root)
            .map_err(|err| format!("could not read {}: {err}", root.display()))?;
        let mut source = VanillaSource::locate(&game, None)
            .ok_or("hgpaktool was not found, or the game has no PCBANKS folder")?;
        Ok(engine::patchdiff::prepare(&scan.active(), &mut source))
    })
    .await
    .map_err(|err| format!("preparing the baseline failed: {err}"))?
}
// ---------------------------------------------------------------------------
// Copies of the save
// ---------------------------------------------------------------------------

/// Every kept copy of a save, newest first, and what they take up.
#[tauri::command]
fn saves_kept(app: tauri::AppHandle) -> Result<SaveCopies, String> {
    let root = watch::backups_dir(&app)?;
    let kept = savewatch::list(&root);
    Ok(SaveCopies {
        bytes: kept.iter().map(|copy| copy.bytes).sum(),
        folders: savewatch::save_dirs()
            .iter()
            .map(|dir| dir.display().to_string())
            .collect(),
        root: root.display().to_string(),
        kept,
    })
}

/// The copies, where they came from, and what they cost.
#[derive(serde::Serialize)]
struct SaveCopies {
    kept: Vec<savewatch::Kept>,
    /// total on disk
    bytes: u64,
    /// the save folders being watched, one per account
    folders: Vec<String>,
    root: String,
}

/// Take a copy of anything that has changed, now, without waiting for the game.
#[tauri::command]
async fn saves_back_up(app: tauri::AppHandle) -> Result<SaveCopies, String> {
    let root = watch::backups_dir(&app)?;
    let keep = settings_now().keep_save_backups;
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let dirs = savewatch::save_dirs();
        if dirs.is_empty() {
            return Err("no save folder found under %APPDATA%\\HelloGames\\NMS".to_string());
        }
        savewatch::back_up(&dirs, &root, keep);
        Ok(())
    })
    .await
    .map_err(|err| format!("the copy failed: {err}"))??;
    saves_kept(handle)
}

/// Put one kept copy back into the game's save folder.
///
/// What it replaces is copied aside first, so restoring the wrong moment is
/// undone by restoring the copy this makes. Refused while the game is running.
#[tauri::command]
async fn save_restore(
    app: tauri::AppHandle,
    kept: savewatch::Kept,
) -> Result<Vec<String>, String> {
    let root = watch::backups_dir(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        savewatch::restore(&kept, &savewatch::save_dirs(), &root)
    })
    .await
    .map_err(|err| format!("the restore failed: {err}"))?
}

/// Delete one kept copy.
#[tauri::command]
fn save_forget(app: tauri::AppHandle, kept: savewatch::Kept) -> Result<SaveCopies, String> {
    let root = watch::backups_dir(&app)?;
    // Only ever inside our own folder, whatever the front end sends.
    if !PathBuf::from(&kept.path).starts_with(&root) {
        return Err(format!("{} is not one of this program's copies", kept.path));
    }
    savewatch::forget(&kept)?;
    saves_kept(app)
}

#[tauri::command]
fn presets_list(app: tauri::AppHandle) -> Result<preset::Presets, String> {
    Ok(preset::Presets::of(&settings::Settings::read(
        &settings_file(&app)?,
    )))
}

/// Save what is switched on right now as a named preset.
#[tauri::command]
fn preset_save(
    app: tauri::AppHandle,
    name: String,
    note: Option<String>,
) -> Result<preset::Presets, String> {
    let book = loadout::Loadout::read(&loadout_file(&app)?);
    let made = preset::capture(&book, &name, note)?;
    let path = settings_file(&app)?;
    let mut stored = settings::Settings::read(&path);
    stored.put_preset(made);
    stored.write(&path)?;
    Ok(preset::Presets::of(&stored))
}

#[tauri::command]
fn preset_delete(app: tauri::AppHandle, name: String) -> Result<preset::Presets, String> {
    let path = settings_file(&app)?;
    let mut stored = settings::Settings::read(&path);
    stored.forget_preset(&name);
    stored.write(&path)?;
    Ok(preset::Presets::of(&stored))
}

// ---------------------------------------------------------------------------
// Mod lists, to send to another player
// ---------------------------------------------------------------------------

/// Everything this library knows about, and the archive each mod came from.
///
/// The union of what is deployed and what the loadout records. A switched-off
/// mod has no files in the game, so no scan can see it -- and leaving those out
/// would mean a list exported from a preset that names one silently loses it.
fn held_library(
    app: &tauri::AppHandle,
    mods_dir: Option<String>,
) -> Result<(Vec<engine::collection::Held>, library::Staged), String> {
    let names = stored_names(app);
    let archives = archive_index(app, mods_dir.clone(), None);
    let book = loadout_file(app)
        .map(|path| loadout::Loadout::read(&path))
        .unwrap_or_default();

    let root = mods_root(mods_dir)?;
    let scan = scancache::get(&root)
        .map_err(|err| format!("could not read {}: {err}", root.display()))?;

    let mut owners: Vec<(String, Option<String>)> = scan
        .mods
        .iter()
        .map(|m| {
            (
                m.name.clone(),
                library::archive_of(m, &archives).map(str::to_string),
            )
        })
        .collect();
    let seen: std::collections::BTreeSet<&str> =
        scan.mods.iter().map(|m| m.name.as_str()).collect();
    for entry in &book.entries {
        if !seen.contains(entry.owner.as_str()) {
            owners.push((
                entry.owner.clone(),
                archives
                    .get(&entry.owner)
                    .cloned()
                    .or_else(|| Some(library::archive_stem(&entry.source).to_string())),
            ));
        }
    }

    let shown = library::names_for(owners.clone(), &names);
    // Switched off unless the loadout says otherwise. A mod deployed by hand
    // has no entry and is plainly on, which is what `unwrap_or(true)` says.
    let held = owners
        .into_iter()
        .map(|(owner, archive)| {
            let parsed = archive.as_deref().and_then(engine::nexusname::parse);
            engine::collection::Held {
                name: shown.get(&owner).cloned().unwrap_or_else(|| owner.clone()),
                mod_id: parsed.as_ref().map(|p| p.mod_id),
                version: parsed.map(|p| p.version),
                enabled: book.get(&owner).map(|e| e.enabled).unwrap_or(true),
                owner,
            }
        })
        .collect();

    Ok((held, archives))
}

/// Build a shareable list out of a preset, or out of what is switched on now.
///
/// Nothing is written. Composing and writing are separate so that a list with
/// nothing on it fails *before* the user has been made to pick a file name for
/// it, and so the caller can show what is about to be sent.
#[tauri::command]
async fn collection_export(
    app: tauri::AppHandle,
    mods_dir: Option<String>,
    preset: Option<String>,
    name: Option<String>,
    note: Option<String>,
) -> Result<engine::collection::Collection, String> {
    let settings_path = settings_file(&app)?;
    let book = loadout::Loadout::read(&loadout_file(&app)?);
    // `game_version` stays empty until something here can read the game's own
    // version. Nothing can: the closest thing in the program is the newest
    // MBINCompiler stamp across the library, which says what the *mods* were
    // built against, and writing that into a field labelled "game version"
    // would be a guess dressed as a fact. The field is in the format so a
    // build that can read it does not need a new list version.
    let game = None;

    tauri::async_runtime::spawn_blocking(move || {
        let (held, archives) = held_library(&app, mods_dir)?;

        // A named preset exports what the preset says, whether or not it is
        // the one switched on -- sharing a list you are not currently running
        // is an ordinary thing to want. Its own note travels with it, since
        // "the one that works with the multiplayer group" is written for
        // exactly the person the list is being sent to.
        let (owners, default_name, default_note) = match preset.as_deref() {
            Some(wanted) => {
                let found = settings::Settings::read(&settings_path)
                    .preset(wanted)
                    .ok_or_else(|| format!("there is no preset called {wanted}"))?
                    .clone();
                (found.enabled, found.name, found.note)
            }
            None => (
                book.entries
                    .iter()
                    .filter(|e| e.enabled)
                    .map(|e| e.owner.clone())
                    .collect(),
                "My mods".to_string(),
                None,
            ),
        };

        engine::collection::compose(
            name.as_deref().unwrap_or(&default_name),
            note.or(default_note),
            game,
            Some(engine::clock::local(engine::clock::now_ms()).written()),
            &owners,
            &held,
            &archives,
        )
    })
    .await
    .map_err(|err| err.to_string())?
}

#[tauri::command]
fn collection_write(
    list: engine::collection::Collection,
    path: String,
) -> Result<(), String> {
    engine::collection::write(&engine::collection::check(list)?, &PathBuf::from(path))
}

#[tauri::command]
fn collection_open(path: String) -> Result<engine::collection::Collection, String> {
    engine::collection::read(&PathBuf::from(path))
}

/// What a list someone sent would mean here. Reads only; writes nothing.
#[tauri::command]
async fn collection_plan(
    app: tauri::AppHandle,
    mods_dir: Option<String>,
    list: engine::collection::Collection,
) -> Result<engine::collection::Plan, String> {
    let list = engine::collection::check(list)?;
    tauri::async_runtime::spawn_blocking(move || {
        let (held, _) = held_library(&app, mods_dir)?;
        Ok(engine::collection::plan(&list, &held))
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Save an imported list as a preset, naming this library's own folders.
///
/// Deliberately stops there. Switching to it is a separate act with its own
/// preview, which is the one that says what leaves the game folder -- and a
/// list is usually imported before the mods on it have been downloaded, when
/// switching to it would be wrong anyway.
#[tauri::command]
async fn collection_import(
    app: tauri::AppHandle,
    mods_dir: Option<String>,
    list: engine::collection::Collection,
    name: Option<String>,
) -> Result<preset::Presets, String> {
    let list = engine::collection::check(list)?;
    let path = settings_file(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let (held, _) = held_library(&app, mods_dir)?;
        let plan = engine::collection::plan(&list, &held);
        let made = plan.preset(name.as_deref().unwrap_or(&plan.name))?;

        let mut stored = settings::Settings::read(&path);
        stored.put_preset(made);
        stored.write(&path)?;
        Ok(preset::Presets::of(&stored))
    })
    .await
    .map_err(|err| err.to_string())?
}

/// What switching to a preset would do, or did.
#[derive(serde::Serialize)]
struct PresetSwitch {
    applied: preset::Applied,
    changes: loadout::Changes,
}

/// Switch to a preset, deploying and undeploying to match it.
///
/// With `dry_run`, reports what would change and writes nothing -- neither the
/// loadout nor the game folder -- so the UI can confirm first.
#[tauri::command]
async fn preset_apply(
    app: tauri::AppHandle,
    name: String,
    mods_dir: Option<String>,
    dry_run: bool,
) -> Result<PresetSwitch, String> {
    let root = mods_root(mods_dir)?;
    let book_path = loadout_file(&app)?;
    let settings_path = settings_file(&app)?;

    tauri::async_runtime::spawn_blocking(move || {
        let wanted = settings::Settings::read(&settings_path)
            .preset(&name)
            .ok_or_else(|| format!("there is no preset called {name}"))?
            .clone();

        let mut book = loadout::Loadout::read(&book_path);
        let applied = preset::apply(&mut book, &wanted);
        let changes = loadout::reconcile(&mut book, &root, dry_run);

        if !dry_run {
            book.write(&book_path)?;
            // Read back rather than holding the copy from above: reconciling a
            // whole library takes long enough for the settings screen to have
            // saved something in the meantime.
            let mut stored = settings::Settings::read(&settings_path);
            stored.active_preset = Some(wanted.name.clone());
            stored.write(&settings_path)?;
        }
        Ok(PresetSwitch { applied, changes })
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Switch mods on or off, and make the game folder match.
///
/// Takes a list rather than one mod so that turning ten off is a single
/// reconcile: doing them one at a time would relink the folder ten times and
/// leave it briefly inconsistent between each.
#[tauri::command]
async fn set_mods_enabled(
    app: tauri::AppHandle,
    owners: Vec<String>,
    enabled: bool,
    mods_dir: Option<String>,
) -> Result<loadout::Changes, String> {
    let root = mods_root(mods_dir)?;
    let book_path = loadout_file(&app)?;

    tauri::async_runtime::spawn_blocking(move || {
        let mut book = loadout::Loadout::read(&book_path);
        let mut missing = Vec::new();
        for owner in &owners {
            match book.entries.iter_mut().find(|e| &e.owner == owner) {
                Some(slot) => slot.enabled = enabled,
                None => missing.push(owner.clone()),
            }
        }
        if !missing.is_empty() {
            return Err(format!(
                "not managed by this program: {}",
                missing.join(", ")
            ));
        }

        let changes = loadout::reconcile(&mut book, &root, false);
        book.write(&book_path)?;
        Ok(changes)
    })
    .await
    .map_err(|err| err.to_string())?
}

/// The folders a delete is allowed to touch.
fn erase_roots(mods_dir: Option<String>, game_root: Option<String>) -> Result<erase::Roots, String> {
    let spots = places(mods_dir, game_root.clone())?;
    let game = game_root
        .map(PathBuf::from)
        .or_else(|| gamefind::find_install().map(|i| i.root));
    Ok(erase::Roots::of(
        &spots.staging,
        &spots.derived,
        &spots.archives,
        &spots.mods_dir,
        game.as_deref(),
    ))
}

/// What deleting these mods would remove. Touches nothing.
#[tauri::command]
async fn delete_preview(
    app: tauri::AppHandle,
    owners: Vec<String>,
    with_archive: bool,
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> Result<Vec<erase::Plan>, String> {
    let roots = erase_roots(mods_dir, game_root)?;
    let book_path = loadout_file(&app)?;

    tauri::async_runtime::spawn_blocking(move || {
        let book = loadout::Loadout::read(&book_path);
        owners
            .iter()
            .filter_map(|owner| book.get(owner))
            .map(|entry| erase::plan(entry, &roots, with_archive))
            .collect()
    })
    .await
    .map_err(|err| err.to_string())
}

/// Delete mods for good: out of the game, off the disk, out of the mod list.
///
/// This is the one operation here that cannot be undone.
#[tauri::command]
async fn delete_mods(
    app: tauri::AppHandle,
    owners: Vec<String>,
    with_archive: bool,
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> Result<Vec<erase::Erased>, String> {
    let roots = erase_roots(mods_dir, game_root)?;
    let book_path = loadout_file(&app)?;
    let edits_path = edits_file(&app)?;

    tauri::async_runtime::spawn_blocking(move || {
        let mut book = loadout::Loadout::read(&book_path);
        let mut done = Vec::new();
        for owner in &owners {
            let Some(entry) = book.get(owner).cloned() else {
                continue; // already gone; nothing to report and nothing to do
            };
            let plan = erase::plan(&entry, &roots, with_archive);
            done.push(erase::erase(&plan, &roots, &mut book)?);
            // The mod is gone, so values set on it can never be applied again.
            // Left behind they would accumulate invisibly, and would come back
            // if the same folder name were ever installed again.
            forget_edits(&edits_path, &entry.owner);
        }
        // Put back whatever the deletion un-blocked. Deleting a merge is the
        // case that needs this: its entry is what held its inputs out of the
        // game, so with it gone they are wanted again and nothing else would
        // notice until the next unrelated reconcile.
        let changes = loadout::reconcile(&mut book, &roots.mods_dir, false);
        // Written once, after every deletion, so a failure part-way through
        // cannot leave the list claiming a mod whose files are gone.
        book.write(&book_path)?;
        scancache::invalidate();
        if let Some(erased) = done.last_mut() {
            erased.problems.extend(changes.problems);
        }
        Ok(done)
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Where a mod's real files are, for a mod this program is responsible for.
///
/// Repairing needs the *staged* files, not the deployed ones. Writing a mend
/// through a deployed name would rewrite the staged file too -- they are the
/// same file -- and destroy the original the repair is supposed to leave
/// alone.
fn staged_source(app: &tauri::AppHandle, owner: &str) -> Result<PathBuf, String> {
    let book = loadout::Loadout::read(&loadout_file(app)?);
    // Found the same way cleaning finds it, so a mod whose staged folder is
    // named after its archive rather than after the folder it deploys is
    // reachable from both. These used to differ, and a mod like that could be
    // cleaned but not mended.
    let entry = entry_deploying(&book, owner).ok_or_else(|| {
        format!(
            "{owner} was not installed by this program, so there is no untouched copy of it \
             to mend from. Reinstall it here first."
        )
    })?;
    // Always the original, never whatever build is deployed now. Mending an
    // already-mended build would find nothing wrong and report failure, and
    // mending a *merged* build would edit the merge instead of the mod.
    Ok(entry.origin_path())
}

/// What putting mods back to the build their author shipped did.
#[derive(serde::Serialize)]
struct Restored {
    /// mods switched back, and which build they were on
    reverted: Vec<(String, String)>,
    /// merges taken apart, which is how their inputs come back
    dissolved: Vec<String>,
    /// mods already on the author's build, so nothing was done
    untouched: Vec<String>,
    problems: Vec<String>,
}

/// Put mods back to the build their author shipped, in one pass.
///
/// `owners` names them; leaving it out means the whole library, which is what
/// "back to stock" wants. One reconcile at the end rather than one per mod, for
/// the same reason [`clean_apply_many`] exists: the expensive part is matching
/// the game folder against the book, and twenty-five separate calls do it
/// twenty-five times.
///
/// # A merge is not put back, it is taken apart
///
/// Everything else here is a build sitting beside an untouched staged copy, so
/// going back is switching which one is deployed. A merge has no such copy — it
/// *is* the build, and `merge::build` records its own folder as its origin — so
/// "the author's build" is not a thing it has. What a person means by putting
/// it back is having the mods it stands in for back in the game, and that is
/// dissolving it. Its files are taken out explicitly, because an entry the book
/// no longer lists is one `reconcile` can no longer undeploy.
#[tauri::command]
async fn restore_originals(
    app: tauri::AppHandle,
    owners: Option<Vec<String>>,
    mods_dir: Option<String>,
) -> Result<Restored, String> {
    let root = mods_root(mods_dir)?;
    let book_path = loadout_file(&app)?;
    let edits_path = edits_file(&app)?;

    tauri::async_runtime::spawn_blocking(move || {
        let mut book = loadout::Loadout::read(&book_path);
        let wanted = |owner: &str| owners.as_ref().is_none_or(|w| w.iter().any(|o| o == owner));

        let mut out = Restored {
            reverted: Vec::new(),
            dissolved: Vec::new(),
            untouched: Vec::new(),
            problems: Vec::new(),
        };

        // Merges first, and by name rather than in place: dissolving one can
        // orphan another, and both change the list being walked.
        let merges: Vec<String> = book
            .entries
            .iter()
            .filter(|e| e.variant == loadout::Variant::Merged && wanted(&e.owner))
            .map(|e| e.owner.clone())
            .collect();
        for owner in merges {
            let Some(entry) = book.get(&owner).cloned() else { continue };
            if let Err(why) = deploy::undeploy(&root, &entry.deployed) {
                out.problems.push(format!("{owner}: {why}"));
                continue;
            }
            // Its inputs go back in the game, so the merge's own build is of no
            // further use. Removed, not kept: a rebuild always goes back to the
            // parents anyway.
            let _ = std::fs::remove_dir_all(std::path::PathBuf::from(&entry.source));
            book.forget(&owner);
            out.dissolved.push(owner);
        }

        for entry in book.entries.iter_mut() {
            if !wanted(&entry.owner) {
                continue;
            }
            // `edited` counts as not-original in its own right: a mod on an
            // otherwise untouched build can still be carrying values the user
            // typed, and "put the originals back" means those too.
            if entry.variant == loadout::Variant::Original && !entry.edited {
                out.untouched.push(entry.owner.clone());
                continue;
            }
            let was = if entry.variant == loadout::Variant::Original {
                "edited".to_string()
            } else if entry.edited {
                format!("{:?} and edited", entry.variant).to_lowercase()
            } else {
                format!("{:?}", entry.variant).to_lowercase()
            };
            // See `forget_edits`: kept but unapplied, they would come back on
            // the next clean without being asked for.
            forget_edits(&edits_path, &entry.owner);
            entry.source = entry.origin_path().display().to_string();
            entry.variant = loadout::Variant::Original;
            entry.edited = false;
            out.reverted.push((entry.owner.clone(), was));
        }

        let changes = loadout::reconcile(&mut book, &root, false);
        out.problems.extend(changes.problems);
        book.write(&book_path)?;
        scancache::invalidate();
        Ok(out)
    })
    .await
    .map_err(|err| err.to_string())?
}

/// What is wrong with a mod's files, and what could be mended. Writes nothing.
#[tauri::command]
async fn repair_preview(
    app: tauri::AppHandle,
    owner: String,
) -> Result<repair::Mended, String> {
    let source = staged_source(&app, &owner)?;
    tauri::async_runtime::spawn_blocking(move || repair::inspect(&source))
        .await
        .map_err(|err| err.to_string())
}

/// Write a mended build of a mod and switch the game over to it.
///
/// The mod as its author shipped it is untouched in staging, so undoing this
/// is switching the variant back rather than restoring from a backup.
#[tauri::command]
async fn repair_apply(
    app: tauri::AppHandle,
    owner: String,
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> Result<loadout::Changes, String> {
    let spots = places(mods_dir, game_root)?;
    let source = staged_source(&app, &owner)?;
    let book_path = loadout_file(&app)?;
    let edits_path = edits_file(&app)?;

    tauri::async_runtime::spawn_blocking(move || {
        let mut book = loadout::Loadout::read(&book_path);
        let edits = edit::Book::read(&edits_path);
        let was = entry_deploying(&book, &owner)
            .cloned()
            .ok_or_else(|| format!("{owner} is no longer in the mod list"))?;

        // Named after the entry, not after the folder that was clicked, so a
        // mend and a clean of the same mod build in the same place and the
        // second replaces the first instead of leaving an orphan beside it.
        let dest = spots.derived.join(&was.owner);
        repair::mend_into(&source, &dest)?;
        book.put(loadout::Entry {
            source: dest.display().to_string(),
            variant: loadout::Variant::Mended,
            // Deliberately kept, so reconcile knows what to take away first:
            // the broken file is deployed under these names right now.
            deployed: was.deployed.clone(),
            ..was.clone()
        });

        // A mend rebuilds the mod from the copy its author shipped, which is
        // the one place the user's own values are not. Re-applied here for the
        // same reason [`run_clean`] does it: otherwise an unrelated verb
        // reverts them in silence.
        if edits.count(&was.owner) > 0 {
            let mut decompiler =
                Decompiler::locate(None, None).ok_or("MBINCompiler was not found")?;
            rebuild_edited(&mut book, &edits, &was.owner, &spots, &mut decompiler)?;
        }

        let changes = loadout::reconcile(&mut book, &spots.mods_dir, false);
        book.write(&book_path)?;
        scancache::invalidate();
        Ok(changes)
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Put a mod back to the build its author shipped, undoing a repair.
///
/// The same switch as [`clean_undo`], and the same code: both variants are a
/// build sitting beside an untouched staged copy, so both come back the same
/// way. They were written out twice and had already drifted -- one of them
/// would happily "undo" a mod that was already on its original build.
#[tauri::command]
async fn repair_undo(
    app: tauri::AppHandle,
    owner: String,
    mods_dir: Option<String>,
) -> Result<loadout::Changes, String> {
    let root = mods_root(mods_dir)?;
    let book_path = loadout_file(&app)?;
    let edits_path = edits_file(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        back_to_original(&book_path, &edits_path, &owner, &root)
    })
        .await
        .map_err(|err| err.to_string())?
}

/// The mods this program is responsible for, and whether each is switched on.
///
/// Heals the record as it reads it: an entry whose builds have all gone from
/// disk is dropped and the book rewritten. The Library list is built from this
/// rather than from a scan — it has to be, or a switched-off mod would vanish
/// instead of offering its switch — so without this a mod deleted by hand stays
/// on the list for good, and clicking it shows an empty pane, because every
/// other surface is built from the scan and agrees it is not there.
///
/// Guarded inside [`loadout::Loadout::forget_missing`]: an unreadable staging
/// folder prunes nothing.
#[tauri::command]
fn loadout_list(
    app: tauri::AppHandle,
    mods_dir: Option<String>,
) -> Result<loadout::Loadout, String> {
    let path = loadout_file(&app)?;
    let mut book = loadout::Loadout::read(&path);
    if let Ok(spots) = places(mods_dir, None) {
        if !book
            .forget_missing(&spots.staging, &spots.derived, &spots.mods_dir)
            .is_empty()
        {
            // A failed write is not worth refusing the list over: the entries
            // are gone from what is returned either way, and the next call
            // simply finds the same dead ones and tries again.
            let _ = book.write(&path);
        }
    }
    Ok(book)
}

/// What taking over another manager's mods folder would do. Writes nothing.
///
/// This is the first-run path for anyone arriving with a library another
/// manager deployed, and it was the one part of the engine with no way in: the
/// whole of `engine::adopt` was reachable only from `examples/cutover.rs`, so a
/// user could see their mods listed and not switch, clean, mend or delete any of
/// them, because all of those need a loadout entry.
///
/// Vortex deploys by hardlink, which is what this program does too, so the
/// cutover is bookkeeping rather than a migration -- nothing is copied, moved or
/// deleted. A mod whose staged copy does not match what is in the game is
/// refused by name rather than adopted on a guess.
#[tauri::command]
async fn adopt_survey(mods_dir: Option<String>) -> Result<engine::adopt::Survey, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // Our own staging folder, as the starting point. A Vortex manifest, if
        // one is there, names its own and wins; without one this is what the
        // deployed mods are matched against. Resolved even when detection is
        // shaky, because a survey that cannot say where to look can only ever
        // report nothing.
        // Staging *and* derived: a mod running a cleaned, mended or merged
        // build is deployed from the second, and a survey that only knew about
        // the first would mistake our own output for someone's mod.
        let spots = places(mods_dir.clone(), None).ok();
        let staging = spots.as_ref().map(|s| s.staging.clone());
        let derived = spots.as_ref().map(|s| s.derived.clone());
        let root = mods_root(mods_dir)?;
        engine::adopt::survey(&root, staging.as_deref(), derived.as_deref())
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Take over every mod the survey found adoptable. Returns how many.
#[tauri::command]
async fn adopt_apply(app: tauri::AppHandle, mods_dir: Option<String>) -> Result<usize, String> {
    let book_path = loadout_file(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let spots = places(mods_dir.clone(), None).ok();
        let staging = spots.as_ref().map(|s| s.staging.clone());
        let derived = spots.as_ref().map(|s| s.derived.clone());
        let root = mods_root(mods_dir)?;
        let survey = engine::adopt::survey(&root, staging.as_deref(), derived.as_deref())?;
        // Surveyed again here rather than taking the one the UI is holding: it
        // names the folders that are about to be written down as ours, and the
        // folder could have changed while the user read the list.
        let taken = engine::adopt::adopt(&survey, &book_path)?;
        scancache::invalidate();
        Ok(taken)
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Work out how to put mods running one of our builds back on the author's.
///
/// Read-only, and surveyed here rather than taking the list the UI is holding:
/// it names folders that are about to be relinked, and the folder could have
/// changed while the user read the survey.
#[tauri::command]
async fn restore_authors_preview(
    mods_dir: Option<String>,
) -> Result<Vec<engine::adopt::Restorable>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let spots = places(mods_dir.clone(), None).ok();
        let staging = spots.as_ref().map(|s| s.staging.clone());
        let derived = spots.as_ref().map(|s| s.derived.clone());
        let root = mods_root(mods_dir)?;
        let survey = engine::adopt::survey(&root, staging.as_deref(), derived.as_deref())?;
        Ok(engine::adopt::restorable(
            staging.as_deref(),
            &root,
            &survey.ours,
        ))
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Put them back on the author's build, and take them over while we are there.
#[tauri::command]
async fn restore_authors(
    app: tauri::AppHandle,
    mods_dir: Option<String>,
) -> Result<engine::adopt::Restoration, String> {
    let book_path = loadout_file(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let spots = places(mods_dir.clone(), None)?;
        let root = mods_root(mods_dir)?;
        let survey = engine::adopt::survey(&root, Some(&spots.staging), Some(&spots.derived))?;
        let plan = engine::adopt::restorable(Some(&spots.staging), &root, &survey.ours);
        let done = engine::adopt::restore(
            &plan,
            &spots.staging,
            &spots.derived,
            &root,
            &book_path,
            false,
        )?;
        scancache::invalidate();
        Ok(done)
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Put the game folder back in line with the loadout.
#[tauri::command]
async fn deploy_reconcile(
    app: tauri::AppHandle,
    mods_dir: Option<String>,
    dry_run: bool,
) -> Result<loadout::Changes, String> {
    let root = mods_root(mods_dir)?;
    let book_path = loadout_file(&app)?;

    tauri::async_runtime::spawn_blocking(move || {
        let mut book = loadout::Loadout::read(&book_path);
        let changes = loadout::reconcile(&mut book, &root, dry_run);
        if !dry_run {
            book.write(&book_path)?;
        }
        Ok(changes)
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Download and install what an `nxm://` link points at.
///
/// Progress is reported on the `install-progress` event rather than by
/// returning at the end: a mod archive can be tens of megabytes and a window
/// that says nothing for a minute looks broken.
#[tauri::command]
async fn install_from_nxm(
    app: tauri::AppHandle,
    url: String,
    mods_dir: Option<String>,
    game_root: Option<String>,
) -> Result<pipeline::Installed, String> {
    let key = stored_key(&app).ok_or(
        "no Nexus API key has been set, so downloads cannot be requested. \
         Connect your account in Settings first.",
    )?;
    let link = nxm::parse(&url, nexus::GAME)?;
    let spots = places(mods_dir, game_root)?;
    let book = loadout_file(&app)?;

    tauri::async_runtime::spawn_blocking(move || {
        let api = nexus::Api::new(key);
        let done = pipeline::install_from_link(&api, &link, &spots, &book, |stage| {
            let _ = app.emit("install-progress", &stage);
        });
        scancache::invalidate();
        done
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Install an archive already on disk, through the same three layers.
#[tauri::command]
async fn install_archive(
    app: tauri::AppHandle,
    archive: String,
    mods_dir: Option<String>,
    game_root: Option<String>,
    overwrite: bool,
) -> Result<pipeline::Installed, String> {
    let spots = places(mods_dir, game_root)?;
    let book = loadout_file(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        let done = pipeline::install_from_file(&PathBuf::from(archive), &spots, &book, overwrite);
        scancache::invalidate();
        done
    })
    .await
    .map_err(|err| err.to_string())?
}

/// One of Nexus's own curated lists, for the Browse tab.
#[tauri::command]
async fn nexus_browse(
    app: tauri::AppHandle,
    list: nexus::Listing,
) -> Result<Vec<nexus::ModPage>, String> {
    let key = stored_key(&app).ok_or("no Nexus API key has been set")?;
    tauri::async_runtime::spawn_blocking(move || {
        nexus::Api::new(key).browse(list).map(|(mods, _)| mods)
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Whether this program currently owns `nxm://`, and who does instead.
#[derive(serde::Serialize)]
struct SchemeOwner {
    ours: bool,
    /// the command line registered for the scheme, when one is readable
    command: Option<String>,
}

/// Who handles `nxm://` on this machine right now.
#[tauri::command]
fn nxm_owner() -> SchemeOwner {
    let command = read_nxm_command();
    let ours = command
        .as_deref()
        .map(|c| c.to_lowercase().contains("nmscheck") || c.to_lowercase().contains(r"\app.exe"))
        .unwrap_or(false);
    SchemeOwner { ours, command }
}

#[cfg(windows)]
fn read_nxm_command() -> Option<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey(r"Software\Classes\nxm\shell\open\command")
        .ok()?
        .get_value::<String, _>("")
        .ok()
}

#[cfg(not(windows))]
fn read_nxm_command() -> Option<String> {
    None
}

/// Take over `nxm://`, so downloads started in a web browser land here too.
///
/// Off by default and never done without being asked. The scheme is global:
/// claiming it takes every game's downloads from whatever had it, which for
/// most people is the mod manager they still use for their other games.
#[tauri::command]
fn claim_nxm_scheme(app: tauri::AppHandle) -> Result<SchemeOwner, String> {
    #[cfg(desktop)]
    {
        use tauri_plugin_deep_link::DeepLinkExt;
        app.deep_link()
            .register("nxm")
            .map_err(|e| format!("could not register nxm:// links: {e}"))?;
    }
    let _ = &app;
    Ok(nxm_owner())
}

/// Give `nxm://` back to whatever had it before.
#[tauri::command]
fn release_nxm_scheme(app: tauri::AppHandle) -> Result<SchemeOwner, String> {
    #[cfg(desktop)]
    {
        use tauri_plugin_deep_link::DeepLinkExt;
        app.deep_link()
            .unregister("nxm")
            .map_err(|e| format!("could not release nxm:// links: {e}"))?;
    }
    let _ = &app;
    Ok(nxm_owner())
}

/// Which files belong to a mod, so the UI can say what removing it takes.
#[tauri::command]
async fn mod_members(owner: String, mods_dir: Option<String>) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = mods_root(mods_dir)?;
        Ok(library::members(&root, &owner))
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Take a mod out of the mods folder, keeping every byte in app data.
#[tauri::command]
async fn remove_mod(
    app: tauri::AppHandle,
    owner: String,
    mods_dir: Option<String>,
) -> Result<library::Removal, String> {
    let trash = app_data(&app).map(|dir| dir.join("removed"))?;

    // This is the only way out of the game for a mod we did not install, and it
    // is the wrong one for a mod we did. It moves files without touching the
    // loadout, so on a managed mod the record would still read "deployed": the
    // next reconcile -- any activate, any preset switch -- would see the files
    // missing and put them straight back, while the moved copy sat in the
    // removed folder for good.
    //
    // The UI does not offer it for managed mods, but the refusal belongs here
    // rather than in the screen that happens to call it: a command that quietly
    // undoes itself is not something to guard by hiding its button.
    if let Ok(path) = loadout_file(&app) {
        if loadout::Loadout::read(&path)
            .entries
            .iter()
            .any(|e| e.owner == owner)
        {
            return Err(format!(
                "{owner} was installed by this program, so it cannot be removed by hand.                  Deactivate it to take it out of the game, or delete it to remove it from                  your library as well."
            ));
        }
    }

    tauri::async_runtime::spawn_blocking(move || {
        let root = mods_root(mods_dir)?;
        let done = library::remove(&root, &owner, &trash);
        scancache::invalidate();
        done
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Put back what one removal took out.
#[tauri::command]
async fn restore_mod(trash: String, mods_dir: Option<String>) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = mods_root(mods_dir)?;
        let done = library::restore(&root, &PathBuf::from(trash));
        scancache::invalidate();
        done
    })
    .await
    .map_err(|err| err.to_string())?
}

/// What installing an archive would produce, without doing it.
#[tauri::command]
async fn install_preview(
    archive: String,
    mods_dir: Option<String>,
) -> Result<engine::archive::Plan, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let root = mods_root(mods_dir)?;
        engine::archive::preview(&PathBuf::from(archive), &root)
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Folders a merge put in the game. See [`clean_context`] and `check_updates`:
/// a merge is this program's own build, so it is neither cleanable nor a thing
/// Nexus has ever heard of.
fn merge_folders(book: &loadout::Loadout) -> std::collections::BTreeSet<String> {
    book.entries
        .iter()
        .filter(|e| e.variant == loadout::Variant::Merged)
        .flat_map(|e| e.deployed.iter().cloned())
        .collect()
}

/// The scan and the two external tools every clean command needs.
///
/// `ours` are folders a merge put in the game, and they are dropped: a merge is
/// this program's own build, and cleaning one cannot work. Its loadout `origin`
/// *is* the derived folder it lives in — a merge has no author's copy behind it
/// — and a clean builds into `derived/<owner>`, the same path, so
/// [`prune::clean_into`] would clear the folder and then fail reading what it
/// had just deleted. Refused there as well, because the cost of getting it
/// wrong is the user's merge; dropped here so the card is never offered in the
/// first place.
fn clean_context(
    mods_dir: Option<String>,
    game_root: Option<String>,
    ours: &std::collections::BTreeSet<String>,
) -> Result<(Vec<engine::model::Mod>, Decompiler, VanillaSource), String> {
    let (root, vanilla_root) = library_roots(mods_dir, game_root)?;
    let vanilla_root = vanilla_root.ok_or("no game install found to compare against")?;

    let scan = scancache::get(&root)
        .map_err(|err| format!("could not read {}: {err}", root.display()))?;
    let mut active = scan.active();
    active.retain(|m| !ours.contains(&m.name));

    let decompiler = Decompiler::locate(None, None).ok_or("MBINCompiler was not found")?;
    let source = VanillaSource::locate(&vanilla_root, None)
        .ok_or("hgpaktool was not found, or the game has no PCBANKS folder")?;
    Ok((active, decompiler, source))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // Must be first. Clicking a download on the website asks Windows to
        // open an `nxm://` URL, and Windows starts a *new* copy of this
        // program to do it. Without this, every download would open another
        // window; with it, the second copy hands its arguments to the one
        // already running and exits.
        //
        // It only raises the window. It must *not* also emit the URL: this
        // plugin is built with its `deep-link` feature, so it already hands
        // the arguments to the deep-link plugin, and emitting here as well
        // made one click on the website start two identical downloads.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(watch::Watcher::new())
        .setup(|app| {
            // A packaged build ships MBINCompiler and hgpaktool as resources.
            // Only the running app can resolve that directory, so tell the
            // engine about it before anything tries to shell out.
            if let Ok(dir) = app.path().resource_dir() {
                tools::set_bundled_dir(dir.join("tools"));
            }

            // Note where settings live, so the commands that resolve paths can
            // read them without an `AppHandle` in reach. See `SETTINGS_PATH`.
            if let Ok(dir) = app_data(&app.handle().clone()) {
                let _ = SETTINGS_PATH.set(dir.join("settings.json"));
            }

            // Start watching for the game. The thread idles in two-second
            // steps while nothing is running and opens the pipe only once
            // `NMS.exe` is there, so this costs nothing until it is needed --
            // and starting it here rather than when the Sessions tab is opened
            // is the point: a session has to be recorded from its first second,
            // which is when the game opens every mod file it is going to open.
            if let Ok(sessions) = watch::sessions_dir(&app.handle().clone()) {
                app.state::<watch::Watcher>()
                    .start(app.handle().clone(), sessions);
            }

            // Listen for `nxm://` links, but do NOT claim the scheme.
            //
            // Windows has one handler for a protocol, for every game at once.
            // Taking it would break Vortex for the games it still manages,
            // which is not a trade this program gets to make on the user's
            // behalf. The links we care about are the ones clicked in our own
            // browser window, and those are caught before they ever reach the
            // operating system -- see `browser::browser_show`.
            //
            // This handler stays wired up because a user who *does* want us to
            // own the scheme can say so, and because an installed build can be
            // launched with a link on its command line either way.
            #[cfg(desktop)]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    if let Some(window) = handle.get_webview_window("main") {
                        let _ = window.set_focus();
                    }
                    for url in event.urls() {
                        let _ = handle.emit("nxm-link", url.to_string());
                    }
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            find_installs,
            find_install,
            analyse_library,
            merge_conflict,
            clean_preview,
            clean_apply,
            clean_apply_many,
            clean_undo,
            edit_survey,
            edit_apply,
            edit_undo,
            edit_forget_values,
            nexus_set_key,
            nexus_account,
            nexus_forget_key,
            check_updates,
            browser::browser_show,
            browser::browser_layout,
            browser::browser_hide,
            browser::browser_close,
            browser::browser_go,
            library_list,
            mod_names,
            resolve_names,
            forget_names,
            release_nxm_scheme,
            claim_nxm_scheme,
            nxm_owner,
            nexus_browse,
            install_archive,
            install_from_nxm,
            nexus_mod,
            mod_members,
            remove_mod,
            restore_mod,
            install_preview,
            settings_read,
            settings_resolve,
            settings_save,
            launch_game,
            hook_state,
            hook_install,
            hook_uninstall,
            watch_state,
            watch_tail,
            sessions_list,
            observe_session,
            vanilla_builds,
            update_impact,
            vanilla_prepare,
            session_text,
            session_forget,
            saves_kept,
            saves_back_up,
            save_restore,
            save_forget,
            presets_list,
            preset_save,
            preset_delete,
            preset_apply,
            collection_export,
            collection_write,
            collection_open,
            collection_plan,
            collection_import,
            set_mods_enabled,
            delete_preview,
            delete_mods,
            loadout_list,
            adopt_survey,
            adopt_apply,
            restore_authors_preview,
            restore_authors,
            deploy_reconcile,
            repair_preview,
            repair_apply,
            repair_undo,
            restore_originals
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

