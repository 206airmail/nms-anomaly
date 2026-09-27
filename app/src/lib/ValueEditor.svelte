<script lang="ts">
  /** Change a value a mod sets.
   *
   * ---------------------------------------------------------------------------
   * The shape, and why it is a list of boxes rather than a diff
   * ---------------------------------------------------------------------------
   *
   * Everywhere else in this app a value is something to *read*: a claim stack
   * shows who claims what and which claim loads, and the amber marks the
   * winner. There is nothing to type. Here the value is the subject, so the box
   * holding it is the row, and the two facts a person needs while typing sit
   * either side of it:
   *
   *   PercentageChance
   *   Table/GenericTable[R_SCRAPHEAP]/List/Reward[0]
   *                         [ 45.000000 ]      GAME 25.000000
   *                           ^ the box            ^ without this mod
   *                           placeholder: 60.000000 — what the author set
   *
   * The placeholder is the mechanism, not decoration. The box always opens
   * holding the value that will load, so an untouched property shows the
   * author's own number — and clearing the box *is* how you go back to it,
   * because an empty box shows it greyed behind the cursor and an empty box
   * means "whatever the author said". There is no separate revert control for
   * the common case and no third column to read.
   *
   * ---------------------------------------------------------------------------
   * What is not here
   * ---------------------------------------------------------------------------
   *
   * Properties the mod does *not* change. The engine offers exactly the set
   * `prune::changed_leaves` computes, which is the same set cleaning keeps, so
   * anything outside it would be an edit that quietly disappeared the next time
   * the mod was cleaned. Browsing the whole vanilla asset to start changing a
   * property the mod never touched is a different feature — a patch author —
   * and this is not a smaller version of it.
   *
   * Amber is used once, on the left edge of a row whose value is not the
   * author's. That does not contradict the colour contract: every box here
   * holds a value the game will load, and the bar marks the ones that load
   * *because you said so*.
   */
  import Awaiting from "./Awaiting.svelte";
  import Button from "./Button.svelte";
  import {
    editApply,
    editForgetValues,
    editSurvey,
    editUndo,
    type Applied,
    type EditAsset,
    type EditField,
    type EditSurvey,
    type StaleValue,
    type ValueSort,
    type WantedValue,
  } from "./engine";
  import { assetName } from "./types";

  interface Props {
    /** the mod folder to edit, as the list knows it */
    owner: string;
    /** the title to show, which is not the folder name */
    name: string;
    modsDir: string | undefined;
    /** reload the library: this changes which build the game reads */
    onchanged: () => void;
    onclose: () => void;
    /**
     * The survey, when the caller already has one.
     *
     * The render check draws this component under Node, where nothing can be
     * awaited, so it hands in `demoSurvey` rather than watching the editor sit
     * on its opening "Reading…" line for ever. Left out everywhere else: the
     * editor reads the mod itself, which is the only way the values can be
     * current.
     */
    initial?: EditSurvey | null;
  }

  let { owner, name, modsDir, onchanged, onclose, initial = null }: Props = $props();

  let survey = $state<EditSurvey | null>(null);
  let error = $state<string | null>(null);
  /** reading the mod, which happens on open and after every change */
  let loading = $state(true);
  /** writing: a build is being made and the mods folder relinked */
  let busy = $state(false);
  let chosen = $state<string | null>(null);
  let filter = $state("");
  let onlyMine = $state(false);
  let result = $state<Applied | null>(null);

  /**
   * What each box holds, keyed by `<file>\0<path>`.
   *
   * A flat map rather than state hung off each field, because the fields are
   * replaced wholesale every time the mod is re-read. A NUL separator because a
   * property path can hold anything a game asset can, including every character
   * that would otherwise do as one.
   *
   * A key is absent until the box is typed in, which is what makes "has this
   * been changed" a question with an answer.
   */
  let draft = $state<Record<string, string>>({});

  const keyOf = (file: string, path: string) => `${file}\u0000${path}`;

  /**
   * Are these the same value?
   *
   * Mirrors `exml::values_equal`, and has to: the engine treats a value typed
   * as `40` against an author's `40.000000` as no change at all, so a UI that
   * counted it as one would offer an Apply that then reported nothing to do.
   */
  function same(a: string, b: string): boolean {
    const [x, y] = [a.trim(), b.trim()];
    if (x === y) return true;
    const [n, m] = [Number(x), Number(y)];
    return x !== "" && y !== "" && Number.isFinite(n) && Number.isFinite(m) && n === m;
  }

  /** The value that loads now: yours if you set one, otherwise the author's. */
  const saved = (field: EditField) => field.yours ?? field.author;

  /** What the box holds: the draft if it has been typed in, else what loads. */
  function held(file: string, field: EditField): string {
    const key = keyOf(file, field.path);
    return key in draft ? draft[key] : saved(field);
  }

  /** Everything this mod has that can be edited at all. */
  const editable = $derived(
    (survey?.assets ?? []).filter((a) => a.refused === null && a.fields.length > 0),
  );
  const refused = $derived((survey?.assets ?? []).filter((a) => a.refused !== null));

  const asset = $derived<EditAsset | null>(
    editable.find((a) => a.file === chosen) ?? editable[0] ?? null,
  );

  /** How many properties this mod changes, across every asset. */
  const changes = $derived(editable.reduce((n, a) => n + a.fields.length, 0));

  /** Values already applied, and carried by the build the game is reading. */
  const applied = $derived(
    editable.reduce((n, a) => n + a.fields.filter((f) => f.yours !== null).length, 0),
  );

  /** The rows to draw: the chosen asset's, filtered. */
  const rows = $derived.by(() => {
    if (!asset) return [];
    const needle = filter.trim().toLowerCase();
    return asset.fields.filter((field) => {
      if (onlyMine && field.yours === null) return false;
      if (!needle) return true;
      return (
        field.path.toLowerCase().includes(needle) ||
        field.author.toLowerCase().includes(needle)
      );
    });
  });

  /**
   * A cap on what goes into the page.
   *
   * A whole-file scene override changes tens of thousands of properties, and
   * the honest answer to "draw all of them" is a window that stops responding.
   * The filter is the way through a list that long, so the cap is stated with
   * the count it is holding back rather than truncating in silence.
   */
  const CAP = 300;
  const shown = $derived(rows.slice(0, CAP));

  /** What a value must look like. Mirrors `edit::check` in the engine. */
  function wrong(sort: ValueSort, value: string): string | null {
    const text = value.trim();
    // Empty is not a bad value, it is "the author's value". See the note above.
    if (text === "") return null;
    if (sort === "bool" && !/^(true|false)$/i.test(text)) return "True or False";
    if (sort === "int" && !/^-?\d+$/.test(text)) return "a whole number";
    if (sort === "float" && !Number.isFinite(Number(text))) return "a number";
    return null;
  }

  /** Every box whose value differs from what is recorded, across every asset. */
  const pending = $derived.by((): WantedValue[] => {
    const out: WantedValue[] = [];
    for (const a of editable) {
      for (const field of a.fields) {
        const key = keyOf(a.file, field.path);
        if (!(key in draft)) continue;
        const text = draft[key].trim();
        // Empty, or typed back to the author's own value: not an edit.
        const want = text === "" || same(text, field.author) ? null : text;
        if (want === field.yours) continue;
        out.push({ file: a.file, path: field.path, value: want, author: field.author });
      }
    }
    return out;
  });

  /** A box that cannot be applied stops the apply, and says which one. */
  const faults = $derived.by((): string[] => {
    const out: string[] = [];
    for (const a of editable) {
      for (const field of a.fields) {
        const key = keyOf(a.file, field.path);
        if (!(key in draft)) continue;
        const must = wrong(field.sort, draft[key]);
        if (must) out.push(`${leaf(field.path)} must be ${must}`);
      }
    }
    return out;
  });

  /** The last segment of a property path: the name of the thing being set. */
  function leaf(path: string): string {
    const at = path.lastIndexOf("/");
    return at === -1 ? path : path.slice(at + 1);
  }

  /** Everything before it, which is where in the asset this sits. */
  function parent(path: string): string {
    const at = path.lastIndexOf("/");
    return at === -1 ? "" : path.slice(0, at);
  }

  /**
   * Read the mod.
   *
   * Called directly rather than from an `$effect`: an effect that reads the
   * survey and then writes it is a loop waiting to happen, and the moments this
   * needs to run are all ones the code already knows about — opening, and after
   * each change, because an apply changes `yours` on every field it touched.
   */
  async function read(): Promise<void> {
    // A survey handed in stands in for the engine, and the whole of this runs
    // before the first `await`, so the list is drawn on the first render rather
    // than after a tick the render check does not get to wait for.
    if (initial !== null) {
      survey = initial;
      loading = false;
      return;
    }
    loading = true;
    error = null;
    try {
      survey = await editSurvey(owner, modsDir);
    } catch (err) {
      error = String(err);
      survey = null;
    } finally {
      loading = false;
    }
  }

  async function apply(): Promise<void> {
    busy = true;
    error = null;
    result = null;
    try {
      result = await editApply(owner, pending, modsDir);
      draft = {};
      onchanged();
      await read();
    } catch (err) {
      error = String(err);
    } finally {
      busy = false;
    }
  }

  async function undoAll(): Promise<void> {
    busy = true;
    error = null;
    result = null;
    try {
      await editUndo(owner, modsDir);
      draft = {};
      onchanged();
      await read();
    } catch (err) {
      error = String(err);
    } finally {
      busy = false;
    }
  }

  /**
   * Drop values the mod can no longer carry.
   *
   * No rebuild after it: the build already does not carry them, so this only
   * stops the record holding something nothing applies.
   */
  async function forgetStale(): Promise<void> {
    const gone: StaleValue[] = survey?.stale ?? [];
    if (gone.length === 0) return;
    busy = true;
    error = null;
    try {
      await editForgetValues(owner, gone);
      await read();
    } catch (err) {
      error = String(err);
    } finally {
      busy = false;
    }
  }

  void read();
</script>

<div class="frame">
  <div class="body">
    {#if error}
      <p class="failed">{error}</p>
    {/if}

    {#if loading && survey === null}
      <Awaiting>
        Reading every asset {name} ships and comparing it with the game's own copies…
      </Awaiting>
    {:else if survey}
      <p class="lede">
        Every property {name} changes, with the value the game has without it. The
        box holds what will load; <b>empty it to go back to the author's value</b>,
        which sits greyed behind the cursor.
      </p>
      <p class="hint">
        {changes === 1 ? "1 property changed" : `${changes} properties changed`}
        {#if applied > 0}· {applied} carrying your value{/if}
        {#if refused.length}
          · {refused.length} asset{refused.length === 1 ? "" : "s"} could not be read
        {/if}
      </p>

      <!-- Values this mod can no longer carry. Said here rather than left to
           turn up as a `lost` line on the next rebuild, which is a message
           nobody would be reading by then. -->
      {#if survey.stale.length}
        <div class="warn">
          <p>
            {survey.stale.length === 1 ? "A value is" : `${survey.stale.length} values are`}
            held for something this mod does not have any more, so
            {survey.stale.length === 1 ? "it can" : "they can"} neither be applied
            nor shown. An update moved or removed
            {survey.stale.length === 1 ? "it" : "them"}.
          </p>
          <ul class="gone">
            {#each survey.stale as value (value.file + value.path)}
              <li>
                <span class="mono">{value.path}</span> = {value.value}
                &mdash; {value.why}
              </li>
            {/each}
          </ul>
          <Button onclick={forgetStale} busy={busy}>Forget them</Button>
        </div>
      {/if}

      {#if result}
        {#if result.done.length}
          <p class="ok">{result.done.join(" · ")}</p>
        {/if}
        {#if result.lost.length}
          <div class="warn">
            {#each result.lost as line (line)}<p>{line}</p>{/each}
          </div>
        {/if}
      {/if}

      {#if editable.length === 0}
        <p class="quiet">
          Nothing here to change.
          {#if refused.length}
            Every asset this mod ships could not be compared with the game's own
            copy — the reasons are below.
          {:else}
            Nothing it ships holds a value that differs from the game's own.
          {/if}
        </p>
      {:else}
        <div class="split">
          <!-- Which file. Only when there is a choice: most mods are one asset,
               and a list of one is furniture. -->
          {#if editable.length > 1}
            <nav class="files">
              {#each editable as a (a.file)}
                {@const mine = a.fields.filter((f) => f.yours !== null).length}
                <button
                  class="pick"
                  class:on={a.file === asset?.file}
                  onclick={() => {
                    chosen = a.file;
                    filter = "";
                  }}
                >
                  <span class="what">{assetName(a.target)}</span>
                  <span class="counts">
                    {a.fields.length}
                    {#if mine > 0}<em>· {mine} yours</em>{/if}
                  </span>
                </button>
              {/each}
            </nav>
          {/if}

          <div class="values">
            {#if asset}
              <header class="head">
                <div class="which">
                  <span class="hud-label">{assetName(asset.target)}</span>
                  <span class="mono where">{asset.folder}/{asset.rel}</span>
                </div>
                <input
                  class="find"
                  type="text"
                  aria-label="Filter properties"
                  placeholder="Filter by property or value"
                  bind:value={filter}
                />
                <label class="toggle">
                  <input type="checkbox" bind:checked={onlyMine} />
                  Only mine
                </label>
              </header>

              {#if !asset.in_game}
                <p class="hint">
                  The game ships no copy of this asset — the mod adds it — so every
                  property in it is the author's own and there is nothing to compare
                  against.
                </p>
              {:else if asset.whole_file}
                <p class="hint">
                  A whole-file override, decompiled to be read. Your values are
                  compiled back into it.
                </p>
              {/if}

              {#if rows.length === 0}
                <p class="quiet">
                  {onlyMine
                    ? "You have not set a value in this asset."
                    : "No property here matches that."}
                </p>
              {:else}
                <ul class="rows">
                  {#each shown as field (field.path)}
                    {@const key = keyOf(asset.file, field.path)}
                    {@const now = held(asset.file, field)}
                    {@const bad = wrong(field.sort, now)}
                    {@const mine = now.trim() !== "" && !same(now, field.author)}
                    <li class="row" class:mine class:bad={bad !== null}>
                      <div class="what">
                        <span class="leaf mono">{leaf(field.path)}</span>
                        {#if parent(field.path)}
                          <span class="in mono" title={field.path}>
                            {parent(field.path)}
                          </span>
                        {/if}
                        {#if field.author_moved}
                          <span class="moved">
                            the author now ships
                            <span class="mono">{field.author}</span>; your value
                            replaced <span class="mono">{field.author_moved}</span>
                          </span>
                        {/if}
                      </div>

                      <div class="box">
                        {#if field.sort === "bool"}
                          <!-- Two values, so a list of them beats a box you can
                               mistype. Blank is still "the author's value", and
                               it is the first option rather than a control of
                               its own. -->
                          <select
                            aria-label={field.path}
                            value={now}
                            onchange={(event) => {
                              draft[key] = event.currentTarget.value;
                            }}
                          >
                            <option value="">author's ({field.author})</option>
                            <option value="True">True</option>
                            <option value="False">False</option>
                          </select>
                        {:else}
                          <input
                            class="mono"
                            type="text"
                            spellcheck="false"
                            autocomplete="off"
                            aria-label={field.path}
                            value={now}
                            placeholder={field.author}
                            oninput={(event) => {
                              draft[key] = event.currentTarget.value;
                            }}
                          />
                        {/if}
                        {#if bad}<span class="says">must be {bad}</span>{/if}
                      </div>

                      <!-- The game's own value. A button, because reading it and
                           wanting it are the same gesture often enough that
                           making someone retype the number would be rude. -->
                      {#if field.vanilla === null}
                        <span class="vanilla none" title="the mod introduces this">
                          not in the game
                        </span>
                      {:else}
                        <button
                          class="vanilla"
                          title="use the game's own value"
                          onclick={() => {
                            draft[key] = field.vanilla ?? "";
                          }}
                        >
                          <span class="tag">game</span>
                          <span class="mono">{field.vanilla}</span>
                        </button>
                      {/if}
                    </li>
                  {/each}
                </ul>
                {#if rows.length > shown.length}
                  <p class="hint">
                    Showing {shown.length} of {rows.length}. Filter to reach the rest.
                  </p>
                {/if}
              {/if}
            {/if}
          </div>
        </div>
      {/if}

      {#if refused.length}
        <div class="refused">
          {#each refused as a (a.file)}
            <p class="hint">
              <span class="mono">{a.folder}/{a.rel}</span> — {a.refused}
            </p>
          {/each}
        </div>
      {/if}
    {/if}
  </div>

  <!-- Pinned under the list, like every sheet's verbs: a long list of
       properties scrolls behind the apply button rather than pushing it off the
       bottom of the window. -->
  <div class="foot">
    {#if faults.length}
      <p class="failed">{faults.join(" · ")}</p>
    {/if}
    <div class="acts">
      <Button
        variant="primary"
        onclick={apply}
        busy={busy}
        disabled={pending.length === 0 || faults.length > 0 || loading}
      >
        {pending.length === 0
          ? "Nothing to apply"
          : pending.length === 1
            ? "Apply 1 change"
            : `Apply ${pending.length} changes`}
      </Button>
      {#if pending.length > 0}
        <Button onclick={() => (draft = {})} disabled={busy}>Discard edits</Button>
      {/if}
      {#if applied > 0}
        <Button
          variant="danger"
          onclick={undoAll}
          busy={busy}
          title="forget every value you have set on this mod"
        >
          Use the mod's own values
        </Button>
      {/if}
      <Button onclick={onclose} disabled={busy}>Close</Button>
    </div>
  </div>
</div>

<style>
  /* The sheet's body is a flex item with a definite height, so this fills it
     and the list inside scrolls. The whole frame is also allowed to scroll, as
     a fallback for a window short enough that the heading matter alone fills
     it -- otherwise the list would be squeezed to nothing. */
  .frame {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  .body {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .quiet {
    margin: 0.875rem 0 0;
    color: var(--faint);
    font-size: var(--t-small);
  }

  .mono {
    font-family: var(--font-value);
  }

  /* Files beside values. The file list is a fixed column: it is navigation, and
     navigation that changes width as you move through it reads as the page
     shifting under the pointer. */
  .split {
    display: flex;
    flex: 1;
    gap: 0.875rem;
    min-height: 14rem;
    margin-top: 0.875rem;
  }

  .files {
    flex: none;
    width: 15rem;
    overflow-y: auto;
    padding-right: 0.25rem;
    border-right: 1px solid var(--rule);
  }

  .pick {
    display: block;
    width: 100%;
    padding: 0.4rem 0.5rem;
    border: 0;
    border-radius: var(--r-sm);
    background: none;
    color: var(--dim);
    text-align: left;
  }

  .pick:hover {
    background: var(--film);
    color: var(--readout);
  }

  .pick.on {
    background: var(--cyan-wash);
    color: var(--readout);
  }

  .pick .what {
    display: block;
    font-size: var(--t-small);
    font-weight: 600;
  }

  .counts {
    display: block;
    color: var(--faint);
    font-size: var(--t-micro);
  }

  .counts em {
    color: var(--signal);
    font-style: normal;
  }

  .values {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-width: 0;
    min-height: 0;
  }

  .head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.5rem 0.75rem;
    padding-bottom: 0.5rem;
    border-bottom: 1px solid var(--rule);
  }

  .which {
    flex: 1;
    min-width: 0;
  }

  .where {
    display: block;
    overflow: hidden;
    color: var(--faint);
    font-size: var(--t-micro);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .find {
    width: 16rem;
    padding: 0.35rem 0.55rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-sm);
    background: var(--inset);
    color: var(--readout);
    font-family: var(--font-ui);
    font-size: var(--t-small);
  }

  .find:focus {
    outline: none;
    border-color: var(--rule-hi);
  }

  .toggle {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    color: var(--dim);
    font-size: var(--t-small);
  }

  .rows {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  /* One property: where it is, the box, and what the game has. Three columns so
     every box starts at the same x -- a column of inputs that steps in and out
     with the length of each name is not readable as a column. */
  .row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 11rem auto;
    align-items: start;
    gap: 0.75rem;
    padding: 0.4rem 0.5rem 0.4rem 0.625rem;
    border-bottom: 1px solid var(--rule-soft);
    border-left: 2px solid transparent;
  }

  /* The one use of amber here: this value loads because you said so. */
  .row.mine {
    border-left-color: var(--signal);
    background: var(--signal-wash);
  }

  .row.bad {
    border-left-color: var(--contest);
  }

  .row .what {
    min-width: 0;
  }

  .leaf {
    display: block;
    color: var(--readout);
    font-size: var(--t-small);
    font-weight: 500;
    overflow-wrap: anywhere;
  }

  /* Where in the asset this sits. Dim, and under the name rather than before
     it: two rows differ by their leaf far more often than by their path, and
     putting the path first means reading the same forty characters every line. */
  .in {
    display: block;
    color: var(--ghost);
    font-size: var(--t-micro);
    overflow-wrap: anywhere;
  }

  .moved {
    display: block;
    margin-top: 0.15rem;
    color: var(--signal);
    font-size: var(--t-micro);
  }

  .box input,
  .box select {
    width: 100%;
    padding: 0.3rem 0.45rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-sm);
    background: var(--inset);
    color: var(--readout);
    font-size: var(--t-small);
  }

  .box input::placeholder {
    color: var(--ghost);
  }

  .box input:focus,
  .box select:focus {
    outline: none;
    border-color: var(--cyan);
  }

  .row.bad .box input {
    border-color: var(--contest);
  }

  .says {
    display: block;
    margin-top: 0.15rem;
    color: var(--contest);
    font-size: var(--t-micro);
  }

  /* What the game has. Quiet: it is a reference, not a claim in contention. */
  .vanilla {
    display: flex;
    align-items: baseline;
    gap: 0.35rem;
    padding: 0.3rem 0.45rem;
    border: 1px solid transparent;
    border-radius: var(--r-sm);
    background: none;
    color: var(--dim);
    font-size: var(--t-small);
    white-space: nowrap;
  }

  button.vanilla:hover {
    border-color: var(--rule);
    background: var(--film);
    color: var(--readout);
  }

  .vanilla .tag {
    color: var(--faint);
    font-size: var(--t-micro);
    letter-spacing: var(--track-label);
    text-transform: uppercase;
  }

  .vanilla.none {
    color: var(--ghost);
    font-size: var(--t-micro);
  }

  .gone {
    margin: 0.35rem 0 0.5rem;
    padding-left: 1rem;
    font-size: var(--t-micro);
  }

  .refused {
    margin-top: 0.75rem;
    padding-top: 0.5rem;
    border-top: 1px solid var(--rule);
  }

  .foot {
    flex: none;
    margin-top: 1rem;
    padding-top: 0.875rem;
    border-top: 1px solid var(--rule);
  }

  .acts {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
  }
</style>
