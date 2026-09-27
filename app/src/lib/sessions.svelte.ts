/**
 * What the game said while it ran, held for the whole life of the app.
 *
 * Outside any component, for the same reason the update check is: a session
 * starts when the *game* starts, which is usually while the user is looking at
 * some other tab, or at some other window entirely. A tab that owned this would
 * begin listening when it was opened -- after the first thirty seconds of the
 * run, which is when the game opens every mod file it is ever going to open.
 *
 * The events arrive in batches rather than one message per line, because the
 * game opens a couple of hundred files in its first second. Only the last few
 * hundred are kept: this is a live view, and the whole record is in the log file
 * the moment it is written.
 */

import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import {
  hookState,
  noCopies,
  noWatch,
  savesKept,
  sessionsList,
  watchState,
  watchTail,
  type HookState,
  type LiveEvent,
  type SaveCopies,
  type Session,
  type WatchState,
} from "./engine";

/** Live events kept on screen. Matches the ring the Rust side keeps. */
const KEEP = 600;

class Sessions {
  /** what the recorder is doing this second */
  state = $state<WatchState>(noWatch());
  /** the events of the session in progress, oldest first */
  events = $state<LiveEvent[]>([]);
  /** every recorded session, newest first */
  list = $state<Session[]>([]);
  /** whether the recorder's DLL is in the game */
  hook = $state<HookState | null>(null);
  /** copies of the save, which are kept whether or not the hook is installed */
  copies = $state<SaveCopies>(noCopies());
  /** true until the first answers have landed, so the pane can settle */
  loading = $state(true);

  #stops: UnlistenFn[] = [];

  /** Everything the game reported that asks for attention, this session. */
  get loud() {
    return this.state.counts.error + this.state.counts.fatal;
  }

  /** The session shown at the top of the pane: the live one, or the last one. */
  get latest(): Session | null {
    return this.state.last ?? this.list[0] ?? null;
  }

  /**
   * Start listening. Called once, from the top of the app.
   *
   * Safe to call twice; the second call replaces the listeners rather than
   * doubling them, which is what a hot reload does during development.
   */
  async begin() {
    await this.end();
    this.state = (await watchState()) ?? noWatch();
    // A session may already be under way -- the app can be opened in the middle
    // of one -- so the events so far are fetched rather than waited for.
    this.events = await watchTail();
    this.hook = await hookState();
    this.list = await sessionsList();
    this.copies = await savesKept();
    this.loading = false;

    this.#stops.push(
      await listen<WatchState>("watch-state", (event) => {
        const was = this.state.recording;
        this.state = event.payload;
        // A new session has begun: the events on screen belong to the last one.
        if (this.state.recording && !was) this.events = [];
      }),
    );
    this.#stops.push(
      await listen<LiveEvent[]>("session-events", (event) => {
        const grown = this.events.concat(event.payload);
        this.events = grown.length > KEEP ? grown.slice(grown.length - KEEP) : grown;
      }),
    );
    // A copy is taken when the game finishes writing a save, so the list of them
    // changes while nobody is looking at it.
    this.#stops.push(
      await listen("saves-kept", () => {
        void this.reloadCopies();
      }),
    );
    this.#stops.push(
      await listen<Session>("session-ended", () => {
        // The brief arrives on the state too; this is for the history list,
        // which now has one more row in it.
        void this.reload();
      }),
    );
  }

  async end() {
    for (const stop of this.#stops) stop();
    this.#stops = [];
  }

  async reload() {
    this.list = await sessionsList();
  }

  async refreshHook() {
    this.hook = await hookState();
  }

  async reloadCopies() {
    this.copies = await savesKept();
  }
}

export const sessions = new Sessions();

/** `3 h 22 m`, `12 m`, `48 s` -- the same spelling the summary uses. */
export function spellDuration(seconds: number): string {
  if (seconds >= 3600) {
    const hours = Math.floor(seconds / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    return minutes === 0 ? `${hours} h` : `${hours} h ${minutes} m`;
  }
  if (seconds >= 60) return `${Math.floor(seconds / 60)} m`;
  return `${Math.round(seconds)} s`;
}

/** What a level means, for the one place that colours a line by it. */
export function levelName(lvl: number): string {
  return ["debug", "info", "warning", "error", "fatal"][lvl] ?? "info";
}
