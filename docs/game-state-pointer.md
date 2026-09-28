# Reaching the game's state

How the API finds No Man's Sky's live data. Measured 2026-09-28 against 7.03
(image stamp `0x6AB0FFC9`). Implemented in `hook/src/gamestate.cpp`.

## The short version

    global   = <the instruction below tells us where>
    root     = *global                       // ~9.76 MB, the game's root singleton
    exosuit  = root + 0xC2D0                 // mInventories[0]
    store[i] = exosuit + i * 0x248           // i in 0..32, see inventory-index-map.md

Two reads. No scan, no code patching, no hook.

## What replaced what

`hook/src/metaprobe.cpp` finds inventories by walking up to 8 GB of heap for
objects of the right shape: **160 seconds when it works, 13 minutes when it
does not**, and the result is an address with no identity attached. It was a
bootstrapping tool and it succeeded — it produced one store address, and that
was enough to find the real mechanism. It is kept for re-deriving offsets after
a patch, not for normal use.

## Finding the global

Scanning `NMS.exe`'s writable `.data` (39.6 MB) for a qword pointing into the
object returned **exactly one hit**: `NMS.exe+0x06E7AAE8`.

The restart test is what makes that a result, and one process cannot show it:

| | first process | second process |
|---|---|---|
| RVA | `NMS.exe+0x06E7AAE8` | same |
| value | `0x243C5990010` | `0x266C8B80010` |
| `+0xC2D0` | 10x12, mask 24, `GPSV3_0000A9 x1…` | identical |

The address moved; the RVA did not.

## Why a signature rather than that RVA

An RVA is worth exactly one game build. The instruction that *assigns* the
global is far more stable, and it carries the global's address in its own
displacement — so finding the instruction re-derives the RVA on whatever build
is running:

    48 89 05 ?? ?? ?? ??     mov [rip+global], rax
    41 B8 ?? ?? ?? ??        mov r8d, 0x0094F120     ; sizeof the object
    48 8B C8                 mov rcx, rax
    E8 ........              call <construct/zero>

Store the pointer, then hand the object and its size to a construct call.

**Uniqueness is measured, not assumed.** Four variants were counted across
54.5 MB of `.text`; all four matched **exactly once**, including the shortest
with both the displacement and the size wildcarded. The short one is used.

`tools/nms_xrefs.py` is what found it, and re-finds it after a patch.

## What the object actually is

Not `cGcPlayerState`. `tools/nms_xrefs.py` counted **26,743 read sites and 3
write sites** across `.text`. With a 4-byte displacement that has to resolve
exactly, expected false positives over the whole section are ~0.001, so those
are all genuine — a global referenced from 26,000 places is the game's root
singleton.

The read sites reach deep into it (`add rcx, 0x579850`, `mov rcx, [rcx+0x925970]`,
`cmp [rbx+0x925EF4], r15d`), and the largest offset seen, `0x925EF4`, falls
inside the `0x94F120` the construction site passes as the size. Confirmed live:
the pointer lands at `AllocationBase + 0x10` — a 16-byte heap header — at the
start of a 400 MB arena.

So player inventories at `+0xC2D0` are 0.5% into a ~9.76 MB aggregate, and the
same anchor should reach many other subsystems. That is what makes this a
foundation for arbitrary plugins rather than an inventory trick.

## Two lifecycle facts that are easy to get wrong

Both were found by accident, and both would have produced confident wrong
answers.

**The object is constructed before a save is loaded.** During loading the
pointer is already non-null and every inventory reads `1x1 / capacity 1 /
empty mask`. A plugin that reads on "pointer is non-null" gets a full set of
plausible, wrong values. `gamestate::Ready()` therefore tests the exosuit's
shape, not the pointer, and `tools/nms_verify_pointer.py` fails rather than
passes on that state deliberately.

**Loading a different save keeps the same object at the same address** and
swaps its contents. A cached pointer stays valid while cached *contents*
silently become another save's. Hence `gamestate::Generation()`: it increments
every time the world becomes ready, so a plugin can distinguish "same world"
from "everything you cached is stale" without diffing anything.

## What this does NOT give you

**Whether the player can open an inventory.** The ownership mask is exactly the
save's `ValidSlotIndices` (checked 25 of 25 against a labelled export), and that
is *capacity*, not access: a brand-new save reports ten 50-slot chests, a
16-slot freighter and a 160-slot Corvette cache the player has no access to. A
byte-for-byte diff of all 33 stores across two saves shows owned-but-empty
chests are **identical** to chests in a save owning none — ownership is not in
the inventory object at all.

It lives in other subsystems: `^FRE_ROOM_STORE{n}` base objects for chests,
`CurrentFreighter.Filename` for the freighter, `Resource.Seed[0]` for ships.
Those are read from the save today; finding them live is open work.

## After a game patch

1. `python tools/nms_xrefs.py` — re-derives the global's RVA from the signature.
   If the signature itself stops matching, the construction site changed and the
   pattern needs rebuilding from the `.data` hunt.
2. `python tools/nms_verify_pointer.py` — one-command check; warns on a stamp
   mismatch.
3. Re-measure the offsets *inside* the object. The signature re-derives the
   global, but `0xC2D0`, `0x248` and the index map are separate measurements and
   a patch can move any of them. `tools/nms_live_walk.py --from-pointer` plus
   `tools/nms_crosscheck_map.py` against a fresh export re-confirms all of it.

## Confirmed in-game, 2026-09-28

First live run of `hook/src/gamestate.cpp`:

    02:21:46  resolved in 79 ms: global NMS.exe+0x06E7AAE8, object 0x94F120 bytes
              (matches the measured RVA)
    02:22:08  world ready (generation 1) -- -> 0x1C0BDBA0010, ready

**79 ms against the probe's 160,953 ms** — about 2,000x — and it returns named
indices rather than anonymous addresses. `0x1C0BDBA0010` is a third distinct
object address across three processes, with the RVA unchanged each time.

The self-test reproduced the whole index map from inside the game: exosuit
`10x12/24`, tech `10x6/10`, multitool `7x3/8`, freighter `7x5/16`, ten `10x6/50`
chests at 12–21, the `48`-slot magic pair, RocketLocker `7x3/21`, FishPlatform
`10x6/60`, the `1x1` bait/food pair, CorvetteStorage `10x16/160`. The `kind`
values came out as predicted: `1` exosuit, `5` freighter, `8` the chest family,
`9` the magic pair and Corvette cache, `10` bait box, `0` every technology
inventory.

Index 28 is correctly absent — `1x1/0` in a fresh save, and the self-test skips
stores with neither slots nor contents.
