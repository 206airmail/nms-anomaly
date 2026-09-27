# Anomaly — No Man's Sky mod manager

Tauri 2 + Svelte 5 + TypeScript front end, and the Rust engine behind it.

Installs, switches, cleans, merges and mends mods; records what the game does
with them; keeps copies of your save. Conflict analysis is the core, not the
whole job — which is why it is no longer called `nmscheck`.

```
npm install
npm run tauri dev      # dev window with hot reload
npm run tauri build    # installer + standalone .exe

npm run check          # svelte-check / TypeScript
npm run check:actions  # runs actions.ts as a program; see tests/
npm run check:ui       # renders the panes under Node; see tests/
cd src-tauri && cargo test
```

Everything the program knows about this machine — settings, `loadout.json`,
resolved mod names, session logs, save copies — lives in
`%APPDATA%\NMS Anomaly`. That path is set by `app_data` in `src-tauri/src/lib.rs`
and deliberately *not* by Tauri's `app_data_dir()`, which builds one out of the
bundle identifier and so produced `com.nsbro.nmscheck` — the right shape for an
identifier and the wrong one for a folder a person has to open.

## Where the data comes from

`src-tauri/src/engine/` **is** the engine. `engine::report::to_json` is the
contract the front end reads, served by the `analyse_library` command. The
Python in `../nmscc/` is kept only as a cross-check —
`../tools/compare_engines.py` diffs the two on a real library, so a mistake in
the Rust shows up as a failing check rather than as a wrong answer on screen.

Two JSON fixtures are the **browser-only fallback**: `npm run dev` without Tauri
cannot call the engine, and the UI still has to be workable there.

| file | from | why |
|---|---|---|
| `src/lib/fixture-report.json` | `python nmscheck.py --json app/src/lib/fixture-report.json` | the real library |
| `src/lib/demo-report.json` | `python ../tools/build_demo_fixture.py` | synthetic library, real engine |

The second exists because the real library is *healthy*: every overlap merges
cleanly, so every `clashes` list is empty and the claim stack — the centre of
the interface — has nothing to draw. The demo fixture invents the mods but
computes every number with the real analysis code, including a real
`GCMODSETTINGS.MXML` so winners come from actual ModPriority values.

The **Sample conflicts** button in the header appears only outside Tauri, where
it swaps between the two. Inside the app `scanned` always wins and neither
fixture is read.

## The design in one paragraph

Two facts about the data drive everything. The common state is *no conflicts*,
so the main screen is a verdict rather than a dashboard of zeroes. And a
conflict here is not a git diff — it is N-way, and one side **wins** by load
order. So contested values render as a **claim stack**: every mod's claim
listed, losers struck through, the survivor carrying the amber bar, while the
mods involved light up in the load-order rail so you can see *why* one won.

`src/lib/tokens.css` holds the design system. Two rules it will not bend on:

- `--signal` (amber) marks the value the game will actually load, and nothing
  else. Amber used as decoration is a bug.
- Monospace is for characters that must align — values and asset paths. Never
  for labels.

Fonts (Archivo, IBM Plex Mono — 63 KB, latin subset) are bundled in
`static/fonts/`, not fetched from Google, so the app works offline and never
flashes unstyled text.

## Four tabs, because eight was seven facts about one noun

The app has one noun — a mod — and it used to have eight tabs, each owning one
attribute of it. Learning what was true of a single mod meant visiting five of
them and matching folder names by eye; two of them listed *every* mod, twice
over, with two different controls for the same verb.

They were never eight screens. They were one list and several questions:

- **Actions** — what to do now.
- **Library** — `ModList` (the only list of mods; the old Updates, Presets and
  "switched off" tabs are filter chips over it) beside `ModRecord` (everything
  known about one mod, in the order it is asked). `PresetBar` sits above the
  list because a preset is one value, not a screen. `Installed | Find more`
  are two modes of this pane, not two tabs.
- **Sessions** — kept separate deliberately; see below.
- **Settings**.

Evidence is the exception that proves it: its unit is a contested *asset*, not
a mod, so it could not fold into the list. It is not a destination either — you
arrive at it from a card asking "why do you say that" — so `EvidenceSheet`
opens over whatever sent you, load-order rail included, and closing returns you
to your place in the list you were working through.

`library.svelte.ts` is the one store behind all of it. Three screens used to
fetch the mod list separately and they disagreed with each other.

## The Sessions tab is the one that hears from the game

Every other tab reasons about mods as files on disk. Sessions is fed by a small
DLL that runs **inside** `NMS.exe` and reports what the game actually did: which
mod files it opened, what it complained about, where it crashed. The user
installs and removes it from that tab; nothing is written into the game's folder
otherwise.

Its state is held in `sessions.svelte.ts`, outside any component, and the
listeners are wired at the top of `+page.svelte` — a session begins when the
*game* starts, which is while the user is looking at some other tab or at the
game itself. A tab that started listening when it was opened would miss the first
thirty seconds, which is when the game opens every mod file it ever opens.

It also holds the two things there that are not about mods at all: **copies of
your save**, taken as the game writes it and restorable from that tab, and the
memory trend and "what changed since your last session" that a crash is read
against.

`../docs/session-recording.md` has the whole design, including the four things
the summary is careful about and why each of them was wrong first.

### And it is the only tab that can check the engine's predictions

`engine/observed.rs`. Everything else here predicts what the game will do with a
library; the hook reports what it did. The hook logs every mod file the game
opens, in order, so grouped by asset that is the sequence the game reads a
contested asset's copies in — which is the one thing the winner prediction had
always had to assume.

Measured on the reference library: all 16 contested assets read in **strictly
descending `ModPriority`**, highest priority *first*. That is the opposite of the
order this project's own documentation claimed, and it is now corrected there.

What it does not do is name the winner, and `LoadOrderMeasured.svelte` is written
so the screen cannot imply otherwise. Reading order plus one unobserved fact —
does a later read overwrite an earlier one, or does the first write stick? — gives
the winner, and a hook watching file opens cannot see that fact. So both readings
are printed with the assumed one marked, and amber never appears on that pane:
`--signal` means "this is the value the game loads", and nothing there has earned
it. The full reasoning is in the root README under "The assumption, now half
measured".

The measurement is still worth having. The candidates for a contested asset are
now two *named* mods instead of a list, the order comes from what the game did
rather than from a file it wrote, and a predicted winner that is neither end of
the observed order is reported as a bug in the prediction. One filtering mistake
was caught this way too: `LocTable.MXML` is not a game asset, and left in the
grouping it was the only sequence disagreeing with the measured direction —
enough on its own to turn a unanimous result into "no rule found".

## Findings, and what to do about them

Two different questions, answered in two files, and the split is load-bearing:

- `src/lib/types.ts` is the engine's shape, plus load-order and claim ordering.
  It says what is *true* about the library.
- `src/lib/actions.ts` restates that as **what to do**, ranked by what it costs
  to leave alone, with the button that performs it. It is the densest piece of
  judgement in the app, so it is the one part of the front end with its own
  tests: `npm run check:actions`.

`mergePlanFor` is the decision worth reading before changing anything here. A
conflict two mods can be combined into one asset has two possible remedies —
clean the whole-file copies, or merge everybody — and picking the wrong one
silently throws away somebody's edits. Both screens that show a merge call the
same function, because when they disagreed they contradicted each other about
the same asset.

## Taking over a library, without asking another program

`engine/adopt.rs` writes down which staged folder each mod in the game came
from, so mods somebody else installed can be switched, cleaned and deleted here.
Nothing is copied or moved — it is bookkeeping.

The interesting part is how it knows. It used to read Vortex's
`vortex.deployment.json`, which was wrong twice: it assumed everyone arrives
from Vortex, and it stopped working the moment Vortex was uninstalled and took
the manifest with it — leaving a mods folder full of perfectly good hardlinks
that nothing could read.

It now reads **the links themselves**. A deployed file and its staged original
are the same file, and NTFS says so: they share a `(volume, file index)` pair
that a byte-identical copy does not. `engine/fileid.rs` asks
`GetFileInformationByHandle` for it (`MetadataExt::file_index` is still
nightly-only), and `adopt` indexes staging by it and looks up what is in the
game. That is a fact read off the disk rather than an inference, it needs no
manager installed, and it cannot be stale — it is reading the very links the
game is loading. On the 62-mod reference library it traces 54 of 64 folders in
about 70 ms.

A manifest, if one is there, is still used first — one file to read beats
walking two trees. Mods that were *copied* into the game rather than linked have
no shared identity to find, so they fall back to a name-and-size match, decided
by majority across the folder and reported as a guess rather than a fact.

## Builds of one mod

A mod this program installed exists in up to four places, and only the last is
the game's business:

```
staging/<archive>/       the mod as its author shipped it     never modified
derived/<owner>/         a cleaned, mended or merged build    rebuilt at will
derived/<owner>__edited/ the same, carrying your own values   rebuilt at will
GAMEDATA\MODS            hardlinks to whichever is chosen     what the game loads
```

Cleaning, mending and editing do not change the mod: they write a *second* build
and change which one is linked. Undoing is pointing back at the staged copy, so
there is nothing that can fail to come back. `loadout.json` in the app's data
folder is the record of which build each mod is on, and `loadout::reconcile` is
what makes the game folder match it.

## Changing a value a mod sets

`engine/edit.rs` and `ValueEditor.svelte`. Every other verb here decides *which*
copy of an asset the game reads; this one changes what is in it, and it is the
only screen whose result the user typed.

It offers exactly the properties the mod changes from vanilla — the set
`prune::changed_leaves` computes, shared with cleaning so the two can never
disagree. That bound is the honest one twice over: a property the mod ships at
the game's own value is not something the mod *does*, and anything outside the
set cleaning keeps would be an edit that vanished the next time the mod was
cleaned. Each row therefore carries three values, and only the third is stored:
what the game has (`vanilla`), what the author ships (`author`, which is the
box's placeholder, so **emptying the box goes back to it**), and yours.

Two consequences worth knowing before changing anything here:

- **The values are data, not a file.** `edits.json` holds `path -> value` and the
  build is produced from it, which is what makes an edit survive a mod update —
  re-applied by path, with any property the update *moved* named rather than
  quietly reverted.
- **Every other build has to re-apply it.** Cleaning and mending rebuild the mod
  from the copy its author shipped, which is the one place your values are not,
  so `run_clean` and `repair_apply` both call `rebuild_edited` afterwards. Miss
  that and an unrelated verb reverts your work in silence — the same shape as the
  bug recorded on `prune::clean_into`, where every reconcile undid a clean.

`edited` is a flag on the loadout entry beside `variant`, not a fifth variant: a
cleaned mod carrying values you set is *both*, and one word could only have said
one of them.

## What a game update changed underneath the mods

`engine/patchdiff.rs` and `UpdatePanel.svelte`. The outdated-mods list is hidden
by default and should be: "built for 6.45" is a date, not a fault, and most mods
with an old stamp work perfectly. This answers the question that list only
gestured at — of the properties this mod sets, which ones did *the game itself*
change since?

It needs no downloads and no guesses. `engine/vanilla.rs` already caches
extracted vanilla assets per game build, so the copies from before an update are
still on disk after it; the survey is the old cache against the new one.

**The discriminator matters more than the diff**, the same way it does for scene
drift. "This property differs from vanilla" is the definition of a mod. What
separates harm from intent is *which* vanilla value the mod agrees with:

| the mod's value | reading |
|---|---|
| equals the **old** vanilla value | carrying a stale baseline; the patch is reverted by accident |
| equals the **new** vanilla value | already agrees with the patched game; nothing to say |
| its own, agreeing with neither | the author meant to set this, and the ground moved under them |

Only the first is a bug. Reporting the third as harm would bury every real
finding under the entire purpose of every mod in the library. Two more kinds fall
out of the same comparison: a property the patch *removed* leaves the mod's edit
with nothing to land on, and a property the patch *added* is one a whole-file copy
deletes by not containing it — the other half of the `SalvageRights` shape, and
why `whole_file` is a parameter here exactly as it is in `prune`.

Three things about this are deliberate and easy to undo by accident:

- **It is not part of `analyse_library`.** It decompiles two copies of every asset
  the library touches, which is minutes on a first run, and the answer only
  changes when Hello Games ship a patch. Folding it into the scan would put it on
  the launch path — the mistake that once cost 76 seconds of start-up.
- **"Could not check" is drawn as its own state.** A machine that has seen one
  game build cannot do this at all, and an empty findings list would say both
  "nothing is wrong" and "nothing was checked". `Impact::blocked` carries the
  reason and the panel prints it instead of a verdict.
- **The gap is nameable and closable.** Only assets cached *before* an update can
  be judged, and ordinary running caches only what it needs. `Impact::uncomparable`
  names the rest, and `patchdiff::prepare` — a button, never automatic — extracts
  vanilla for everything the library touches so the next update is judged in full.

Runnable outside the app, including on a machine that has only ever seen one
build:

```
cargo run --example patch_impact            # the real comparison
cargo run --example patch_impact -- --demo  # prove it without waiting for a patch
```

`--demo` fabricates the missing build by standing a mod's own whole-file copy in
as what the game used to ship — which is not an arbitrary way to make a
difference, it *is* the scenario. A mod ships a whole table because it was built
against a game that looked like that table. Nothing is written to the live cache;
see `vanilla::CACHE_OVERRIDE`.

## Layout

```
src/lib/
  tokens.css            design system: colour, type, base
  components.css        the shapes shared across panes
  types.ts              the engine's JSON contract, load order, claim ordering
  actions.ts            findings restated as ranked things to do
  ActionRow.svelte      one thing to do, and the button that does it
  library.svelte.ts     the one store: what is installed, and which build of it
                        (engine side: adopt.rs + fileid.rs take over a library)
  LibraryPane.svelte    the list, the record, the preset bar and Find more
  ModList.svelte        the only list of mods; the old tabs are its chips
  ModRecord.svelte      everything known about one mod, in one column
  ValueEditor.svelte    change a value the mod sets; vanilla and author beside it
  PresetBar.svelte      which set of mods is on, as a control not a screen
  EvidenceSheet.svelte  the claim stack, over whatever sent you to it
  LoadOrderRail.svelte  the spine: mods in ModPriority order
  ClaimStack.svelte     N mods claim a value; one survives
  ConflictDetail.svelte one contested asset
  SessionsPane.svelte   what the game itself said, run by run
  LoadOrderMeasured.svelte  the order the game *actually* read a contested
                        asset's copies in, and the one step still inferred
  UpdatePanel.svelte    what the last game update changed underneath the mods
  UpdateImpactDetail.svelte  one mod file's four verdicts, as a table
  sessions.svelte.ts    the live session, held outside any tab
src/routes/+page.svelte the verdict, the four tabs, Play, and event plumbing
tests/                  runs actions.ts and renders the panes, via Vite + Node
src-tauri/src/engine/   the engine
                        observed.rs  load order, read back off a session log
                        patchdiff.rs what an update did to the library
src-tauri/examples/     one-off tools: dump a stage, time a start-up
```
