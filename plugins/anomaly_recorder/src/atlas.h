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
 * WRITING
 * -------
 * Atlas can change an inventory as well as read it, and that took a deliberate
 * experiment to justify. A single item stack had been observed at EIGHT
 * addresses in one process -- live state, save-document copies and what look
 * like UI buffers -- so a write could easily have landed somewhere that looked
 * correct and did nothing.
 *
 * Measured 2026-09-28 against the copy this API exposes, in this order,
 * because each step proves something the previous one does not:
 *
 *   - the open inventory screen followed ten consecutive writes live, with no
 *     reopen, so it is what the game DISPLAYS;
 *   - across those ten writes the slot always still held our previous value,
 *     so nothing else rebuilds it -- it is not a downstream cache;
 *   - the written value survived a save, a full process restart and a reload,
 *     so it is what the game SERIALISES;
 *   - harvesting 34 more of the item took 4321 to 4355, so the game's OWN
 *     logic reads and updates the same field.
 *
 * That last one is what makes writing safe rather than merely possible: the
 * game and a plugin share one field, so a plugin that rearranges an inventory
 * will not be silently undone the next time the player picks something up.
 */

#ifndef ATLAS_H
#define ATLAS_H

#include <stdint.h>
#include <wchar.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ALPHA. The API is not stable and makes no compatibility promise.
 *
 * Nothing is distributed to end users yet, so this number goes up whenever the
 * shape changes and plugins are simply rebuilt. Do not contort the design to
 * preserve a promise nobody is relying on -- if a call is wrong, change it.
 *
 * At public release this becomes 1 and the rules below start to bind:
 *
 *   - new fields are APPENDED to AtlasApi, never inserted (a shifted function
 *     pointer makes an old plugin call the wrong function through a
 *     valid-looking struct);
 *   - structs Atlas writes into are NEVER grown, because the host would write
 *     past the end of an older plugin's buffer;
 *   - anything that must change in a way breaking either rule waits for a
 *     major version, and plugin authors rebuild.
 *
 * Both of those rules were learned here rather than assumed: the first cost a
 * launch when SetElementAmount went in the middle of the struct. They are
 * written down now so that v1 starts with them already in force. */
#define ATLAS_API_VERSION 6

/* The oldest plugin Atlas will load.
 *
 * A plugin declares the header it was built against, and Atlas refuses one
 * that is too old BEFORE handing it anything. That is not bureaucracy: while
 * the API is alpha, calls change SHAPE, and a plugin built against version 5
 * calling a version 6 function passes its arguments in the wrong registers.
 * ReadInventory gained a parameter in 6, so an old caller's `out` pointer
 * lands in the register the new one reads `index` from -- and Atlas would
 * write a struct through whatever happened to be in the third register.
 *
 * Refusing to load is a log line. The alternative is a crash whose cause is
 * three layers away from its symptom. */
#define ATLAS_MIN_PLUGIN_API_VERSION 6

enum {
    ATLAS_LOG_DEBUG = 0,
    ATLAS_LOG_INFO  = 1,
    ATLAS_LOG_WARN  = 2,
    ATLAS_LOG_ERROR = 3,
    ATLAS_LOG_FATAL = 4
};

/* Which of the game's inventory arrays an inventory lives in.
 *
 * WHY THIS IS NOT ONE FLAT INDEX
 * ------------------------------
 * The player's stores are not one array. They are six fixed-size arrays laid
 * end to end inside the same object, with 16-byte members sitting between some
 * of them, and the game addresses each array from its own zero. Ship number 0
 * is `mShipInventories[0]`, not "inventory 47".
 *
 * Atlas did present them as one flat run, and that was a bug waiting for a
 * patch. `mInventories` grew from 28 entries at game version 4.13 to 33 at
 * 7.03 -- so every flat index above it MOVED BY FIVE, and a plugin holding
 * "inventory 47" would have carried on writing, to a different ship. Naming
 * the array makes that impossible: (ATLAS_INV_SHIP, 0) is the first ship on
 * any build where the arrays exist at all.
 *
 * Measured 2026-09-28; docs/inventory-arrays.md records how each was pinned. */
enum AtlasInventoryGroup {
    /* Exosuit, its cargo and technology, the freighter, the storage chests,
     * the Corvette cache. 33 entries at 7.03. Index 0 is the exosuit. */
    ATLAS_INV_GENERAL      = 0,
    ATLAS_INV_VEHICLE      = 1,   /* exocraft, 7 */
    ATLAS_INV_VEHICLE_TECH = 2,   /* exocraft technology, 7 */
    ATLAS_INV_SHIP         = 3,   /* starships, 12 */
    ATLAS_INV_SHIP_CARGO   = 4,   /* starship cargo, 12 */
    ATLAS_INV_SHIP_TECH    = 5,   /* starship technology, 12 */
    ATLAS_INV_GROUP_COUNT  = 6
};

/* AN ENTRY EXISTING DOES NOT MEAN THE PLAYER HAS ONE.
 *
 * Every array is fully allocated from the first second of a new save. In a save
 * owning no exocraft at all, all seven ATLAS_INV_VEHICLE_TECH entries are
 * populated with Fusion Engines and Exocraft Boosters -- they are default
 * templates, byte-identical in a fresh save and a veteran one. Unowned ship
 * slots read 1x1 with an empty ownership mask, kept in place so that ship
 * indices stay put when one is sold.
 *
 * So iterating a group and acting on everything in it will cheerfully sort
 * seven exocraft the player does not have. Ownership is not recorded in the
 * inventory object AT ALL: a byte-for-byte diff of every store across two saves
 * found owned-but-empty storage chests identical to chests in a save owning
 * none. It lives in three unrelated subsystems elsewhere in the save.
 *
 * The usable proxy, and what it is worth: an entry with a grid bigger than 1x1
 * and a non-empty ownership mask is one the game has set up. That is true of
 * every store the player can open, and also true of the exocraft templates.
 * Treat it as "not obviously absent", never as "owned". */

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
    int32_t  group;            /* AtlasInventoryGroup -- which array */
    int32_t  index;            /* position WITHIN that array, counting from 0 */
    uint64_t address;          /* the store itself, for callers reading further */
    uint32_t width, height;    /* grid, as the store was built */
    int32_t  startingCapacity;
    int32_t  ownedSlots;       /* cells the store has; -1 if unreadable */
    int32_t  usedSlots;        /* elements actually present */
    int32_t  allocatedSlots;   /* vector capacity -- NOT the element count */
    uint32_t kind;
    uint64_t elements;         /* element array */
} AtlasInventory;

/* One occupied slot. x and y are the grid position and match the UI exactly.
 *
 * FROM v1, DO NOT ADD FIELDS TO THIS STRUCT. Atlas WRITES INTO memory the
 * caller owns, so growing it would make the host write past the end of an
 * older plugin's buffer -- a far worse break than a shifted function pointer,
 * because it corrupts the plugin's stack rather than merely misbehaving.
 * Appending a function pointer to AtlasApi is recoverable; growing a struct
 * the host fills is not. After v1, more per-element detail means a new call
 * with its own new struct, leaving this one exactly as it is.
 *
 * While the API is alpha this is simply rebuilt, which is why max_amount and
 * type could be added -- the sorter wants the game's own category rather than
 * guessing from id prefixes, and CATALYST1 being Sodium while CATALYST2 is
 * Sodium Nitrate shows how badly that guessing goes. */
typedef struct AtlasElement {
    char    id[24];            /* e.g. "FUEL1"; a leading '^' means installed tech */
    int32_t x, y;
    int32_t amount;
    int32_t max_amount;        /* what this stack can hold */
    int32_t type;              /* the game's own category for the item */
} AtlasElement;

/* Atlas's services. Check structSize before using anything documented as
 * added in a later version.
 *
 * NEW FIELDS GO AT THE END -- binding from v1, and worth keeping to now.
 *
 * structSize only protects a plugin built against an older header if the
 * fields it already knows about stay where they were. Inserting one in the
 * middle shifts every pointer after it, and the plugin then calls the wrong
 * function through a perfectly valid-looking struct.
 *
 * This is not hypothetical: SetElementAmount was first added before
 * GetGameRoot, and the recorder plugin -- built against v1 -- called
 * SetElementAmount when it meant to ask for the game's folder, failed to
 * start, and was unloaded. It cost one game launch to find because a v1 plugin
 * happened to be installed. Keep one around. */
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

    /* How many entries the named group has, or 0 if it is not a group Atlas
     * knows. Iterate a group with this; do not hard-code 33. */
    int32_t (*GetInventoryCount)(int32_t group);

    /* Read one inventory, addressed by (group, index). Returns 1 on success.
     *
     * An AtlasInventory filled in here is the HANDLE for everything else: the
     * write calls take it rather than a pair of integers, so addressing is
     * stated once, at the one point where it can be checked.
     *
     * Atlas validates a handle's address against the group and index it
     * carries and refuses one that does not match, so a fabricated or
     * corrupted handle is rejected rather than pointed at arbitrary memory.
     * That is a structural check, not a freshness one: a handle from the
     * PREVIOUS save passes it perfectly and describes another save's contents.
     * Re-read after a generation change. */
    int32_t (*ReadInventory)(int32_t group, int32_t index, AtlasInventory* out);

    /* Read one occupied slot. Returns 1 on success, 0 on failure.
     *
     * `slot` is a position in the element array, not a grid cell, and means
     * nothing once that array next changes. */
    int32_t (*ReadElement)(const AtlasInventory* inv, int32_t slot, AtlasElement* out);

    /* ---- paths, for plugins that keep their own files ---- */
    const wchar_t* (*GetGameRoot)(void);    /* folder containing Binaries\ */
    const wchar_t* (*GetPluginDir)(void);   /* where plugins are loaded from */

    /* ---- writing (API version 2 and later) ----
     *
     * Set one slot's amount. Returns 1 on success, 0 if it was refused.
     *
     * `expect_id` is required and is not a convenience: pass the item id you
     * believe is in that slot (e.g. "FUEL1"). If the slot holds something else
     * the write is refused. Between your reading an inventory and writing to
     * it the player may have moved the stack, dropped it, or loaded another
     * save entirely -- and the element array is reused, so the address stays
     * perfectly valid while meaning something different. Without this check a
     * stale read turns into a write onto an unrelated item.
     *
     * The amount is clamped to the slot's own maximum rather than refused,
     * because that is what the game does to its own values.
     *
     * Writes race the game: it is running, on its own threads, and may be
     * adding to the same stack. A 4-byte aligned store cannot tear, so you
     * will never produce a nonsense number -- but you can lose a concurrent
     * update, so do not use this to implement a counter. Read, decide, write.
     *
     * Check structSize before calling: a host older than this header does not
     * have it. */
    int32_t (*SetElementAmount)(const AtlasInventory* inv, int32_t slot,
                                const char* expect_id, int32_t amount);

    /* Move a slot to a grid position (API version 3 and later).
     *
     * x and y are the cell the player sees; they match the screen exactly.
     * Returns 1 on success, 0 if refused.
     *
     * `expect_id` works as it does for SetElementAmount, and for the same
     * reason: the element array is reused, so a stale read would otherwise
     * move an unrelated item.
     *
     * THE CELL MUST BE ONE THE PLAYER OWNS. An inventory's grid is larger than
     * the part that has been unlocked, and an item placed in a cell the player
     * has not bought is not reachable -- from their side it has simply
     * vanished. Atlas checks the ownership mask and refuses rather than
     * quietly losing someone's cargo.
     *
     * COLLISIONS ARE NOT CHECKED, deliberately. Applying any permutation one
     * element at a time passes through states where two items share a cell,
     * so refusing them would make rearranging impossible. The game reads these
     * values per frame, so the worst case is a frame or two of overlap while a
     * sort is applied. Work out the whole target layout first, then write it.
     *
     * Position survives a save and a reload, so a layout written here is not
     * undone by reloading. */
    int32_t (*SetElementPosition)(const AtlasInventory* inv, int32_t slot,
                                  const char* expect_id, int32_t x, int32_t y);

    /* Does the player own this cell? (API version 4 and later.)
     *
     * Returns 1 if the cell is unlocked and usable, 0 otherwise.
     *
     * An inventory's grid is LARGER than the part that has been unlocked: the
     * exosuit reports 10x12 while its owner may own 24 cells of it. So "fill
     * the grid left to right" is wrong, and anything laying items out has to
     * know which cells are real. Without this a plugin would have to discover
     * them by attempting placements and seeing which are refused, which is
     * guessing by exception.
     *
     * ownedSlots on AtlasInventory is the COUNT of owned cells. This is which
     * ones. */
    int32_t (*IsCellOwned)(const AtlasInventory* inv, int32_t x, int32_t y);

    /* Move a whole stack to another inventory (API version 6 and later).
     *
     * Returns 1 on success, 0 if refused. Pass x = -1 to drop it in the first
     * free cell the player owns; pass a cell to choose one.
     *
     * THIS IS A DIFFERENT KIND OF OPERATION TO THE TWO ABOVE. Amount and
     * position are single fields -- write four bytes and stop. Moving a stack
     * is STRUCTURAL: the element array holds exactly `usedSlots` entries, so
     * the stack has to be appended to the destination and its size grown, then
     * removed from the source and its size shrunk, while the game reads both
     * arrays from its own threads.
     *
     * IT REFUSES WHEN THE DESTINATION IS FULL, and that is not a policy
     * choice. Appending is only safe because the destination's vector already
     * has spare capacity (`allocatedSlots` above `usedSlots`); growing it would
     * mean reallocating memory the game owns and freeing the block it is
     * reading from. There is no safe way to do that from here, so a full
     * destination is refused rather than attempted.
     *
     * THE ORDER IS DELIBERATE: appended to the destination first, then removed
     * from the source. Between those two writes the stack exists TWICE, for
     * microseconds. The other order would have it exist nowhere, and anything
     * going wrong at that instant would destroy it outright. Transient
     * duplication is recoverable; transient deletion is not.
     *
     * AFTER A SUCCESSFUL MOVE, EVERY SLOT NUMBER YOU HOLD FOR THE SOURCE IS
     * STALE. Removal copies the array's last element over the vacated slot and
     * shrinks the size, so one other stack changes slot number. Re-read the
     * source. Grid positions are untouched -- the array is a bag, and position
     * is what orders the display.
     *
     * IT ALSO RESPECTS THE GAME'S OWN REACH LIMIT, which is the one refusal
     * here that is about play rather than about memory. The player cannot put
     * things into a starship, freighter, exocraft or storage container they
     * are not standing near, and the allowed distance depends on installed
     * technology. Atlas cannot measure that distance yet, so it allows what it
     * can prove is fine -- both ends carried on the player's own body -- and
     * refuses everything else unless AllowRemoteTransfer is set in atlas.ini.
     * Ask GetInventoryReach first rather than discovering it from a refusal. */
    int32_t (*MoveElement)(const AtlasInventory* from, int32_t slot,
                           const char* expect_id,
                           const AtlasInventory* to, int32_t x, int32_t y);

    /* Can the player reach this inventory to move things in or out of it?
     * (API version 6 and later.)
     *
     *    1  yes -- it is carried on the player, so no distance rule applies
     *    0  no
     *   -1  NOT KNOWABLE YET
     *
     * The three-valued answer is deliberate and -1 is the common one today.
     * The game gates transfers into a ship, freighter, exocraft or container
     * by how close the player is standing, with the range depending on
     * installed technology. Neither the player's position nor the ship's has
     * been located in the root singleton, so Atlas genuinely does not know --
     * and "I do not know" is not "no".
     *
     * That distinction matters to a plugin. Treating -1 as no refuses every
     * legitimate transfer at a landing pad; treating it as yes is a decision
     * to bypass a rule the game enforces. Neither is wrong, but the choice
     * should be made on purpose, so this does not make it for you.
     *
     * Reading is never gated -- a plugin may always LOOK inside any inventory,
     * because looking is what the save editors already do with the file. */
    int32_t (*GetInventoryReach)(const AtlasInventory* inv);
} AtlasApi;

/* ------------------------------------------------------------------ *
 *  What YOUR plugin exports.                                          *
 * ------------------------------------------------------------------ */

/* REQUIRED. Return 0 to stay loaded; anything else and Atlas logs it and
 * unloads you -- which is the supported way to say "not applicable on this
 * build". Keep it short: a plugin that blocks here delays every plugin
 * after it. */
/* REQUIRED. Return ATLAS_API_VERSION -- the version of THIS HEADER that your
 * plugin was compiled against.
 *
 * Atlas reads this before it calls anything else, and refuses to load a plugin
 * that is too old or that does not export it at all. Declare it with the macro,
 * once, at file scope, in exactly one of your source files:
 *
 *     ATLAS_DECLARE_PLUGIN_API_VERSION
 *
 * WHY THE HOST CANNOT SIMPLY COPE. AtlasApi::structSize lets a plugin built
 * against an older header avoid calling functions that did not exist yet, and
 * that works for as long as the functions it DOES know about stay where they
 * are and keep their shape. During alpha neither holds. Version 6 gave
 * ReadInventory a parameter, so a version 5 plugin calling it passes `out` in
 * the register the host now reads `index` from -- and the host then writes a
 * struct through whatever the caller happened to leave in the third register.
 * structSize cannot see that; nothing about the struct changed. Only the plugin
 * knows which header it was built from, so only the plugin can say. */
#define ATLAS_PLUGIN_API_VERSION_NAME "AtlasPluginApiVersion"
typedef int32_t (*AtlasPluginApiVersionFn)(void);

#ifdef __cplusplus
#define ATLAS_DECLARE_PLUGIN_API_VERSION \
    extern "C" __declspec(dllexport) int32_t AtlasPluginApiVersion(void) { \
        return ATLAS_API_VERSION; \
    }
#else
#define ATLAS_DECLARE_PLUGIN_API_VERSION \
    __declspec(dllexport) int32_t AtlasPluginApiVersion(void) { \
        return ATLAS_API_VERSION; \
    }
#endif

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
