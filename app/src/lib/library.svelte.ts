/**
 * What is installed, and which build of it the game is reading.
 *
 * ---------------------------------------------------------------------------
 * Why this is a store and not a field on the Library pane
 * ---------------------------------------------------------------------------
 *
 * Three screens used to fetch this, and they fetched different halves of it.
 * `LibraryPane` read `library_list` *and* `loadout_list` and joined them into
 * rows; `PresetsPane` read `loadout_list` on its own and drew a second list of
 * the same mods with a different control on each row; and the app itself read
 * `loadout_list` a third time, in `readFixed`, purely to learn which mods were
 * mended and which were standing behind a merge.
 *
 * Three copies of one answer, fetched at three different moments, is three
 * chances to disagree — and they did: switching a mod off in Presets left the
 * Library list showing it as active until that pane happened to remount.
 *
 * So the answer is fetched once, here, and every screen reads the same rows.
 *
 * ---------------------------------------------------------------------------
 * The join, which is the part worth understanding
 * ---------------------------------------------------------------------------
 *
 * Neither source is sufficient alone.
 *
 * `library_list` walks the game's mods folder, so it sees exactly what the game
 * sees — including mods this program did not install — and it is the only
 * source of a Nexus id. But a mod switched *off* has no files in the game, so
 * it appears here not at all, and it is precisely the one the user has come
 * looking for in order to switch it back on.
 *
 * `loadout_list` is this program's own record: everything it manages, on or
 * off, and which build of each the game is reading. But it knows nothing about
 * mods installed by hand or by another manager.
 *
 * A row is the union, with the loadout winning wherever both know a mod.
 */

import { libraryList, loadoutList, type Identity, type Loadout } from "./engine";
import { names } from "./names.svelte";
import { mergeAssetName } from "./types";

export interface Row {
  owner: string;
  /** the title to show; see `names.svelte.ts` */
  name: string;
  /** installed through this program, so it can be switched and deleted */
  managed: boolean;
  /** switched on by the user */
  enabled: boolean;
  /** in the game right now: on, and not standing aside for a merge */
  active: boolean;
  /** the merge holding it out of the game, when one is */
  mergedInto: string | null;
  /** `cleaned`, `mended`, `merged` — or null for the build its author shipped */
  variant: string | null;
  /**
   * true when the build carries values the user set by hand.
   *
   * Separate from `variant` because it is orthogonal to it: a cleaned mod can
   * be edited and is still cleaned. The list therefore shows both words, and a
   * mod that is only edited shows "edited" where a variant would have gone.
   */
  edited: boolean;
}

/**
 * What the list's batch bar can ask for.
 *
 * Lives here rather than in [`ModList`] because a Svelte instance script
 * cannot export a type, and because both ends of the call — the bar that
 * raises it and the pane that performs it — need to agree on the word.
 */
export type Batch = "activate" | "deactivate" | "revert" | "delete";

class Library {
  /** what the game's mods folder holds, which is not everything installed */
  mods = $state<Identity[]>([]);
  /** what this program manages, which is not everything in the game */
  book = $state<Loadout>({ entries: [] });
  loading = $state(true);
  error = $state<string | null>(null);

  /** Only one read at a time, however many callers ask for one. */
  #inFlight: Promise<void> | null = null;

  /**
   * Mods a merge is standing in for, and which merge.
   *
   * These are switched *on* and still not in the game, which is the one state
   * a list could otherwise not explain: without this a mod that a merge has
   * replaced shows a lit lamp while the game is not loading a byte of it.
   * Only an enabled merge suppresses anything — switching it off is how the
   * originals come back — so this matches `Loadout::superseded` exactly.
   */
  readonly supersededBy = $derived.by((): Record<string, string> => {
    const out: Record<string, string> = {};
    for (const entry of this.book.entries) {
      if (!entry.enabled) continue;
      for (const owner of entry.replaces ?? []) out[owner] = entry.owner;
    }
    return out;
  });

  /** Every installed mod, sorted by the name it displays. See the join above. */
  readonly rows = $derived.by((): Row[] => {
    const seen = new Map<string, Row>();
    for (const entry of this.book.entries) {
      const mergedInto = this.supersededBy[entry.owner] ?? null;
      seen.set(entry.owner, {
        owner: entry.owner,
        // A merge is the one mod in the list nobody named. Its folder is built
        // out of the asset it settles -- `zzz_nmscheck_METADATA_REALITY_TABLES_
        // REWARDTABLE` -- and `names.of` has no page to improve on that, so the
        // row read as machine noise. That mattered once this became the only
        // list: switching a merge off is how the mods behind it come back, and
        // the record of a held-back mod sends you here to do it by name.
        name:
          entry.variant === "merged"
            ? mergeAssetName(entry.owner)
            : names.of(entry.owner),
        managed: true,
        enabled: entry.enabled,
        active: entry.enabled && mergedInto === null,
        mergedInto,
        variant: entry.variant === "original" ? null : entry.variant,
        edited: entry.edited ?? false,
      });
    }
    for (const mod of this.mods) {
      if (seen.has(mod.owner)) continue;
      // In the game folder but not ours: installed by hand or by another
      // manager. Shown, but not offered a switch or a delete.
      seen.set(mod.owner, {
        owner: mod.owner,
        name: mod.name,
        managed: false,
        enabled: !mod.disabled,
        active: !mod.disabled,
        mergedInto: null,
        variant: null,
        edited: false,
      });
    }
    // Sorted by what is shown: sorting by folder while displaying titles makes
    // a sorted list look shuffled.
    return [...seen.values()].sort((a, b) =>
      a.name.localeCompare(b.name, undefined, { sensitivity: "base" }),
    );
  });

  /** Mods running a build we mended. The clean preview cannot supply these:
   *  it plans prunes, and a mend is not one. */
  readonly mended = $derived(
    this.book.entries.filter((e) => e.variant === "mended").map((e) => e.owner),
  );

  /** Folders a merge put in the game. Ours, so nobody should ask Nexus. */
  readonly mergeFolders = $derived(
    this.book.entries.filter((e) => e.variant === "merged").flatMap((e) => e.deployed),
  );

  /** Mods an enabled merge is standing in for: installed, but not deployed. */
  readonly heldByMerge = $derived(
    this.book.entries.filter((e) => e.enabled).flatMap((e) => e.replaces ?? []),
  );

  /** How many mods the game is actually loading. */
  readonly activeCount = $derived(this.rows.filter((r) => r.active).length);

  /** The mod folder as the game's own folder scan found it, when it did. */
  identity(owner: string): Identity | null {
    return this.mods.find((m) => m.owner === owner) ?? null;
  }

  row(owner: string | null): Row | null {
    if (!owner) return null;
    return this.rows.find((r) => r.owner === owner) ?? null;
  }

  /**
   * Read both halves again.
   *
   * Together rather than in sequence: neither waits on the other, and drawing
   * the list from one before the other lands shows every managed mod as
   * unmanaged for as long as the second call takes.
   */
  async load(modsDir: string | undefined): Promise<void> {
    if (this.#inFlight) return this.#inFlight;
    this.loading = true;
    this.error = null;
    this.#inFlight = (async () => {
      try {
        const [mods, book] = await Promise.all([
          libraryList(modsDir),
          loadoutList(modsDir),
        ]);
        this.mods = mods;
        this.book = book;
      } catch (err) {
        this.error = String(err);
      } finally {
        this.loading = false;
        this.#inFlight = null;
      }
    })();
    return this.#inFlight;
  }
}

export const library = new Library();
