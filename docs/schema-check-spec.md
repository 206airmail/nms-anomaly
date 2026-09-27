# Schema check — does a mod still match the game?

**Status:** the motivating bug turned out to be invalid XML, which is now
detected and reported — see the box below. A positional version of the
vanilla-diff design **has** shipped for scene graphs, see the box after next, and
property data now has one too — but not the one described here. See the box
immediately below. Every number in the body of this document was measured on the
live 57-mod library against game 7.03 on 2026-09-22.

> ## Shipped, differently — update impact, 2026-09-26
>
> `engine/patchdiff.rs` diffs property data against vanilla, which is what the
> rest of this document asked for. It does it along a different axis, and the
> difference is the interesting part.
>
> This document proposed comparing a mod against *the installed game*: does this
> mod still match what is there? That question turns out to be unanswerable in
> the form that matters, because "differs from vanilla" is the definition of a
> mod. `prune` already computes that set and uses it for exactly what it is good
> for — deciding what a sparse patch must keep — and it cannot tell a deliberate
> edit from a stale one, because both look identical from where it stands.
>
> Adding a *second* baseline settles it. Comparing vanilla-before-the-update
> against vanilla-after, and then asking which of the two the mod agrees with,
> splits the set cleanly:
>
> | the mod's value | reading |
> |---|---|
> | equals the **old** vanilla value | stale baseline; the patch is being reverted by accident |
> | equals the **new** vanilla value | already agrees with the patched game |
> | its own, agreeing with neither | deliberate, and the ground moved underneath it |
>
> Same discriminator shape as scene drift's "node moved but its contents did
> not": the raw diff is noise, and one extra fact turns it into a finding. Four
> verdicts come out of it — `reverts`, `drops`, `dead`, `overridden` — and only the
> first three need a decision.
>
> **It costs no new machinery.** `vanilla.rs` was already caching extracts per
> game build, so the older baseline was on disk the whole time; what was missing
> was a stamp saying which build came first, which is now written when a build is
> first seen.
>
> **Two limits, both stated in the UI rather than hidden.** A machine that has
> seen one game build cannot run this at all, and that is drawn as its own state
> so an empty findings list cannot read as a clean library. And only assets cached
> *before* an update can be judged, because the old archives are gone once the
> patch lands — `patchdiff::prepare` closes that gap in advance, as a button, never
> automatically.
>
> `cargo run --example patch_impact -- --demo` proves it end to end on real data
> with the real toolchain, on a machine that has never seen an update.

> ## Shipped — scene drift, 2026-09-23
>
> A second real bug drove the first piece of vanilla diffing into the tool.
> `FreighterSalvageTerminals.v4.8` added two terminals to the freighter hangar
> and, as a side effect of re-export, dropped the `FREIGHTERBASE` locator
> 24.281 units while re-offsetting its children so nothing visibly moved.
> Symptom in game: geometry vanishing and backfaces showing once the player
> gained elevation inside the hangar bay. No file conflicted with anything.
>
> `nmscc/scene.py` composes world transforms and diffs two scene graphs;
> `nmscc/vanilla.py` extracts the counterpart from PCBANKS with a per-build
> cache; `nmscc/drift.py` joins them. Reported as `SceneDrift`, surfaced in the
> terminal, the HTML report and the JSON contract.
>
> **The discriminator matters more than the diff.** Raw "this node moved"
> is noisy: on the live library, `Convenient Corvette Teleporters` legitimately
> moves 16 nodes, because moving teleporters is what it does. What separates a
> bug from an edit is what happened *underneath*: a deliberate move carries its
> contents along, whereas a rebake displaces a parent and leaves every child
> where it was. Measured on the live library, that rule gives **0 false
> positives across 30 scene assets** and catches the real fault in both damaged
> files. Everything else is reported as INFO.
>
> **Rejected: the vanilla-free variant check.** The plan was to compare a mod's
> own copies of an asset family (`HANGAR` / `HANGARGHOST` / `HANGARPIRATE`) and
> flag nodes they disagree about, since that needs no tools. Measurement killed
> it: vanilla itself places `HangarA/Approach3a` differently in the normal and
> pirate hangars, so the check reported a fault on an untouched library. Worse,
> `HANGARGHOST` turned out to be a 7-node landing pad that does not contain
> `FREIGHTERBASE` at all — so the "two of three variants damaged" reading that
> first motivated the check was itself wrong. Recorded in `drift.py` so it is
> not rediscovered.
>
> **Ported to Rust, which is the implementation.** `engine/scene.rs`,
> `engine/vanilla.rs`, `engine/drift.rs`, plus `engine/decompile.rs` (the
> MBINCompiler bridge the Rust side previously lacked entirely). The app calls
> it through the `analyse_library` command and renders it in `DriftDetail.svelte`.
> `tools/compare_engines.py` grew a `scene drift` section: both engines produce
> identical findings on the live library and on a library holding the two
> damaged files.

> ## Root cause found — 2026-09-22
>
> The original broken `Increased S Class Chance` was recovered. Two differences
> from the working version:
>
> 1. **`<Property name="ClassProbabilityData">` is never closed.** The file is
>    not well-formed XML — `mismatched tag: line 36, column 2`. This is the
>    defect. The game's parser rejects the file and the mod does nothing.
> 2. `template="GcInventoryTable"` vs vanilla's `"cGcInventoryTable"`.
>
> **(1) is now handled.** Parse failures were already detected by the engine but
> buried: a header footnote in the terminal, absent from the HTML report and the
> app, and not counted toward exit status. They are now a first-class finding —
> see `BrokenFile` in `nmscc/model.py` and `engine/model.rs` — reported above
> conflicts, and `Report.needs_attention` makes them exit non-zero.
>
> **(2) is a weak signal and must not be reported as breakage.** Measured across
> the library: 14 of 54 mod EXML files with a vanilla counterpart declare an
> unprefixed template where vanilla uses the `c` form, plus at least 10 GLOBALS
> files. `Better Frigate View 2.3` says `GcCameraGlobals` where vanilla says
> `cGcCameraGlobals`, and it is installed and working. So the prefix is not
> required, and the template change to the S-Class mod was probably incidental.
> Report a template mismatch as INFO at most, never as a fault, unless in-game
> testing shows otherwise.
>
> ## Path mapping — GLOBALS are not where mods put them
>
> Mods ship globals at `GLOBALS/GCCAMERAGLOBALS.GLOBAL.EXML`. Inside the paks
> the file sits at the **root**: `GCCAMERAGLOBALS.GLOBAL.MBIN`, with no
> `GLOBALS/` component. A filter of `*GLOBALS*GCCAMERAGLOBALS*` matches nothing.
>
> This is why the first bulk extraction reported "102 of 103" while silently
> retrieving **no GLOBALS at all** — and globals are among the most-modded
> assets in the library. Any vanilla lookup must map a mod-relative target to
> its in-pak path, not assume they are the same.
>
> **Solved.** `nmscc/vanilla.py` filters on the *basename* and resolves the
> result by longest matching path suffix, so it is correct whether or not the
> directory prefixes agree. `GLOBALS/GCCAMERAGLOBALS.GLOBAL.MBIN` now extracts;
> `tests/test_scene.py` pins both that case and the ambiguous-basename case.
>
> ## What `array_size` turned out to be: nothing
>
> An earlier draft claimed `array_size` was a required loader directive. It is
> not, and that claim is withdrawn. Measured: of 45 sparse patches in the
> library that patch a container property, **44 declare none** and work; vanilla
> decompiled output never emits it; and the two working variants of the S-Class
> file — one with it, one without — **flatten identically**, since the parser
> ignores the attribute.

## Why

The tool currently reports a mod as "outdated" when its MBINCompiler version
stamp is older than the rest of the library. That is a weak signal, and you
were right to push back on it: 33 of your mods carry an old stamp and all of
them work. A stamp says when a mod was *built*, not whether it still *applies*.

A patch that names a property the game no longer has does not error. It does
nothing. The mod appears installed, the feature silently never happens, and
nothing in the log says so. That is the failure this check exists to find, and
it is the honest version of the "outdated" idea.

## The one hard problem

**Vanilla instance data is not the schema.**

The obvious design — extract vanilla, flatten it the same way we flatten mods,
and report any mod path that vanilla lacks — runs straight into this. Our
flattener only emits a path for a field that is *populated*. If vanilla leaves
a list empty, that subtree produces no paths at all, and a mod that fills it in
looks like it invented the field.

Measured: comparing shapes (every `[key]` stripped) against vanilla instance
data leaves **45% of mod paths "unknown"**, nearly all of them legitimate. That
rules out naive absence as a signal on its own.

The real schema lives in MBINCompiler's struct definitions, not in the data.
`MBINCompiler list` emits 2,916 template names with GUIDs — but names only, no
field layouts. So the authoritative schema is reachable only by reflecting over
`libMBIN.dll` or parsing the MBINCompiler source, neither of which the project
does today.

## What the data actually looks like

Extracting every asset the library touches and diffing against it:

| | count | share |
|---|---|---|
| mod property paths checked | 2,147,693 | |
| present in vanilla | 30,348 | 1.41% |
| absent, introduces a new keyed row (**addition**) | 2,117,089 | 98.58% |
| absent, parent row exists (**orphan**) | 220 | 0.01% |
| absent, even the first segment is missing (**root-miss**) | 36 | 0.00% |

The 98.58% is almost entirely one mod (`alchemist_GPS`) adding thousands of
missions — legitimate new content, and exactly why "absent from vanilla" cannot
be the rule on its own.

What survives is small and reviewable: **10 mod/asset pairs across 5 mods.**

```
alchemist_GPS                        NMS_REALITY_GCPRODUCTTABLE   orphan=84
alchemist_GPS                        SIGNALSCANNER.ENTITY         orphan=68
alchemist_GPS                        STATGROUPSTABLE              root-miss=36
Firmware Update for the Signal Boost COSTTABLE                    orphan=22
alchemist_GPS                        CONSUMABLEITEMTABLE          orphan=18
Firmware Update for the Signal Boost STATDEFINITIONSTABLE         orphan=15
Quick Damaged Machineries 2.9        TECHDEBRIS.ENTITY            orphan=5
Quick Crates 3.9                     CRATE_LARGE_RARE.ENTITY      orphan=4
No Charge Portals 6.45.1.0           BUTTON.ENTITY                orphan=3
Firmware Update for the Signal Boost STATGROUPSTABLE              orphan=1
```

### One confirmed false positive, already found

`Firmware Update for the Signal Booster` produces orphans like
`StatDefinitionTable/StatDefinitionTable`. That is **our flattener**, not the
mod: it indexes only *repeated* siblings, so a sparse patch holding one row
renders a bare name while the full vanilla table renders `[0]`, `[1]`, …. Same
field, different string.

Any implementation must treat `name` and `name[…]` as the same position. That
is also a latent issue in the conflict engine itself — a one-row patch and a
full table are currently never compared — and is worth fixing in both places.

### One likely true positive

`alchemist_GPS`'s `STATGROUPSTABLE` patches a root property called
`GcStatGroupTable`. Current vanilla output calls it `StatGroupTable`. The game
matches patches by property name, so this patch probably does nothing.

Note what is *not* wrong with it: the `template="GcStatGroupTable"` attribute is
correct. MBINCompiler's canonical template names are unprefixed, and the
`cGcStatGroupTable` seen in vanilla output is an artifact of how the decompiler
writes the root template. **Template names are not a usable signal** — checking
them would flag every vanilla file.

## Getting vanilla, and what it costs

Both tools are already in `tools/` and already used by
`build_salvagerights_sparse.py`.

```
hgpaktool -U --upper -O <out> -f=<glob> [-f=<glob> …] <PCBANKS>
MBINCompiler convert <dir>
```

`-f` may repeat and the filters are OR'd, so one invocation covers the whole
library. Measured against 98 paks totalling 31 GB:

| step | time | output |
|---|---|---|
| extract 103 targets | 19.6 s | 28.7 MB (102 found, 1 missing) |
| decompile all | 10.3 s | 257 MB XML |
| **total** | **~30 s** | one-time per game build |

Fast enough that no precomputed baseline needs shipping. One target was not
found in PCBANKS — worth identifying before building this, since it is either a
mod-only asset or a filter bug.

**Cache:** 257 MB of XML is not worth keeping. Store only what the check needs:
per target, the set of vanilla property paths, as sorted 64-bit hashes, keyed by
game build. Roughly 25 MB for the whole library, under
`%LOCALAPPDATA%\nmscheck\vanilla\<build>\`. Invalidate on build change.

## Proposed design

### Tier 1 — instance diff (build this)

Cheap, uses tools already present, no new dependencies. Classify every mod
property path against the cached vanilla path set:

| verdict | rule | reported as |
|---|---|---|
| `present` | exact match, or match ignoring `[key]` at the same position | nothing |
| `addition` | the missing segment introduces a keyed row vanilla lacks | nothing (or INFO behind a flag) |
| `orphan` | the parent path exists in vanilla, the leaf does not | **MAJOR** — the field is gone; this edit cannot apply |
| `root-miss` | the first segment is absent from vanilla | **MAJOR** — the patch targets a structure the game does not have |

Severity caveat worth encoding in the UI text: an orphan means *this edit does
nothing*, not *this mod is broken*. A mod with 84 orphans and 400 working paths
is partly stale, not dead. Report a count and a ratio, never "broken".

### Tier 2 — true schema (later, if Tier 1 proves noisy)

Reflect over `libMBIN.dll` to get real field layouts, which removes the
empty-list false negatives entirely. Only worth the dependency if Tier 1's
false-positive rate turns out worse than the 10 pairs above suggest.

### Not in scope

Type checking (a string where an int belongs), enum-value validation, and array
bounds. All need Tier 2. Worth revisiting once the schema is reachable.

## How it surfaces

This replaces the outdated-mods section as the *evidence-based* version of the
same question. Suggested wording, following the existing "verdict first" rule:

```
2 mods patch fields the game no longer has        →
```

Expanded, per mod, in the claim-stack idiom — the path and what happened to it,
not a stack trace:

```
alchemist_GPS  ·  STATGROUPSTABLE
  36 of 36 patched fields are missing from game 7.03
  GcStatGroupTable/…/GroupName      ✕ no such field
      vanilla has StatGroupTable/…/GroupName
```

Keep the version-stamp list too, demoted further: it is a weak hint, and this
is the strong one.

## Open questions

1. **Which asset failed to extract**, and why. Answer before building.
2. **Does an orphan ever apply anyway?** The assumption is that the game matches
   patches by property name, so a name vanilla lacks cannot match. Worth
   confirming in-game with one deliberately broken patch before shipping a
   MAJOR severity built on it.
3. **Offer to fix?** Once a stale path is identified and its vanilla equivalent
   is known, rewriting the patch is mechanical — and is the same machinery the
   planned merge feature needs.

## Plan

1. Identify the missing asset (question 1).
2. `vanilla.rs` — extract + decompile + cache, keyed by game build.
3. Extend the flattener so `name` and `name[…]` compare equal by position; add
   the regression test from the `StatDefinitionTable` false positive.
4. `schema.rs` — the four-verdict classifier, with the 10 pairs above as
   fixtures.
5. Port to the Python reference first or write Rust first? **Python first**, so
   `tools/compare_engines.py` keeps working as the acceptance test.
6. UI: one line in the verdict, expanding to per-mod detail.
