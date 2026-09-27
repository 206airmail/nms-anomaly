<script lang="ts">
  import Button from "./Button.svelte";
  import { bandOf, type Action } from "./actions";

  interface Props {
    action: Action;
    /** runs the action's verb; resolves to a line to show, or throws */
    run: (action: Action) => Promise<string>;
    /** open the evidence for this action */
    inspect?: (action: Action) => void;
    /**
     * Present when this card is one of several the same button can do at once.
     *
     * A tick rather than a click on the card itself: this card carries two
     * buttons of its own, and a card that also means "pick me" when clicked
     * anywhere is a card where the two most useful things on it are the two
     * places you must not click.
     */
    onpick?: (event: MouseEvent) => void;
    picked?: boolean;
  }

  let { action, run, inspect, onpick, picked = false }: Props = $props();

  let busy = $state(false);
  let result = $state<string | null>(null);
  let failed = $state<string | null>(null);

  const LABEL: Record<string, [string, string]> = {
    combine: ["Combine them", "Combining…"],
    clean: ["Clean it", "Cleaning…"],
    undo: ["Put the original back", "Restoring…"],
    repair: ["Fix it", "Fixing…"],
  };

  /** The uppercase tag on the card, so severity is readable, not just coloured.
   *  Each says what is *wrong*, never what to do -- the heading does that. */
  const TAG: Record<string, string> = {
    broken: "Not running",
    wrong: "Wrong in game",
    unblock: "Overriding other mods",
    merge: "Only one of them loads",
    decide: "Needs your decision",
    tidy: "Improvement",
  };

  // Stroke icons, inline so nothing is fetched and nothing can fail to load.
  const PATHS: Record<string, string> = {
    // a warning plate: this one is not running at all
    broken: "M12 8v5M12 16h.01M5.6 4h12.8L21 8.6v6.8L18.4 20H5.6L3 15.4V8.6z",
    // a triangle: the game is doing something visibly wrong
    wrong: "M12 3l9 16H3zM12 10v4M12 17h.01",
    // a wide bar covering narrower ones: one mod sitting on top of others
    unblock: "M3 6h18M6 12h12M9 18h6",
    // two strands becoming one
    merge: "M6 3v5a4 4 0 004 4h4a4 4 0 014 4v5M18 3v5a4 4 0 01-4 4",
    // a fork: it goes one way or the other, and you choose
    decide: "M12 21V11M12 11L6 5M12 11l6-6M6 3v3H3",
    // a tick: nothing is wrong
    tidy: "M5 13l4 4L19 7",
  };

  /** What the icon tile becomes while this card is picked for a batch. */
  const TICK = "M5 13l4 4L19 7";

  // Three steps, not six: the order of the list carries the ranking, and the
  // colour only has to say which of three kinds of thing this is. The *word*
  // for that step is on the band heading above the card, so the card itself
  // says the specific fault and never repeats the severity.
  const band = $derived(bandOf(action.urgency));

  async function press() {
    busy = true;
    failed = null;
    try {
      result = await run(action);
    } catch (err) {
      failed = String(err);
    } finally {
      busy = false;
    }
  }
</script>

{#snippet glyph(path: string)}
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75"
    stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
    <path d={path} />
  </svg>
{/snippet}

<article class="action {band}" class:done={result} class:picked={picked && onpick}>
  {#if onpick}
    <!-- The icon tile doubles as the control, so a band that can be batched is
         exactly the same shape as one that cannot: nothing is added to the
         card's grid, and there is no second thing on the left to look past. -->
    <button
      class="chip pick"
      class:on={picked}
      onclick={onpick}
      aria-pressed={picked}
      aria-label="Pick {action.title} to do with the others"
      title="Pick this one"
    >
      {@render glyph(picked ? TICK : PATHS[action.urgency])}
    </button>
  {:else}
    <div class="chip" aria-hidden="true">{@render glyph(PATHS[action.urgency])}</div>
  {/if}

  <div class="body">
    <span class="tag hud-label">{TAG[action.urgency]}</span>
    <h3>{action.title}</h3>
    <p class="why">{action.why}</p>

    {#if action.metric}
      <p class="metric path">{action.metric}</p>
    {/if}

    {#if action.bonus}
      <p class="bonus">{action.bonus}</p>
    {/if}

    {#if result}
      <p class="result">{result}</p>
    {/if}
    {#if failed}
      <p class="failed">{failed}</p>
    {/if}
  </div>

  <div class="controls">
    {#if action.verb && !result}
      <!-- Putting originals back is not the thing to do, it is the way out of
           having done it, so it does not wear the action colour. -->
      <Button
        variant={action.verb === "undo" ? "ghost" : "primary"}
        onclick={press}
        busy={busy}
      >
        {busy ? LABEL[action.verb][1] : LABEL[action.verb][0]}
      </Button>
    {/if}
    {#if inspect && action.target}
      <Button onclick={() => inspect(action)}>Show me</Button>
    {/if}
  </div>
</article>

<style>
  .action {
    display: grid;
    grid-template-columns: auto 1fr auto;
    align-items: start;
    gap: 0 1rem;
    margin-bottom: 0.625rem;
    padding: 1rem 1.125rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-lg);
    background: var(--card);
    box-shadow: var(--lip);
    transition:
      background 140ms ease,
      border-color 140ms ease,
      box-shadow 140ms ease;
  }

  .action:hover {
    background: var(--card-hi);
    border-color: var(--rule-hi);
    box-shadow: var(--lip), var(--shadow-md);
  }

  .action.done {
    opacity: 0.66;
  }

  .chip {
    display: grid;
    place-items: center;
    width: 2.25rem;
    height: 2.25rem;
    border-radius: var(--r-md);
    color: var(--cyan);
    background: var(--cyan-wash);
    border: 1px solid var(--rule);
  }

  .chip svg {
    width: 1.0625rem;
    height: 1.0625rem;
  }

  /* Three bands, and the same colour on the card's edge, its icon and its
     tag, so one glance down the left-hand side reads as a ramp.

     Green is deliberately *not* one of them. In this app the action colour
     means "this is the button", and a card that was green all over would argue
     with its own button about where to look. */
  .critical .chip {
    color: var(--contest);
    background: var(--contest-wash);
    border-color: rgba(255, 95, 86, 0.26);
  }
  .recommended .chip {
    color: var(--signal);
    background: var(--signal-wash);
    border-color: rgba(245, 166, 35, 0.26);
  }
  .optional .chip {
    color: var(--cyan);
    background: var(--cyan-wash);
    border-color: var(--rule);
  }

  /* A lit edge, which is what makes the ranking visible without reading a word
     of it: a column of red, then amber, then nothing. */
  .action.critical {
    border-left-color: var(--contest);
    box-shadow: var(--lip), inset 2px 0 0 -1px var(--contest);
  }
  .action.recommended {
    border-left-color: var(--signal);
    box-shadow: var(--lip), inset 2px 0 0 -1px var(--signal);
  }

  /* ---- picked for a batch ---------------------------------------------- */

  /* The tile is the control, so it is the one thing that has to look pressable
     before anything is picked. */
  .chip.pick {
    padding: 0;
    cursor: pointer;
  }

  .chip.pick:hover {
    border-color: var(--accent);
    color: var(--accent);
  }

  .chip.pick.on,
  .critical .chip.pick.on,
  .recommended .chip.pick.on {
    color: var(--accent-ink);
    background: var(--accent);
    border-color: var(--accent);
  }

  /* Whole-card, not tile-only. A tick you have to hunt for is not an answer to
     "which of these did I choose"; the Optional list already lights a chosen
     row this way, and this is the same choice in a bigger shape.

     After the band rules on purpose: same specificity, so source order decides,
     and while a card is picked that is what its edge should say. */
  .action.picked {
    background: var(--accent-wash);
    border-color: rgba(138, 224, 60, 0.32);
    border-left-color: var(--accent);
    box-shadow: var(--lip), inset 2px 0 0 -1px var(--accent);
  }

  .action.picked:hover {
    background: var(--accent-wash);
    border-color: rgba(138, 224, 60, 0.45);
    box-shadow: var(--lip), inset 2px 0 0 -1px var(--accent), var(--shadow-md);
  }

  .tag {
    display: block;
    margin-bottom: 0.125rem;
  }
  .critical .tag {
    color: var(--contest);
  }
  .recommended .tag {
    color: var(--signal);
  }
  .optional .tag {
    color: var(--cyan-dim);
  }

  h3 {
    margin: 0;
    font-size: var(--t-lead);
    font-weight: 600;
    letter-spacing: -0.01em;
    line-height: 1.35;
  }

  .why {
    margin: 0.25rem 0 0;
    font-size: var(--t-small);
    color: var(--dim);
    max-width: 62ch;
  }

  /* A well, not a line of text: it is data, and it should look recessed. */
  .metric {
    margin: 0.625rem 0 0;
    padding: 0.375rem 0.5625rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-sm);
    background: var(--inset);
    font-size: var(--t-micro);
    color: var(--dim);
    word-break: break-all;
  }

  .bonus {
    margin: 0.5rem 0 0;
    font-size: var(--t-micro);
    color: var(--cyan);
    max-width: 62ch;
  }

  /* An outcome and a failure are the same shape in the same place, because
     they answer the same question: what happened when I pressed that. Only the
     colour differs. This deliberately keeps its own box rather than taking the
     shared `.failed`, which is sized for a paragraph rather than for the foot
     of a card. */
  .result,
  .failed {
    margin: 0.625rem 0 0;
    padding: 0.4375rem 0.625rem;
    border-radius: var(--r-sm);
    font-size: var(--t-micro);
    color: var(--readout);
    max-width: 62ch;
  }

  .result {
    background: var(--accent-wash);
    border: 1px solid rgba(138, 224, 60, 0.24);
  }

  .failed {
    background: var(--contest-wash);
    border: 1px solid rgba(255, 95, 86, 0.26);
  }

  .controls {
    display: flex;
    flex-direction: column;
    align-items: stretch;
    gap: 0.375rem;
    padding-top: 0.9375rem;
  }

  /* The controls stack rather than sitting side by side: this card is narrow,
     and "Show me" under the verb reads as the quieter of the two. Which of
     them is the recommendation is said by the button's own variant -- undo is
     a ghost, because putting originals back is not the thing to do, it is the
     way out of having done it. */
</style>
