<script lang="ts">
  /** The order the game actually read a contested asset's copies in.
   *
   * Everywhere else in this app the load order is *predicted*: `ModPriority` is
   * read out of the game's own settings file and sorted. This is the one place
   * showing what the game did, taken from the hook's record of every mod file it
   * opened, in the order it opened them.
   *
   * ## Why this does not print a winner
   *
   * It would be a shorter screen and a wrong one. The measurement settles the
   * *order*; turning an order into a surviving value needs one more fact — does a
   * copy read later overwrite one read earlier, or does the first value written
   * stick? — and a hook watching file opens cannot see it. Both answers stay
   * live, so both are printed, with the one the analysis currently assumes marked.
   *
   * That is why the amber rule is obeyed strictly here. `--signal` means "this is
   * the value the game will load", and nothing on this screen is entitled to say
   * that yet. The two candidate ends of each sequence are therefore drawn as
   * ordinary text with their positions labelled, not as a winner and some losers.
   */
  import { names } from "./names.svelte";
  import type { Observation } from "./engine";

  interface Props {
    observation: Observation;
    /** open a mod's record from its name, as the rest of the app does */
    onmod?: (folder: string) => void;
  }

  let { observation, onmod }: Props = $props();

  const m = $derived(observation.measured);

  /** The badge the direction earns. Never `signal`: see the note above. */
  const tone = $derived(
    m.direction === "mixed"
      ? "badge-alert"
      : m.direction === "untestable"
        ? "badge-quiet"
        : "badge-info",
  );

  const word = $derived(
    m.direction === "ascending"
      ? "lowest priority first"
      : m.direction === "descending"
        ? "highest priority first"
        : m.direction === "mixed"
          ? "no consistent order"
          : "not measurable here",
  );

  /** Sequences worth drawing, longest first: more copies, more to see. */
  const sequences = $derived(
    [...observation.applied].sort((a, b) => b.order.length - a.order.length),
  );

  /** Predictions the observed order cannot produce, whichever loader model holds. */
  const doubtful = $derived(
    observation.checked.filter((c) => c.predicted !== null && !c.plausible),
  );

  function who(folder: string): string {
    return names.of(folder);
  }
</script>

<h4>Load order, as the game ran it</h4>

{#if !m.loads}
  <p class="hint">
    This session recorded no mod files being opened, so there is no order to read.
    That happens when the recorder was attached after the game had already
    started: every mod file is opened in the first second of a run.
  </p>
{:else}
  <p class="lede">
    <span class="badge {tone}">{word}</span>
    {m.testable} of {m.contested} contested asset(s) could be measured
  </p>

  <p class="hint">{observation.basis}</p>

  {#if m.readings.length}
    <!-- The two live conclusions, side by side. Printing only the one we assume
         would turn a measurement into a claim it does not support. -->
    <ul class="readings">
      {#each m.readings as reading}
        <li class:current={reading.is_current}>
          <span class="model">if {reading.model}</span>
          <span class="arrow">&rarr;</span>
          <span class="rule">
            {reading.rule === "last"
              ? "the highest ModPriority survives"
              : "the lowest ModPriority survives"}
          </span>
          {#if reading.is_current}
            <span class="badge badge-quiet">what this app assumes</span>
          {/if}
        </li>
      {/each}
    </ul>
  {/if}

  {#if m.exceptions.length}
    <h5>Assets that ran against the rest</h5>
    <p class="hint">
      These are the interesting ones. Every other asset agreed, so either
      something is different about these or the rule is not what it looks like.
    </p>
    <ul class="plainlist">
      {#each m.exceptions as row}
        <li>
          <span class="assetpath">{row.target}</span>
          <span class="badge badge-alert">{row.direction}</span>
          <span class="chain">
            {#each row.order as folder, i}
              {#if i}<span class="arrow">&rarr;</span>{/if}
              <span class="modname">{who(folder)}</span
              ><span class="prio">[{row.priorities[i]}]</span>
            {/each}
          </span>
        </li>
      {/each}
    </ul>
  {/if}

  {#if doubtful.length}
    <h5>Predictions the game's order cannot produce</h5>
    <p class="hint">
      The analysis named a winner that is neither the first nor the last copy the
      game read, so it cannot win under either reading. This is a bug in the
      prediction, not a problem with your mods.
    </p>
    <ul class="plainlist">
      {#each doubtful as row}
        <li>
          <span class="assetpath">{row.target}</span>
          predicted <span class="modname">{who(row.predicted ?? "")}</span>; the game
          read <span class="modname">{who(row.read_first ?? "")}</span> first and
          <span class="modname">{who(row.read_last ?? "")}</span> last
        </li>
      {/each}
    </ul>
  {/if}

  {#if sequences.length}
    <h5>What the game read, per asset</h5>
    <ul class="plainlist">
      {#each sequences as seq}
        <li>
          <span class="assetpath">{seq.target}</span>
          <ol class="chainlist">
            {#each seq.order as folder, i}
              <li>
                {#if onmod}
                  <button class="whobtn" onclick={() => onmod?.(folder)}>
                    {who(folder)}
                  </button>
                {:else}
                  <span class="modname">{who(folder)}</span>
                {/if}
                <span class="prio">
                  {seq.priorities[i] === null
                    ? "not registered"
                    : `priority ${seq.priorities[i]}`}
                </span>
                {#if i === 0}<span class="end">read first</span>{/if}
                {#if i === seq.order.length - 1}<span class="end">read last</span>{/if}
              </li>
            {/each}
          </ol>
        </li>
      {/each}
    </ul>
  {/if}
{/if}

<style>
  /* Local names throughout. A class spelled the same as one in components.css
     would have the global rule leak in underneath this component's, which is the
     trap recorded against the shared shapes. */
  h4 {
    margin: 1rem 0 0.375rem;
    font-size: var(--t-small);
    color: var(--readout);
  }

  h5 {
    margin: 1.1rem 0 0.25rem;
    font-size: 0.625rem;
    font-weight: 600;
    letter-spacing: var(--track-label);
    text-transform: uppercase;
    color: var(--cyan-dim);
  }

  .readings {
    list-style: none;
    margin: 0.6rem 0 0;
    padding: 0;
    display: grid;
    gap: 0.25rem;
  }

  .readings li {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0.5rem;
    padding: 0.45rem 0.6rem;
    border-left: 2px solid var(--rule);
    background: var(--inset);
    font-size: var(--t-small);
  }

  .readings li.current {
    border-left-color: var(--cyan-dim);
  }

  .model {
    color: var(--faint);
  }

  .rule {
    color: var(--readout);
  }

  .plainlist {
    list-style: none;
    margin: 0.4rem 0 0;
    padding: 0;
    display: grid;
    gap: 0.7rem;
    font-size: var(--t-small);
  }

  .plainlist > li {
    display: grid;
    gap: 0.25rem;
  }

  /* Monospace only where characters must line up: asset paths and priority
     numbers. Never for a label. */
  .assetpath,
  .prio {
    font-family: var(--font-value);
    font-size: var(--t-micro);
    color: var(--dim);
  }

  .assetpath {
    word-break: break-all;
  }

  .chain,
  .chainlist li {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0.4rem;
  }

  .chainlist {
    list-style: none;
    margin: 0.2rem 0 0;
    padding: 0 0 0 0.9rem;
    border-left: 1px solid var(--rule);
    display: grid;
    gap: 0.15rem;
  }

  .arrow {
    color: var(--faint);
  }

  .modname {
    color: var(--readout);
  }

  /* "read first" / "read last" name the two ends of the order. Deliberately not
     amber: that colour means "this is the value the game loads", and which end
     that is has not been established. */
  .end {
    font-size: 0.625rem;
    font-weight: 600;
    letter-spacing: var(--track-label);
    text-transform: uppercase;
    color: var(--cyan-dim);
  }

  .whobtn {
    background: none;
    border: 0;
    padding: 0;
    color: var(--readout);
    text-align: left;
    cursor: pointer;
    text-decoration: underline dotted var(--rule);
  }

  .whobtn:hover {
    text-decoration-color: currentColor;
  }
</style>
