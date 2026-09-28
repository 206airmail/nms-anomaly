<script lang="ts">
  import ActionRow from "$lib/ActionRow.svelte";
  import Button from "$lib/Button.svelte";
  import TidyList from "$lib/TidyList.svelte";
  import EvidenceSheet from "$lib/EvidenceSheet.svelte";
  import LibraryPane from "$lib/LibraryPane.svelte";
  import SessionsPane from "$lib/SessionsPane.svelte";
  import SettingsPane from "$lib/SettingsPane.svelte";
  import UpdatePanel from "$lib/UpdatePanel.svelte";
  import InstallStrip from "$lib/InstallStrip.svelte";
  import { listen } from "@tauri-apps/api/event";
  // Aliased: `installs` in this file already means the game installs found
  // on disk, which is a different thing entirely.
  import { installs as queue } from "$lib/installs.svelte";
  import { updates } from "$lib/updates.svelte";
  import { names } from "$lib/names.svelte";
  import { browser } from "$lib/browser.svelte";
  import { sessions } from "$lib/sessions.svelte";
  import { library } from "$lib/library.svelte";
  import type { InstallStage } from "$lib/engine";
  import { onMount } from "svelte";
  import {
    analyseLibrary,
    cleanApply,
    cleanApplyMany,
    cleanPreview,
    cleanUndo,
    repairPreview,
    repairApply,
    findInstall,
    findInstalls,
    mergeConflict,
    appVersion,
    settingsRead,
    settingsResolve,
    launchGame,
    blankSettings,
    type Resolved,
    type CleanPlan,
    type Install,
    type Settings,
  } from "$lib/engine";
  import {
    bandOf,
    banded,
    buildActions,
    pending,
    plural,
    reversible,
    scope,
    verdict,
    type Action,
  } from "$lib/actions";
  import {
    actionable,
    combinedNote,
    describeDelta,
    diffReports,
    seriousDrift,
    type Conflict,
    type Report,
  } from "$lib/types";
  import fixture from "$lib/fixture-report.json";
  import demo from "$lib/demo-report.json";

  // The fixtures are the browser-only fallback: `npm run dev` without Tauri
  // cannot call the engine, and the UI still has to be workable there. Inside
  // the app, `scanned` wins and neither fixture is read.
  let sample = $state(false);
  let scanned = $state<Report | null>(null);
  let scanning = $state(false);
  let failure = $state<string | null>(null);

  /**
   * What the last scan found that the one before it did not.
   *
   * It answers a question asked at one moment — "I just installed something;
   * did it arrive?" — and it is the only place that gets answered, which is why
   * it is worth having. But it was written as a permanent strip: no dismiss, no
   * expiry, and outside the pane so it survived every tab change. Install one
   * mod and "added Unpredictable Shelters 1.4" sat across the top of the window
   * for the rest of the session, describing something that had stopped being
   * news within seconds of being read.
   *
   * So it is a notice with an end: [`note`] clears it after a while, the × ends
   * it sooner, and the next scan replaces or removes it as it always did.
   */
  let changed = $state<string | null>(null);
  let noticeTimer: ReturnType<typeof setTimeout> | null = null;

  /** How long one line about a finished scan stays worth the space. */
  const NOTICE_MS = 12000;

  function note(said: string | null) {
    if (noticeTimer) clearTimeout(noticeTimer);
    noticeTimer = null;
    changed = said;
    if (said) noticeTimer = setTimeout(() => (changed = null), NOTICE_MS);
  }

  /**
   * Nothing, in the shape of a report.
   *
   * Stood in for by the fixture until the scan lands, which is right in a
   * browser and wrong after a failed scan: the screens that do not depend on
   * the scan stay open now, and every one of them would have been drawing the
   * fixture's twelve conflicts over a library the app could not read.
   */
  const NOTHING: Report = {
    roots: [],
    winner_rule: "last",
    reference_version: null,
    stats: {
      mods: 0,
      files: 0,
      targets: 0,
      exml: 0,
      mbin: 0,
      dds: 0,
      lua: 0,
      decompiled: 0,
      parse_errors: 0,
      error_details: [],
    },
    actionable_count: 0,
    merged_count: 0,
    real_load_order: false,
    manager: null,
    disable_all: false,
    disabled: [],
    unregistered: [],
    mods: [],
    conflicts: [],
    broken: [],
    drift: [],
    tools: {
      mbincompiler: null,
      hgpaktool: null,
      scene_check: false,
      scene_check_note: null,
    },
    stale: [],
    loc_clashes: [],
  };

  const report = $derived(
    (scanned ??
      (failure ? NOTHING : ((sample ? demo : fixture) as unknown))) as Report,
  );
  const live = $derived(scanned !== null);

  /** What the bundle says it is, for the status bar. See [`appVersion`]. */
  let version = $state("");
  void appVersion().then((said) => {
    if (said) version = said;
  });

  /** One figure in the Last scan readout: a count, a wait, or no answer. */
  function tally(count: number): string {
    if (failure) return "—";
    return checking ? "…" : String(count);
  }

  // The clean comparison decompiles every whole-file override against the
  // game's own copy, so it is slower than the scan and lands after it.
  let plans = $state<CleanPlan[]>([]);
  let weighing = $state(false);

  /**
   * True from the moment a pass over the library starts until every part of it
   * has landed.
   *
   * One pass is three answers arriving at three different times: the scan, the
   * names, and the whole-file comparison. Drawing the list as they arrived meant
   * it grew, reordered, and *changed its mind* in front of the reader — the
   * comparison landing turns "combine these two" into "clean this one", so a
   * card that had been readable for a second became a different card with a
   * different button in the same place.
   *
   * So nothing is drawn until the whole pass is in, and then all of it at once.
   * A list that appears late is worth far more than one that appears early and
   * then argues with itself.
   */
  let checking = $state(true);


  /**
   * What the pass is doing, for the working screen.
   *
   * Named steps rather than one spinner, because the second one decompiles
   * every whole-file override against the game's own copy and is much the
   * slower of the two: a reader who can see which step is running can tell a
   * big library from a stuck app.
   *
   * Read off `weighing` alone rather than off both flags. `scanning` spans the
   * whole pass, so it overlaps the comparison; and it is still false for the
   * first few hundred milliseconds after start-up, while the install is being
   * found — which had this screen ticking both steps as finished before either
   * had begun. This is only ever drawn while `checking`, so "neither has
   * finished" is the only honest reading of "not weighing yet".
   */
  const stages = $derived([
    { name: "Reading the mods folder", at: weighing ? "done" : "now" },
    {
      name: "Comparing whole-file mods against the game",
      at: weighing ? "now" : "next",
    },
  ]);

  // Actions are addressed to a person, so they are phrased with the names a
  // person recognises -- while everything the buttons *act on* stays a folder.
  //
  // `library.mended` is read rather than fetched: the clean preview cannot
  // supply mended mods, because it plans prunes and a mend is not one. Without
  // them a mended mod appeared here nowhere at all -- the Library said it was
  // running a mended build and offered the way back, while this list, which is
  // where every other fix of ours is listed and undone, had never heard of it.
  const actions = $derived(
    buildActions(report, plans, (owner) => names.of(owner), library.mended),
  );
  // What is still being asked for, and what has already been done. An undo is
  // not work outstanding: it is the way back out of a fix, and it belongs under
  // its own heading rather than under one promising to make the library tidier.
  const asked = $derived(pending(actions));
  const undoable = $derived(reversible(actions));
  // Grouped under headings that say what the group *means*, so "critical" and
  // "optional" are words on the screen rather than two shades of a border.
  const groups = $derived(banded(asked));
  const critical = $derived(asked.filter((a) => bandOf(a.urgency) === "critical"));
  const todo = $derived(asked.filter((a) => a.urgency !== "tidy"));
  const tidy = $derived(asked.filter((a) => a.urgency === "tidy"));

  const conflicts = $derived(actionable(report));
  const drift = $derived(seriousDrift(report));
  const informational = $derived(
    (report.drift ?? []).filter((d) => d.severity === "INFO"),
  );
  const locClashes = $derived(report.loc_clashes ?? []);

  // A run that could not check is not a run that found nothing.
  const unchecked = $derived(
    report.tools && !report.tools.scene_check ? report.tools.scene_check_note : null,
  );

  // Everything the scan noticed that asks nothing of anybody. Built as rows
  // rather than written out as markup so a count of zero simply is not a row:
  // a list of zeroes is the dashboard this app was specifically not going to
  // be.
  const alsoSeen = $derived(
    [
      { count: report.merged_count, what: "overlaps the game merges cleanly" },
      {
        count: report.stale.length,
        what: "built for an older game version, usually fine",
      },
      {
        count: informational.length,
        what: "scene files that reposition geometry on purpose",
      },
      { count: report.disabled.length, what: "mods switched off, and not analysed" },
      {
        count: report.unregistered.length,
        what: "folders the game has not registered yet",
      },
      {
        count: locClashes.length,
        what: "shared text ids — one wins, the others never appear",
      },
    ].filter((row) => row.count > 0),
  );

  let installs = $state<Install[]>([]);
  let detected = $state<Install | null>(null);
  let chosen = $state<Install | null>(null);
  /** What the user has chosen in Settings. Read once, on start-up. */
  let prefs = $state<Settings>(blankSettings());

  /**
   * Where everything actually is, which is not the same as what was chosen.
   *
   * Read here only for the Play button: `Settings.game_exe` is null whenever
   * the exe was *detected* rather than picked, which is the ordinary case, so
   * a button gated on the setting would be greyed out on almost every machine.
   * `settingsResolve` is what folds detection and choice into one answer, and
   * it is what the Settings pane has always gated its own Play button on.
   */
  let resolved = $state<Resolved | null>(null);

  /**
   * Launching the game.
   *
   * Up here rather than in Settings because this is the one verb in the app
   * that is not about mods: it is what you do *after* everything else, and it
   * was reachable only by opening Settings and finding it beside the heading —
   * three clicks from the screen you were actually on, in the pane you visit
   * least. It now lives in the window chrome, which is on screen whatever tab
   * you are reading.
   */
  let launching = $state(false);

  async function play() {
    launching = true;
    try {
      const exe = await launchGame();
      // Through `note`, which is the strip that already exists for things that
      // are worth saying once: the game takes a few seconds to appear, so
      // "Started NMS.exe" earns a moment and nothing after it. It expires and
      // can be dismissed, like every other notice.
      if (exe) note(`Started ${exe.split(/[\\/]/).pop()}`);
    } catch (err) {
      note(String(err));
    } finally {
      launching = false;
    }
  }

  /**
   * Four screens, where there were eight.
   *
   * The four that are left are four *questions*, not four kinds of fact:
   *
   *   Actions   what should I do right now
   *   Library   what do I have, and what is true of this one
   *   Sessions  what did the game itself make of it
   *   Settings  where is everything, and what does this program do on its own
   *
   * The four that went were not screens. Updates, Presets and Evidence each
   * held one fact about the same sixty mods the Library already listed, so
   * learning what was true of a single mod meant visiting all of them and
   * matching folder names by eye; they are now filter chips and sections in
   * [`ModRecord`], and the evidence opens over whatever sent you to it. Browse
   * is the second mode of the Library rather than a peer of it, because
   * finding a mod and managing the ones you have is one activity.
   *
   * Sessions stays separate deliberately. It is the only screen whose source is
   * not the files on disk — it is what the *game* did with them — and it is the
   * one thing here that can contradict everything else.
   */
  type Tab = "actions" | "library" | "sessions" | "settings";
  let tab = $state<Tab>("actions");

  /** Which half of the Library pane is showing. See its `mode` prop. */
  let libraryMode = $state<"installed" | "find">("installed");

  /**
   * Asks the Library to open its mod-lists sheet, where importing lives.
   *
   * The sheet belongs to [`PresetBar`], which sits in the list's header, which
   * is a snippet inside [`LibraryPane`] -- so a request is passed down rather
   * than the state being hoisted up. It is cleared once acted on, because this
   * click also *mounts* the pane it is aimed at; see the prop's own note.
   */
  let listsWanted = $state(false);

  function openLists() {
    tab = "library";
    libraryMode = "installed";
    listsWanted = true;
  }

  const NAV: { id: Tab; name: string }[] = [
    { id: "actions", name: "Actions" },
    { id: "library", name: "Library" },
    { id: "sessions", name: "Sessions" },
    { id: "settings", name: "Settings" },
  ];

  async function scan(install?: Install | null) {
    const target = install ?? chosen ?? detected;
    scanning = true;
    checking = true;
    failure = null;
    note(null);
    try {
      /**
       * What is installed, started before the scan and awaited after it.
       *
       * Before, because it does not depend on the scan — they read the same
       * folder and neither waits on the other, so starting it here costs
       * nothing and finishes it sooner.
       *
       * Outside the `if`, because the scan can come back with nothing: in a
       * plain browser `analyseLibrary` returns null and the fixtures stand in
       * for it. A library read that only ran inside the guard left the Library
       * tab reading "Reading the mods folder…" for ever under `npm run dev`,
       * which is the one place the fixtures exist to keep workable.
       */
      const reading = library.load(target?.mods_dir);
      const result = await analyseLibrary(target?.mods_dir, target?.root);
      if (!result) await reading;
      if (result) {
        const delta = scanned ? diffReports(scanned, result) : null;
        scanned = result;
        note(delta ? describeDelta(delta) : null);
        // All three awaited: the comparison rewrites the action list rather
        // than merely adding to it, a scan can bring in a mod we have no name
        // for, and the library read says which mods are on a build of ours and
        // which are switched off entirely. Together, because none of them waits
        // on the others.
        //
        // The library read is driven from here rather than from the pane that
        // draws it because a change from *outside* has to reach it: installing
        // from a Nexus link brings the Library forward before the install
        // finishes, so a pane loading on mount would read a mods folder the new
        // mod is not in yet and then never look again.
        await Promise.all([weigh(target), names.load(target?.mods_dir), reading]);

        // Which mods an update verdict can be about — after the library read,
        // because it is not simply what the scan found. A remembered verdict
        // about a mod that has since been deleted is neither listed nor
        // counted; the check is throttled to four hours and nothing
        // invalidates it when the library changes, which is what leaves those
        // rows behind.
        updates.installedNow(askable(result));
        // And ask about any of them the remembered report does not cover — a
        // mod installed since the last check, or one whose archive we could not
        // read then and can now. One request each, none when nothing is missing,
        // and never awaited: it is a refinement of a list that already reads
        // correctly.
        void updates.fill(target?.mods_dir);
      }
    } catch (err) {
      failure = String(err);
    } finally {
      scanning = false;
      checking = false;
    }
  }

  /**
   * The mods an update verdict can be about, which is not what the scan found.
   *
   * A **merge** is ours: it has no Nexus page and never will, so it is taken
   * out — otherwise it holds a permanent "could not ask" row, and `fill`, which
   * chases every installed mod the report says nothing about, would ask after
   * every scan forever.
   *
   * The mods a merge **stands in for** are put back. They are still installed;
   * they are only held out of the game, so no scan can see them. They are also
   * where an update matters most: a merge carries their edits at the version it
   * was built from, so a new version of one of them reaches the game only once
   * the merge is rebuilt.
   */
  function askable(from: Report): string[] {
    const ours = new Set(library.mergeFolders);
    const named = from.mods.map((m) => m.name).filter((name) => !ours.has(name));
    return [...new Set([...named, ...library.heldByMerge])];
  }

  /** Part of the pass, not a straggler after it. See [`checking`]. */

  async function weigh(target?: Install | null) {
    const at = target ?? chosen;
    weighing = true;
    try {
      plans = await cleanPreview(at?.mods_dir, at?.root);
    } catch {
      plans = []; // a missing tool is already reported by the scan itself
    } finally {
      weighing = false;
    }
  }

  async function run(action: Action): Promise<string> {
    switch (action.verb) {
      case "combine": {
        // `only` is present when some of the mods contesting this asset are
        // better cleaned than merged; the merge is then built from the rest.
        await mergeConflict(action.subject!, action.only, chosen?.mods_dir, chosen?.root);
        await scan();
        // Said with the real count: this used to claim "the two it was made
        // from" however many there were.
        return combinedNote(action.only ?? action.mods);
      }
      case "clean": {
        const lines = await cleanApply(action.subject!, chosen?.mods_dir, chosen?.root);
        await scan();
        return lines.join("   ·   ");
      }
      case "undo": {
        await cleanUndo(action.subject!, chosen?.mods_dir, chosen?.root);
        await scan();
        return "Back to the build its author shipped.";
      }
      case "repair": {
        const owner = action.subject!;
        // Look before writing. Whether a broken file can be mended safely is
        // decided by reading it, not by the parser's error message, so a mod
        // that cannot be mended has to say so rather than fail halfway.
        const found = await repairPreview(owner);
        if (!found || found.fixed.length === 0) {
          const why = found?.refused?.[0]?.[1];
          throw new Error(
            why
              ? `This one cannot be mended automatically: ${why}`
              : "Nothing here can be mended automatically.",
          );
        }
        const unsure = found.fixed.filter((f) => !f.fault.confident);
        await repairApply(owner, chosen?.mods_dir, chosen?.root);
        await scan();

        const what = found.fixed
          .map((f) => `${f.rel_path} (${f.fault.unclosed.join(", ")})`)
          .join("   ·   ");
        const caution = unsure.length
          ? ` Worth a look: in ${unsure.length === 1 ? "one file" : `${unsure.length} files`} the layout did not settle where the tag belonged.`
          : "";
        return `Mended ${what}. The original is untouched in staging, so this can be switched back.${caution}`;
      }
      default:
        throw new Error("nothing to run");
    }
  }

  /**
   * Clean several mods in one pass.
   *
   * One engine call and one rescan, not one of each per mod: the planning is
   * per library and every separate call would reconcile the mods folder again.
   * Twenty-five mods used to mean twenty-five rescans at about two seconds
   * apiece, on top of twenty-five clicks.
   */
  async function cleanMany(owners: string[]): Promise<string> {
    const done = await cleanApplyMany(owners, chosen?.mods_dir, chosen?.root);
    await scan();

    const cleaned = done.filter((d) => !d.failed);
    const refused = done.filter((d) => d.failed);
    const files = cleaned.reduce((sum, d) => sum + d.done.length, 0);

    let said = cleaned.length
      ? `${plural(cleaned.length, "mod")} cleaned, ${plural(files, "file")} replaced with the edits they actually make.`
      : "Nothing was cleaned.";
    if (refused.length) {
      // Named, not counted: which one was left alone is the useful part, and
      // the rest of the batch still happened.
      said += ` Left alone: ${refused.map((d) => `${names.of(d.owner)} — ${d.failed}`).join("   ·   ")}`;
    }
    return said;
  }

  // ---------------------------------------------------------------------
  // Picking several cards at once
  //
  // The Optional band has had this since it existed, because there it is
  // twenty-five rows of the same verb. The bands above it were one or two
  // findings each and a card apiece was right — until a library with several
  // contested assets made it several presses of the same button, each one
  // followed by its own rescan.
  // ---------------------------------------------------------------------

  let chosenActions = $state<Set<string>>(new Set());
  let actionAnchor: string | null = null;
  let batching = $state(false);
  let batchNote = $state<string | null>(null);
  let batchFailed = $state<string | null>(null);

  /**
   * The rows in a band one call can do together.
   *
   * Cleans only. It is the one verb with a real batch behind it — `cleanMany`
   * plans once and reconciles once — and offering a tick against a verb that
   * would simply be run in a loop would promise something this does not do.
   */
  function batchable(rows: Action[]): Action[] {
    return rows.filter((a) => a.verb === "clean" && a.subject);
  }

  /** Picked *and* still on the list: a rescan rebuilds it under the selection. */
  const pickedActions = $derived(
    asked.filter((a) => a.verb === "clean" && a.subject && chosenActions.has(a.id)),
  );

  function pickAction(rows: Action[], action: Action, event: MouseEvent) {
    const ids = batchable(rows).map((a) => a.id);
    const next = new Set(chosenActions);
    if (event.shiftKey && actionAnchor) {
      const from = ids.indexOf(actionAnchor);
      const to = ids.indexOf(action.id);
      if (from !== -1 && to !== -1) {
        const [lo, hi] = from < to ? [from, to] : [to, from];
        for (const id of ids.slice(lo, hi + 1)) next.add(id);
        chosenActions = next;
        return;
      }
    }
    if (next.has(action.id)) next.delete(action.id);
    else next.add(action.id);
    chosenActions = next;
    actionAnchor = action.id;
  }

  function pickEvery(rows: Action[], all: boolean) {
    const ids = batchable(rows).map((a) => a.id);
    const next = new Set(chosenActions);
    for (const id of ids) {
      if (all) next.add(id);
      else next.delete(id);
    }
    chosenActions = next;
  }

  async function cleanChosen() {
    if (!pickedActions.length) return;
    batching = true;
    batchNote = null;
    batchFailed = null;
    try {
      // `cleanMany` rescans once at the end, which also rebuilds this list —
      // so the selection is cleared rather than left pointing at cards that
      // have just become undos.
      batchNote = await cleanMany(pickedActions.map((a) => a.subject!));
      chosenActions = new Set();
      actionAnchor = null;
    } catch (err) {
      batchFailed = String(err);
    } finally {
      batching = false;
    }
  }

  /**
   * Show the evidence, over whatever the reader was looking at.
   *
   * It used to be a tab, and arriving at it meant leaving the list of findings
   * you were working through — which came back scrolled to the top with the
   * selection gone. Evidence is the footnote to a claim, not a destination, so
   * it opens on top and closing it puts you back exactly where you were.
   *
   * `null` is a real answer: a scene finding has evidence but no conflict, and
   * so does "show me everything contested".
   */
  let evidence = $state<{ focus: Conflict | null } | null>(null);

  function inspect(action: Action) {
    evidence = { focus: conflicts.find((c) => c.target === action.target) ?? null };
  }

  /**
   * Show one mod's record, from wherever its name was clicked.
   *
   * The Sessions pane names mods constantly and, until the record existed,
   * naming them was all anybody could do about it. Timestamped rather than
   * bare, so asking for the same mod twice is not swallowed as "no change";
   * see `wanted` on LibraryPane.
   */
  let wanted = $state<{ owner: string; at: number } | null>(null);

  function openMod(owner: string) {
    wanted = { owner, at: Date.now() };
    tab = "library";
    libraryMode = "installed";
  }

  async function pick(install: Install) {
    chosen = install;
    scanned = null; // never show one library's findings under another's path
    plans = [];
    await scan(install);
  }

  onMount(async () => {
    [prefs, resolved, detected, installs] = await Promise.all([
      settingsRead(),
      settingsResolve(),
      findInstall(),
      findInstalls(),
    ]);
    chosen = detected;
    // Outside Tauri this returns null and the fixtures stay in place.
    await scan();

    // Ask Nexus what is current, once the library is known and without making
    // anything wait for it. Throttled inside: a relaunch within a few hours
    // shows the remembered answer rather than spending sixty requests again.
    //
    // Honoured rather than assumed: this switch is in Settings, and until now
    // it was written to disk and read by nobody, so turning it off did nothing
    // at all.
    if (prefs.check_updates_on_start) void updates.check(chosen?.mods_dir, false);

    // And ask it what these mods are actually called. Unawaited for the same
    // reason, but for a different one too: every row already reads correctly
    // from the archive names, so this only sharpens what is already on screen.
    // Costs one request per page never seen before, which after the first run
    // is none.
    void names.resolve(chosen?.mods_dir);
  });

  // A download can start from a click on nexusmods.com in any window, so
  // these listeners live at the top of the app rather than in a tab that may
  // not be open. Both are torn down when the page goes away.
  onMount(() => {
    const stops: Array<() => void> = [];

    listen<string>("nxm-link", async (event) => {
      // The installed side, not the browser: the download is already under way
      // and what the user wants to see is where it is landing.
      tab = "library";
      libraryMode = "installed";
      try {
        await queue.fromLink(event.payload, chosen?.mods_dir, chosen?.root);
        await scan();
      } catch {
        // The queue already holds the reason; the strip shows it.
      }
    })
      .then((stop) => stops.push(stop))
      .catch(() => {});

    listen<InstallStage>("install-progress", (event) => queue.report(event.payload))
      .then((stop) => stops.push(stop))
      .catch(() => {});

    listen<string>("browser-navigated", (event) => {
      browser.url = event.payload;
    })
      .then((stop) => stops.push(stop))
      .catch(() => {});

    // A link that leaves Nexus opens in the real browser: this window holds a
    // signed-in session and should not follow a mod description anywhere.
    listen<string>("browser-external", async (event) => {
      const { openUrl } = await import("@tauri-apps/plugin-opener");
      await openUrl(event.payload).catch(() => {});
    })
      .then((stop) => stops.push(stop))
      .catch(() => {});

    return () => stops.forEach((stop) => stop());
  });

  // Recording a game session is watched from the top of the app for the same
  // reason: a session starts when the *game* starts, which is while the user is
  // looking at some other tab — or at the game. A tab that started listening
  // when it was opened would begin after the first thirty seconds of the run,
  // which is when the game opens every mod file it is ever going to open.
  onMount(() => {
    void sessions.begin();
    return () => void sessions.end();
  });

  /** The mods folder being read, for the status bar along the bottom. Not the
   *  `library` store, which is what is *in* it. */
  const libraryPath = $derived(
    live || !detected || sample ? (report.roots[0] ?? "") : detected.mods_dir,
  );

  /** True while the Nexus browser has the run of the window. */
  const browsing = $derived(tab === "library" && libraryMode === "find");

  // Opening a mod page from anywhere brings the Library forward, in its Find
  // mode. This reads the request counter and writes `tab`, which are different
  // pieces of state, so it settles instead of re-triggering itself.
  let servedRequest = 0;
  $effect(() => {
    if (browser.requestId !== servedRequest) {
      servedRequest = browser.requestId;
      tab = "library";
      libraryMode = "find";
    }
  });

  // The browser is a native surface floating over the window, not part of this
  // document. Leaving the Find mode unmounts the component that draws around
  // it but does nothing to the webview itself, which would otherwise sit on
  // top of whatever the user switched to. So the parking is done from here,
  // which is always mounted.
  $effect(() => {
    if (!browsing) void browser.hide();
  });
</script>

<div class="app">
  <header class="chrome">
    <div class="brand">
      <div class="glyph" aria-hidden="true">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6"
          stroke-linecap="round" stroke-linejoin="round">
          <path d="M12 3l8 3.6v5.2c0 4.4-3.3 8.3-8 9.2-4.7-.9-8-4.8-8-9.2V6.6z" />
          <path d="M9 12l2.2 2.2L15.5 10" />
        </svg>
      </div>
      <div class="name">
        <h1>Anomaly</h1>
        <p>No Man&rsquo;s Sky mod manager</p>
      </div>
    </div>

    <div class="trace" aria-hidden="true"></div>

    <!-- The chrome used to carry three readouts: Library, Found via, and Needs
         you. All three are gone, and none of them took anything with it.

         **Library** was the last two segments of the mods folder — a path you
         set once and then never think about, printed in full in the status bar
         along the bottom of the window anyway.

         **Found via** said "steam". It was also the install picker when more
         than one install exists, which is a real control and the only one of
         its kind, so it moved to Settings where the rest of the paths live
         rather than being dropped.

         **Needs you** was a count of outstanding findings — the same count the
         Actions tab carries as a pip a few pixels below it, in the same colours,
         next to the word for what it counts. A number on the title bar that is
         already on the nav is not a second opinion, it is the same opinion
         twice.

         What is left is the one thing a title bar is good for: the verb you
         reach for from any screen. -->

    <!-- The last thing you do, and so the last thing on the bar. In the chrome
         rather than in a pane because it is the one verb here that is not
         about mods: whatever tab you are reading, this is how you leave. It is
         also the only accent-filled control outside the panes, which is safe
         precisely because it is outside them -- it can never sit beside a
         pane's own primary button and argue with it about which is the action. -->
    <button
      class="play"
      onclick={play}
      disabled={launching || !resolved?.game_exe.path}
      title={resolved?.game_exe.path ?? "Set the game program in Settings"}
    >
      {#if launching}
        <span class="spin" aria-hidden="true"></span>
      {:else}
        <svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">
          <path d="M8 5.14v13.72a1 1 0 001.52.85l11.14-6.86a1 1 0 000-1.7L9.52 4.29A1 1 0 008 5.14z" />
        </svg>
      {/if}
      <span>Play</span>
    </button>
  </header>

  {#if report.disable_all}
    <!-- Nothing below this matters if the game is ignoring every mod, so it
         goes above everything and does not scroll away. -->
    <div class="alarm">
      <b>DisableAllMods is on.</b> The game is ignoring every mod in this folder,
      so none of the findings below are happening in game. Turn it off in
      GCMODSETTINGS.MXML.
    </div>
  {/if}

  <div class="shell">
    <nav class="nav">
      {#each NAV as item}
        <button class:on={tab === item.id} onclick={() => (tab = item.id)}>
          <span>{item.name}</span>
          {#if item.id === "actions"}
            {#if checking}
              <span class="spin" aria-label="checking every mod"></span>
            {:else if todo.length}
              <span class="pip" class:alarm={critical.length}>{todo.length}</span>
            {/if}
          {:else if item.id === "library"}
            <!-- What the Updates tab's badge used to say. It stays in the nav
                 rather than moving entirely onto the Update chip inside the
                 pane, because it is the one thing about the library worth
                 knowing while you are looking at some other screen. -->
            {#if updates.checking}
              <span class="spin" aria-label="checking for updates"></span>
            {:else if updates.outdated.length}
              <span class="pip">{updates.outdated.length}</span>
            {/if}
          {:else if item.id === "sessions"}
            <!-- While the game is running this is the only place the count of
                 things it has complained about is visible, and the tab is
                 usually not the one in front. -->
            {#if sessions.state.recording}
              {#if sessions.loud}
                <span class="pip alarm">{sessions.loud}</span>
              {:else}
                <span class="spin" aria-label="recording this session"></span>
              {/if}
            {:else if sessions.latest?.crashed}
              <span class="pip alarm">!</span>
            {/if}
          {/if}
        </button>
      {/each}

      <div class="spacer"></div>

      <!-- `failure` as well as `live`: a scan that threw leaves `scanned` null,
           which used to swap this for the fixture toggle — so the one control
           that could have recovered from a wrong mods folder turned into
           "Sample conflicts" at exactly the moment it was needed. -->
      {#if live || failure}
        <button class="btn scan" onclick={() => scan()} disabled={scanning}>
          {#if scanning}<span class="spin" aria-hidden="true"></span>{/if}
          {scanning ? "Scanning…" : "Rescan library"}
        </button>
      {:else}
        <!-- Browser-only fallback; unreachable inside the app. -->
        <button class="btn scan" onclick={() => (sample = !sample)}>
          {sample ? "Your library" : "Sample conflicts"}
        </button>
      {/if}

      <section class="readoutcard">
        <header class="hud-label">Last scan</header>
        <!-- Held together with the list: these are the same pass, and a
             readout that updates a row at a time is the thing being fixed.

             A failed scan reads "—" rather than a number. Zero is an answer
             here — an empty mods folder genuinely scans to four zeroes — and a
             run that could not get that far has no business borrowing it. -->
        <dl>
          <div><dt>Mods</dt><dd class="path">{tally(report.stats.mods)}</dd></div>
          <div><dt>Assets</dt><dd class="path">{tally(report.stats.targets)}</dd></div>
          <div><dt>Files</dt><dd class="path">{tally(report.stats.files)}</dd></div>
          <div>
            <dt>Overweight</dt>
            <dd class="path">{tally(tidy.length)}</dd>
          </div>
        </dl>
      </section>
    </nav>

    <main>
      {#if changed}
        <div class="delta">
          <span>{changed}</span>
          <button class="close" onclick={() => note(null)} aria-label="Dismiss">&times;</button>
        </div>
      {/if}

      {#if tab === "actions"}
        <div class="pane">
          {#if failure}
            <!-- Inside the tab, not instead of the window.

                 A failed scan used to replace `main` entirely, so every tab
                 drew "Could not scan" and none of them drew itself. That made
                 the failure unrecoverable from inside the app for exactly the
                 cases it reports: the mods folder is wrong (Settings), the
                 game was not found (Settings), there is nothing installed yet
                 (Library › Find). Each fix was one click away and behind the
                 message telling you it was needed.

                 A scan is one tab's subject. The other three read the mods
                 folder, the session log and the settings file, and not one of
                 them needs this answer to draw. -->
            <section class="panel hero">
              <header class="hud-label">Scan failed</header>
              <h2>Could not scan.</h2>
              <p class="failed">{failure}</p>
              <p class="hint">
                Nothing else is blocked: <b>Settings</b> holds the game and mods
                folder, and <b>Library</b> still lists what is installed.
              </p>
              <div class="retry">
                <Button variant="primary" busy={scanning} onclick={() => scan()}>
                  {scanning ? "Scanning…" : "Try again"}
                </Button>
                <Button onclick={() => (tab = "settings")}>Open Settings</Button>
              </div>
            </section>
          {:else if checking}
            <!-- The whole tab, while a pass is in flight. See `checking`: the
                 alternative is a list that rewrites itself as the slower half
                 of the pass lands, which is worse than a list that is late. -->
            <section class="panel hero">
              <header class="hud-label">Recommended actions</header>
              <h2>Checking every mod…</h2>
              <p class="lede">
                Nothing is shown until all of them are done, so the list cannot
                change under you while you are reading it.
              </p>

              <ol class="steps">
                {#each stages as stage (stage.name)}
                  <li class={stage.at}>
                    {#if stage.at === "now"}
                      <span class="spin" aria-hidden="true"></span>
                    {:else if stage.at === "done"}
                      <svg class="tick" viewBox="0 0 24 24" fill="none" stroke="currentColor"
                        stroke-width="3" stroke-linecap="round" stroke-linejoin="round"
                        aria-hidden="true"><path d="M5 13l4 4L19 7" /></svg>
                    {:else}
                      <span class="pending" aria-hidden="true"></span>
                    {/if}
                    <span>{stage.name}</span>
                  </li>
                {/each}
              </ol>
            </section>
          {:else if live && !report.mods.length}
            <!-- A library with nothing in it to analyse. It scans, and it
                 scans clean; it is only that `verdict` would answer "None."
                 over "0 mods, 0 game assets checked", which is true, unhelpful,
                 and indistinguishable from a library that was checked and
                 found spotless. The two reasons a scan can see nothing want
                 different next moves, so each says its own. -->
            <section class="panel hero">
              <header class="hud-label">Recommended actions</header>
              {#if report.disabled.length}
                <h2>Every mod is switched off.</h2>
                <p class="lede">
                  {plural(report.disabled.length, "mod is", "mods are")} installed
                  and none of them is loading, so nothing can be contested. Switch
                  them back on in the Library.
                </p>
              {:else}
                <h2>No mods installed.</h2>
                <p class="lede">
                  Nothing is in the mods folder yet. Find one on Nexus, or open
                  a list another player sent you &mdash; a list names every mod
                  in their set and links each page, so an empty library is the
                  case it is most use in.
                </p>
                <p class="hint">
                  If you do have mods and this is the wrong folder,
                  <b>Settings</b> is where the game and the mods folder are set.
                </p>
              {/if}
              <div class="retry">
                <Button
                  variant="primary"
                  onclick={() => {
                    tab = "library";
                    libraryMode = report.disabled.length ? "installed" : "find";
                  }}
                >
                  {report.disabled.length ? "Open Library" : "Find mods"}
                </Button>
                <!-- Opens the sheet itself rather than pointing at the button
                     that opens it. The lists sheet is three components down,
                     so this asks for it by bumping a counter; see `openLists`
                     on [`PresetBar`]. -->
                <Button onclick={openLists}>Import a list…</Button>
                <Button onclick={() => (tab = "settings")}>Open Settings</Button>
              </div>
            </section>
          {:else}
            <section class="panel hero">
              <header class="hud-label">Recommended actions</header>
              <h2>{verdict(actions)}</h2>
              <p class="lede">{scope(report)}</p>

              {#if unchecked}
                <p class="warn">
                  Scene files were <b>not checked</b> &mdash; {unchecked}. A mod could
                  be misplacing game geometry without this run noticing.
                </p>
              {/if}
            </section>

            <!-- Three named bands rather than one flat list plus an "Optional"
                 afterthought. The order already ranked the list; the headings are
                 what make the ranking readable without knowing the colours. -->
            {#each groups as group (group.id)}
              <h3 class="band {group.id}">
                <span class="hud-label">{group.name}</span>
                <span class="note">{group.means}</span>
              </h3>
              {#if group.id === "optional"}
                <!-- Many rows, one verb. A card each would be twenty-five
                     paragraphs saying the same thing about different mods; a list
                     keeps what differs and lets them be done in one pass. -->
                <TidyList actions={group.rows} {run} runMany={cleanMany} />
              {:else}
                {@const many = batchable(group.rows)}
                {@const here = many.filter((a) => chosenActions.has(a.id))}
                {#if many.length > 1}
                  <!-- Only worth a bar when there is more than one to gather: a
                       "select all" over a single card is a second way to press
                       the button already on it. -->
                  <div class="pickbar">
                    <span class="hint">
                      {here.length
                        ? `${here.length} of ${many.length} selected`
                        : `${many.length} can be cleaned together`}
                    </span>
                    <button
                      class="btn-link"
                      onclick={() => pickEvery(group.rows, here.length !== many.length)}
                    >
                      {here.length === many.length ? "Clear" : "Select all"}
                    </button>
                    {#if here.length}
                      <Button variant="primary" busy={batching} onclick={cleanChosen}>
                        {batching ? "Cleaning…" : `Clean ${here.length}`}
                      </Button>
                    {/if}
                  </div>
                {/if}
                {#each group.rows as action (action.id)}
                  <ActionRow
                    {action}
                    {run}
                    {inspect}
                    picked={chosenActions.has(action.id)}
                    onpick={many.length > 1 && many.includes(action)
                      ? (event) => pickAction(group.rows, action, event)
                      : undefined}
                  />
                {/each}
                {#if batchNote}<p class="allclear">{batchNote}</p>{/if}
                {#if batchFailed}<p class="failed">{batchFailed}</p>{/if}
              {/if}
            {/each}

            <!-- No "still comparing…" line here any more: this branch is only
                 reached once the comparison is in, so the list below is the
                 whole list. -->
            {#if !asked.length}
              <p class="allclear">
                Nothing is contested, nothing is misplaced, and no mod is carrying
                more than it changes.
              </p>
            {/if}

            <!-- Fixes already applied. Below everything, under a heading that
                 says what they are: these used to sit inside the Optional band,
                 which promised work that would "make the library tidier" over
                 twenty-five rows of work already done, and kept that heading on
                 screen on a library with nothing left to do. -->
            {#if undoable.length}
              <h3 class="band done">
                <span class="hud-label">Already done</span>
                <span class="note">
                  builds this program made; the copy each author shipped is untouched
                </span>
              </h3>
              <TidyList actions={undoable} {run} runMany={cleanMany} />
            {/if}

            <!-- What the scan found that needs nothing doing about it. This used
                 to live in a branch of the tab chain that no tab could reach, so
                 the merged overlaps, the stale mods, the switched-off mods and
                 the shared text ids were counted by the engine and shown to
                 nobody. It belongs here: it is the answer to "so what *did* you
                 find", and it reads as a footnote because that is what it is. -->
            {#if alsoSeen.length}
              <section class="panel quietpanel">
                <header class="hud-label">Also seen</header>
                <dl class="aside">
                  {#each alsoSeen as row}
                    <div>
                      <dt class="path">{row.count}</dt>
                      <dd>{row.what}</dd>
                    </div>
                  {/each}
                </dl>

                <!-- The Settings switch that names these. It used to be saved and
                     read by nobody, so "List mods built for an older game
                     version" listed nothing however it was set. -->
                {#if prefs.show_outdated && report.stale.length}
                  <ul class="stale">
                    {#each report.stale as entry (entry.mod)}
                      <li>
                        <span class="who">{names.of(entry.mod)}</span>
                        <span class="ver path">{entry.version}</span>
                        <span class="against">against {entry.reference}</span>
                      </li>
                    {/each}
                  </ul>
                {/if}
              </section>
            {/if}

            <!-- Last on the tab, and asked for rather than run: the survey
                 decompiles two copies of every asset the library touches, and
                 the answer only changes when the game does. It sits *below* the
                 bands because it is not part of this scan's verdict -- it is a
                 different question about the same library, and one nobody needs
                 answered twice in a day. -->
            <UpdatePanel
              modsDir={chosen?.mods_dir}
              gameRoot={chosen?.root}
              onmod={openMod}
            />
          {/if}
        </div>
      {:else if tab === "library"}
        <LibraryPane
          modsDir={chosen?.mods_dir}
          gameRoot={chosen?.root}
          visible={tab === "library"}
          bind:mode={libraryMode}
          openLists={listsWanted}
          onListsOpened={() => (listsWanted = false)}
          {actions}
          {run}
          {conflicts}
          {drift}
          onevidence={(conflict) => (evidence = { focus: conflict })}
          {wanted}
          onChanged={() => scan()}
        />
      {:else if tab === "sessions"}
        <SessionsPane onmod={openMod} />
      {:else if tab === "settings"}
        <SettingsPane
          modsDir={chosen?.mods_dir}
          {installs}
          activeInstall={chosen?.root}
          onInstall={(i) => {
            const picked = installs[i];
            if (picked) void pick(picked);
          }}
          onChanged={() => scan()}
        />
      {/if}
    </main>
  </div>

  <!-- Over whatever sent you here, and outside `main` so it is the same sheet
       from the Actions list and from a mod's record. -->
  {#if evidence}
    <EvidenceSheet
      {conflicts}
      {drift}
      mods={report.mods}
      realOrder={report.real_load_order}
      {plans}
      focus={evidence.focus}
      onclose={() => (evidence = null)}
      onMerged={() => scan()}
    />
  {/if}

  <InstallStrip />

  <footer class="status">
    <span class="path" title={libraryPath}>{libraryPath}</span>
    <span class="hud-label">Anomaly {version}</span>
  </footer>
</div>

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }

  /* ---- header ---------------------------------------------------------- */

  /* The title bar reads as a piece of hardware the panes are mounted in: a
     hair lighter than the page, with its own lit lower edge. */
  .chrome {
    position: relative;
    display: flex;
    align-items: center;
    gap: 1.25rem;
    flex: none;
    padding: 0.75rem var(--pad);
    background: linear-gradient(180deg, var(--glass-2), transparent);
    border-bottom: 1px solid var(--rule);
  }

  .chrome::after {
    content: "";
    position: absolute;
    left: 0;
    right: 0;
    bottom: -1px;
    height: 1px;
    background: linear-gradient(
      90deg,
      transparent,
      rgba(53, 214, 200, 0.22) 22%,
      rgba(138, 224, 60, 0.16) 68%,
      transparent
    );
  }

  .brand {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    flex: none;
  }

  /* Launching the game. The one accent-filled control in the chrome, and the
     only one anywhere that is on screen whatever tab is open. */
  .play {
    display: flex;
    align-items: center;
    gap: 0.4375rem;
    flex: none;
    padding: 0.5rem 1.125rem;
    border: 1px solid var(--accent);
    border-radius: var(--r-md);
    background: var(--accent);
    color: var(--accent-ink);
    font-size: var(--t-small);
    font-weight: 600;
    letter-spacing: var(--track-tight);
    box-shadow: var(--accent-glow);
    transition:
      box-shadow var(--quick) var(--ease),
      filter var(--quick) var(--ease);
  }

  .play svg {
    width: 0.8125rem;
    height: 0.8125rem;
  }

  .play:hover:not(:disabled) {
    box-shadow: var(--lift-accent);
    filter: brightness(1.06);
  }

  /* No exe yet. Kept in place rather than hidden: the button going missing
     would read as the feature not existing, and the title says where to set
     it. */
  .play:disabled {
    border-color: var(--rule);
    background: none;
    color: var(--faint);
    box-shadow: none;
  }

  .glyph {
    display: grid;
    place-items: center;
    width: 2.375rem;
    height: 2.375rem;
    border: 1px solid var(--rule-hi);
    border-radius: var(--r-md);
    background: var(--cyan-wash);
    color: var(--cyan);
    box-shadow: var(--lip), 0 0 16px rgba(53, 214, 200, 0.16);
  }

  .glyph svg {
    width: 1.25rem;
    height: 1.25rem;
  }

  .name h1 {
    margin: 0;
    font-size: var(--t-lead);
    font-weight: 700;
    letter-spacing: -0.015em;
    color: var(--cyan);
  }

  .name p {
    margin: -0.125rem 0 0;
    font-size: var(--t-micro);
    color: var(--faint);
  }

  /* The thin diagonal filament across the header. Decoration, and the only
     piece of it in the app -- it earns its place by making the bar read as
     an instrument rather than a title bar. */
  .trace {
    flex: 1;
    height: 2.375rem;
    min-width: 0;
    background:
      linear-gradient(
          to right,
          transparent 10%,
          rgba(53, 214, 200, 0.22) 60%,
          transparent
        )
        0 40% / 100% 1px no-repeat,
      linear-gradient(
          to right,
          transparent 35%,
          rgba(138, 224, 60, 0.16) 85%,
          transparent
        )
        0 68% / 100% 1px no-repeat;
  }

  .alarm {
    flex: none;
    padding: 0.625rem var(--pad);
    background: var(--contest-wash);
    border-bottom: 1px solid rgba(255, 95, 86, 0.4);
    font-size: var(--t-small);
  }

  /* ---- shell ----------------------------------------------------------- */

  .shell {
    display: flex;
    flex: 1;
    min-height: 0;
  }

  .nav {
    display: flex;
    flex-direction: column;
    gap: 0.375rem;
    flex: none;
    width: var(--nav-w);
    padding: var(--pad) 0.875rem;
    border-right: 1px solid var(--rule);
  }

  .nav > button {
    position: relative;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    padding: 0.5625rem 0.875rem;
    border: 1px solid transparent;
    border-radius: var(--r-md);
    font-size: var(--t-small);
    font-weight: 500;
    color: var(--dim);
    text-align: left;
    transition:
      background var(--quick) var(--ease),
      border-color var(--quick) var(--ease),
      color var(--quick) var(--ease);
  }

  .nav > button:hover {
    color: var(--readout);
    background: var(--glass-2);
  }

  /* The active item is marked twice over: a lit left edge and a raised
     surface. Position and elevation carry the state, so it does not depend on
     noticing one more shade of dark. */
  .nav > button.on {
    color: var(--readout);
    background: var(--glass-hi);
    border-color: var(--rule);
    box-shadow: var(--cast-near), var(--lip);
  }

  .nav > button.on::before {
    content: "";
    position: absolute;
    left: -0.875rem;
    top: 50%;
    width: 2px;
    height: 1.125rem;
    margin-top: -0.5625rem;
    border-radius: 0 2px 2px 0;
    background: var(--accent);
    box-shadow: 0 0 10px rgba(138, 224, 60, 0.55);
  }

  .pip {
    min-width: 1.25rem;
    padding: 0 0.3125rem;
    border-radius: var(--r-pill);
    font-family: var(--font-value);
    font-size: 0.6875rem;
    line-height: 1.25rem;
    text-align: center;
    color: var(--void);
    background: var(--signal);
  }

  /* Same count, louder, when any of it is critical. */
  .pip.alarm {
    background: var(--contest);
  }

  .spacer {
    flex: 1;
  }

  /* The one control wearing the structure colour rather than the action one.
     It is deliberate: rescanning reads the library again, it does not change
     anything, so it must not look like the buttons that do. */
  .scan {
    border-color: var(--rule-hi);
    color: var(--cyan);
    background: linear-gradient(180deg, rgba(53, 214, 200, 0.14), var(--cyan-wash));
    box-shadow: var(--elev-1), var(--lip);
  }

  .scan:hover:not(:disabled) {
    background: rgba(53, 214, 200, 0.2);
    box-shadow: var(--lift-cyan), var(--lip);
  }

  /* The instrument readout in the corner: what the last scan actually saw. */
  .readoutcard {
    margin-top: 0.75rem;
    padding: 0.75rem 0.875rem 0.5rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-md);
    background: var(--hull);
    box-shadow: var(--lip);
  }

  .readoutcard dl {
    margin: 0.5rem 0 0;
  }

  .readoutcard div {
    display: flex;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.25rem 0;
    border-top: 1px solid var(--rule);
  }

  .readoutcard dt {
    font-size: var(--t-micro);
    color: var(--faint);
  }

  .readoutcard dd {
    margin: 0;
    font-size: var(--t-micro);
    color: var(--readout);
  }

  /* ---- panes ----------------------------------------------------------- */

  main {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
  }

  .delta {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    flex: none;
    padding: 0.4375rem var(--pad);
    border-bottom: 1px solid var(--rule);
    font-size: var(--t-micro);
    color: var(--signal);
  }

  .delta .close {
    flex: none;
    padding: 0 0.25rem;
    border: 0;
    background: none;
    color: inherit;
    opacity: 0.6;
    font-size: var(--t-small);
    line-height: 1;
    cursor: pointer;
  }

  .delta .close:hover {
    opacity: 1;
  }

  /* A panel is a translucent film over the page, not a lighter grey box, so
     stacking reads as height. The top-edge highlight is the light catching
     its lip -- it is what separates it from the panel behind it far more
     cheaply than a stronger border would. */
  /* A footnote, not a finding: no cast shadow, so it sits flat on the page
     while every panel above it is lifted off it. */
  .quietpanel {
    margin-top: 1.75rem;
    box-shadow: var(--lip);
  }

  .hero h2 {
    margin: 0;
    font-size: var(--t-verdict);
    font-weight: 700;
    letter-spacing: -0.025em;
    line-height: 1.15;
  }

  /* The way out of a hero that has no list under it: a failed scan, or a
     library with nothing in it. Both say what happened and then hand over the
     one or two screens that can change it. */
  .retry {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
    margin-top: 1rem;
  }

  h2 {
    margin: 0;
    font-size: var(--t-head);
    font-weight: 600;
    letter-spacing: -0.02em;
  }

  /* The band heading is the only place the severity is stated in words, so it
     carries the band's colour on its label and nothing else does. */
  .band {
    display: flex;
    align-items: baseline;
    gap: 0.625rem;
    margin: 1.75rem 0 0.625rem;
  }

  .band:first-of-type {
    margin-top: 1.25rem;
  }

  .band .note {
    font-size: var(--t-micro);
    font-weight: 400;
    color: var(--faint);
  }

  .band.critical .hud-label {
    color: var(--contest);
  }

  .band.recommended .hud-label {
    color: var(--signal);
  }

  .band.optional .hud-label {
    color: var(--cyan-dim);
  }

  /* Not one of the three severities: this heading is over things that are
     finished, so it takes the accent that means "done" everywhere else rather
     than a step on a ramp it is not on. */
  .band.done .hud-label {
    color: var(--accent);
  }

  /* The working screen's two steps. A short ladder, not a progress bar: the
     pass has no percentage to report, and two named lines say more about what
     is taking the time than a bar that guesses would. */
  .steps {
    margin: 0.875rem 0 0;
    padding: 0;
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 0.4375rem;
  }

  .steps li {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    font-size: var(--t-small);
    color: var(--faint);
  }

  /* The one that is running is the only one lit; the one that is finished
     stays legible but stops asking for attention. */
  .steps li.now {
    color: var(--cyan);
  }

  .steps li.done {
    color: var(--dim);
  }

  .steps .tick {
    flex: none;
    width: 0.7rem;
    height: 0.7rem;
    color: var(--accent);
  }

  /* A step that has not started: the same footprint as the spinner and the
     tick, so the text of the three lines stays on one left edge. */
  .steps .pending {
    flex: none;
    width: 0.7rem;
    height: 0.7rem;
    border-radius: 50%;
    border: 1.5px solid var(--rule-hi);
  }

  /* The same shape as TidyList's head: a count, the way to take all of them,
     and the verb. Above the cards rather than pinned to the bottom, because a
     band is two or three cards rather than twenty-five and the bar would
     otherwise float over nothing. */
  .pickbar {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    margin: 0 0 0.625rem;
    padding: 0.375rem 0.25rem 0.375rem 0.5rem;
  }

  .pickbar .hint {
    margin: 0;
    flex: 1;
  }

  .allclear {
    margin: 1.25rem 0 0;
    font-size: var(--t-small);
    color: var(--faint);
  }

  .aside {
    margin: 0;
  }

  .aside div {
    display: flex;
    align-items: baseline;
    gap: 0.875rem;
    padding: 0.4375rem 0.25rem;
    border-top: 1px solid var(--rule);
  }

  .aside dt {
    min-width: 2.5rem;
    text-align: right;
    color: var(--cyan);
  }

  .aside dd {
    margin: 0;
    color: var(--dim);
    font-size: var(--t-small);
  }

  /* Opt-in, and deliberately the quietest thing on the screen: these mods are
     almost always fine, which is why the list is off by default. */
  .stale {
    list-style: none;
    margin: 0.75rem 0 0;
    padding: 0.5rem 0 0;
    border-top: 1px solid var(--rule);
    font-size: var(--t-micro);
  }

  .stale li {
    display: flex;
    align-items: baseline;
    gap: 0.625rem;
    padding: 0.1875rem 0.25rem;
  }

  .stale .who {
    color: var(--dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .stale .ver {
    margin-left: auto;
    color: var(--readout);
  }

  .stale .against {
    color: var(--faint);
  }

  /* ---- status bar ------------------------------------------------------ */

  .status {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    flex: none;
    padding: 0.375rem var(--pad);
    border-top: 1px solid var(--rule);
    background: var(--hull);
    font-size: var(--t-micro);
    color: var(--faint);
  }

  .status .path {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    direction: rtl;
    text-align: left;
  }

  /* In the nav it means "this is working", not "wait for me": quiet, and it
     never moves the label, so a check finishing does not shift the layout. */
  .nav .spin {
    color: var(--cyan-dim);
  }
</style>
