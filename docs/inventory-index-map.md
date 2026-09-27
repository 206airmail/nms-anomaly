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

## Reproducing this

1. `InstProbeOnSave=0` in `<game>\Binaries\NMSLogger\config.ini`
2. Load a save and be in world -- at the main menu no inventories exist yet, and a
   run started there finds nothing however long it scans
3. Create `<game>\Binaries\NMSLogger\probe.now`
4. ~160 s later, read the `LATTICE WALK` section of `instprobe_<n>.txt`

`runNo` restarts at 1 each session, so a report worth keeping must be copied out
before the next run overwrites it.
