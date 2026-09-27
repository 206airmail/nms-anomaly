<script lang="ts">
  /** The Optional band: many rows, one verb, done in batches.
   *
   * # Why this is a list and the bands above it are cards
   *
   * A card carries prose because the things above it are *findings* — usually
   * one or two, each needing a sentence to say what it costs to ignore. This
   * band is a different animal: on the measured library it is twenty-five rows
   * that all say the same thing in the same words about a different mod, and a
   * column of identical paragraphs is not more informative than a column of
   * names, it is less. So the sentence moves to the tooltip and the row keeps
   * what actually differs — which mod, who it steps on, and how much it is
   * carrying.
   *
   * # Selection has no mode here
   *
   * `LibraryPane` makes picking-several a mode, because there a click already
   * means "show me this one" and the two meanings would collide. Nothing on
   * this list has a detail view, so a click has no other job and selection is
   * simply always on. Shift-click takes a range, the same as there.
   *
   * Cleaning is reversible — the mod its author shipped is untouched in
   * staging — so there is no confirmation sheet. That matches the rule the
   * Library already follows: `Sheet` guards deleting, not deactivating.
   */
  import Button from "./Button.svelte";
  import type { Action } from "./actions";

  interface Props {
    actions: Action[];
    /** clean these, in one pass; resolves to a line to show, or throws */
    runMany: (owners: string[]) => Promise<string>;
    /** run a single row's own verb, for the ones that are not cleans */
    run: (action: Action) => Promise<string>;
  }

  let { actions, runMany, run }: Props = $props();

  // Cleans can be batched; an undo is the opposite operation and is not
  // something to do to twenty mods by sweeping a range over them.
  const cleanable = $derived(actions.filter((a) => a.verb === "clean"));
  const others = $derived(actions.filter((a) => a.verb !== "clean"));

  let chosen = $state<Set<string>>(new Set());
  let anchor = $state<string | null>(null);
  let busy = $state(false);
  let result = $state<string | null>(null);
  let failed = $state<string | null>(null);

  // Rows can disappear under a selection — a cleaned mod leaves this list and
  // comes back as an undo — so the set is filtered rather than trusted.
  const picked = $derived(cleanable.filter((a) => chosen.has(a.subject ?? a.id)));
  const allPicked = $derived(cleanable.length > 0 && picked.length === cleanable.length);

  /** The saving, added up, so the batch button says what it buys. */
  const saving = $derived.by(() => {
    let carried = 0;
    for (const a of picked) {
      const [from, to] = (a.metric ?? "").match(/[\d,]+/g)?.slice(0, 2) ?? [];
      if (from && to) carried += Number(from.replace(/,/g, "")) - Number(to.replace(/,/g, ""));
    }
    return carried;
  });

  function key(action: Action): string {
    return action.subject ?? action.id;
  }

  function press(action: Action, event: MouseEvent) {
    const id = key(action);
    if (event.shiftKey && anchor) {
      const from = cleanable.findIndex((a) => key(a) === anchor);
      const to = cleanable.findIndex((a) => key(a) === id);
      if (from !== -1 && to !== -1) {
        const [lo, hi] = from < to ? [from, to] : [to, from];
        chosen = new Set([...chosen, ...cleanable.slice(lo, hi + 1).map(key)]);
        return;
      }
    }
    const next = new Set(chosen);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    chosen = next;
    anchor = id;
  }

  function toggleAll() {
    chosen = allPicked ? new Set() : new Set(cleanable.map(key));
  }

  async function clean() {
    if (!picked.length) return;
    busy = true;
    failed = null;
    result = null;
    try {
      result = await runMany(picked.map((a) => a.subject!));
      chosen = new Set();
      anchor = null;
    } catch (err) {
      failed = String(err);
    } finally {
      busy = false;
    }
  }

  /** Who this one is stepping on, short enough for one line. */
  function stepping(action: Action): string {
    const victims = action.mods.slice(1);
    if (!victims.length) return "";
    return victims.length <= 2
      ? `overriding ${victims.join(", ")}`
      : `overriding ${victims.length} other mods`;
  }
</script>

<div class="tidy" class:picking={picked.length > 0}>
  {#if cleanable.length}
    <div class="head">
      <span class="hint">
        {picked.length ? `${picked.length} of ${cleanable.length} selected` : `${cleanable.length} mods`}
      </span>
      <button class="btn-link" onclick={toggleAll}>{allPicked ? "Clear" : "Select all"}</button>
    </div>

    <ol>
      {#each cleanable as action (action.id)}
        {@const id = key(action)}
        {@const note = stepping(action)}
        <li>
          <button
            class="row"
            class:ticked={chosen.has(id)}
            aria-pressed={chosen.has(id)}
            title={action.why}
            onclick={(event) => press(action, event)}
          >
            <span class="dot" aria-hidden="true"></span>
            <span class="who">{action.title.replace(/^Clean /, "")}</span>
            {#if note}<span class="note">{note}</span>{/if}
            <span class="metric path">{action.metric ?? ""}</span>
          </button>
        </li>
      {/each}
    </ol>
  {/if}

  {#each others as action (action.id)}
    <!-- Already cleaned: the opposite operation, so it sits outside the
         selectable list rather than inside it wearing a different colour. -->
    <div class="done">
      <span class="dot lit" aria-hidden="true"></span>
      <span class="who">{action.title}</span>
      <span class="why">{action.why}</span>
      <Button variant="ghost" onclick={() => run(action)}>Put the original back</Button>
    </div>
  {/each}

  {#if result}<p class="result">{result}</p>{/if}
  {#if failed}<p class="failed">{failed}</p>{/if}

  {#if picked.length}
    <!-- Sticky, so a selection made at the top of twenty-five rows is still
         actionable at the bottom of them. -->
    <div class="batch">
      <span class="hint">
        {picked.length === 1 ? "1 mod" : `${picked.length} mods`}
        {#if saving > 0}
          &middot; drops {saving.toLocaleString()} properties they do not change
        {/if}
      </span>
      <Button variant="primary" onclick={clean} busy={busy}>
        {busy ? "Cleaning…" : `Clean ${picked.length === 1 ? "it" : "them"}`}
      </Button>
    </div>
  {/if}
</div>

<style>
  .tidy {
    position: relative;
  }

  /* The batch bar is pinned to the bottom of the scrollport, so without room to
     scroll past it, it sits on top of the last row of the list. Only while
     something is selected -- the bar does not exist otherwise, and permanent
     dead space under the list would be paying for it all the time. */
  .tidy.picking {
    padding-bottom: 3.25rem;
  }

  .head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 0.5rem;
    padding: 0 0.25rem 0.375rem;
  }

  ol {
    list-style: none;
    margin: 0;
    padding: 0;
    border: 1px solid var(--rule);
    border-radius: var(--r-lg);
    background: var(--card);
    box-shadow: var(--lip);
    overflow: hidden;
  }

  li + li .row {
    border-top: 1px solid var(--rule);
  }

  /* One line per mod: the dot for the band, the name, who it steps on, and the
     number that says how much it is carrying. Everything else is the tooltip.

     The name column is fixed rather than elastic so that every row's note
     starts at the same place. Letting the name take the slack pushed each note
     hard against the metric at the far right of a wide window, a screen's width
     away from the mod it was describing. */
  .row {
    display: grid;
    grid-template-columns: auto minmax(0, 20rem) minmax(0, 1fr) auto;
    align-items: baseline;
    gap: 0.625rem;
    width: 100%;
    padding: 0.4375rem 0.875rem;
    text-align: left;
    font-size: var(--t-small);
    color: var(--dim);
    transition:
      background var(--quick) var(--ease),
      color var(--quick) var(--ease);
  }

  .row:hover {
    background: var(--glass-2);
    color: var(--readout);
  }

  /* Picked. A filled bar down the edge, the same shape the Library uses, so
     "in the set" reads the same way in both places. */
  .row.ticked {
    background: var(--accent-wash);
    color: var(--readout);
    box-shadow: inset 2px 0 0 0 var(--accent);
  }

  /* The band's colour, once per row instead of once per card. The word for it
     is on the heading above, which is where severity is stated.

     Filled, not outlined. The Library's lamp is hollow because "off" is a real
     state there; nothing on this list is off, so a ring just read as a speck of
     dust at this size. */
  .dot {
    align-self: center;
    width: 0.4375rem;
    height: 0.4375rem;
    border-radius: 50%;
    background: var(--cyan-dim);
  }

  .dot.lit {
    border-color: var(--accent);
    background: var(--accent);
    box-shadow: 0 0 6px rgba(138, 224, 60, 0.6);
  }

  .who {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .row .note {
    font-size: var(--t-micro);
    color: var(--faint);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  /* Right-aligned and tabular, so the before-and-after numbers form a column
     you can read down rather than a ragged edge.

     The explicit column is load-bearing. A row with nothing to say about who it
     overrides renders no note element at all, so auto-placement put its metric
     in the *third* track instead of the fourth -- one gap-width left of every
     row that did have a note, which is exactly the kind of misalignment a
     column of numbers exists to avoid. */
  .metric {
    grid-column: 4;
    justify-self: end;
    font-size: var(--t-micro);
    font-variant-numeric: tabular-nums;
    color: var(--faint);
    white-space: nowrap;
  }

  .row.ticked .metric,
  .row.ticked .note {
    color: var(--dim);
  }

  /* An already-cleaned mod. Not a row in the list: it is the reverse of what
     the list does, and sweeping a range over it should be impossible. */
  .done {
    display: flex;
    align-items: center;
    gap: 0.625rem;
    margin-top: 0.625rem;
    padding: 0.5rem 0.875rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-lg);
    background: var(--card);
    box-shadow: var(--lip);
    font-size: var(--t-small);
  }

  .done .who {
    color: var(--readout);
    flex: none;
  }

  .done .why {
    flex: 1;
    min-width: 0;
    font-size: var(--t-micro);
    color: var(--faint);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .batch {
    position: sticky;
    bottom: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.875rem;
    margin-top: 0.625rem;
    padding: 0.5rem 0.875rem;
    border: 1px solid var(--rule-hi);
    border-radius: var(--r-lg);
    background: linear-gradient(180deg, rgba(8, 16, 15, 0.9), var(--hull));
    backdrop-filter: blur(12px);
    box-shadow: var(--cast-near), var(--lip);
  }

  .result {
    margin: 0.625rem 0 0;
    padding: 0.4375rem 0.625rem;
    border: 1px solid rgba(138, 224, 60, 0.24);
    border-radius: var(--r-sm);
    background: var(--accent-wash);
    font-size: var(--t-micro);
    color: var(--readout);
  }
</style>
