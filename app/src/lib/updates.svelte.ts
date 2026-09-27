/**
 * What Nexus says about the installed library, held for the whole session.
 *
 * Outside any component, for two reasons. The check runs on start-up, before
 * the Updates tab has ever been opened, so there is no component to own it.
 * And a tab that owned it would throw the answer away every time the user
 * looked at something else, which is how a sixty-request check ended up
 * being run by hand to see its own results.
 */

import {
  checkUpdates,
  nexusAccount,
  type NexusAccount,
  type UpdateCheck,
  type UpdateReport,
} from "./engine";

/** How long a result stays good enough to skip the network on start-up. */
const FRESH_FOR = 4 * 60 * 60 * 1000;

// Renamed with the program. This is a cache of what Nexus said, so the
// worst a fresh key costs is one check on the next launch.
const REMEMBERED = "anomaly.updates";

interface Remembered {
  at: number;
  report: UpdateReport;
  /** Remembered too, so a throttled start does not look unconfigured. */
  account?: NexusAccount | null;
}

/**
 * Read the last result, so a relaunch shows something before the network does.
 *
 * Non-answers are dropped on the way *in* as well as on the way out. `remember`
 * strips them before writing, but an entry written before it did would
 * otherwise keep its stale `unknown` rows for good: the mod has a row, so
 * [`Updates.fill`] counts it as asked and never asks again, and nothing else
 * ever revisits it. Measured on a real one — a mod whose archive was recorded
 * after the last check sat under "could not ask: no Nexus archive is recorded
 * for this folder" while the very same card showed the Nexus page title it had
 * just looked up with the archive it supposedly did not have.
 */
function keptChecks(report: UpdateReport): UpdateReport {
  return { ...report, checks: report.checks.filter((c) => c.state !== "unknown") };
}

function recall(): Remembered | null {
  try {
    const raw = localStorage.getItem(REMEMBERED);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as Remembered;
    if (!parsed?.report?.checks) return null;
    return { ...parsed, report: keptChecks(parsed.report) };
  } catch {
    // Private windows, cleared storage, a shape from an older version: all
    // mean "no cached answer", none of them mean "stop".
    return null;
  }
}

/**
 * Keep the answers, but not the non-answers.
 *
 * An `unknown` check is almost always "no Nexus archive is recorded for this
 * folder", which is a statement about *this program's* records, not about
 * Nexus. Those records change with no network involved — installing a mod under
 * its real archive name, mending one, adopting one — and a remembered
 * non-answer would go on being shown for four hours and across restarts,
 * describing a gap that had already been filled. Dropped here rather than
 * filtered at the screen, so it also cannot be counted anywhere, and so
 * [`Updates.fill`] sees the mod as unasked and asks about it.
 */
function remember(full: UpdateReport, account: NexusAccount | null) {
  const report = keptChecks(full);
  try {
    localStorage.setItem(
      REMEMBERED,
      JSON.stringify({ at: Date.now(), report, account }),
    );
  } catch {
    // Not being able to cache is not a failure worth telling anyone about.
  }
}

class Updates {
  account = $state<NexusAccount | null>(null);
  report = $state<UpdateReport | null>(null);
  checking = $state(false);
  error = $state<string | null>(null);
  /** when `report` was fetched, or last recalled from a previous run */
  checkedAt = $state<number | null>(null);
  /** true once start-up has decided whether to check, so the tab can settle */
  settled = $state(false);
  /** pages asked about so far, and how many there are. One request each, so a
   *  sixty-mod library needs a sign that it is moving rather than stuck. */
  progress = $state<{ done: number; total: number } | null>(null);

  /** Only one check at a time, however many places ask for one. */
  #inFlight: Promise<void> | null = null;

  /**
   * The mod folders the last scan found, or null before one has landed.
   *
   * A report is remembered for four hours and across restarts, and *nothing*
   * invalidates it when the library changes — that is deliberate, because a
   * recheck costs one request per Nexus page. But it means a remembered verdict
   * outlives its mod: delete something and it goes on being listed under
   * Updates, and counted in the nav pip, with an answer about a folder that is
   * no longer on disk. A remembered answer about a mod that no longer exists is
   * not an answer, so it is dropped.
   *
   * Held here rather than filtered at each screen so that the pane, the pip and
   * the Library's "behind" marks cannot disagree about which mods count.
   */
  #here = $state<Set<string> | null>(null);

  /** Tell the store what the last scan found. See [`#here`]. */
  installedNow(owners: string[]) {
    this.#here = new Set(owners);
  }

  /** Before the first scan nothing is filtered: one stale row beats no list. */
  #live = (c: UpdateCheck): boolean => !this.#here || this.#here.has(c.owner);

  /** Every check still worth showing, which is every check about a live mod. */
  get checks() {
    return (this.report?.checks ?? []).filter(this.#live);
  }

  /** Mods with a newer version actually published. */
  get outdated() {
    return this.checks.filter((c) => c.state === "outdated");
  }

  /** Anything not plainly current, including the ones we could not ask about. */
  get notable() {
    return this.checks.filter((c) => c.state !== "current");
  }

  constructor() {
    const last = recall();
    if (last) {
      this.report = last.report;
      this.checkedAt = last.at;
      // Without this the pane would offer to connect an account that is
      // already connected, every time a recent result let us skip the check.
      this.account = last.account ?? null;
    }
  }

  /** Who the stored key belongs to, or null when none is stored. */
  async loadAccount(): Promise<NexusAccount | null> {
    try {
      this.account = await nexusAccount();
    } catch (err) {
      // Could be a revoked key, could be no network. Either way, keep whatever
      // we remembered: being offline is not the same as being signed out, and
      // throwing the account away would offer to reconnect one that is fine.
      this.error = String(err);
    }
    return this.account;
  }

  /**
   * Check, unless a recent answer will do.
   *
   * Called on start-up with `force: false`, and by the button with `true`.
   * Costs one request per Nexus page -- about sixty for a library this size,
   * against an hourly allowance of two thousand -- so the throttle is about
   * not making start-up wait on the network, not about the budget.
   */
  async check(modsDir: string | undefined, force: boolean): Promise<void> {
    if (this.#inFlight) return this.#inFlight;

    // Establish who we are first, and only then decide whether to ask Nexus
    // anything. Doing it the other way round meant a throttled start returned
    // before the account was known, and the tab showed its "connect an
    // account" screen to someone whose account was already connected.
    if (!this.account) {
      await this.loadAccount();
    }
    if (!this.account) {
      // No key, nothing to ask with. Not an error; the tab explains it.
      this.settled = true;
      return;
    }
    if (!force && this.checkedAt && Date.now() - this.checkedAt < FRESH_FOR) {
      this.settled = true;
      return;
    }

    this.checking = true;
    this.error = null;
    this.progress = null;
    this.#inFlight = (async () => {
      const stop = await this.#watchProgress();
      try {
        const fresh = await checkUpdates(modsDir);
        if (fresh) {
          this.report = fresh;
          this.checkedAt = Date.now();
          remember(fresh, this.account);
        }
      } catch (err) {
        this.error = String(err);
      } finally {
        stop();
        this.checking = false;
        this.settled = true;
        this.progress = null;
        this.#inFlight = null;
      }
    })();
    return this.#inFlight;
  }

  /**
   * Ask about the installed mods the remembered report says nothing about.
   *
   * The gap this closes: the full check is throttled to four hours and runs
   * only at start-up, so between one check and the next the library moves and
   * the report does not. A mod installed since has no row at all. A mod whose
   * archive we could not read last time has no row either, because `remember`
   * refuses to keep a non-answer — and unlike a real verdict, that one would
   * never have corrected itself, since what was wrong was our record and not
   * anything at Nexus.
   *
   * One request per mod page, so this is a handful after an install and nothing
   * at all on a library that has not moved. Called after every scan; it decides
   * for itself whether there is anything to ask.
   */
  async fill(modsDir: string | undefined): Promise<void> {
    if (this.#inFlight || this.checking || !this.account || !this.#here) return;
    const asked = new Set((this.report?.checks ?? []).map((c) => c.owner));
    const missing = [...this.#here].filter((owner) => !asked.has(owner));
    if (!missing.length) return;

    this.checking = true;
    this.#inFlight = (async () => {
      try {
        const fresh = await checkUpdates(modsDir, missing);
        if (fresh) {
          // Merged, not replaced: this asked about a handful of mods and knows
          // nothing about the rest, so overwriting the report with it would
          // throw away every answer it did not cover.
          const held = (this.report?.checks ?? []).filter(
            (c) => !fresh.checks.some((f) => f.owner === c.owner),
          );
          const merged = { ...fresh, checks: [...held, ...fresh.checks] };
          this.report = merged;
          this.checkedAt = this.checkedAt ?? Date.now();
          remember(merged, this.account);
        }
      } catch (err) {
        this.error = String(err);
      } finally {
        this.checking = false;
        this.settled = true;
        this.#inFlight = null;
      }
    })();
    return this.#inFlight;
  }

  /**
   * Listen for the engine's page counter for the length of one check.
   *
   * Subscribed per check rather than for the life of the app, so a stale count
   * cannot survive into the next one, and imported lazily so this module still
   * loads in a plain browser where there is no Tauri to listen with.
   */
  async #watchProgress(): Promise<() => void> {
    try {
      const { listen } = await import("@tauri-apps/api/event");
      return await listen<{ done: number; total: number }>(
        "update-progress",
        (event) => (this.progress = event.payload),
      );
    } catch {
      return () => {};
    }
  }

  /** Forget the key and everything it told us. */
  forget() {
    this.account = null;
    this.report = null;
    this.checkedAt = null;
    this.error = null;
    try {
      localStorage.removeItem(REMEMBERED);
    } catch {
      // see `remember`
    }
  }
}

export const updates = new Updates();

/** "just now", "12 minutes ago", "3 hours ago", "yesterday". */
export function ago(at: number | null): string {
  if (!at) return "never";
  const seconds = Math.max(0, Math.round((Date.now() - at) / 1000));
  if (seconds < 90) return "just now";
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes} minutes ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} ${hours === 1 ? "hour" : "hours"} ago`;
  const days = Math.round(hours / 24);
  return days === 1 ? "yesterday" : `${days} days ago`;
}
