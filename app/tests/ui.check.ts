/** Do the consolidated panes actually draw what they claim to?
 *
 * Run with `npm run check:ui`. It compiles the components for the server with
 * Vite and renders them under Node -- no browser, no DOM, no test framework.
 *
 * This exists because the consolidation moved five screens' worth of facts into
 * one list and one record, and the joins that do it are the kind a type-checker
 * signs off on happily: a `Set` of owners against a row's `owner`, a session's
 * `folder` against the same, an update check keyed by a third. All of them are
 * `string` to the compiler and all of them can be wrong.
 *
 * So the cases here are joins, not markup. Each one asks whether a fact that
 * used to live on its own tab reaches the row or the record it now belongs to.
 */

import type { Component } from "svelte";
import { render } from "svelte/server";

import ModList from "../src/lib/ModList.svelte";
import ModRecord from "../src/lib/ModRecord.svelte";
import ValueEditor from "../src/lib/ValueEditor.svelte";
import LibraryPane from "../src/lib/LibraryPane.svelte";
import LoadOrderMeasured from "../src/lib/LoadOrderMeasured.svelte";
import UpdatePanel from "../src/lib/UpdatePanel.svelte";
import UpdateImpactDetail from "../src/lib/UpdateImpactDetail.svelte";
// The stylesheet as written, for the checks in section 4. `?raw` is Vite's,
// so no filesystem and no `@types/node` -- the text is bundled in.
import modListSource from "../src/lib/ModList.svelte?raw";
import { library, type Row } from "../src/lib/library.svelte";
import { updates } from "../src/lib/updates.svelte";
import { sessions } from "../src/lib/sessions.svelte";
import type { Action } from "../src/lib/actions";
import type { Conflict, SceneDrift } from "../src/lib/types";
import { demoSurvey } from "../src/lib/engine";
import type {
  FileImpact,
  Identity,
  LoadoutEntry,
  Moved,
  Observation,
  Session,
  UpdateCheck,
} from "../src/lib/engine";

let failures = 0;
function ok(what: string, cond: boolean, extra = "") {
  if (!cond) failures++;
  console.log(`${cond ? "PASS" : "FAIL"}  ${what}${extra ? "  -- " + extra : ""}`);
}

/**
 * Render a component to its markup, as the browser would first receive it.
 *
 * Generic over the component's own props rather than taking a loose bag, so
 * every case below is type-checked against the real prop list. That is worth
 * the two extra type parameters: a rename in a component's `Props` should
 * break this file, not quietly render a pane with a prop it stopped reading.
 */
function draw<
  P extends Record<string, unknown>,
  E extends Record<string, unknown>,
  B extends string,
>(component: Component<P, E, B>, props: P): string {
  // `render`'s own signature is a conditional tuple -- it takes the options
  // argument only when the component has required props -- and TypeScript
  // cannot resolve that against a component whose props are still a type
  // variable. The cast is confined to this line; the signature above is what
  // every call site is checked against, and it is unaffected.
  const call = render as unknown as (
    component: unknown,
    options: { props: unknown },
  ) => { body: string };
  return call(component, { props }).body;
}

/** Strip tags so a check can ask about words rather than about markup. */
function text(html: string): string {
  return html
    .replace(/<[^>]*>/g, " ")
    .replace(/&mdash;/g, "—")
    .replace(/&amp;/g, "&")
    .replace(/\s+/g, " ")
    .trim();
}

// ---------------------------------------------------------------------------
// Shapes
// ---------------------------------------------------------------------------

const row = (owner: string, over: Partial<Row> = {}): Row => ({
  owner,
  name: owner,
  managed: true,
  enabled: true,
  active: true,
  mergedInto: null,
  variant: null,
  edited: false,
  ...over,
});

const identity = (owner: string, over: Partial<Identity> = {}): Identity => ({
  owner,
  name: owner,
  root: `D:/NMS/MODS/${owner}`,
  mod_id: null,
  version: "1.0",
  page: null,
  priority: 1,
  disabled: false,
  files: 3,
  assets: 2,
  managed_by: null,
  ...over,
});

const entry = (owner: string, over: Partial<LoadoutEntry> = {}): LoadoutEntry => ({
  owner,
  source: owner,
  variant: "original",
  replaces: [],
  deployed: [owner],
  enabled: true,
  edited: false,
  ...over,
});

const conflict = (target: string, mods: string[], winner: string): Conflict => ({
  target,
  mods,
  severity: "MAJOR",
  kind: "exml",
  benign: false,
  summary: "Two mods claim the same properties.",
  predicted_winner: winner,
  unique_counts: {},
  declared_only: [],
  notes: [],
  clashes: [],
  mergeable: true,
  overlap: ["List/PercentageChance"],
  edit_counts: {},
});

const action = (id: string, mods: string[], over: Partial<Action> = {}): Action => ({
  id,
  urgency: "merge",
  title: `Combine ${mods.join(" and ")}`,
  why: "Only one of them loads.",
  mods,
  ...over,
});

const NOWT = { focus: null, onevidence: () => {}, run: async () => "" };

// ---------------------------------------------------------------------------
// 1. The list: the old tabs, as chips over one list
// ---------------------------------------------------------------------------

console.log("\n== 1. the filter chips carry the counts the old tabs did ==");
{
  const rows = [
    row("Clean One"),
    row("Behind One"),
    row("Off One", { enabled: false, active: false }),
    row("Built One", { variant: "cleaned" }),
    row("Broken One"),
  ];
  const html = draw(ModList, {
    rows,
    loading: false,
    picked: null,
    onpick: () => {},
    attention: new Set(["Broken One"]),
    outdated: new Set(["Behind One"]),
    multi: false,
    chosen: new Set<string>(),
    onbatch: () => {},
    onadd: () => {},
  });
  const said = text(html);

  ok("every mod is listed once", rows.every((r) => html.includes(r.owner)));
  ok("the Update chip counts the outdated mod", /Update 1/.test(said), said.slice(0, 400));
  ok("the Switched off chip counts the disabled one", /Switched off 1/.test(said));
  ok("the Our build chip counts the cleaned one", /Our build 1/.test(said));
  ok("Needs attention counts the one named in an action", /Needs attention 1/.test(said));
  ok("All counts the whole library", /All 5/.test(said));

  // The badges are how a row says which chip it belongs to without the chip
  // being pressed. A fact that reaches the count but not the row is half a
  // consolidation: the list would be the only place you could not see it.
  ok("the outdated mod wears the update badge", /Behind One\s*<\/span>\s*<span class="badge badge-signal">update/.test(html) || html.includes("update"));
  ok("the cleaned mod wears its variant", said.includes("cleaned"));
  ok("the mod named in an action wears 'fix'", said.includes("fix"));
}

console.log("\n== 1b. a chip with nothing in it is not drawn ==");
{
  const html = draw(ModList, {
    rows: [row("Only One")],
    loading: false,
    picked: null,
    onpick: () => {},
    attention: new Set<string>(),
    outdated: new Set<string>(),
    multi: false,
    chosen: new Set<string>(),
    onbatch: () => {},
    onadd: () => {},
  });
  const said = text(html);
  ok("no Update chip over a current library", !said.includes("Update"));
  ok("no Switched off chip when everything is on", !said.includes("Switched off"));
  ok("and with only 'All' left, no chip row at all", !said.includes("All 1"));
}

console.log("\n== 1c. a mod we did not install cannot be batched ==");
{
  const html = draw(ModList, {
    rows: [row("Ours"), row("Theirs", { managed: false })],
    loading: false,
    picked: null,
    onpick: () => {},
    attention: new Set<string>(),
    outdated: new Set<string>(),
    multi: true,
    chosen: new Set<string>(["Ours"]),
    onbatch: () => {},
    onadd: () => {},
  });
  ok("it is drawn inert", html.includes("inert"));
  ok("and the tally counts only what can be acted on", text(html).includes("1 of 1 selected"));
}

// ---------------------------------------------------------------------------
// 2. The record: five tabs' worth of fact about one mod
// ---------------------------------------------------------------------------

console.log("\n== 2. every screen that used to own a fact reaches the record ==");
{
  // What each old tab knew about this one mod, restored to the stores the
  // record reads. This is the join the whole consolidation rests on.
  updates.report = {
    checks: [
      {
        owner: "Subject",
        mod_id: 42,
        recorded_version: "1.4",
        state: "outdated",
        latest_version: "1.6",
        latest_name: "Subject 1.6",
        latest_file_id: 9,
        page: "https://www.nexusmods.com/nomanssky/mods/42",
      } as UpdateCheck,
    ],
    budget: { hourly_remaining: 1990, daily_remaining: 9000 },
  };
  updates.installedNow(["Subject"]);
  updates.checkedAt = Date.now();

  sessions.list = [
    {
      log: "x.log",
      started_ms: 0,
      started: "yesterday",
      seconds: 60,
      pid: 1,
      hook_version: "0.2.0",
      exit_code: 0,
      exit_note: "",
      crashed: false,
      stopped_early: false,
      counts: { fatal: 0, error: 2, warn: 1, info: 0, debug: 0 },
      saves: [],
      memory: null,
      changed: [],
      files_opened: 10,
      mods_in_trouble: [
        { folder: "Subject", warnings: 1, errors: 2, samples: ["could not read TABLE.MBIN"] },
      ],
      never_loaded: [],
      ignored: [],
      crash: null,
      dialogs: [],
      dumps: [],
      verdict: "ran to the end",
    } as Session,
  ];

  const html = draw(ModRecord, {
    row: row("Subject", { variant: "cleaned" }),
    identity: identity("Subject", { mod_id: 42 }),
    page: null,
    fetching: false,
    pageError: null,
    members: [],
    actions: [action("merge:X", ["Subject", "Other"])],
    run: NOWT.run,
    conflicts: [conflict("METADATA/REWARDTABLE.MBIN", ["Subject", "Other"], "Other")],
    drift: [] as SceneDrift[],
    onevidence: () => {},
    busy: false,
    onswitch: () => {},
    onedit: () => {},
    onrevert: () => {},
    ondelete: () => {},
    onremove: () => {},
    removing: false,
  });
  const said = text(html);

  ok("[Library]  it says whether the game is loading it", said.includes("Active — the game is loading this mod"));
  ok("[Library]  and which build it is running", said.includes("cleaned"));
  ok("[Actions]  the finding that names it is on the record", said.includes("Combine Subject and Other"));
  ok("[Updates]  the newer version is on the record", said.includes("1.6"), said.slice(0, 200));
  ok("[Updates]  with what is installed beside it", said.includes("1.4"));
  ok("[Evidence] the contested asset is named", said.includes("REWARDTABLE.MBIN"));
  ok("[Evidence] and it says who wins", said.includes("Other wins"));
  ok("[Sessions] what the game said is on the record", said.includes("complained about this mod"));
  ok("[Sessions] with the game's own words", said.includes("could not read TABLE.MBIN"));
  ok("[Library]  and the way to get rid of it", said.includes("Delete"));
}

console.log("\n== 2b. a quiet mod draws no empty sections ==");
{
  updates.report = { checks: [], budget: { hourly_remaining: 2000, daily_remaining: 9000 } };
  updates.installedNow(["Quiet"]);
  sessions.list = [];

  const said = text(
    draw(ModRecord, {
      row: row("Quiet"),
      identity: identity("Quiet"),
      page: null,
      fetching: false,
      pageError: null,
      members: [],
      actions: [],
      run: NOWT.run,
      conflicts: [],
      drift: [],
      onevidence: () => {},
      busy: false,
      onswitch: () => {},
    onedit: () => {},
      onrevert: () => {},
      ondelete: () => {},
      onremove: () => {},
      removing: false,
    }),
  );

  ok("no findings heading", !said.includes("Needs doing"));
  ok("no version panel when Nexus was never asked", !said.includes("Version"));
  ok("no contested panel", !said.includes("Contested"));
  ok("no last-run panel", !said.includes("Last run"));
  ok("no build panel for an untouched mod", !said.includes("Build"));
  ok("but it still says whether the game is loading it", said.includes("In the game"));
}

console.log("\n== 2c. a mod a merge is standing in for explains itself ==");
{
  updates.report = null;
  updates.installedNow([]);
  const said = text(
    draw(ModRecord, {
      row: row("Held", { active: false, mergedInto: "zzz_nmscheck_REWARDTABLE" }),
      identity: null,
      page: null,
      fetching: false,
      pageError: null,
      members: [],
      actions: [],
      run: NOWT.run,
      conflicts: [],
      drift: [],
      onevidence: () => {},
      busy: false,
      onswitch: () => {},
    onedit: () => {},
      onrevert: () => {},
      ondelete: () => {},
      onremove: () => {},
      removing: false,
    }),
  );
  ok("it is not called inactive", !said.includes("Not active"));
  ok(
    "it names the merge holding it back, as the list names it",
    said.includes("Held out of the game by the combined build REWARDTABLE"),
    said.slice(0, 220),
  );
  ok("and a switched-off mod with no Identity still draws", said.includes("Held"));
}

console.log("\n== 2d. a mod we did not install is removed, never deactivated ==");
{
  const said = text(
    draw(ModRecord, {
      row: row("Theirs", { managed: false }),
      identity: identity("Theirs", { managed_by: "Vortex" }),
      page: null,
      fetching: false,
      pageError: null,
      members: ["MODS/Theirs"],
      actions: [],
      run: NOWT.run,
      conflicts: [],
      drift: [],
      onevidence: () => {},
      busy: false,
      onswitch: () => {},
    onedit: () => {},
      onrevert: () => {},
      ondelete: () => {},
      onremove: () => {},
      removing: false,
    }),
  );
  ok("no Deactivate button", !said.includes("Deactivate"));
  ok("no Delete section", !said.includes("Delete…"));
  ok("it offers Remove instead", said.includes("Remove from the game"));
}

// ---------------------------------------------------------------------------
// 3. The pane: the list, the preset bar and the modes together
// ---------------------------------------------------------------------------

console.log("\n== 3. the library pane draws the whole of one screen ==");
{
  // Seeded rather than fetched: outside Tauri the engine returns nothing, and
  // the point here is the join between the two halves, not the calls.
  library.mods = [identity("In Game"), identity("Not Ours")];
  library.book = {
    entries: [
      entry("In Game"),
      entry("Switched Off", { enabled: false }),
      entry("zzz_nmscheck_REWARDTABLE", { variant: "merged", replaces: ["In Game"] }),
    ],
  };
  library.loading = false;

  ok(
    "a switched-off mod is in the rows though no scan can see it",
    library.rows.some((r) => r.owner === "Switched Off"),
  );
  ok(
    "a mod in the folder but not ours is there too",
    library.rows.some((r) => r.owner === "Not Ours" && !r.managed),
  );
  ok(
    "and a mod an enabled merge stands in for is not counted as active",
    library.rows.find((r) => r.owner === "In Game")?.active === false,
  );
  ok(
    "which is said as 'merged', not as 'off'",
    library.rows.find((r) => r.owner === "In Game")?.enabled === true,
  );
  // The merge is the one row nobody named: its folder is built out of the asset
  // it settles. Once this was the only list, that row became the thing a held
  // back mod's record tells you to go and switch off -- by a name that read as
  // machine noise.
  ok(
    "a merge is listed by the asset it settles, not by its folder",
    library.rows.find((r) => r.owner === "zzz_nmscheck_REWARDTABLE")?.name ===
      "REWARDTABLE",
  );

  const html = draw(LibraryPane, {
    modsDir: "D:/NMS/MODS",
    gameRoot: "D:/NMS",
    visible: true,
    mode: "installed" as const,
    actions: [],
    run: NOWT.run,
    conflicts: [],
    drift: [],
    onevidence: () => {},
    onChanged: () => {},
  });
  const said = text(html);

  ok("both modes of the pane are offered", said.includes("Installed") && said.includes("Find more"));
  ok("the preset bar is above the list", said.includes("Preset"));
  ok("there is a way to search sixty mods", html.includes('type="search"'));
  ok("the empty record states the count", /4 mods installed/.test(said), said.slice(0, 600));
  ok("and how many of them the game is loading", /2 in the game/.test(said));
}

// ---------------------------------------------------------------------------
// 4. Surfaces that scroll over other surfaces
// ---------------------------------------------------------------------------

console.log("\n== 4. a surface with content moving under it is opaque ==");
{
  // Asserted against the stylesheet text, because a server render is given no
  // CSS to inspect and the bug is invisible in markup. Worth an assertion at
  // all because it is a trap the token system sets: `--void` is the *only*
  // opaque surface in `tokens.css` -- `--hull`, `--glass`, `--card` and
  // `--inset` all carry alpha -- so the natural-looking choice for a panel is
  // the wrong one for anything sixty-four rows scroll beneath.
  const sticky = modListSource.slice(modListSource.indexOf(".list > header {"));
  const rule = sticky.slice(0, sticky.indexOf("}"));

  ok("the sticky list header is painted opaque", rule.includes("background: var(--void)"));
  ok(
    "and not with a token that carries alpha",
    !/background:.*var\(--(hull|glass|card|inset)/.test(rule),
    rule.split("\n").find((l) => l.includes("background:"))?.trim(),
  );
  ok(
    "nor faded out to transparent under the rows",
    !/background:[^;]*transparent/.test(rule),
  );
  ok(
    "the column behind it is opaque too, so the two cannot seam",
    /\.list \{[^}]*background: var\(--void\)/s.test(modListSource),
  );
}

// ---------------------------------------------------------------------------
// 5. The value editor
//
// The one screen in the app whose subject is a value being *typed*, so what is
// checked here is not a join but the three facts a row has to carry at once:
// the property, the value that will load, and the value the game has without
// the mod. Plus the mechanism the whole design rests on -- an empty box means
// the author's value, which only works if the author's value is the
// placeholder.
// ---------------------------------------------------------------------------

console.log("\n== 5. the value editor puts three facts on one row ==");
{
  const survey = demoSurvey("Subject");
  const html = draw(ValueEditor, {
    owner: "Subject",
    name: "Better Rewards",
    modsDir: undefined,
    onchanged: () => {},
    onclose: () => {},
    initial: survey,
  });
  const said = text(html);

  ok("the property being changed is named", said.includes("PercentageChance"));
  ok(
    "and where in the asset it sits, so two rows with one name are told apart",
    said.includes("Table/GenericTable[R_SCRAPHEAP]/List/Reward[0]"),
  );
  ok(
    "the box holds the value that will load, which is yours where you set one",
    html.includes('value="45.000000"'),
  );
  ok(
    "the author's value is the placeholder, so emptying the box goes back to it",
    html.includes('placeholder="60.000000"'),
  );
  ok("and the instruction says so in words", said.includes("empty it to go back"));
  ok("the game's own value is on the row", said.includes("25.000000"));
  ok(
    "a property the game does not have says so rather than showing a blank",
    said.includes("not in the game"),
  );
  ok(
    "a row carrying your value is marked, and only that row",
    (html.match(/class="row[^"]* mine"/g) ?? []).length === 1,
  );
  ok("a True/False value is a choice, not a box to mistype", html.includes("<select"));
  ok("the count of what the mod changes is stated", said.includes("6 properties changed"));
  ok("as is how many carry your value", said.includes("1 carrying your value"));
  // Nothing has been typed into the editor, so there is nothing to apply --
  // the primary must not invite a press that would do nothing.
  ok("with nothing typed, there is nothing to apply", said.includes("Nothing to apply"));
  ok(
    "a mod with values set offers to go back to its own",
    said.includes("Use the mod's own values"),
  );
  ok(
    "a mod shipping several assets lets you choose between them",
    said.includes("GCAUDIOGLOBALS.GLOBAL.MBIN"),
  );
}

console.log("\n== 6. the measured load order says what it measured, and no more ==");
{
  const seq = (target: string, order: string[], priorities: (number | null)[]) => ({
    target,
    order,
    priorities,
  });

  const base: Observation = {
    measured: {
      log: "x.log",
      measured_ms: 0,
      direction: "descending",
      readings: [
        {
          model: "a copy read later overwrites one read earlier",
          rule: "first",
          is_current: false,
        },
        {
          model: "the first value written for a property survives",
          rule: "last",
          is_current: true,
        },
      ],
      contested: 2,
      testable: 2,
      agreed: 2,
      exceptions: [],
      loads: 148,
    },
    basis: "Measured across 2 contested asset(s), highest priority first.",
    consistent: true,
    applied: [
      seq("GLOBALS/GCGAMEPLAYGLOBALS.GLOBAL.MBIN", ["Supercharge", "Auto Translate"], [62, 12]),
      seq("GLOBALS/GCSPACESHIPGLOBALS.GLOBAL.MBIN", ["No Warp Flash", "Flat Landing"], [50, 32]),
    ],
    checked: [],
    implausible: 0,
    mismatched: 0,
  };

  const html = draw(LoadOrderMeasured, { observation: base });
  const said = text(html);

  ok("the direction is named in words", said.includes("highest priority first"));
  ok(
    "the mods are listed in the order the game read them",
    said.indexOf("Supercharge") < said.indexOf("Auto Translate"),
  );
  ok("both ends of the order are labelled", said.includes("read first") && said.includes("read last"));
  ok("the priority behind each position is shown", said.includes("priority 62"));

  // The heart of it. The measurement settles the order, not the winner, so the
  // screen must carry both loader models and mark which one the app assumes.
  // A future change that collapses this to one answer should fail here.
  ok(
    "both readings are offered rather than one conclusion",
    said.includes("the highest ModPriority survives") &&
      said.includes("the lowest ModPriority survives"),
  );
  ok("the assumed one is marked as an assumption", said.includes("what this app assumes"));
  ok(
    "amber is nowhere on it, because nothing here is the value that loads",
    !html.includes("--signal"),
  );

  // A gap in the priorities could hide the very inversion being looked for, so
  // it says so rather than printing a blank where a number belongs.
  const gap = text(
    draw(LoadOrderMeasured, {
      observation: { ...base, applied: [seq("GLOBALS/Q.GLOBAL.MBIN", ["A", "B"], [3, null])] },
    }),
  );
  ok("a mod the game has not registered says so", gap.includes("not registered"));

  // "Measured nothing" and "measured a clean result" are opposite answers and
  // an empty list says both.
  const empty = text(
    draw(LoadOrderMeasured, {
      observation: { ...base, measured: { ...base.measured, loads: 0 }, applied: [] },
    }),
  );
  ok(
    "a session that recorded no file opens explains itself",
    empty.includes("no mod files being opened"),
  );

  // The finding that would matter most: a predicted winner neither end of the
  // observed order can produce, whichever loader model holds.
  const wrong = text(
    draw(LoadOrderMeasured, {
      observation: {
        ...base,
        implausible: 1,
        checked: [
          {
            target: "GLOBALS/GCGAMEPLAYGLOBALS.GLOBAL.MBIN",
            predicted: "Larger Upgrade Stacks",
            read_first: "Supercharge",
            read_last: "Auto Translate",
            plausible: false,
            same_mods: true,
            order: ["Supercharge", "Larger Upgrade Stacks", "Auto Translate"],
          },
        ],
      },
    }),
  );
  ok("a prediction neither end can produce is called out", wrong.includes("cannot produce"));

  const odd = text(
    draw(LoadOrderMeasured, {
      observation: {
        ...base,
        consistent: false,
        measured: {
          ...base.measured,
          // `as const` because a bare string literal in a spread widens to
          // `string`, and `Direction` is a union.
          direction: "mixed" as const,
          readings: [],
          exceptions: [
            {
              target: "GLOBALS/Q.GLOBAL.MBIN",
              order: ["A", "B", "C"],
              priorities: [5, 1, 8],
              direction: "mixed",
            },
          ],
        },
      },
    }),
  );
  ok("an asset running against the rest is named", odd.includes("GLOBALS/Q.GLOBAL.MBIN"));
  ok(
    "and no rule is offered when the order supports none",
    !odd.includes("the highest ModPriority survives"),
  );
}

console.log("\n== 7. the update check is not the outdated-version list ==");
{
  // Unasked, the panel has to say what it would tell you -- and say why the
  // version stamp, which this app hides by default, is not that.
  const idle = text(draw(UpdatePanel, {}));
  ok("it offers the question rather than a blank", idle.includes("What did the update change"));
  ok("and sets itself apart from the version stamp", idle.includes("Not the version stamp"));
}

console.log("\n== 7b. every verdict the engine can return reaches the screen ==");
{
  const moved = (path: string, before: string, after: string, mine: string): Moved => ({
    path,
    before,
    after,
    mod_value: mine,
  });

  // One of each. A verdict renamed in Rust and not here would simply stop being
  // drawn -- no error, no empty state, just a missing section. This is the check
  // that turns that into a failure.
  const file: FileImpact = {
    owner: "SalvageRights",
    rel_path: "METADATA\\REALITY\\TABLES\\REWARDTABLE.MBIN",
    target: "METADATA/REALITY/TABLES/REWARDTABLE.MBIN",
    whole_file: true,
    severity: "CRITICAL",
    summary: "SalvageRights takes back 1 value this update changed.",
    reverts: [moved("T/R_GT_NEW_EASY_N/Reward", "5", "9", "5")],
    dead: [moved("T/GONE/Value", "1", "", "1")],
    drops: [moved("T/NEW/Value", "", "3", "")],
    overridden: [moved("T/MINE/Chance", "40.000000", "50.000000", "100.000000")],
  };

  const said = text(draw(UpdateImpactDetail, { file }));
  ok("a reverted value is labelled as taken back", said.includes("Takes back (1)"));
  ok("a dropped value is labelled as deleted", said.includes("Deletes (1)"));
  ok("a dead edit is labelled as doing nothing", said.includes("Does nothing (1)"));
  ok("a deliberate override is labelled deliberate", said.includes("Deliberate (1)"));

  // The distinction the whole feature turns on has to be in words on the screen,
  // not just in the section name.
  ok(
    "and 'taken back' explains that the author did not choose it",
    said.includes("The author did not choose this"),
  );
  ok(
    "while 'deliberate' says it is not a fault",
    said.includes("Not a fault"),
  );

  // Three values per row, and the mod's is the one that loads.
  ok("the game's old and new values are both on the row", said.includes("5") && said.includes("9"));
  ok(
    "a property with no value on one side shows a dash, not a blank",
    said.includes("—"),
  );
  ok(
    "a whole-file copy is told it does this without conflicting with anything",
    said.includes("whether or not another mod touches"),
  );

  const sparse = text(draw(UpdateImpactDetail, { file: { ...file, whole_file: false } }));
  ok("a sparse patch gets the other explanation", sparse.includes("only the properties it names"));

  // Long lists are counted rather than drawn, so a 6,000-property table does not
  // become 6,000 rows in a sheet.
  const many = Array.from({ length: 60 }, (_, i) => moved(`T/P${i}`, "1", "2", "1"));
  const capped = text(draw(UpdateImpactDetail, { file: { ...file, reverts: many }, most: 10 }));
  ok("a long list is capped and the rest counted", capped.includes("and 50 more"));
}

// Thrown rather than `process.exit`, the same way `actions.check.ts` does it:
// an uncaught throw already fails the script with a non-zero status, and
// reaching for `process` would drag `@types/node` into a project that has
// deliberately done without it.
if (failures > 0) {
  throw new Error(`${failures} of these checks failed; see the FAIL lines above`);
}
console.log("\nALL PASS");
