/**
 * What to call each mod, for the whole app.
 *
 * ---------------------------------------------------------------------------
 * Why this is one store and not a field on each screen
 * ---------------------------------------------------------------------------
 *
 * A mod's folder is not its name. On a library installed from Nexus the folder
 * is the archive stem, so the honest identifier for `Buy All Corvette Parts`
 * is `Buy All Corvette Parts Cosmos 7.01 Non Expedition 4447 1.2
 * 2026-09-11T13-48Z L5bXM34yG`, and that is what the game, GCMODSETTINGS and
 * every part of the engine work in.
 *
 * Eleven places used to print that folder: the library list, the load-order
 * rail, the claim stack, the conflict heading, every action title, the updates
 * rows, the presets list and the delete sheet. Fixing them one at a time means
 * fixing them one at a time again next month, so the lookup lives here and
 * each of those places asks it.
 *
 * ---------------------------------------------------------------------------
 * Two stages, because one of them is free and the other is not
 * ---------------------------------------------------------------------------
 *
 * `load()` reads names the engine can work out on its own -- Nexus bakes the
 * display name into the archive -- and costs nothing. It runs on start-up and
 * after every scan, and it is what stops a folder name ever being on screen.
 *
 * `resolve()` asks Nexus for the true title of pages we have never read, one
 * request each, and the engine keeps the answers on disk for good. So the
 * first run spends about sixty of two thousand an hour and later runs spend
 * none. It follows an author's rename, which the archive name cannot.
 */

import { modNames, resolveNames, type NameMap } from "./engine";

class Names {
  #map = $state<NameMap>({});

  /** true while Nexus is being asked for the titles we do not have */
  resolving = $state(false);
  /** how many names the last resolve added, for the one line that reports it */
  gained = $state(0);

  /** Only one resolve at a time, however many callers ask. */
  #inFlight: Promise<void> | null = null;

  /**
   * The name to show for a mod folder.
   *
   * Falls back to the folder rather than to a placeholder: before the first
   * load, and for a mod installed by hand, the folder is genuinely the only
   * name there is, and it is a real answer rather than an apology.
   */
  of(owner: string): string {
    return this.#map[owner] ?? owner;
  }

  /** For sorting a list by what it displays. Folder order is not name order. */
  compare = (a: string, b: string): number =>
    this.of(a).localeCompare(this.of(b), undefined, { sensitivity: "base" });

  /** Read what the engine already knows. Free, offline, no API key needed. */
  async load(modsDir: string | undefined): Promise<void> {
    try {
      const found = await modNames(modsDir);
      // Outside Tauri this is `{}`; keeping whatever we had beats blanking
      // every row because one call could not reach the engine.
      if (Object.keys(found).length) this.#map = found;
    } catch {
      // Names are a convenience. Failing to get them is not worth a message:
      // the folder name is still a usable identifier on every screen.
    }
  }

  /**
   * Ask Nexus for the titles of pages never read before.
   *
   * Unawaited by its callers by design: nothing on screen should wait on sixty
   * requests, and every row already reads correctly from `load()`. The rows
   * simply sharpen when this lands.
   */
  async resolve(modsDir: string | undefined): Promise<void> {
    if (this.#inFlight) return this.#inFlight;
    this.resolving = true;
    this.#inFlight = (async () => {
      const before = Object.entries(this.#map).filter(([owner, name]) => name !== owner).length;
      try {
        const found = await resolveNames(modsDir);
        if (Object.keys(found).length) {
          this.#map = found;
          const after = Object.entries(found).filter(([owner, name]) => name !== owner).length;
          this.gained = Math.max(0, after - before);
        }
      } catch {
        // The usual reason is no API key, which the Updates tab already
        // explains at length. Saying it again here, over the mod list, would
        // be nagging about a feature the user has not asked for yet.
      } finally {
        this.resolving = false;
        this.#inFlight = null;
      }
    })();
    return this.#inFlight;
  }
}

export const names = new Names();
