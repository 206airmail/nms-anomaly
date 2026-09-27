<script lang="ts">
  /** The evidence behind a verdict, over whatever you were reading.
   *
   * ---------------------------------------------------------------------------
   * Why this is not a tab any more
   * ---------------------------------------------------------------------------
   *
   * Evidence is the only screen in the app that is not about a mod. Its unit is
   * a contested *asset* — one game file that several mods claim — so it could
   * never be folded into the mod list the way Updates and Presets were.
   *
   * But it was never a destination either. Nobody opens it to browse; they
   * arrive at it from a card on the Actions list, or from a mod's record,
   * asking "why do you say that". It is the footnote to a claim, and a footnote
   * that costs you your place on the page is a bad footnote: reading the
   * evidence for one of five findings meant leaving the list of five, and
   * coming back to it scrolled to the top with the selection gone.
   *
   * So it opens over the screen that sent you, and closing it puts you back
   * exactly where you were.
   *
   * ---------------------------------------------------------------------------
   * The rail
   * ---------------------------------------------------------------------------
   *
   * The load order is not navigation and never was. It is the thing that
   * decides every winner, and it is on screen so that a verdict is never
   * unexplained — when a conflict is open its mods light up in it and you can
   * see the distance between them. That only works next to the claim stack, so
   * it comes along rather than staying behind on a tab of its own.
   */
  import ConflictDetail from "./ConflictDetail.svelte";
  import DriftDetail from "./DriftDetail.svelte";
  import LoadOrderRail from "./LoadOrderRail.svelte";
  import Sheet from "./Sheet.svelte";
  import Button from "./Button.svelte";
  import type { CleanPlan } from "./engine";
  import { assetName, type Conflict, type Mod, type SceneDrift } from "./types";

  interface Props {
    conflicts: Conflict[];
    drift: SceneDrift[];
    /** every mod, for the load order rail */
    mods: Mod[];
    realOrder: boolean;
    plans: CleanPlan[];
    /** the one to open on, when the caller had a particular one in mind */
    focus: Conflict | null;
    onclose: () => void;
    onMerged: () => void;
  }

  let { conflicts, drift, mods, realOrder, plans, focus, onclose, onMerged }: Props =
    $props();

  /**
   * Which conflict the rail is lighting up.
   *
   * Whatever sent us here until the reader scrolls somewhere else, then the one
   * they are reading, and the first on the list if neither is still contested.
   *
   * `reading` starts empty rather than at `focus`, so that this is derived from
   * the prop rather than copied out of it once: a rescan that settles the
   * conflict we were opened on rebuilds the list, and a copy taken at mount
   * would go on pointing the rail at an asset nobody is arguing over.
   */
  let reading = $state<Conflict | null>(null);
  const selected = $derived.by(() => {
    const want = reading ?? focus;
    return want && conflicts.includes(want) ? want : (conflicts[0] ?? null);
  });
  const involved = $derived(selected?.mods ?? []);

  /**
   * Bring the one we were sent for into view.
   *
   * The column holds every contested asset, and arriving at the top of it to
   * hunt for the one named on the card you just pressed is the thing this
   * sheet exists to avoid.
   */
  let column = $state<HTMLDivElement | null>(null);
  $effect(() => {
    if (!focus || !column) return;
    const target = column.querySelector(`[data-target="${CSS.escape(focus.target)}"]`);
    target?.scrollIntoView({ block: "start" });
  });

  /**
   * The rail follows what is being read, rather than what was clicked.
   *
   * A click on the panel would have been the obvious way to point the rail at
   * it, and it is the wrong one twice over: a panel is not a control and
   * should not behave like one, and the panels carry real buttons, so the two
   * most useful things on one would become the two places you must not press.
   *
   * Reading position says the same thing without asking for anything. The
   * conflict nearest the top of the column is the one being read.
   */
  function follow() {
    if (!column) return;
    const top = column.getBoundingClientRect().top;
    let best: Conflict | null = null;
    let nearest = Infinity;
    for (const conflict of conflicts) {
      const panel = column.querySelector(`[data-target="${CSS.escape(conflict.target)}"]`);
      if (!panel) continue;
      // Distance from the top edge, counting a panel scrolled just past it as
      // further away than one just below it -- otherwise the rail jumps to the
      // next asset while most of the current one is still on screen.
      const offset = panel.getBoundingClientRect().top - top;
      const distance = offset < 0 ? -offset * 2 : offset;
      if (distance < nearest) {
        nearest = distance;
        best = conflict;
      }
    }
    if (best) reading = best;
  }
</script>

<Sheet
  wide
  title={conflicts.length || drift.length
    ? `Evidence — ${conflicts.length + drift.length} contested`
    : "Evidence"}
  {onclose}
>
  <div class="split">
    <LoadOrderRail
      {mods}
      {involved}
      winner={selected?.predicted_winner ?? null}
      {realOrder}
    />

    <div class="column" bind:this={column} onscroll={follow}>
      {#if !conflicts.length && !drift.length}
        <section class="panel">
          <header class="hud-label">Evidence</header>
          <h2>Nothing contested.</h2>
          <p class="lede">
            No two mods claim the same property, and no mod has moved a scene
            anchor it did not mean to.
          </p>
        </section>
      {/if}

      {#each drift as entry (entry.mod + entry.target)}
        <section class="panel" data-target={entry.target}>
          <DriftDetail drift={entry} />
        </section>
      {/each}

      {#each conflicts as conflict (conflict.target)}
        <section class="panel" class:lit={selected === conflict} data-target={conflict.target}>
          <ConflictDetail {conflict} {plans} {realOrder} {onMerged} />
        </section>
      {/each}
    </div>
  </div>

  {#snippet verbs()}
    <span class="hint">
      {#if selected}
        Showing {assetName(selected.target)} in the load order.
      {/if}
    </span>
    <Button onclick={onclose}>Close</Button>
  {/snippet}
</Sheet>

<style>
  .split {
    display: grid;
    grid-template-columns: 15rem minmax(0, 1fr);
    height: 100%;
    min-height: 0;
  }

  .column {
    padding: 0 0.25rem 0 1rem;
    overflow-y: auto;
  }

  /* The one the rail is lit for, which is the one being read. A quiet edge
     rather than a colour change: the panel's own content is the evidence, and
     the rail beside it is where "which one is this" already lives. */
  .panel.lit {
    border-color: var(--rule-hi);
    box-shadow: var(--cast-near), var(--lip);
  }

  h2 {
    margin: 0.25rem 0 0;
    font-size: var(--t-title);
    font-weight: 600;
  }

  /* Pushed left so the Close button stays on the right where every other
     sheet keeps it. */
  .hint {
    margin-right: auto;
  }
</style>
