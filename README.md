# Anomaly — No Man's Sky mod manager

**Anomaly** is the program: a desktop app in [app/](app/) that installs,
switches, cleans, merges and mends mods, records what the game does with them,
and keeps copies of your save. See [app/README.md](app/README.md).

`nmscheck.py`, described below, is the original Python conflict engine. It is
kept as a cross-check against the Rust port and is not where features go.

## Install

Download the installer from [Releases](../../releases) and run it. Windows only.

The build is **not code-signed**, so SmartScreen will show "Windows protected
your PC" the first time: choose *More info* -> *Run anyway*. That warning is
about the absence of a certificate, not about anything the installer does.

Nothing else is required. The tools Anomaly needs to read the game's own files
-- MBINCompiler, hgpaktool and 7-Zip -- are installed alongside it, and it finds
No Man's Sky itself. See [tools/THIRD-PARTY.md](tools/THIRD-PARTY.md) for what
those are and their licences.

## Licence

MIT, for Anomaly's own source -- see [LICENSE](LICENSE). The bundled binaries
keep their own terms; none of them is linked into this program.

## nmscheck.py — the Python conflict engine

Finds conflicts between unpacked No Man's Sky mods by comparing them
**property by property**, not just "these two mods touch the same file".

```
py -3 nmscheck.py                         # finds the game -> conflicts.html
py -3 nmscheck.py --where                 # show which install was found
py -3 nmscheck.py --html elsewhere.html
py -3 nmscheck.py "...\No Man's Sky\GAMEDATA\MODS"
```

With no argument the game is located automatically and its `GAMEDATA\MODS`
folder is scanned — see [Finding the game](#finding-the-game). With neither
`--html` nor `--json` it also writes `conflicts.html` into the current
directory; `--no-html` turns that off.

No dependencies beyond the standard library. Python 3.9+. No mod manager
required.

The Python engine is the reference implementation; its `--json` output is the
contract the Rust port in [app/](app/) reproduces.

---

## What it actually checks

### 1. Property-level conflicts

Every mod folder is scanned for `.EXML` and `.MBIN` assets. Paths are
canonicalised (case-folded, separators unified, `.EXML` → `.MBIN`) so
`Globals\GCGAMEPLAYGLOBALS.GLOBAL.EXML` and
`GLOBALS/GCGAMEPLAYGLOBALS.GLOBAL.MBIN` are recognised as the same game asset.

Each EXML is flattened into `property path → value` pairs and the copies are
diffed:

**Only findings that need a decision are reported.** The game merges patches
that do not overlap, so two mods editing the same file in different places is
not a conflict and is not listed — it is counted in one header line, and
`--show-merged` spells those out if you want them.

| reported | severity | meaning |
|---|---|---|
| yes | `CRITICAL` | Two copies set the **same property to different values**. Only one can win — pick one. |
| yes | `MAJOR` | The copies **cannot be compared** (mixed MBIN/EXML), or differ as opaque binaries. Unverifiable is not the same as safe. |
| no | `MINOR` | Disjoint properties, identical values, or identical files. The game merges these cleanly; nothing is lost. |

Exit status follows the same split: a library whose only overlaps merge cleanly
exits `0`.

The distinction matters. Five mods in a real library all write
`GCGAMEPLAYGLOBALS`, but each sets a *different single property*
(`ShipInteractRadius`, `MaxNumSameGroupTech`, `AutoTranslateWordChance`…) —
that is not a conflict, and the game merges them.

### 1b. How mods are loaded (this is not the old pak workflow)

Current No Man's Sky (Cosmos 7.x) reads loose **sparse EXML patches** straight
from `GAMEDATA/MODS/<ModFolder>/` and applies them natively — no `.pak`, no
AMUMSS, no MBINCompiler. Non-overlapping edits merge; where two patches touch
the same property, mod order decides. Use `--assume-pak` to analyse a library
built the older way, where each mod was its own `.pak` and the whole file was
replaced rather than patched.

### 1c. Row identity: `_id` and `_index`

A sparse patch names the row it edits with an `_id` attribute — that is how the
game matches the patch onto the real table:

```xml
<Property name="Table" value="GcTechnology" _id="LAUNCHER">
  <Property name="StatBonuses">
    <Property name="StatBonuses" value="GcStatsBonus" _index="1">
```

Honouring those attributes is not optional. Numbering rows positionally instead
makes the first row of one patch collide with the unrelated first row of
another: on the real library that reported two CRITICAL conflicts where
`LaunchThrustersReworked` (patching `LAUNCHER`) appeared to fight
`Long Range Freighters` (patching `F_HYPERDRIVE`). Both were artefacts. Explicit
keys are rendered in the same shape as inferred ones, so a sparse patch and a
full decompiled table describing the same row produce the same path.

### 2. Why property overlap is the right signal

Mods ship two different kinds of EXML and they look identical from the outside:

* **fragments** — only the properties the mod overrides (*Slippery Slopes* is
  2 properties, 184 bytes)
* **full assets** — the entire decompiled table (*0-Ultra Base Building* ships
  a 6.8 MB objects table)

Telling them apart reliably needs vanilla game data, which this tool
deliberately does not require. Property overlap works regardless: if two mods
both write `ShipInteractRadius` and disagree, that is a conflict whatever the
file size.

### 2b. Load order, straight from the game

Once the game has been launched with mods it writes
`Binaries/SETTINGS/GCMODSETTINGS.MXML`, listing every mod with a `ModPriority`
and an `Enabled` flag. When that file exists the reported winner is the real
one rather than a guess, and disabled mods are skipped (`--include-disabled`
overrides). A `DisableAllMods` of true is called out prominently, since it means
the game is ignoring every mod.

None of this needs a mod manager. `GCMODSETTINGS.MXML` is written by the *game*,
so hand-installed mods get the same real load order as managed ones. If Vortex
happens to have deployed the folder, its `vortex.deployment.json` is read as a
bonus so mods can be traced back to the Nexus archive they came from; without
it, nothing is lost but that one label.

### 2c. Finding the game

With no folder argument, the install is located in this order, and the first
hit that contains a `GAMEDATA` folder wins:

| order | source | how |
|---|---|---|
| 1 | `NMS_GAME_DIR` | environment variable, overrides everything |
| 2 | **Steam** | `HKLM\...\Uninstall\Steam App 275850` → `InstallLocation` |
| 3 | **Steam** | `libraryfolders.vdf` → `appmanifest_275850.acf` → `installdir` |
| 4 | **GOG Galaxy** | `HKLM\SOFTWARE\GOG.com\Games\*`, matched on `gameName` |
| 5 | **Epic** | `%PROGRAMDATA%\Epic\...\Manifests\*.item` |
| 6 | **Microsoft Store** | `<drive>:\XboxGames\No Man's Sky\Content` |
| 7 | anything else | a sweep of the usual folders on every fixed drive |

Only the first install is ever scanned, never several at once: a mod present in
two of them would otherwise be reported as conflicting with its own other copy.
`--where` prints everything that was found and which one would be used.

Two deliberate details: GOG and Epic are matched on the game's *title* rather
than a product id, so a re-issue under a new id keeps working; and the drive
sweep asks Windows for fixed disks only, because probing a disconnected network
drive can block for seconds.

### 3. Change attribution

AMUMSS marks the lines it rewrote with a trailing `!# CHANGED` / `!# ADDED`.
That text is *not* an XML comment — it lands in the element's `text` or `tail`,
so a normal parse preserves it. Where present it is direct evidence of which
mod authored an edit. Otherwise, with three or more copies, the majority value
stands in for vanilla and the odd one out is the author. With only two
unannotated copies the clash is reported without assigning blame, and the
report says so.

### 3b. Decompiling compiled assets (optional)

A mod shipping a compiled `.MBIN` is opaque: it can be hashed but not compared
field by field, so a clash against mods shipping readable EXML can only be
reported as *"cannot be compared"*. Drop **MBINCompiler** at
`tools/MBINCompiler.exe` (or pass `--mbincompiler`, or put it on `PATH`) and
those assets are converted to XML and compared properly.

Match the build to your game: the version stamp the checker reports
(`MBINCompiler 7.03.2.2`) corresponds to release `v7.03.2-pre2` from
[monkeyman192/MBINCompiler](https://github.com/monkeyman192/MBINCompiler/releases).
Output is `.MXML`, structurally identical to `.EXML` — same `Property` tree,
same `_id`/`_index` keys — so it parses unchanged.

Two things keep it cheap: only assets *contested by two or more mods* are
converted (nothing else can conflict), and results are cached by content hash
under `%LOCALAPPDATA%\nmscheck\mbin-cache`, so repeat runs pay nothing.

This is strictly optional. Without the executable everything still works; with
it, `MAJOR: cannot be compared` findings become real answers. On the real
library it turned the single unresolved MAJOR into a CRITICAL with 1271
concrete field disagreements.

### 4. Outdated mods (counted, not listed)

Every compiled `.MBIN` carries an MBINCompiler version stamp at offset `0x18`
(one byte each of major/minor/patch/build), and every EXML carries
`<!--File created using MBINCompiler version (6.34.0.3)-->`. Verified against
1870 real MBIN files: all decode, and the binary stamps match the EXML comments
exactly.

An old stamp is **not** evidence that anything is wrong. Most mods built for an
earlier version keep working indefinitely, because the structures they patch
did not change. So the count is shown and the list is not, unless you ask with
`--show-outdated`. It never affects the exit status either — only real
conflicts do.

### 5. Other checks

* **Localisation collisions** — two mods contributing the same `LocTable.MXML` id.
* **Unbuilt AMUMSS recipes** — a `.lua` declaring `MBIN_FILE_SOURCE` for an asset
  another mod already ships, which will collide once built.
* **Binary assets** — `.DDS` textures are compared by hash; identical is
  harmless, differing is flagged (textures are replaced whole). Contested
  `.MBIN` files are decompiled instead — see below.
* **Manager noise** — if a manager is in use, its bookkeeping is skipped so it
  cannot inflate file counts: Vortex leaves a `__folder_managed_by_vortex`
  marker in every directory (319 of them in a real library), plus
  `FILES.SHA256` and its deployment manifest; MO2 leaves `meta.ini`. A
  hand-installed folder simply has none of these.

---

## Options

| flag | effect |
|---|---|
| `--html FILE` | self-contained HTML report (no external assets); defaults to `conflicts.html` when neither `--html` nor `--json` is given |
| `--no-html` | skip that default `conflicts.html` |
| `--json FILE` | machine-readable dump; the planned Qt GUI consumes this |
| `--min-severity {CRITICAL,MAJOR,MINOR,INFO}` | hide quieter findings in the terminal |
| `--verbose` | list every differing property instead of the first six |
| `--assume-pak` | analyse as the older one-pak-per-mod workflow, where a shared asset means one copy loads and the other contributes nothing |
| `--include-disabled` | also analyse mods `GCMODSETTINGS.MXML` lists as off |
| `--show-merged` | also list overlaps the game merges cleanly (hidden by default) |
| `--show-outdated` | also list mods built for an older game version (counted by default; they usually work fine) |
| `--where` | print the game install(s) found, then exit |
| `--mbincompiler EXE` | path to MBINCompiler; found automatically in `tools/` or on `PATH` |
| `--no-decompile` | skip decompilation even when MBINCompiler is available |
| `--game-version X.YY` | judge staleness against this version instead of the newest in the library |
| `--winner {first,last}` | which end of alphabetical load order survives (default `last`) |
| `--fast` | skip EXML field parsing; file overlaps and versions only |
| `--no-color`, `--quiet` | output control |

Exit codes: `0` clean, `1` CRITICAL or MAJOR conflicts found, `2` error.
Suitable for gating a build script.

### The assumption, now half measured

Load order itself is read from `GCMODSETTINGS.MXML` when the game has written
it, so the ordering is real. What was assumed is which *end* of that order wins:
the tool takes the highest `ModPriority` as the winner. Flip it with
`--winner first`. Every winner line says which basis it used (`ModPriority` or
`assumed alphabetical order`).

**The reasoning printed beside that used to be wrong, and measurement caught
it.** The claim was that the highest `ModPriority` is applied *last*. Anomaly's
hook logs every mod file the game opens, in order; grouped by asset, that is the
sequence the game reads a contested asset's copies in. On the 63-mod reference
library, all 16 contested assets ran **strictly descending** `ModPriority` — the
highest priority is read **first**, not last:

```
GLOBALS/GCGAMEPLAYGLOBALS.GLOBAL.MBIN
    Supercharge Multiplier       [62]
 -> Quick Scan with Range Boost  [56]
 -> Larger Upgrade Stacks        [39]
 -> Better Ship Transfer Range   [18]
 -> Auto Translate Words         [12]
 -> More Freighter Battles 50%   [8]
```

Sixteen assets, no exceptions, so this is not coincidence: the game walks
assets, and within one asset reads the providers highest priority first.

That does **not** settle the winner, and the tool does not pretend it does. It
narrows the question to one thing the hook cannot see, because it watches file
opens rather than the table in memory:

| if the loader | the survivor is | i.e. |
|---|---|---|
| lets a later read overwrite an earlier one | the copy read **last** — the *lowest* priority | `--winner first` |
| keeps the first value written for a property | the copy read **first** — the *highest* priority | `--winner last` |

The default, `last`, is the second row, which is also the design the name
*ModPriority* implies: read highest-first, keep the first write. So the tool's
conclusion is probably right and its stated reason was not.

What is gained is concrete even so — the candidates for any contested asset are
now two *named* mods rather than a list, the order is read off what the game did
instead of inferred from a file it wrote, and any asset that stops following the
rule is reported rather than averaged away. It is `engine/observed.rs`, shown per
session in the Sessions tab, and runnable on any log:

```
cargo run --example observe -- <path-to-session.log>
```

Settling the last step needs the value itself, not the file opens: two patches
setting one harmless property to distinguishable values at known priorities, then
reading back which one the game ended up with.

---

## Rebuilding a full-table mod as a sparse patch

`tools/build_salvagerights_sparse.py` is a worked example of the repair this
checker exists to find, and it is reproducible: re-run it after a game update.

The published **SalvageRights** mod shipped the entire 1.2 MB
`REWARDTABLE.MBIN`. Diffed against vanilla it turned out to change **17**
properties, of which only **2** are the advertised feature:

| entry | property | vanilla | mod |
|---|---|---|---|
| `R_SCRAPHEAP` (planetary Salvage Container) | `List[1].PercentageChance` | 40 | 100 |
| `R_SPACEBIGGS` (space wreck) | `List[1].PercentageChance` | 40 | 100 |

The other 15 were a stale baseline: the mod was built for game build 25351301,
so shipping the whole table silently reverted the Nexus mission reward
(`R_GT_NEW_EASY_N`) and expedition Phase 4 ship costs
(`SeasonRewardTable23[RS_S23_PHASE4]`) on a newer build.

Rebuilt as a 978-byte sparse patch, the conflict disappears:

```
BEFORE (original 1.2 MB MBIN):  CRITICAL: 1   1271 properties disagree
AFTER  (sparse EXML):           CRITICAL: 0   no conflicts that need a decision
```

The script extracts vanilla from `GAMEDATA/PCBANKS` with **hgpaktool**,
decompiles it with MBINCompiler, asserts the two target properties still exist
and still read `40.000000`, writes the patch, re-parses the file it just wrote,
fails the build if any node carries a value vanilla does not have, and packages
`dist/SalvageRightsSparse-1.0.zip` for Vortex.

Getting the optional tools:

```
py -3 -m venv tools/venv            # do not pip-upgrade pip inside it
tools/venv/Scripts/python -m pip install hgpaktool
# plus tools/MBINCompiler.exe, matching your game (see section 3b)
```

NMS `.pak` files stopped being PSARC at the 5.50 update; they are Hello Games'
own `HGPAK` container now, which is why hgpaktool is needed rather than a
generic archive reader.

---

## Layout

```
nmscc/
  paths.py        canonical asset keys
  gamefind.py     locate the install: Steam, GOG, Epic, MS Store, or a sweep
  hostenv.py      GCMODSETTINGS.MXML load order + optional Vortex manifest
  decompile.py    optional MBINCompiler bridge, cached by content hash
  model.py        dataclasses shared by every layer
  mbin.py         MBIN header + version stamp reader
  exml.py         EXML flattener, change markers, malformed-XML repair
  luascript.py    AMUMSS MBIN_FILE_SOURCE extraction
  discovery.py    filesystem scan -> Mod objects
  analyze.py      conflict detection, attribution, severity
  report.py       terminal + JSON output
  html_report.py  standalone HTML output
  cli.py          argument parsing
nmscheck.py       entry point
tools/            MBINCompiler.exe + hgpaktool venv (untracked), build scripts
dist/             packaged mods ready to install
tests/            32 tests; python -m unittest discover -s tests
tools/            drop MBINCompiler.exe here (not checked in)
```

The analysis layer imports no UI code, so the CLI and the planned Qt front end
share exactly the same results — the GUI only has to render a `Report`.

## Notes on robustness

* Mod authors comment blocks out and annotate them
  (`<!--<Property .../> --perfect-->`). A double hyphen inside a comment is
  illegal XML; a strict parse fails. Comments are stripped on a repair pass, so
  disabled properties correctly do not count as changes.
* Table rows are keyed by `_id`/`_index` attributes, falling back to an
  `Id`/`Name` child and only then to position, so neither an inserted row nor a
  sparse patch shifts paths and invents conflicts.
* `1.0` and `1.000000` are treated as equal so float formatting does not
  manufacture conflicts.
