# Open work

Kept in rough priority order within each section. Items are written so that the
*next* action is obvious and so that anything already ruled out stays ruled out --
several of these were expensive to learn and are easy to re-attempt by accident.

Last updated 2026-09-27.

## THE API IS NOW ITS OWN PRODUCT -- read this before touching hook/

Nick, 2026-09-28: the plugin API is a standalone product. Anomaly may install
it, list installed plugins and manage them, but people must be able to use it
without Anomaly.

**Repo: `206airmail/nms-atlas`** (private, MIT, fresh history).
Local: `D:\Users\brown\Documents\Misc Stuff\Atlas`.

**The constraint that forced the split:** the DLL loads because it is named
`xinput9_1_0.dll`. Only ONE mod can occupy that filename. So Atlas and
Anomaly's hook cannot both be installed, and "separate them and both keep
working" was never available.

**Therefore: Anomaly's session recording becomes an Atlas PLUGIN.** Not a
compromise -- Anomaly then consumes the same public contract as a third party
and cannot get privileged access plugin authors lack.

### What lives where

| tree | what it is |
|---|---|
| `Atlas/` (separate repo) | **the product.** The host, the API, the example plugin. |
| `plugins/anomaly_recorder/` | Anomaly's session recording, **as an Atlas plugin**. |
| `hook/` | **a research DLL only** -- `metaprobe.cpp` and `nopause.cpp`. |

The duplicated API copies under `hook/` are gone; Atlas is the only place
`gamestate` and `plugins` exist.

### Why `hook/` still builds, which I got wrong at first

The first version of this plan said to delete `hook/`'s API copies as though
`hook/` were finished. It is not: **`metaprobe.cpp` has to run inside the
game**, and Atlas now owns the only filename the game will load. So `hook/`
stays buildable as a research DLL you swap into the proxy slot temporarily
when you need the from-nothing heap scan, then swap back out.

In practice that should be rare now. The bootstrapping path is
`tools/nms_xrefs.py` (offline, finds the global by signature) ->
`tools/nms_live_walk.py` (external, via ReadProcessMemory). The 8 GB in-game
scan is only needed if the signature itself ever stops matching.

### The port

1. ~~`anomaly_recorder` as an Atlas plugin~~ **done 2026-09-28.** Same config
   file, same log path, same pipe protocol, so the Anomaly app needs no change.
   Brings its own MinHook. **One permanent tradeoff:** hooks now install when
   Atlas loads the plugin rather than in `DllMain`, so a few of the earliest
   file opens are no longer recorded. Module loads are caught up on install.
   See `plugins/anomaly_recorder/README.md`.
2. ~~`nopause.cpp` becomes its own plugin~~ **done 2026-09-28**, as its own
   repository: `206airmail/nms-nopause` (private, MIT). It is the first plugin
   that CHANGES the game rather than reading or writing its data, and it proves
   a useful point: it touches nothing inside NMS, so a game patch cannot break
   it -- the opposite of anything built on the player state. It exports only
   Start and Stop, because it does not care when a save loads, and brings its
   own MinHook. Removed from `hook/`.

   Note it had been silently OFF since Atlas took the proxy slot, because it
   lived in the combined DLL.
3. ~~An inventory sorter~~ **done 2026-09-28**, as its own repository:
   `206airmail/nms-inventory-sorter` (private, MIT). F9 packs an inventory into
   its owned cells grouped by the game's own item category; F10 undoes it.

   It writes grid positions and nothing else, so the worst a bug in it can do
   is arrange someone's things badly. It is the first plugin to use Atlas's
   grouped addressing, so it can sort ships (`ship:*`) as well as the exosuit.

   **Built and its exports verified; NOT yet run in-game.** That needs a human
   looking at a screen.
4. `metaprobe.cpp` stays a research tool; it is not a plugin. See above.
5. ~~Anomaly learns to install Atlas~~ **done 2026-09-28.** `engine/hook.rs`
   now installs and removes BOTH halves, and "installed" means both -- Atlas
   alone records nothing while looking exactly like a working install, so one
   half must not read as one. It recognises three slot owners (Atlas, the
   pre-split combined DLL, foreign) and upgrades from the old single DLL
   **without** asking permission, because that file is ours. 18 tests.
   Still to do: list/enable/disable plugins in the UI.

### The game's transfer range -- the open measurement

Nick, 2026-09-28: the game only lets you move items into a starship (or any
other remote inventory) when you are close enough, and the distance depends on
installed technology. A tool acting on live memory has to honour that.

This is the first NMS inventory tool for which the rule is even answerable --
every existing one edits the save file with the game closed, where "how far is
the player from the ship" is not a question. So there is no prior art to copy,
including in `altmank/NoMansSky-Inventory-Manager-Save-Editor`, which has no
proximity handling of any kind and is right not to.

**Shipped in Atlas 0.6.0, partially.** `GetInventoryReach` returns 1 for the
four inventories carried on the player, and **-1 (not knowable)** for
everything else. `MoveElement` refuses a move involving anything not on the
player unless `AllowRemoteTransfer=1`. The sorter is deliberately exempt:
rearranging one container is not a transfer.

**Still to measure, when the game is next up** -- the plan is in Atlas's
`docs/reach.md`. Player position, then target position, then the threshold by
bisection against the UI. The threshold is the part that must be *observed*
rather than reasoned about: it is a design decision inside the game, and a
guessed number would refuse legitimate transfers and look like a bug.

Better still would be reading the game's own decision rather than reproducing
it, since the UI greys the button out and something must set that. Larger job;
should not block the bisection.

### A live inventory manager

Nick, 2026-09-28: eventually something like
`altmank/NoMansSky-Inventory-Manager-Save-Editor` (MIT, source in Downloads),
but driven by our plugin instead of by reading the save file.

Worth noting what that changes, because it is more than the data source. That
tool is a *distribution* engine -- bucket rules that drain named sources into
named stores, a plan the user approves, then a write. Ours would do the same
against a running game, which means: no decode/encode round trip, no "close the
game first", edits visible immediately -- and the reach rule above becomes
load-bearing rather than absent, because every one of those moves is a transfer.

Depends on: the reach measurement, and an Anomaly UI that can talk to a plugin
(see the hotkeys/settings work below).

### How Anomaly gets Atlas: bundle now, offer updates later

Decided 2026-09-28 (Nick). Anomaly ships a reviewed copy of Atlas, and once
the Atlas repo is public it will additionally **offer** newer releases rather
than requiring an Anomaly release for every Atlas update.

Why not fetch-only, which was the instinct: Anomaly does not merely install
Atlas, it ships `anomaly_recorder.dll` **built against a specific
`ATLAS_API_VERSION`**. Fetching "latest Atlas" could therefore stop the
recorder loading, and the symptom would be "recording silently stopped". So an
update check must be gated on an API version the bundled recorder can still
talk to -- a negotiation, not a download. Bundling also keeps the review
posture Anomaly already has for MBINCompiler, hgpaktool and 7-Zip: executable
code that goes into the user's game process is pinned and recorded in
`tools/THIRD-PARTY.md`.

Deferred until Atlas is public (nothing can be fetched from a private repo):

- Atlas publishes release metadata carrying its version, its
  `ATLAS_API_VERSION` and a checksum.
- Anomaly checks the releases API, compares against the installed copy (Atlas
  exports `AtlasVersion()`, so the installed version is readable), and offers
  an upgrade only when the API version is one the recorder supports.
- Never install silently.

The urgency is lower than it looks: Atlas re-derives the global by signature
rather than a fixed address, and warns on an image-stamp mismatch instead of
reading garbage, so a game patch degrades loudly.

A backup of the old combined hook sits at
`<game>\Binaries\xinput9_1_0.anomaly.dll` if a fallback is ever needed.

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

### 1. ~~Does a brand-new save start with 24 exosuit slots?~~ ANSWERED 2026-09-28

**Yes -- 24.** Nick read it off a fresh save and the live mask agrees (`24/24`
where the veteran save reads `24/31`). So `miCapacity` on index 0 is frozen at
its starting value.

It is frozen *only* there, which is the part the fresh save corrected: exosuit
tech reads `10/10` fresh against `18/18` veteran and the freighter `16` against
`33`, so `miCapacity` tracks the mask everywhere else. It is not a
creation-time constant in general. **Prefer the mask** -- it is right in all 33
stores in both saves and equals the save's `ValidSlotIndices` 25 for 25.

Also settled in the same run, and it was a wrong prediction of mine: the
exocraft `+8` is not an anomaly. Those seven read the identical `miCapacity`
sequence with identical contents in *both* saves, in neither of which any
exocraft is owned. They are default templates, so neither number ever described
the player. Nothing to explain.

### 1b. WHICH INVENTORIES CAN THE PLAYER ACTUALLY REACH? -- answered, and it
### refuted a rule we had recorded as verified

`len(ValidSlotIndices) > 0` is **not** an ownership test. A fresh save owning no
freighter and no base still reads ten chests at 50, the Corvette cache at 160,
FishPlatform at 60 and the freighter at 16. The old rule was only ever checked
against a save where everything was owned.

A byte-for-byte diff of all 33 stores across both saves shows ownership is not
in `cGcInventoryStore` at all: owned-but-empty chests are **identical** to
chests in a save owning none. The real tests, from the labelled save:

| inventory | test |
|---|---|
| `ChestN` | `^FRE_ROOM_STORE{N-1}` in a `FreighterBase` in `PersistentPlayerBases` |
| freighter | `CurrentFreighter.Filename != ""` |
| ships | `ShipOwnership[i].Resource.Seed[0] is true` (1 fresh, 3 veteran) |

**Trap:** `CurrentFreighter.Seed[0]` is `true` with no freighter, so the rule
that works for ships is a false positive on the freighter.

Open: all three of these have been read from the *save*. Finding them in live
memory is the remaining work, and it is what an accessibility query in the API
needs.

### 1c. Old question, kept because it is still unresolved

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

### 2. ~~Map index -> inventory, once~~ DONE, and independently confirmed

Built 2026-09-27 (`docs/inventory-index-map.md`), reproduced on a second save
2026-09-28, and cross-checked **25 of 25** against a labelled export from the
same game state -- grid and mask both, nothing fitted, the map predates the
export. `python tools/nms_crosscheck_map.py <dump> <save>` re-runs it.

Only index 28 remains unidentified. It reads `1x1/0` in a fresh save and
`8x4/32` in the veteran one, so it is something the player **acquires**, and it
is the only member of its `+0x7C` kind (`0x0C`). That narrows it a lot.

The historical detail below is kept because the offsets still hold.

### 2b. The original note

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

### 3. ~~Stop scanning~~ DONE 2026-09-28 -- and the constructor hook is now easier

The 8 GB walk is retired. `cGcPlayerState` is reachable through one static
global:

    exosuit = *(NMS.exe + 0x06E7AAE8) + 0xC2D0      // 7.03, stamp 0x6AB0FFC9

Found by scanning `.data` (39.6 MB, milliseconds) for a single qword pointing
into the object -- exactly one hit -- and **confirmed across a process restart**:
the value moved `0x243C5990010 -> 0x266C8B80010`, the RVA and the delta did not.
`tools/nms_verify_pointer.py` re-checks it in one command and warns if the image
stamp changed; `tools/nms_live_walk.py --anchor <hex>` re-derives it after a
patch.

Two lifecycle facts, both found by accident, both load-bearing for a host:

- The object is constructed **before the save loads**. During loading the
  pointer is non-null and every store reads `1x1 / miCap 1 / mask 0`. So a host
  can resolve singletons at startup, but "non-null" is not "world ready".
- It **survives a save switch in place**. Loading another save from the in-game
  menu kept the same address with different contents, so a cached pointer stays
  valid while cached contents silently become another save's.

**The signature is done too, and it removed the need for the hook.**
`tools/nms_xrefs.py` found 26,743 reads and 3 writes to that global; the first
write is the construction site:

    48 89 05 ?? ?? ?? ??   mov [rip+global], rax
    41 B8 ?? ?? ?? ??      mov r8d, 0x0094F120    ; sizeof the object
    48 8B C8               mov rcx, rax

Measured **unique in 54.5 MB of .text** across four variants, including the
shortest with both the displacement and the size wildcarded. It carries the
global's address in its own displacement, so it re-derives the RVA on whatever
build is running. Discovery is now a read-only scan that patches nothing --
a constructor hook would only buy exact timing, and 1 Hz polling buys that for
no risk.

**The global is the ROOT SINGLETON, not `cGcPlayerState`** -- ~9.76 MB, with
read sites reaching 9.6 MB into it, and player inventories only 0.5% in at
`+0xC2D0`. That is why this is a foundation for arbitrary plugins rather than
an inventory trick.

Shipped as `hook/src/gamestate.cpp`; see `docs/game-state-pointer.md`.

### 4. Which copy is authoritative?

`FUEL1 x2163` existed at **eight** addresses -- live state, save-document copies,
probably UI buffers. Nothing may write until we know which is real. (2) probably
answers it: the copy inside the player-state object is the one, and the lattice
identifies it. Confirm by writing to it and watching the UI, as was done once
already from Cheat Engine.

### 5. Loose ends in the layout

- ~~**Exocraft tech stores read `mask popcount == miCapacity + 8`**~~ **EXPLAINED
  2026-09-28 -- there was nothing to explain.** The seven read the identical
  `miCapacity` sequence `[28, 26, 30, 26, 26, 28, 28]` with identical contents in
  both a veteran and a brand-new save, in neither of which any exocraft is owned.
  They are default templates, so neither their mask nor their `miCapacity` ever
  described the player. Do not build on either number.
- **A kind enum at `store+0x7C`** groups inventories by type: `1` exosuit, `2` its
  cargo, `5`/`6` freighter and cargo, `8` all ten chests + rocket locker + fish
  platform + food unit, `9` the magic pair and the Corvette cache, `0x0A` bait box,
  `0x0C` index 28 alone, `0` every technology inventory *and* every unused slot.
  Useful for naming by kind; useless for ownership, and it does not carry the
  parent's class.
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
