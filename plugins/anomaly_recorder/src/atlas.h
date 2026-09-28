/* atlas.h -- the contract between Atlas and a plugin.
 *
 * Copy this one file into your plugin. It has no dependencies beyond the C
 * standard headers, is valid C and C++, and is the ONLY thing you need: there
 * is no import library and nothing to link against. Atlas hands you a pointer
 * to an AtlasApi and you call through it.
 *
 * HOW CAPABILITIES WORK, AND WHY
 * ------------------------------
 * A plugin exports AtlasPluginStart. Everything else is OPTIONAL and is found
 * by Atlas with GetProcAddress, by name. Atlas calls what it finds and ignores
 * what it does not.
 *
 * That means:
 *   - a plugin implements only what it cares about;
 *   - Atlas can gain a capability without breaking a single existing plugin,
 *     because an old plugin simply does not export the new name;
 *   - a plugin can be built against a newer header than the Atlas it runs on
 *     -- check AtlasApi::structSize before calling anything added later.
 *
 * The shape is taken from Ant2888/NMSExtender, which is MIT-licensed. The
 * alternative -- one fat interface every plugin must implement -- breaks every
 * plugin each time it grows, which is why it is not used here.
 *
 * WHAT ATLAS GUARANTEES
 * ---------------------
 *   - AtlasPluginStart is called once, on a background thread, after the game
 *     has loaded but very possibly BEFORE a save has been loaded.
 *   - Every call into your plugin is wrapped so a fault inside it is caught,
 *     logged, and disables your plugin rather than killing the game. Do not
 *     rely on this; it is a safety net, not a contract.
 *   - The AtlasApi pointer stays valid for the life of the process.
 *
 * WHAT IT DOES NOT
 * ----------------
 *   - Ordering. Do not assume another plugin has loaded.
 *   - Thread affinity. Callbacks arrive on Atlas's thread, not the game's. If
 *     you touch game memory you are racing the game; reads of inventory data
 *     are cheap and tolerant, writes are NOT supported (see below).
 *   - Per-frame timing. AtlasPluginOnTick is roughly 1 Hz, not a frame hook. A
 *     true per-frame callback needs a Vulkan implicit layer and does not exist
 *     yet; do not busy-wait in a tick trying to fake one.
 *   - Hooking. If your plugin wants to detour a function, bring your own
 *     library. Atlas itself patches no code at all. Note that two plugins
 *     hooking the same function have nobody arbitrating between them.
 *
 * WHY THERE IS NO WRITE API
 * -------------------------
 * A single item stack was observed at EIGHT addresses in one process -- live
 * state, save-document copies, and what appear to be UI buffers. Until it is
 * established which copy the game actually reads, a write could land somewhere
 * that looks correct and does nothing. Offering that would be worse than
 * offering nothing.
 */

#ifndef ATLAS_H
#define ATLAS_H

#include <stdint.h>
#include <wchar.h>

#ifdef __cplusplus
extern "C" {
#endif

#define ATLAS_API_VERSION 1

enum {
    ATLAS_LOG_DEBUG = 0,
    ATLAS_LOG_INFO  = 1,
    ATLAS_LOG_WARN  = 2,
    ATLAS_LOG_ERROR = 3,
    ATLAS_LOG_FATAL = 4
};

/* One of the player's inventory stores.
 *
 * ownedSlots is how many cells the store HAS -- it is exactly the save's
 * ValidSlotIndices, confirmed 25 of 25 against a decompiled save. It is NOT a
 * test of whether the player can open the inventory: a brand-new save reports
 * ten 50-slot storage chests, a 16-slot freighter and a 160-slot Corvette
 * cache the player has no access to. Ownership is not recorded in the
 * inventory object at all -- a byte-for-byte diff of every store across two
 * saves found owned-but-empty chests identical to chests in a save owning
 * none. Do not infer access from any field here.
 *
 * startingCapacity is what the store was built with. It tracks ownedSlots
 * everywhere except the exosuit, which stays at its starting 24 forever.
 * Prefer ownedSlots.
 *
 * kind groups inventories by type (1 exosuit, 2 its cargo, 5 freighter,
 * 8 the storage-chest family, 9 the magic pair and Corvette cache, 0 every
 * technology inventory). It is a type, not an identity, and not ownership.
 */
typedef struct AtlasInventory {
    int32_t  index;            /* position in the array; this is what names it */
    uint64_t address;          /* the store itself, for callers reading further */
    uint32_t width, height;    /* grid, as the store was built */
    int32_t  startingCapacity;
    int32_t  ownedSlots;       /* cells the store has; -1 if unreadable */
    int32_t  usedSlots;        /* elements actually present */
    int32_t  allocatedSlots;   /* vector capacity -- NOT the element count */
    uint32_t kind;
    uint64_t elements;         /* element array */
} AtlasInventory;

/* One occupied slot. x and y are the grid position and match the UI exactly. */
typedef struct AtlasElement {
    char    id[24];            /* e.g. "FUEL1"; a leading '^' means installed tech */
    int32_t x, y;
    int32_t amount;
} AtlasElement;

/* Atlas's services. Check structSize before using anything documented as
 * added in a later version. */
typedef struct AtlasApi {
    uint32_t structSize;       /* sizeof(AtlasApi) as ATLAS knows it */
    uint32_t apiVersion;       /* ATLAS_API_VERSION Atlas was built with */

    /* ---- logging: goes to Atlas's log, tagged with your category ---- */
    void (*Log)(int level, const char* category, const char* message);

    /* ---- game state ---- */

    /* The game's root singleton, or 0 if it has not been resolved. Player
     * inventories live inside it. Re-read it rather than caching: loading a
     * different save keeps the same object and replaces its contents. */
    uint64_t (*GetRoot)(void);

    /* 1 only when a save is actually loaded. A non-zero GetRoot() is NOT
     * enough -- the object exists during loading with every inventory reading
     * 1x1 and an empty ownership mask, so reading then yields a full set of
     * plausible, wrong values. */
    int32_t (*IsWorldReady)(void);

    /* Increments each time the world becomes ready. If it has changed,
     * everything you cached about the world is stale -- including across a
     * save switch, which leaves the pointer itself valid. */
    int32_t (*GetGeneration)(void);

    int32_t (*GetInventoryCount)(void);
    /* Both return 1 on success, 0 on failure. */
    int32_t (*ReadInventory)(int32_t index, AtlasInventory* out);
    int32_t (*ReadElement)(const AtlasInventory* inv, int32_t slot, AtlasElement* out);

    /* ---- paths, for plugins that keep their own files ---- */
    const wchar_t* (*GetGameRoot)(void);    /* folder containing Binaries\ */
    const wchar_t* (*GetPluginDir)(void);   /* where plugins are loaded from */
} AtlasApi;

/* ------------------------------------------------------------------ *
 *  What YOUR plugin exports.                                          *
 * ------------------------------------------------------------------ */

/* REQUIRED. Return 0 to stay loaded; anything else and Atlas logs it and
 * unloads you -- which is the supported way to say "not applicable on this
 * build". Keep it short: a plugin that blocks here delays every plugin
 * after it. */
#define ATLAS_PLUGIN_START_NAME "AtlasPluginStart"
typedef int32_t (*AtlasPluginStartFn)(const AtlasApi* api);

/* Optional. Called when a save becomes ready, and again after each save
 * switch, with the new generation. This -- not AtlasPluginStart -- is where
 * reading player data belongs. */
#define ATLAS_PLUGIN_WORLD_READY_NAME "AtlasPluginOnWorldReady"
typedef void (*AtlasPluginOnWorldReadyFn)(int32_t generation);

/* Optional. Called when a loaded save goes away. */
#define ATLAS_PLUGIN_WORLD_UNLOADED_NAME "AtlasPluginOnWorldUnloaded"
typedef void (*AtlasPluginOnWorldUnloadedFn)(void);

/* Optional. Roughly once a second, on Atlas's thread. NOT a frame hook. */
#define ATLAS_PLUGIN_TICK_NAME "AtlasPluginOnTick"
typedef void (*AtlasPluginOnTickFn)(void);

/* Optional. Called on the way out when Atlas can manage it. Not guaranteed:
 * the game often terminates itself outright, so do not rely on this for
 * anything that must not be lost. */
#define ATLAS_PLUGIN_STOP_NAME "AtlasPluginStop"
typedef void (*AtlasPluginStopFn)(void);

#ifdef __cplusplus
}   /* extern "C" */
#endif

#endif /* ATLAS_H */
