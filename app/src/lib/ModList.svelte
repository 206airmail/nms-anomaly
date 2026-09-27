<script lang="ts">
  /** The one list of mods in the app.
   *
   * ---------------------------------------------------------------------------
   * Why there is only one
   * ---------------------------------------------------------------------------
   *
   * There used to be five. The Library listed every mod; Presets listed every
   * managed mod again with a checkbox on each row; Updates listed the ones
   * Nexus had something to say about; the load-order rail listed all of them a
   * fourth time; and the Sessions pane listed the ones the game complained
   * about. Five lists of the same sixty things, each owning one fact about
   * them, so learning what was true of a single mod meant visiting five
   * screens and matching folder names by eye.
   *
   * They are not five lists. They are one list and five *questions*, which is
   * what the filter chips are: each chip is a tab that used to exist, and the
   * count on it is the badge that used to sit in the sidebar. Picking a row
   * answers all five questions at once, in [`ModRecord`].
   *
   * ---------------------------------------------------------------------------
   * Two things about the interaction worth knowing before changing it
   * ---------------------------------------------------------------------------
   *
   * **A click selects, and that is all a click does.** The selected mod is the
   * one shown beside the list; there is no checkbox on any row. Acting on
   * several mods at once is a *mode*, entered by a button, because it is the
   * rarer job and it should not put a control on all sixty rows to pay for
   * itself. In that mode a click adds or removes, shift-click takes a range,
   * and nothing changes in the record beside it.
   *
   * That is also why Presets lost its checkbox column rather than this list
   * gaining one: two ways to switch a mod on, with different shapes, on two
   * screens, is how the two screens came to disagree about which mods were on.
   *
   * **A filter narrows what is listed, never what is acted on.** "All" in
   * select mode means every *listed* mod, so filtering to "Switched off" and
   * pressing All selects exactly those — which is the point of having both.
   */
  import type { Snippet } from "svelte";
  import Awaiting from "./Awaiting.svelte";
  import Button from "./Button.svelte";
  import type { Batch, Row } from "./library.svelte";

  interface Props {
    rows: Row[];
    loading: boolean;
    error?: string | null;

    /** the mod being read, which is what the record beside this is about */
    picked: string | null;
    onpick: (owner: string) => void;

    /**
     * Memberships this list cannot work out for itself.
     *
     * `attention` is the mods named in an action that is not merely tidying —
     * the same list the Actions tab ranks. `outdated` is the mods Nexus has a
     * newer file for. Both are passed in rather than read from their stores
     * here, so that the chip counts and the screens those facts come from
     * cannot disagree about which mods count.
     */
    attention: Set<string>;
    outdated: Set<string>;

    /** the select mode, owned by the parent so it can clear it after a delete */
    multi: boolean;
    chosen: Set<string>;

    batching?: boolean;
    batchError?: string | null;
    batchNote?: string | null;
    onbatch: (verb: Batch, owners: string[]) => void;

    onadd: () => void;
    adding?: boolean;

    /** drawn above the search box: the preset bar */
    header?: Snippet;
  }

  let {
    rows,
    loading,
    error = null,
    picked,
    onpick,
    attention,
    outdated,
    multi = $bindable(false),
    chosen = $bindable(new Set<string>()),
    batching = false,
    batchError = null,
    batchNote = null,
    onbatch,
    onadd,
    adding = false,
    header,
  }: Props = $props();

  type Filter = "all" | "attention" | "update" | "off" | "built";

  let filter = $state<Filter>("all");
  let query = $state("");

  /** Where a shift-click measures its range from. Meaningless outside `multi`. */
  let anchor = $state<string | null>(null);

  const TESTS: Record<Filter, (row: Row) => boolean> = {
    all: () => true,
    attention: (row) => attention.has(row.owner),
    update: (row) => outdated.has(row.owner),
    off: (row) => !row.enabled,
    built: (row) => row.variant !== null || row.edited,
  };

  /**
   * The chips, with their counts, in the order a person would ask them.
   *
   * A chip that would read zero is not drawn. A row of mostly-zero counters is
   * the dashboard this app was specifically not going to be, and a filter that
   * leads to an empty list is worse than one that is not offered.
   */
  const CHIPS: { id: Filter; name: string; hint: string }[] = [
    { id: "all", name: "All", hint: "Everything installed" },
    {
      id: "attention",
      name: "Needs attention",
      hint: "Named in something the Actions list wants doing",
    },
    { id: "update", name: "Update", hint: "Nexus has a newer version" },
    { id: "off", name: "Switched off", hint: "Installed, and out of the game" },
    {
      id: "built",
      name: "Our build",
      hint: "Cleaned, mended, combined or carrying values you set",
    },
  ];

  const chips = $derived(
    CHIPS.map((chip) => ({ ...chip, count: rows.filter(TESTS[chip.id]).length })).filter(
      (chip) => chip.id === "all" || chip.count > 0,
    ),
  );

  /**
   * The filter actually in force.
   *
   * Derived rather than corrected in an effect, because the chip a filter
   * belongs to can vanish underneath it: clean the last overweight mod and the
   * "Needs attention" chip goes, leaving a list filtered by a question nobody
   * can see being asked and no way back to the whole library but a rescan.
   */
  const active = $derived(chips.some((c) => c.id === filter) ? filter : "all");

  const needle = $derived(query.trim().toLowerCase());

  const shown = $derived(
    rows.filter((row) => {
      if (!TESTS[active](row)) return false;
      if (!needle) return true;
      // The folder as well as the title: the folder is what the game, the log
      // and every error message use, so it is worth being able to search for.
      return (
        row.name.toLowerCase().includes(needle) ||
        row.owner.toLowerCase().includes(needle)
      );
    }),
  );

  /** What "All" means in the batch bar: every *listed* mod this program owns. */
  const selectable = $derived(shown.filter((r) => r.managed));
  const picks = $derived(rows.filter((r) => chosen.has(r.owner) && r.managed));
  const manageable = $derived(rows.filter((r) => r.managed));
  const allPicked = $derived(
    selectable.length > 0 && selectable.every((r) => chosen.has(r.owner)),
  );
  /** Picked mods on a build of ours, so there is something to put back. */
  const fixedPicks = $derived(picks.filter((p) => p.variant || p.edited));

  /**
   * What a click on a row means, which depends only on the mode.
   *
   * Ordinarily: this is the selection, and the thing shown beside the list.
   * While picking several: add or remove it, and leave the record showing
   * whatever it was showing — the point of the mode is to build a set, not to
   * read each one.
   */
  function press(row: Row, event: MouseEvent) {
    if (!multi) {
      onpick(row.owner);
      return;
    }
    // Only mods this program installed can be switched or deleted, so there is
    // nothing a selection could do with the others.
    if (!row.managed) return;

    if (event.shiftKey && anchor) {
      // The range is taken over what is *listed*, not over the whole library:
      // shift-clicking down a filtered list must not quietly take in the mods
      // the filter is hiding between the two ends.
      const from = shown.findIndex((r) => r.owner === anchor);
      const to = shown.findIndex((r) => r.owner === row.owner);
      if (from !== -1 && to !== -1) {
        const [lo, hi] = from < to ? [from, to] : [to, from];
        const range = shown.slice(lo, hi + 1).filter((r) => r.managed);
        chosen = new Set([...chosen, ...range.map((r) => r.owner)]);
        return;
      }
    }
    const next = new Set(chosen);
    if (next.has(row.owner)) next.delete(row.owner);
    else next.add(row.owner);
    chosen = next;
    anchor = row.owner;
  }

  function toggleAll() {
    if (allPicked) {
      // Clears only what is listed, so a filtered "Clear" cannot silently drop
      // mods picked under a different filter a moment ago.
      const next = new Set(chosen);
      for (const row of selectable) next.delete(row.owner);
      chosen = next;
      return;
    }
    chosen = new Set([...chosen, ...selectable.map((r) => r.owner)]);
  }

  /**
   * Turn picking-several on or off.
   *
   * Entering carries the mod you were already looking at into the selection:
   * you reached for this button *because* you wanted that one and others too,
   * so starting from empty would throw away the choice you had already made.
   * Leaving drops the set, because outside the mode it means nothing.
   */
  function setMulti(on: boolean) {
    multi = on;
    if (on) {
      const here = rows.find((r) => r.owner === picked && r.managed);
      chosen = new Set(here ? [here.owner] : []);
      anchor = here?.owner ?? null;
    } else {
      chosen = new Set();
      anchor = null;
    }
  }
</script>

<aside class="list" class:selecting={multi}>
  <header>
    {#if header}
      {@render header()}
    {/if}

    <div class="tally">
      <span class="hud-label">Your mods</span>
      <span class="hint">
        {#if loading}
          reading…
        {:else if multi}
          {picks.length} of {manageable.length} selected
        {:else if needle || active !== "all"}
          {shown.length} of {rows.length}
        {:else}
          {rows.length}
        {/if}
      </span>
      <div class="controls">
        {#if manageable.length}
          <!-- A mode switch, not an action, so it never takes the action colour
               even while engaged -- "Add a mod" is the one thing here that
               does something, and two greens side by side would argue about
               which. Being engaged is said by the outline and by the list
               itself, which visibly changes underneath it. -->
          <span class="mode" class:on={multi}>
            <Button
              onclick={() => setMulti(!multi)}
              title={multi ? "Go back to one at a time" : "Act on several mods at once"}
            >
              {multi ? "Done" : "Select"}
            </Button>
          </span>
        {/if}
        <Button variant="primary" onclick={onadd} disabled={adding}>Add a mod</Button>
      </div>
    </div>

    <!-- Sixty mods and, until this, no way to find one by name but the eye.
         Above the chips because it is the commoner question by far. -->
    <div class="find">
      <input
        type="search"
        bind:value={query}
        placeholder="Find a mod…"
        spellcheck="false"
        aria-label="Find a mod by name or folder"
      />
    </div>

    {#if chips.length > 1}
      <div class="chips" role="group" aria-label="Filter the list">
        {#each chips as chip (chip.id)}
          <button
            class="chip"
            class:on={active === chip.id}
            title={chip.hint}
            aria-pressed={active === chip.id}
            onclick={() => (filter = chip.id)}
          >
            <span>{chip.name}</span>
            <span class="n">{chip.count}</span>
          </button>
        {/each}
      </div>
    {/if}
  </header>

  {#if loading}
    <Awaiting>Reading the mods folder…</Awaiting>
  {:else if error}
    <p class="failed">{error}</p>
  {:else if !shown.length}
    <p class="hint empty">
      {#if needle}
        Nothing here is called “{query.trim()}”.
      {:else}
        Nothing installed yet.
      {/if}
    </p>
  {:else}
    <ol>
      {#each shown as row (row.owner)}
        <li>
          <button
            class="row"
            class:shown={!multi && picked === row.owner}
            class:ticked={multi && chosen.has(row.owner)}
            class:off={!row.enabled}
            class:inert={multi && !row.managed}
            title={multi && !row.managed
              ? `${row.owner} — installed outside this program, so it cannot be switched or deleted here`
              : row.owner}
            aria-pressed={multi ? chosen.has(row.owner) : undefined}
            onclick={(event) => press(row, event)}
          >
            <!-- Whether the game is loading this mod right now, on every row
                 rather than only on the ones that are off: "no badge" is not a
                 state anybody can read, and the question "which of these is
                 actually on" is the one this list is most often opened for.
                 The word beside it carries the same fact, because a colour on
                 its own is not a cue. -->
            <span
              class="lamp"
              class:lit={row.active}
              aria-hidden="true"
              title={row.active
                ? "Active in the game"
                : row.mergedInto
                  ? "Held out of the game by a merge"
                  : "Not active"}
            ></span>

            <span class="who">{row.name}</span>

            <!-- At most three, and never two that say the same thing. The
                 first is why you would come looking for this mod; the second
                 is what the game is doing with it; the third is whose build
                 it is running. -->
            {#if attention.has(row.owner)}
              <span class="badge badge-alert">fix</span>
            {/if}
            {#if row.mergedInto}
              <!-- Switched on, and still not in the game. Saying "inactive"
                   here would look like the user had turned it off. -->
              <span class="badge badge-info">merged</span>
            {:else if !row.enabled}
              <span class="badge badge-quiet">inactive</span>
            {:else if outdated.has(row.owner)}
              <span class="badge badge-signal">update</span>
            {/if}
            {#if row.variant}
              <span class="badge badge-info">{row.variant}</span>
            {/if}
            <!-- Beside the variant rather than instead of it: a cleaned mod
                 carrying values you set is both, and one badge could only have
                 said one of them. Amber because it is the one thing in the row
                 that loads because you said so. -->
            {#if row.edited}
              <span class="badge badge-signal">edited</span>
            {/if}
          </button>
        </li>
      {/each}
    </ol>
  {/if}

  <!-- Present for the whole mode, not only once something is picked, so the
       way back out and "select all" are never somewhere you have to guess. -->
  {#if multi}
    <div class="batch">
      <div class="line">
        <span class="hint">
          {#if picks.length === 0}
            Click the mods you want
          {:else if picks.length === 1}
            {picks[0].name}
          {:else}
            {picks.length} mods
          {/if}
        </span>
        <button class="btn-link" onclick={toggleAll}>
          {allPicked ? "Clear" : "All"}
        </button>
      </div>
      <div class="verbs">
        <Button
          onclick={() => onbatch("activate", picks.map((p) => p.owner))}
          busy={batching}
          disabled={!picks.length}
        >
          Activate
        </Button>
        <Button
          onclick={() => onbatch("deactivate", picks.map((p) => p.owner))}
          busy={batching}
          disabled={!picks.length}
        >
          Deactivate
        </Button>
        <!-- Only when something picked is actually on a build of ours: a
             button that would say "nothing needed putting back" however it is
             pressed is one more thing to read past. -->
        {#if fixedPicks.length}
          <Button
            onclick={() => onbatch("revert", fixedPicks.map((p) => p.owner))}
            busy={batching}
          >
            Use the author's build ({fixedPicks.length})
          </Button>
        {/if}
        <Button
          variant="danger"
          onclick={() => onbatch("delete", picks.map((p) => p.owner))}
          busy={batching}
          disabled={!picks.length}
        >
          Delete…
        </Button>
      </div>
    </div>
  {/if}
  {#if batchError}<p class="failed">{batchError}</p>{/if}
  {#if batchNote}<p class="hint">{batchNote}</p>{/if}
</aside>

<style>
  /* The list is recessed relative to the record beside it: the thing you are
     reading should sit in front of the thing you are choosing from. */
  /* The list is recessed relative to the record beside it: the thing you are
     reading should sit in front of the thing you are choosing from.

     Recession used to be a black wash fading out over the top 40% of the
     column, translucent like every other surface here. That had to go opaque,
     because the header below is sticky and four rows tall and sixty-four mods
     scroll under it -- see the note there. Once the header is solid, a
     translucent column behind it leaves a seam wherever the two disagree, so
     the whole column is solid and they cannot.

     `--void` because it is the only token that *is* opaque; the rest carry
     alpha, `--hull` included at 0.86. Recession is now carried by the border
     and by the record side, whose panels are lighter glass sitting on the same
     void -- which is the more honest way round: the list is the page, and the
     thing you are reading floats on it. */
  .list {
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-right: 1px solid var(--rule);
    background: var(--void);
    padding: var(--pad) 0.75rem 1.25rem;
    overflow-y: auto;
  }

  /* The header carries four things now — the preset bar, the tally and its
     buttons, the search box and the chips — so it is a stack rather than a
     row, and it stays put while sixty rows scroll under it. */
  .list > header {
    position: sticky;
    top: calc(var(--pad) * -1);
    z-index: 2;
    margin: calc(var(--pad) * -1) -0.75rem 0.5rem;
    padding: var(--pad) 0.75rem 0.625rem;

    /* Opaque, and the same opaque as the column above, so there is no seam.

       It was `--hull` fading to transparent, which was fine for the one-line
       header this replaced: a 30px strip leaking
       14% of whatever passed under it is a strip nobody looks at. This header
       is four stacked rows -- preset, tally, search, chips -- and sixty-four
       mods scroll under all of it, so the leak became mod titles legibly
       crossing the search box. Translucency is the look everywhere else in this
       app; on a sticky header it is a bug.

       The rule and the cast beneath it do the work the fade-to-transparent used
       to: rows read as tucking under the header rather than being clipped by
       it. */
    background: var(--void);
    box-shadow:
      0 1px 0 var(--rule),
      0 10px 16px -12px rgba(0, 0, 0, 0.85);
  }

  .tally {
    display: flex;
    align-items: center;
    gap: 0.4375rem;
    min-width: 0;
  }

  .tally .hint {
    margin: 0;
  }

  .controls {
    display: flex;
    flex: none;
    align-items: center;
    gap: 0.375rem;
    margin-left: auto;
  }

  /* Only a hook for the engaged state; it must not become a layout of its own
     between the flex row and the button. */
  .mode {
    display: flex;
  }

  /* Engaged: lit outline in the structure colour, the same way the active nav
     item is marked. Read as "this is switched on", not as "press me". */
  .mode.on :global(.btn) {
    border-color: var(--cyan);
    color: var(--cyan);
    background: var(--cyan-wash);
  }

  .mode.on :global(.btn:hover) {
    background: rgba(53, 214, 200, 0.2);
    color: var(--cyan);
  }

  /* The header is tight once it holds two buttons, so they shrink rather than
     wrap the title onto its own line. */
  .controls :global(.btn) {
    padding: 0.3125rem 0.625rem;
    min-height: 1.875rem;
    font-size: var(--t-micro);
  }

  /* ---- finding ---- */

  .find {
    margin-top: 0.5rem;
  }

  .find input {
    width: 100%;
    padding: 0.375rem 0.5625rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-sm);
    background: var(--inset);
    color: var(--readout);
    font: inherit;
    font-size: var(--t-small);
  }

  .find input:focus {
    outline: none;
    border-color: var(--cyan);
    box-shadow: 0 0 0 1px var(--cyan-wash);
  }

  .find input::placeholder {
    color: var(--faint);
  }

  /* Each chip is a tab that used to be in the sidebar, and the number on it is
     the badge that used to be beside that tab's name. Wrapping rather than
     scrolling: five short words fit two lines in this column, and a row that
     scrolls sideways hides the very counts it exists to show. */
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
    margin-top: 0.4375rem;
  }

  .chips .chip {
    display: flex;
    align-items: center;
    gap: 0.3125rem;
    padding: 0.1875rem 0.4375rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-pill);
    background: none;
    color: var(--faint);
    font-size: var(--t-micro);
    transition:
      border-color var(--quick) var(--ease),
      color var(--quick) var(--ease),
      background var(--quick) var(--ease);
  }

  .chips .chip:hover {
    color: var(--readout);
    border-color: var(--rule-hi);
    background: var(--glass-2);
  }

  .chips .chip.on {
    border-color: var(--cyan);
    background: var(--cyan-wash);
    color: var(--cyan);
  }

  .chips .chip .n {
    font-family: var(--font-value);
    font-size: 0.6875rem;
    color: var(--dim);
  }

  .chips .chip.on .n {
    color: var(--cyan);
  }

  /* ---- the rows ---- */

  ol {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .empty {
    padding: 0.75rem 0.5rem;
  }

  /* One row, one control, no column of boxes. Everything a row can say about
     itself -- shown, picked, switched off, not ours -- is said by the strip. */
  .list li .row {
    position: relative;
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    align-items: center;
    gap: 0.4375rem;
    width: 100%;
    padding: 0.4375rem 0.5rem 0.4375rem 0.625rem;
    border: 1px solid transparent;
    border-radius: var(--r-md);
    text-align: left;
    font-size: var(--t-small);
    color: var(--dim);
    transition:
      background var(--quick) var(--ease),
      color var(--quick) var(--ease),
      box-shadow var(--quick) var(--ease);
  }

  .list li .row:hover {
    background: var(--glass-2);
    color: var(--readout);
  }

  /* A switched-off mod is still a mod: dimmed, not struck through.
     Strike-through reads as "deleted", which is the one thing it is not. */
  .list li .row.off {
    opacity: 0.5;
  }

  .list li .row.off:hover {
    opacity: 0.85;
  }

  /* The row being read, which is the ordinary meaning of "selected" here. It is
     the only thing on this side with real elevation, so it reads as "in front"
     rather than merely as a different colour. */
  .list li .row.shown {
    background: var(--glass-hi);
    border-color: var(--rule-hi);
    color: var(--readout);
    box-shadow: var(--cast-near), var(--lip);
  }

  /* Picked, while picking several. Deliberately a *different* shape from
     `.shown` -- a filled bar down the edge rather than a raised surface -- so
     that "in the set" and "the one I am reading" can never be confused for one
     another, and so it does not look like a checkbox by other means. */
  .list li .row.ticked {
    background: var(--accent-wash);
    color: var(--readout);
  }

  .list li .row.ticked::before {
    content: "";
    position: absolute;
    left: 0;
    top: 0.3125rem;
    bottom: 0.3125rem;
    width: 2px;
    border-radius: 0 2px 2px 0;
    background: var(--accent);
    box-shadow: 0 0 8px rgba(138, 224, 60, 0.5);
  }

  /* While picking several, a mod this program did not install cannot join the
     set. Saying so by making it unreactive is quieter than a badge on every
     such row, and the title says why. */
  .list li .row.inert {
    opacity: 0.35;
    cursor: default;
  }

  .list li .row.inert:hover {
    background: none;
    color: var(--dim);
  }

  .who {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Three badges can land on one row, so they are allowed to sit together in
     the last column rather than each claiming a track of the grid. */
  .list li .row :global(.badge) {
    flex: none;
  }

  /* Lit when the game is loading this mod. Small enough to scan a column of
     sixty without it becoming the loudest thing in the list, and never the only
     thing saying so -- an inactive row is dimmed and carries the word too. */
  .lamp {
    flex: none;
    width: 0.4375rem;
    height: 0.4375rem;
    border: 1px solid var(--faint);
    border-radius: 50%;
    background: none;
  }

  .lamp.lit {
    border-color: var(--accent);
    background: var(--accent);
    box-shadow: 0 0 6px rgba(138, 224, 60, 0.6);
  }

  /* ---- acting on several ---- */

  /* Pinned to the bottom of the list and sitting *above* it: it appears only
     when a selection exists, and it must not push the list or be scrolled away
     from the rows it acts on. */
  .batch {
    position: sticky;
    bottom: -1.25rem;
    margin: 0.75rem -0.75rem -1.25rem;
    padding: 0.5rem 0.75rem 0.625rem;
    border-top: 1px solid var(--rule);
    background: linear-gradient(180deg, rgba(8, 16, 15, 0.86), var(--hull));
    backdrop-filter: blur(12px);
    box-shadow: 0 -12px 28px -18px rgba(0, 0, 0, 0.9);
  }

  .batch .line {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 0.5rem;
    margin: 0 0 0.375rem;
  }

  .batch .hint {
    min-width: 0;
    margin: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .batch .line .btn-link {
    flex: none;
    font-size: var(--t-micro);
  }

  /* "Activate" and "Deactivate" do not fit beside "Delete…" in this column, so
     the row is allowed to wrap rather than the words being shortened back into
     something that does not say what the button does. */
  .batch .verbs {
    margin-top: 0;
    gap: 0.375rem;
  }

  .batch :global(.btn) {
    flex: 1;
    min-height: 1.875rem;
    padding: 0.3125rem 0.5rem;
    font-size: var(--t-micro);
  }
</style>
