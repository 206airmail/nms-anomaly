# Open work

Kept in rough priority order within each section. Items are written so that the
*next* action is obvious and so that anything already ruled out stays ruled out --
several of these were expensive to learn and are easy to re-attempt by accident.

Last updated 2026-09-27.

## Plugin UI

Designed 2026-09-27 in `docs/plugin-ui.md`. The short version: **define the
plugin-facing API before building any renderer**, because three of the four use
cases Nick named need only a hotkey and a line of confirmation text, and the
fourth -- authoring sort rules -- belongs in the desktop app rather than in-game.

Order: hotkeys first (the message pump is already ours), then plugin settings over
the existing pipe, then a toast as a Vulkan **implicit layer** (not a
`vkQueuePresentKHR` detour), and an interactive panel only if a plugin truly needs
one. NMS is Vulkan with NVIDIA Streamline in the chain -- measured from the import
table, because the loaded-module list misleadingly suggests D3D12.

## Live memory / the native API

The API's purpose is to be a **host for separate, independently developed plugins**.
Everything below serves that.

### 1. Does a brand-new save start with 24 exosuit slots?

The exosuit's `miCapacity` reads **24** while Nick owns 31. It did not move when he
bought a slot, and it did not move across a process restart, so it is a persistent
value that does not track ownership. His hypothesis is that it is the number of
slots the exosuit *started* with.

What has already been ruled out, so nobody repeats it:

- `DEFAULTSAVEDATA.MBIN` **is** the new-game save, and every inventory in it has
  `ValidSlotIndices` empty. Starting slots are granted by code at load, not stored.
  So the vanilla data cannot answer this.
- Reading it live at a different moment: no. The value survived a restart.

So the only test left is the direct one:

1. Start a new save (any mode -- but note the mode, since Survival/Permadeath may
   differ and `DEFAULTINVENTORYBALANCESURVIVAL.MBIN` exists alongside the normal one)
2. Load in far enough to have an exosuit
3. Trigger the probe (drop `probe.now`, see below)
4. Read `miCapacity` on the 10x12 store

If it reads 24 with 24 owned, the field is "capacity the store was built with" and
the exosuit is only anomalous because it is the one inventory whose owned count
grows far past its starting size. If it reads something else, the field means
something we have not worked out.

Worth doing at the same time, since a fresh save is cheap to probe and has *known*
contents: check whether `miCapacity` equals owned for every other inventory in a
save where nothing has been expanded yet. That isolates "expansion" as the variable.

### 2. Map index -> inventory, once

`sizeof(cGcInventoryStore) = 0x248`, the stores sit in `cTkFixedArray` runs inside
`cGcPlayerState`, and **the internal offsets are identical across process restarts**
(measured twice: exosuit -> first exocraft is `0x5B50` in both). So one map holds
for every future session.

Known so far, indexed from the exosuit at 0:

| index | inventory |
|---|---|
| 0 | exosuit general (10x12) |
| 7 | `FreighterInventory` (7x5) |
| 8 | `FreighterInventory_TechOnly` (7x3) |
| 12, 13, 14, 17 | storage chests (10x6) |
| 32 | `CorvetteStorageInventory` (10x16) |
| +0x10 off the lattice, 7 contiguous | exocraft tech (`mVehicleTechInventories`) |
| +0x20 off the lattice | ship tech |

The gaps are the empty ones -- the scan only keeps stores holding at least one item.
**Filling them in needs a probe that reports empty stores too**, which is a small
change: keep the object, drop the "at least one item" rule, and report it as empty
rather than rejecting it. Do that and the whole map falls out of one run.

Note index 32 is past the 28 ReNMS records for `mInventories` at 4.13, so the array
grew by 7.03. Measure it, do not assume it.

### 3. Stop scanning: hook the constructor

The 8 GB walk takes 162 s when it succeeds and 13 minutes when it does not. It is a
bootstrapping tool, not the mechanism. NoMansSky.Api hooks `cGcPlayerState::Construct`
and keeps `self`; the game then hands over the instance *and its identity*, and with
(2) every inventory is a fixed offset away.

Costs one signature, which we must derive ourselves (its patterns live in a binary
assembly and are 2023 anyway). A signature is a smaller and far more testable
problem than an 8 GB heuristic.

Free confirmation available with no signature at all: `muUnits` and `muNanites` sit
before the inventories in the same object, so reading them and comparing against the
HUD proves the base.

### 4. Which copy is authoritative?

`FUEL1 x2163` existed at **eight** addresses -- live state, save-document copies,
probably UI buffers. Nothing may write until we know which is real. (2) probably
answers it: the copy inside the player-state object is the one, and the lattice
identifies it. Confirm by writing to it and watching the UI, as was done once
already from Cheat Engine.

### 5. Loose ends in the layout

- **Exocraft tech stores read `mask popcount == miCapacity + 8`**, and 36 bits on a
  10x3 grid is more bits than cells. Everywhere else the mask is the owned count and
  tracks changes. Unexplained; use `miCapacity` for those seven.
- `mStoreHistory` is read and reported but its purpose is unknown. It is frequently
  `0/N` -- allocated, unused.
- Two multitool-shaped stores sit **off** the player-state lattice entirely
  (`0x...A42BF50` in the last run). Other players' multitools? NPC? UI copies?

### 6. Plugins, once the above is solid

- Inventory sorter -- read and write are both proven; it needs (2) and (4).
- Shared clan/tribe storage. Overlay only, never NMS's own netcode. The hard part is
  **duplication**: a naive implementation is an item duplicator. Needs one authority
  owning the stash and local writes only after confirmation, plus reconciliation
  after the game rewrites inventories on save/load.
- Corvette take-off/landing animations.
- Frigate expeditions past 5/day -- probably a plain EXML mod, not a plugin.

Architecture: load plain DLLs, narrow API (log, read/write by name, frame callback),
and let plugins bring their own dependencies. Capabilities opt-in **by export name**
(the host probes with `GetProcAddress`), so adding one never breaks an existing
plugin. That shape is from NMSExtender, which is MIT, so it may be copied.

## The probe itself

- The probe is **on demand only** now (`InstProbeOnSave=0`). Trigger it by creating
  `<game>\Binaries\NMSLogger\probe.now`; the thread checks once a second, deletes it
  and runs.
- `runNo` restarts at 1 each session, so `instprobe_1.txt` **overwrites** the
  previous session's. Either name reports by timestamp or do not leave a report you
  care about in place. A reference copy of the 2026-09-27 run that settled the layout
  is worth keeping outside that directory.
- Report empty stores (see (2)).

## Mod manager

- ~~Track the third-party binaries~~ **done 2026-09-27.** It was worse than
  recorded here: the whole 7-Zip bundling change had never been applied, so 0.1.0
  shipped without it and `find_7z` never looked in the bundle. Now bundled,
  licensed, documented and guarded by two tests. See `docs/packaging.md`.
- Install the NSIS bundle and exercise the installed layout. It is the only untested
  difference from the verified `target/release` tree.
- Consume a **build -> libMBIN version map** instead of matching release tags by
  hand. NMSModBuilder maintains `Common/cmkNMSReleases.txt` as
  `release, name, date, mbin version, mbin tag`, merged from GitHub at runtime. It is
  a table of public facts, but that project is AGPL with a no-compete clause, so
  fetching it is fine and vendoring it is a decision to make on purpose.
- ~~The A/B that verified the memory fix ran on a clean library~~ **covered
  2026-09-27** by `a_clash_is_still_a_clash_with_properties_on_disk` and its
  disjoint mirror, both verified by deliberately breaking `props_of` and watching
  the clash become "disjoint". Still worth running the real A/B against a library
  with genuine clashes if one ever exists, but the silent-false-negative path is
  no longer untested.
- `hook.saveBytes` is misleading: the hook only ever sees `mf_*.hg` writes, never the
  main save file, which is likely memory-mapped. Either measure it properly or stop
  reporting it as if it were the save size.

## Housekeeping

- Rotate the Nexus API key. It was printed in plaintext into a session transcript on
  2026-09-27 while looking up library paths. Nothing left the machine.
