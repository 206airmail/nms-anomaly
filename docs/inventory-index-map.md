# The live inventory index map

Measured 2026-09-27 against No Man's Sky 7.03 by walking the `cGcInventoryStore`
lattice inside the runtime `cGcPlayerState` and matching each slot against a
decompiled save.

**Why an index map is worth having:** the internal offsets of `cGcPlayerState` are
stable across process restarts. Two runs in two different processes put the exosuit
at index 0 and the first exocraft store exactly `0x5B50` further on, every time.
Only the base moves. So this map is built once and holds.

    sizeof(cGcInventoryStore) = 0x248 (584 bytes)
    mInventories has 33 entries in 7.03 (ReNMS records 28 at 4.13 -- it grew)

## `mInventories`, indexed from the exosuit at 0

Confidence is stated per row, because it differs and the difference matters:

- **contents** -- the live item list matches that container's items in the save
- **shape** -- the (width, height, owned) triple is unique among all 27 containers
- **position** -- shape is ambiguous, but the slot's place in a run settles it

| idx | grid | owned | inventory | how |
|---|---|---|---|---|
| 0 | 10x12 | 31 | `Inventory` (exosuit general) | contents |
| 1 | 10x6 | 18 | `Inventory_TechOnly` | shape |
| 2 | 7x5 | 0 | `Inventory_Cargo` | position |
| 3 | 8x3 | 24 | `WeaponInventory` (multitool) | shape |
| 4, 5, 6 | 1x1 | 0 | unowned | - |
| 7 | 7x5 | 33 | `FreighterInventory` | shape |
| 8 | 7x3 | 19 | `FreighterInventory_TechOnly` | shape |
| 9 | 7x5 | 0 | `FreighterInventory_Cargo` | position |
| 10, 11 | 1x1 | 0 | unowned | - |
| 12 | 10x6 | 50 | `Chest1Inventory` | contents |
| 13 | 10x6 | 50 | `Chest2Inventory` | contents |
| 14 | 10x6 | 50 | `Chest3Inventory` | contents |
| 15 | 10x6 | 50 | `Chest4Inventory` | position |
| 16 | 10x6 | 50 | `Chest5Inventory` | position |
| 17 | 10x6 | 50 | `Chest6Inventory` | contents |
| 18..21 | 10x6 | 50 | `Chest7..Chest10Inventory` | position |
| 22, 23 | 10x6 | 48 | `ChestMagicInventory`, `ChestMagic2Inventory` | shape (as a pair) |
| 24, 25 | 1x1 | 0 | unowned | - |
| 26 | 10x6 | 50 | `CookingIngredientsInventory` | position |
| 27 | 7x3 | 21 | `RocketLockerInventory` | shape |
| 28 | 8x4 | 32 | **unidentified** -- see below | - |
| 29 | 10x6 | 60 | `FishPlatformInventory` | shape |
| 30, 31 | 1x1 | 1 | `FishBaitBoxInventory`, `FoodUnitInventory` | shape (as a pair) |
| 32 | 10x16 | 160 | `CorvetteStorageInventory` | shape |

The chest run is the strongest result here: **index = chest number + 11**, ten
consecutive identical `10x6` stores with 50-bit masks. Four were pinned by contents
and the other six follow from being in the run. Chest 4, 5, 7, 8, 9 and 10 are all
empty, which is exactly why a scan that requires an item could never see them --
they exist in this map only because the walk reads by position.

### After index 32

Indices 33 and up read as `0x0` grids with plausible-looking masks. They are **not**
inventories: `mInventories` is followed by a 16-byte `cTkVector`
(`mShelvedInventories`), so every later read is misaligned by 16 bytes into
`mVehicleInventories`. The exocraft stores are found on their own lattice offset by
`+0x10`, and the ship tech stores at `+0x20` (two intervening vectors). Do not trust
anything the walk prints past 32 without re-anchoring.

## Two open entries

**Index 28, `8x4`, 32 owned.** No container in the save's 27 has that shape. Either
it is a type this save has never instantiated, or it is live-only and not
serialised. Worth identifying before anything writes by index.

**Index 0's owned count.** The save says `Inventory` has 30 valid slots; the live
mask says 31. That is not a discrepancy -- it is the slot bought between the save
dump and the run, and it is the observation that proved the mask tracks ownership
while `miCapacity` does not (it still reads 24).

`GraveInventory` and `ShipInventory` are both `0x0` with no slots in the save and
were not matched. They are presumably among the `1x1` unowned entries.

## Confirmed against a SECOND save (2026-09-28)

The map above was measured on a veteran save. It has now been walked on a
brand-new one, and **every index agrees**. That is what makes it an asset
rather than a description of one save file. Full record:
`docs/measurements/lattice-walk_2026-09-28_fresh-save.txt`.

The only entries that differ are the ones that have to:

| idx | veteran | fresh | why |
|---|---|---|---|
| 3 | `8x3`, 24 | `7x3`, 8 | a different multitool |
| 7 | `7x5`, 33 | `7x5`, 16 | freighter not earned |
| 28 | `8x4`, 32 | `1x1`, **0** | **not instantiated in a new save** |

Index 28 was the one unidentified entry in the table. A fresh save reading
`1x1/0` means it is something the player **acquires**, not a type the game
always allocates -- which is the first real constraint on what it can be.

Two things the second save also settled:

* **`miCapacity` tracks the mask almost everywhere.** Exosuit tech reads
  `10/10` fresh against `18/18` veteran, the freighter `16` against `33`. So
  `miCapacity` is not the creation-time constant it looked like. The single
  exception is index 0, the exosuit general inventory, frozen at **24** while
  the mask reads 31 -- and 24 is confirmed to be what a new exosuit starts
  with. Prefer the mask; it is right in every store in both saves.
* **The exocraft `+8` is not an anomaly and not player state.** The seven
  `mVehicleTechInventories` entries read the identical `miCapacity` sequence
  `[28, 26, 30, 26, 26, 28, 28]` with identical contents in *both* saves, in
  neither of which any exocraft is owned. They are default templates, so
  neither their mask nor their `miCapacity` describes ownership. Nothing here
  needs explaining and nothing should be built on those two numbers.

## Reproducing this

**Two seconds, no scan** -- the object is reachable through a static pointer
(see `tools/nms_verify_pointer.py` for the RVA and what confirms it):

    python tools/nms_live_walk.py --from-pointer

**Bootstrapping from nothing**, which is what to do after a game patch moves
the RVA:

1. `InstProbeOnSave=0` in `<game>\Binaries\NMSLogger\config.ini`
2. Load a save and be in world -- at the main menu the object exists but every
   store reads `1x1 / miCap 1 / mask 0`, so a run started there finds nothing
   however long it scans
3. Create `<game>\Binaries\NMSLogger\probe.now`
4. ~160 s later, read the `LATTICE WALK` section of `instprobe_<n>.txt`
5. Feed the exosuit address back in to re-derive the pointer:
   `python tools/nms_live_walk.py --anchor <hex>`

`runNo` restarts at 1 each session, so a report worth keeping must be copied
out before the next run overwrites it; the ones that mattered are in
`docs/measurements/`.

**Careful with the discovery scan's anchor.** On the fresh save the probe
anchored its walk on the *vehicle-tech* array, not `mInventories`, because it
picks the lattice holding the most populated stores -- and the seven exocraft
templates are always populated while a new player's own inventories mostly are
not. The walk was correct and the indices meant something else entirely. Check
what the anchor is before reading indices as names.

## Cross-checked against a labelled save, 25/25 (2026-09-28)

The map is built from process memory, which carries no names. A save exported
from the *same* game state carries names and no addresses. If index -> name is
right, then for every index the live grid and ownership mask must equal that
member's `Width`/`Height` and `len(ValidSlotIndices)`.

    python tools/nms_crosscheck_map.py <store-dump.json> <save.json>

**25 of 25 matched exactly, zero mismatches** -- including all ten chests at
`10x6/50`, the magic pair at `48`, the freighter at `7x5/33`, RocketLocker
`7x3/21`, FishPlatform `10x6/60`, the `1x1/1` bait-box and food-unit pair and
CorvetteStorage at `10x16/160`. Nothing was fitted: the map predates the export.

That settles two claims at once -- that the index names the inventory, and that
the `-0x80` mask is exactly the save's `ValidSlotIndices`.

**It does NOT settle ownership, and cannot.** `ValidSlotIndices` is how many
cells a store has; every container is pre-allocated at full size in a save
owning none of them. See `docs/todo.md` and the ownership notes: chests are
granted by `^FRE_ROOM_STORE{n}` base objects, the freighter by
`CurrentFreighter.Filename`, ships by `Resource.Seed[0]`.
