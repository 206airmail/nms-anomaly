/**
 * The built-in Nexus browser.
 *
 * The page itself is a native webview stacked over the window by Tauri, not
 * anything in this document. So this holds only what our own chrome needs to
 * know — whether it is open and what it is showing — and the job of telling
 * the webview where to sit belongs to whichever component is drawing the
 * space for it.
 */

import { invoke } from "@tauri-apps/api/core";

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

const inTauri = () =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

class Browser {
  /** true once a page has been opened and not closed */
  open = $state(false);
  /** what it is showing, for the address line */
  url = $state("");
  /** bumped by `request`, so the app can switch to the Browse tab */
  requestId = $state(0);
  /** the page asked for but not yet handed to the webview */
  pending = $state<string | null>(null);
  error = $state<string | null>(null);

  /**
   * Ask for a page. Does not open it: the Browse tab does that once it has
   * measured the space, because the webview needs a box before it can exist.
   */
  request(url: string) {
    this.pending = url;
    this.url = url;
    this.error = null;
    this.requestId += 1;
  }

  /** Hand the pending page to the webview, in the box just measured. */
  async place(rect: Rect) {
    if (!inTauri()) return;
    const url = this.pending;
    this.pending = null;
    try {
      if (url) {
        // Set *before* awaiting, because from this point a webview may exist
        // whether or not the call comes back clean — and `open` is what mounts
        // the component that can move it. A show that created the view and then
        // failed to position it would otherwise leave it stacked over the
        // window with nothing able to reach it.
        this.open = true;
        await invoke("browser_show", { url, rect });
      } else if (this.open) {
        await invoke("browser_layout", { rect });
      }
    } catch (err) {
      this.error = String(err);
    }
  }

  /**
   * Park it offscreen. The page stays loaded, so coming back is instant.
   *
   * Deliberately asks every time, rather than only while `open`. That guard was
   * an optimisation that failed in the one case that matters: the webview is a
   * native surface stacked over the window, and `BrowsePane` mounts the
   * component that measures and moves it *only while `open` is true*. So the
   * moment the flag and the webview disagree — a `close` that threw on a hung
   * page, a `show` that created the view and then failed — a live page is left
   * floating over every other tab with nothing in the app able to move it, and
   * the only way out is to restart. `browser_hide` is a no-op when there is no
   * webview, so asking costs nothing and closes the trap.
   */
  async hide() {
    if (!inTauri()) return;
    try {
      await invoke("browser_hide");
    } catch {
      // Nothing useful to say: the browser is simply not showing.
    }
  }

  async close() {
    if (!inTauri()) {
      this.open = false;
      return;
    }
    try {
      await invoke("browser_close");
    } catch (err) {
      // It did not close, so it is still there. Reporting it shut would unmount
      // the only thing that can move it — see `hide`. Park it instead and say
      // what happened; the page is out of the way either way.
      this.error = String(err);
      await this.hide();
      return;
    }
    this.open = false;
    this.url = "";
    this.pending = null;
  }

  async go(what: "back" | "forward" | "reload") {
    if (!inTauri() || !this.open) return;
    try {
      await invoke("browser_go", { what });
    } catch (err) {
      this.error = String(err);
    }
  }
}

export const browser = new Browser();

/** The bit of a Nexus URL worth showing in an address line. */
export function tidyUrl(url: string): string {
  return url.replace(/^https:\/\/(www\.)?/, "").replace(/\/$/, "");
}
