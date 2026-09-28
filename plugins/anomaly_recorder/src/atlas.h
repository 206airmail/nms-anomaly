/* atlas.h -- the contract between Atlas and a plugin.
 *
 * Atlas is a plugin host for No Man's Sky. It loads into the game, finds the
 * game's state, and hands plugins an API for reading and changing it.
 *
 * COPY THIS ONE FILE. It has no dependencies beyond the C standard headers, is
 * valid C and C++, and is all you need: there is no import library, nothing to
 * link against, and no SDK to install. Atlas hands you a pointer to an AtlasApi
 * and you call through it.
 *
 * A COMPLETE PLUGIN
 * -----------------
 *
 *     #include "atlas.h"
 *
 *     ATLAS_DECLARE_PLUGIN_API_VERSION
 *     ATLAS_DECLARE_PLUGIN_INFO("Hello", "1.0.0", "you",
 *                               "prints a line when a save loads")
 *
 *     static const AtlasApi* g_api;
 *
 *     ATLAS_EXPORT int32_t AtlasPluginStart(const AtlasApi* api) {
 *         g_api = api;
 *         return 0;                      // non-zero = "not applicable", unloaded
 *     }
 *
 *     ATLAS_EXPORT void AtlasPluginOnWorldReady(int32_t generation) {
 *         AtlasInventory exosuit;
 *         if (g_api->ReadInventory(ATLAS_INV_GENERAL, 0, &exosuit))
 *             g_api->Log(ATLAS_LOG_INFO, "hello", "the exosuit exists");
 *     }
 *
 * Build it as a DLL, drop it in <game>\Binaries\Atlas\plugins\, done.
 * docs/getting-started.md walks through it; templates/plugin/ is a folder you
 * can copy.
 *
 * THE TWO WAYS TO REACH GAME STATE
 * --------------------------------
 * 1. TYPED CALLS for inventories. ReadInventory, ReadElement, MoveElement and
 *    friends. These exist because inventories are what most plugins want, the
 *    layout took real measurement, and nobody should have to redo it.
 *
 * 2. REFLECTION for everything else. The game ships descriptors for the 2741
 *    classes it serialises, with real member names, offsets and types. Atlas
 *    loads them and lets you read or write any field of any type BY NAME:
 *
 *        AtlasType t;
 *        if (api->FindType("cGcDifficultySettingsData", &t)) {
 *            int32_t always = 0;
 *            api->ReadBool(address, t.id, "InventoriesAlwaysInRange", &always);
 *        }
 *
 *    This is what makes Atlas an API for anything rather than an API for
 *    whatever we happened to write a function for. See docs/reflection.md.
 *
 *    The honest limit: the descriptors cover classes the game SERIALISES.
 *    cGcPlayerState and cGcInventoryStore are runtime-only and have no
 *    descriptor at all, which is exactly why the typed calls above exist.
 *
 * HOW CAPABILITIES WORK
 * ---------------------
 * A plugin exports AtlasPluginStart and AtlasPluginApiVersion. Everything else
 * is OPTIONAL and located by Atlas with GetProcAddress, by name. Atlas calls
 * what it finds and ignores what it does not, so a plugin implements only what
 * it cares about and Atlas can gain a capability without invalidating a single
 * existing plugin.
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
 *   - Reads of game memory never fault the process, however wrong the address.
 *
 * WHAT IT DOES NOT
 * ----------------
 *   - Ordering. Do not assume another plugin has loaded.
 *   - Thread affinity. Callbacks arrive on Atlas's thread, not the game's. If
 *     you touch game memory you are racing the game.
 *   - Per-frame timing. AtlasPluginOnTick is roughly 1 Hz, not a frame hook. A
 *     true per-frame callback needs a Vulkan implicit layer and does not exist;
 *     do not busy-wait in a tick trying to fake one.
 *   - Hooking. If your plugin wants to detour a function, bring your own
 *     library -- Atlas itself patches no code at all. Two plugins hooking the
 *     same function have nobody arbitrating between them. See docs/hooking.md.
 *   - Inter-plugin messaging. A plugin that needs a socket should open one.
 *
 * WRITING
 * -------
 * Atlas can change the game as well as observe it, and that took a deliberate
 * experiment to justify. A single item stack had been observed at EIGHT
 * addresses in one process -- live state, save-document copies and what look
 * like UI buffers -- so a write could easily have landed somewhere that looked
 * correct and did nothing.
 *
 * Measured against the copy this API exposes, in this order, because each step
 * proves something the previous one does not:
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
 *
 * Every write is gated by AllowWrites in atlas.ini. Reads never are.
 */

#ifndef ATLAS_H
#define ATLAS_H

#include <stdint.h>
#include <wchar.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ------------------------------------------------------------------ *
 *  Versioning                                                         *
 * ------------------------------------------------------------------ */

/* The contract version. Atlas 1.0 ships version 1.
 *
 * FROM VERSION 1 THESE RULES BIND:
 *
 *   - new fields are APPENDED to AtlasApi, never inserted. A shifted function
 *     pointer makes an old plugin call the wrong function through a
 *     valid-looking struct -- which is not hypothetical: during development
 *     SetElementAmount went in the middle of the struct and a plugin built
 *     against the previous header called it when it meant to ask for the
 *     game's folder. It cost a launch to find.
 *
 *   - structs Atlas WRITES INTO are never grown. Growing one makes the host
 *     write past the end of an older plugin's buffer, corrupting its stack --
 *     far worse than a shifted pointer, because it is not recoverable. If more
 *     per-element detail is ever needed, it arrives as a new call with its own
 *     new struct, leaving the existing one exactly as it is.
 *
 *   - anything that must break either rule waits for version 2, and plugin
 *     authors rebuild.
 *
 * Check AtlasApi::structSize before calling anything documented as added in a
 * later version. */
#define ATLAS_API_VERSION 1

/* The oldest plugin this Atlas will load. A plugin declares the header it was
 * built against and Atlas refuses one it cannot call safely, BEFORE calling
 * anything. See ATLAS_DECLARE_PLUGIN_API_VERSION for why structSize is not
 * enough on its own. */
#define ATLAS_MIN_PLUGIN_API_VERSION 1

/* Exporting. MSVC needs the declspec; this keeps plugin source portable and
 * stops a missing export turning into a plugin that builds and never loads. */
#if defined(_WIN32)
#  define ATLAS_EXPORT __declspec(dllexport)
#else
#  define ATLAS_EXPORT
#endif

/* ------------------------------------------------------------------ *
 *  Logging                                                            *
 * ------------------------------------------------------------------ */

enum {
    ATLAS_LOG_DEBUG = 0,
    ATLAS_LOG_INFO  = 1,
    ATLAS_LOG_WARN  = 2,
    ATLAS_LOG_ERROR = 3,
    ATLAS_LOG_FATAL = 4
};

/* ------------------------------------------------------------------ *
 *  Inventories                                                        *
 * ------------------------------------------------------------------ */

/* Which of the game's inventory arrays an inventory lives in.
 *
 * WHY THIS IS NOT ONE FLAT INDEX
 * ------------------------------
 * The player's stores are not one array. They are six fixed-size arrays laid
 * end to end inside the same object, with 16-byte members sitting between some
 * of them, and the game addresses each array from its own zero. Ship number 0
 * is mShipInventories[0], not "inventory 47".
 *
 * A flat index is also unsafe across a patch: mInventories grew from 28
 * entries at game version 4.13 to 33 at 7.03, so every flat index above it
 * MOVED BY FIVE. A plugin holding "inventory 47" would not have failed -- it
 * would have carried on writing, to a different ship. (ATLAS_INV_SHIP, 0) is
 * the first ship on any build where the arrays exist at all.
 *
 * Measured 2026-09-28; docs/inventory-arrays.md records how each was pinned. */
enum AtlasInventoryGroup {
    /* Exosuit (0), its technology (1), its cargo (2), the multitool (3), the
     * freighter (7-9), the storage chests (12-21), and more. 33 at 7.03.
     * docs/inventory-index-map.md names every index. */
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
 * Every array is fully allocated from the first second of a new save. In a
 * save owning no exocraft at all, all seven ATLAS_INV_VEHICLE_TECH entries are
 * populated with Fusion Engines and Exocraft Boosters -- they are default
 * templates, byte-identical in a fresh save and a veteran one. Unowned ship
 * slots read 1x1 with an empty ownership mask, kept in place so that ship
 * indices stay put when one is sold.
 *
 * So iterating a group and acting on everything in it will cheerfully sort
 * seven exocraft the player does not have. Ownership is not recorded in the
 * inventory object AT ALL: a byte-for-byte diff of every store across two
 * saves found owned-but-empty storage chests identical to chests in a save
 * owning none. It lives in three unrelated subsystems elsewhere in the save.
 *
 * The usable proxy, and what it is worth: an entry with a grid bigger than 1x1
 * and a non-empty ownership mask is one the game has set up. That is true of
 * every store the player can open, and also true of the exocraft templates.
 * Treat it as "not obviously absent", never as "owned". */

/* One of the player's inventory stores.
 *
 * ownedSlots is how many cells the store HAS -- exactly the save's
 * ValidSlotIndices, confirmed 25 of 25 against a decompiled save. It is NOT a
 * test of whether the player can open the inventory: a brand-new save reports
 * ten 50-slot storage chests, a 16-slot freighter and a 160-slot Corvette
 * cache the player has no access to.
 *
 * startingCapacity is what the store was built with. It tracks ownedSlots
 * everywhere except the exosuit, which stays at its starting 24 forever.
 * Prefer ownedSlots.
 *
 * kind groups inventories by type (1 exosuit, 2 its cargo, 5 freighter, 8 the
 * storage-chest family, 9 the magic pair and Corvette cache, 0 every
 * technology inventory). It is a type, not an identity, and not ownership. */
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
 * DO NOT ADD FIELDS TO THIS STRUCT. Atlas writes into memory the caller owns,
 * so growing it would make the host write past the end of an older plugin's
 * buffer -- see the versioning rules above. More per-element detail means a
 * new call with its own new struct. */
typedef struct AtlasElement {
    char    id[24];            /* e.g. "FUEL1"; a leading '^' means installed tech */
    int32_t x, y;
    int32_t amount;
    int32_t max_amount;        /* what this stack can hold */
    int32_t type;              /* the game's own category for the item */
} AtlasElement;

/* ------------------------------------------------------------------ *
 *  Reflection                                                         *
 * ------------------------------------------------------------------ */

/* A field's kind, as the game's own metadata encodes it.
 *
 * These are the game's numbers, not ours, which is why they are not
 * consecutive. A kind not listed here is still reported faithfully by
 * AtlasField::kind, and FieldAddress still works on it -- so an unrecognised
 * kind means "Atlas has no typed accessor for this", not "you cannot reach
 * it". */
enum AtlasKind {
    ATLAS_KIND_BOOL      = 0x01,
    ATLAS_KIND_STRUCT    = 0x03,
    ATLAS_KIND_DYNARRAY  = 0x07,
    ATLAS_KIND_FLOAT     = 0x0E,
    ATLAS_KIND_STRING16  = 0x10,
    ATLAS_KIND_INT32     = 0x15,
    ATLAS_KIND_STRING256 = 0x1E
};

/* One of the game's classes. `id` is the handle every other reflection call
 * takes; it is an index into Atlas's table and is stable for the life of the
 * process, but NOT across game builds -- look types up by name. */
typedef struct AtlasType {
    int32_t  id;
    const char* name;          /* points into Atlas's table; do not free */
    uint32_t size;             /* the class's own size, where it can be derived */
    int32_t  fieldCount;
    uint64_t guid;             /* the MBIN template GUID */
} AtlasType;

/* One member of a class. */
typedef struct AtlasField {
    int32_t  id;
    const char* name;          /* points into Atlas's table; do not free */
    uint32_t offset;           /* from the start of the object */
    uint32_t size;
    int32_t  kind;             /* AtlasKind */
    int32_t  typeId;           /* the field's own type, or -1 if it has no named one */
} AtlasField;

/* ------------------------------------------------------------------ *
 *  Hotkeys                                                            *
 * ------------------------------------------------------------------ */

enum {
    ATLAS_MOD_NONE  = 0,
    ATLAS_MOD_CTRL  = 1,
    ATLAS_MOD_SHIFT = 2,
    ATLAS_MOD_ALT   = 4
};

/* Called on Atlas's hotkey thread, once per press -- not once per frame the
 * key is held. `user` is whatever you registered with. */
typedef void (*AtlasHotkeyFn)(int32_t id, void* user);

/* ------------------------------------------------------------------ *
 *  The API                                                            *
 * ------------------------------------------------------------------ */

/* Atlas's services.
 *
 * NEW FIELDS GO AT THE END. structSize only protects a plugin built against an
 * older header if the fields it already knows about stay where they were. */
typedef struct AtlasApi {
    uint32_t structSize;       /* sizeof(AtlasApi) as ATLAS knows it */
    uint32_t apiVersion;       /* ATLAS_API_VERSION Atlas was built with */

    /* ---- logging ---- */

    /* Goes to Atlas's log, tagged with your category, so a noisy or broken
     * plugin is attributable from the log alone. */
    void (*Log)(int32_t level, const char* category, const char* message);

    /* ---- the world ---- */

    /* The game's root singleton, or 0 if it has not been resolved. Player
     * state lives inside it. Re-read it rather than caching: loading a
     * different save keeps the same object and replaces its contents. */
    uint64_t (*GetRoot)(void);

    /* 1 only when a save is actually loaded. A non-zero GetRoot() is NOT
     * enough -- the object exists during loading with every inventory reading
     * 1x1 and an empty ownership mask, so reading then yields a full set of
     * plausible, wrong values. This is the single most common way to get
     * nonsense out of Atlas. */
    int32_t (*IsWorldReady)(void);

    /* Increments each time the world becomes ready. If it has changed,
     * everything you cached about the world is stale -- including across a
     * save switch, which leaves the pointer itself valid. */
    int32_t (*GetGeneration)(void);

    /* ---- paths ---- */
    const wchar_t* (*GetGameRoot)(void);    /* folder containing Binaries\ */
    const wchar_t* (*GetPluginDir)(void);   /* where plugins are loaded from */

    /* ---- settings ----
     *
     * An ini file beside your plugin. `file` is a bare name without the
     * extension: "sorter" means <plugin dir>\sorter.ini.
     *
     * A READ WRITES THE DEFAULT BACK when the key is absent, and that is the
     * point of using these rather than GetPrivateProfileInt directly. A
     * setting that exists only as a fallback inside your code is a setting
     * nobody can discover or change: it takes effect from a file that does not
     * mention it. Atlas learned this the hard way -- its own atlas.ini went
     * several versions without ever listing AllowWrites.
     *
     * Get* return the value; Set* return 1 on success. */
    int32_t (*GetSettingInt)(const char* file, const char* section,
                             const char* key, int32_t fallback);
    int32_t (*GetSettingText)(const char* file, const char* section,
                              const char* key, const char* fallback,
                              char* out, uint32_t cap);
    int32_t (*SetSettingInt)(const char* file, const char* section,
                             const char* key, int32_t value);
    int32_t (*SetSettingText)(const char* file, const char* section,
                              const char* key, const char* value);

    /* ---- hotkeys ----
     *
     * Returns a registration id, or 0 if it could not be registered.
     *
     * `vk` is a Windows virtual-key code (VK_F9 is 0x78). `mods` is a
     * combination of ATLAS_MOD_*; a modifier not named must NOT be held, so
     * ATLAS_MOD_NONE means the bare key.
     *
     * Atlas polls one thread for every plugin rather than each plugin starting
     * its own, which is what every plugin did before this existed. Your
     * callback fires once per press, on that thread, guarded like any other
     * entry point.
     *
     * This is polling, not a keyboard hook: it cannot interfere with the
     * game's own input, and it does not see keys while another application has
     * focus. A missed keypress is a non-event; a broken input path would not
     * be. */
    int32_t (*RegisterHotkey)(int32_t vk, int32_t mods, AtlasHotkeyFn fn,
                              void* user);
    int32_t (*UnregisterHotkey)(int32_t id);

    /* ---- inventories: reading ---- */

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

    /* Read one occupied slot. Returns 1 on success.
     *
     * `slot` is a position in the element array, not a grid cell, and means
     * nothing once that array next changes. */
    int32_t (*ReadElement)(const AtlasInventory* inv, int32_t slot,
                           AtlasElement* out);

    /* Does the player own this cell? 1 if it is unlocked and usable.
     *
     * An inventory's grid is LARGER than the part that has been unlocked: the
     * exosuit reports 10x12 while its owner may own 24 cells of it. So "fill
     * the grid left to right" is wrong, and anything laying items out has to
     * know which cells are real. ownedSlots is the COUNT of owned cells; this
     * is which ones. */
    int32_t (*IsCellOwned)(const AtlasInventory* inv, int32_t x, int32_t y);

    /* Can the player reach this inventory to move things in or out of it?
     *
     *    1  yes -- it is carried on the player, so no distance rule applies
     *    0  no
     *   -1  NOT KNOWABLE YET
     *
     * The three-valued answer is deliberate and -1 is the common one today.
     * The game gates transfers into a ship, freighter, exocraft or container
     * by how close the player is standing, with the range depending on
     * installed technology AND on mods, which can change it. Atlas does not
     * yet read the game's own verdict, so it genuinely does not know -- and "I
     * do not know" is not "no".
     *
     * That distinction matters to a plugin. Treating -1 as no refuses every
     * legitimate transfer at a landing pad; treating it as yes is a decision
     * to bypass a rule the game enforces. Neither is wrong, but the choice
     * should be made on purpose, so this does not make it for you.
     *
     * Reading is never gated -- a plugin may always LOOK inside any inventory.
     * See docs/reach.md. */
    int32_t (*GetInventoryReach)(const AtlasInventory* inv);

    /* ---- inventories: writing ----
     *
     * All three are gated by AllowWrites in atlas.ini and return 1 on success,
     * 0 if refused.
     *
     * `expect_id` is required on each and is not a convenience: pass the item
     * id you believe is in that slot (e.g. "FUEL1"). If the slot holds
     * something else the write is refused. Between your reading an inventory
     * and writing to it the player may have moved the stack, dropped it, or
     * loaded another save entirely -- and the element array is reused, so the
     * address stays perfectly valid while meaning something different. Without
     * this check a stale read turns into a write onto an unrelated item.
     *
     * Writes race the game: it is running, on its own threads, and may be
     * changing the same stack. A 4-byte aligned store cannot tear, so you will
     * never produce a nonsense number -- but you can lose a concurrent update,
     * so do not build a counter on these. Read, decide, write. */

    /* Set one slot's amount. Clamped to the slot's own maximum rather than
     * refused, because that is what the game does to its own values. */
    int32_t (*SetElementAmount)(const AtlasInventory* inv, int32_t slot,
                                const char* expect_id, int32_t amount);

    /* Move a slot to a grid position within its own inventory.
     *
     * x and y are the cell the player sees; they match the screen exactly.
     * Position survives a save and a reload, so a layout written here is not
     * undone by reloading.
     *
     * THE CELL MUST BE ONE THE PLAYER OWNS. An item placed in a cell they have
     * not bought is not reachable -- from their side it has simply vanished,
     * with whatever was in it. Atlas checks the ownership mask and refuses
     * rather than quietly losing someone's cargo.
     *
     * COLLISIONS ARE NOT CHECKED, deliberately. Applying any permutation one
     * element at a time passes through states where two items share a cell, so
     * refusing them would make rearranging impossible. The game reads these
     * values per frame, so the worst case is a frame or two of overlap. Work
     * out the whole target layout first, then write it. */
    int32_t (*SetElementPosition)(const AtlasInventory* inv, int32_t slot,
                                  const char* expect_id, int32_t x, int32_t y);

    /* Move a whole stack to ANOTHER inventory. Pass x = -1 to drop it in the
     * first free cell the player owns; pass a cell to choose one.
     *
     * THIS IS A DIFFERENT KIND OF OPERATION TO THE TWO ABOVE. Amount and
     * position are single fields -- write four bytes and stop. Moving a stack
     * is STRUCTURAL: the element array holds exactly usedSlots entries, so the
     * stack has to be appended to the destination and its size grown, then
     * removed from the source and its size shrunk, while the game reads both
     * arrays from its own threads.
     *
     * IT REFUSES WHEN THE DESTINATION IS FULL, and that is not a policy
     * choice. Appending is only safe because the destination's vector already
     * has spare capacity (allocatedSlots above usedSlots); growing it would
     * mean reallocating memory the game owns and freeing the block it is
     * reading from. There is no safe way to do that from here.
     *
     * THE ORDER IS DELIBERATE: appended to the destination first, then removed
     * from the source. Between those two writes the stack exists TWICE, for
     * microseconds. The other order would have it exist nowhere, and anything
     * going wrong at that instant would destroy it outright. Transient
     * duplication is recoverable; transient deletion is not.
     *
     * IT ALSO RESPECTS THE GAME'S REACH LIMIT. A move with both ends carried
     * on the player always goes through; anything else needs
     * AllowRemoteTransfer in atlas.ini, because Atlas cannot yet read the
     * game's own verdict on whether the player is close enough. Ask
     * GetInventoryReach rather than discovering it from a refusal.
     *
     * AFTER A SUCCESSFUL MOVE, EVERY SLOT NUMBER YOU HOLD FOR THE SOURCE IS
     * STALE. Removal copies the array's last element over the vacated slot and
     * shrinks the size, so one other stack changes slot number. Re-read the
     * source. Grid positions are untouched -- the array is a bag, and position
     * is what orders the display. */
    int32_t (*MoveElement)(const AtlasInventory* from, int32_t slot,
                           const char* expect_id,
                           const AtlasInventory* to, int32_t x, int32_t y);

    /* ---- reflection ----
     *
     * The game's own type information: 2741 classes with named members,
     * offsets and types, extracted offline and shipped beside Atlas.
     *
     * HasSymbols returns 0 when the database is missing or was built for a
     * different game build. A build mismatch disables reflection ENTIRELY
     * rather than serving offsets from another version, because wrong offsets
     * do not fault -- they read the wrong field and return a plausible number,
     * forever. Check it once and degrade gracefully.
     *
     * Look types up BY NAME. The ids are indices into Atlas's table, stable
     * for the life of the process and meaningless across builds. */
    int32_t (*HasSymbols)(void);
    int32_t (*GetTypeCount)(void);
    int32_t (*FindType)(const char* name, AtlasType* out);
    int32_t (*GetTypeByIndex)(int32_t index, AtlasType* out);
    int32_t (*FindField)(int32_t type_id, const char* name, AtlasField* out);
    int32_t (*GetFieldByIndex)(int32_t type_id, int32_t n, AtlasField* out);

    /* The address of a field within an object, or 0. This is the navigator:
     * for a struct field, the result plus AtlasField::typeId is the sub-object,
     * which is how you walk an arbitrarily deep graph with no further help
     * from Atlas. It is also the escape hatch for kinds Atlas has no typed
     * accessor for. */
    uint64_t (*FieldAddress)(uint64_t object, int32_t type_id,
                             const char* field);

    /* Typed reads. Return 1 on success, 0 if the field does not exist or is
     * not of that kind.
     *
     * A field is always read at its DECLARED size and kind, never at the size
     * you ask for. Reading an int32 field as a float would hand back a
     * denormal rather than an error, so it is refused. ReadInt32 accepts any
     * four-byte non-bool field, because the game stores enums that way and
     * reading one as an integer is reasonable.
     *
     * ATLAS DOES NOT VERIFY THAT `object` IS REALLY OF THAT TYPE. It cannot:
     * nothing in an object identifies its class. What is guaranteed is that
     * the offset is right for the named field of the named type, that the
     * access is the declared width, and that a bad address cannot fault the
     * process. Prefer starting from a known anchor over an address you
     * computed yourself. */
    int32_t (*ReadBool)(uint64_t object, int32_t type_id, const char* field,
                        int32_t* out);
    int32_t (*ReadInt32)(uint64_t object, int32_t type_id, const char* field,
                         int32_t* out);
    int32_t (*ReadFloat)(uint64_t object, int32_t type_id, const char* field,
                         float* out);
    /* Fixed-size strings, which the game does not guarantee to terminate when
     * full. Always NUL-terminates what it gives you. */
    int32_t (*ReadText)(uint64_t object, int32_t type_id, const char* field,
                        char* out, uint32_t cap);

    /* Typed writes. Gated by AllowWrites; same kind rules as the reads.
     *
     * Reflection makes it far easier to write somewhere catastrophic than the
     * inventory calls ever did -- there are 18,091 reachable fields and nobody
     * has vetted them individually. The size and kind checks are what stand
     * between a typo and a corrupted save: writing four bytes into a one-byte
     * bool would quietly take out the three fields packed after it. */
    int32_t (*WriteBool)(uint64_t object, int32_t type_id, const char* field,
                         int32_t value);
    int32_t (*WriteInt32)(uint64_t object, int32_t type_id, const char* field,
                          int32_t value);
    int32_t (*WriteFloat)(uint64_t object, int32_t type_id, const char* field,
                          float value);
} AtlasApi;

/* ------------------------------------------------------------------ *
 *  What YOUR plugin exports                                           *
 * ------------------------------------------------------------------ */

/* REQUIRED. Return ATLAS_API_VERSION -- the version of THIS HEADER that your
 * plugin was compiled against. Declare it with the macro below, once, at file
 * scope, in exactly one of your source files.
 *
 * Atlas reads this before it calls anything else and refuses a plugin that is
 * too old, or that does not export it at all.
 *
 * WHY structSize IS NOT ENOUGH ON ITS OWN. structSize lets a plugin built
 * against an older header avoid calling functions that did not exist yet, and
 * that works for as long as the functions it DOES know about keep their shape.
 * It cannot see a function that CHANGED shape: a call with one parameter too
 * few passes its arguments in the wrong registers, and the host then reads a
 * pointer out of whatever the caller happened to leave behind. Nothing about
 * the struct changes, so nothing on the host's side can detect it. Only the
 * plugin knows which header it was built from, so the plugin says. */
#define ATLAS_PLUGIN_API_VERSION_NAME "AtlasPluginApiVersion"
typedef int32_t (*AtlasPluginApiVersionFn)(void);

#ifdef __cplusplus
#define ATLAS_DECLARE_PLUGIN_API_VERSION \
    extern "C" ATLAS_EXPORT int32_t AtlasPluginApiVersion(void) { \
        return ATLAS_API_VERSION; \
    }
#else
#define ATLAS_DECLARE_PLUGIN_API_VERSION \
    ATLAS_EXPORT int32_t AtlasPluginApiVersion(void) { \
        return ATLAS_API_VERSION; \
    }
#endif

/* Optional but strongly encouraged: who you are, for logs and for any manager
 * listing installed plugins. Without it a plugin is just a filename.
 *
 * All four strings must stay valid for the life of the process; string
 * literals via the macro are the intended use. */
#define ATLAS_PLUGIN_INFO_NAME "AtlasPluginInfo"
typedef struct AtlasPluginInfo {
    uint32_t structSize;       /* sizeof(AtlasPluginInfo) as YOU know it */
    const char* name;
    const char* version;
    const char* author;
    const char* description;
    const char* url;           /* may be NULL */
} AtlasPluginInfo;
typedef const struct AtlasPluginInfo* (*AtlasPluginInfoFn)(void);

/* The exported FUNCTION is deliberately named the same as the struct: the
 * export name is part of the contract and "AtlasPluginInfo" is the obvious one
 * to use. In C++ that makes the function name hide the class name at this
 * scope, so every mention of the type inside the macro is elaborated as
 * `struct AtlasPluginInfo`. Without that the compiler resolves the type to the
 * function and reports a missing type specifier -- a baffling error to hit in
 * a template you have just copied and not yet edited. */
#ifdef __cplusplus
#define ATLAS_DECLARE_PLUGIN_INFO(NAME, VERSION, AUTHOR, DESCRIPTION) \
    extern "C" ATLAS_EXPORT const struct AtlasPluginInfo* AtlasPluginInfo(void) { \
        static const struct AtlasPluginInfo s_info = { \
            (uint32_t)sizeof(struct AtlasPluginInfo), \
            (NAME), (VERSION), (AUTHOR), (DESCRIPTION), 0 }; \
        return &s_info; \
    }
#else
#define ATLAS_DECLARE_PLUGIN_INFO(NAME, VERSION, AUTHOR, DESCRIPTION) \
    ATLAS_EXPORT const struct AtlasPluginInfo* AtlasPluginInfo(void) { \
        static const struct AtlasPluginInfo s_info = { \
            (uint32_t)sizeof(struct AtlasPluginInfo), \
            (NAME), (VERSION), (AUTHOR), (DESCRIPTION), 0 }; \
        return &s_info; \
    }
#endif

/* REQUIRED. Return 0 to stay loaded; anything else and Atlas logs it and
 * unloads you -- which is the supported way to say "not applicable on this
 * build". Keep it short: a plugin that blocks here delays every plugin after
 * it.
 *
 * THIS IS NOT WHERE YOU READ PLAYER DATA. The game's state object exists
 * before a save is loaded, and during loading every inventory reads 1x1 with
 * an empty ownership mask -- a full set of plausible, wrong answers through a
 * perfectly valid pointer. Use AtlasPluginOnWorldReady. */
#define ATLAS_PLUGIN_START_NAME "AtlasPluginStart"
typedef int32_t (*AtlasPluginStartFn)(const AtlasApi* api);

/* Optional. Called when a save becomes ready, and again after each save
 * switch, with the new generation. This -- not AtlasPluginStart -- is where
 * reading player data belongs. */
#define ATLAS_PLUGIN_WORLD_READY_NAME "AtlasPluginOnWorldReady"
typedef void (*AtlasPluginOnWorldReadyFn)(int32_t generation);

/* Optional. Called when a loaded save goes away. The pointer stays valid and
 * its contents do not. */
#define ATLAS_PLUGIN_WORLD_UNLOADED_NAME "AtlasPluginOnWorldUnloaded"
typedef void (*AtlasPluginOnWorldUnloadedFn)(void);

/* Optional. Roughly once a second, on Atlas's thread. NOT a frame hook. */
#define ATLAS_PLUGIN_TICK_NAME "AtlasPluginOnTick"
typedef void (*AtlasPluginOnTickFn)(void);

/* Optional. Called on the way out when Atlas can manage it. NOT guaranteed:
 * the game often terminates itself outright, so do not rely on this for
 * anything that must not be lost. */
#define ATLAS_PLUGIN_STOP_NAME "AtlasPluginStop"
typedef void (*AtlasPluginStopFn)(void);

#ifdef __cplusplus
}   /* extern "C" */
#endif

#endif /* ATLAS_H */
