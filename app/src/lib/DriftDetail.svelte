<script lang="ts">
  import { names } from "./names.svelte";
  import { assetFolder, assetName, rebaked, type SceneDrift } from "./types";

  interface Props {
    drift: SceneDrift;
  }

  let { drift }: Props = $props();

  const moves = $derived(rebaked(drift));

  /** Three coordinates, fixed width, so before and after line up vertically. */
  function xyz(at: [number, number, number]): string {
    return at.map((n) => n.toFixed(3).padStart(9)).join("  ");
  }
</script>

<article>
  <h2>
    <span class="file">{assetName(drift.target)}</span>
    <span class="folder path">{assetFolder(drift.target)}</span>
  </h2>
  <p class="who" title={drift.mod}>{names.of(drift.mod)}</p>

  {#each moves as move (move.path)}
    <!-- The distance is the finding, so it leads and carries the signal
         colour: it is the number that says how far out of place the game
         will look for this. -->
    <div class="move">
      <div class="headline">
        <span class="distance">{move.distance.toFixed(3)}</span>
        <span class="unit">units{move.axis ? ` on ${move.axis}` : ""}</span>
        <span class="node">
          <span class="kind">{move.kind}</span>
          {move.path}
        </span>
      </div>

      <div class="coords">
        <span class="label">game</span>
        <span class="value">{xyz(move.before)}</span>
      </div>
      <div class="coords">
        <span class="label">mod</span>
        <span class="value shifted">{xyz(move.after)}</span>
      </div>

      <p class="why">Everything under it stayed where it was.</p>
    </div>
  {/each}

  <p class="context">
    {drift.added} node{drift.added === 1 ? "" : "s"} added,
    {drift.removed} removed, {drift.moved.length} moved in total.
  </p>
</article>

<style>
  article {
    padding: 0;
  }

  h2 {
    margin: 0;
    display: flex;
    align-items: baseline;
    gap: 0.625rem;
    font-size: var(--t-body);
    font-weight: 600;
  }

  .folder {
    font-size: var(--t-micro);
    color: var(--dim);
    font-weight: 400;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    min-width: 0;
  }

  .who {
    margin: 0.25rem 0 0.875rem;
    font-size: var(--t-small);
    color: var(--dim);
  }

  /* The left bar marks the thing that is wrong, matching how the claim stack
     marks the value that loads. */
  .move {
    border-left: 2px solid var(--contest);
    padding: 0.5rem 0 0.5rem 0.875rem;
    margin-bottom: 0.875rem;
  }

  .headline {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    flex-wrap: wrap;
  }

  .distance {
    font-family: var(--font-value);
    font-variant-numeric: tabular-nums;
    font-size: var(--t-lead);
    color: var(--signal);
  }

  .unit {
    font-size: var(--t-micro);
    color: var(--dim);
  }

  .node {
    margin-left: auto;
    font-size: var(--t-small);
    color: var(--readout);
  }

  .kind {
    font-size: var(--t-micro);
    color: var(--dim);
    margin-right: 0.375rem;
  }

  .coords {
    display: flex;
    align-items: baseline;
    gap: 0.75rem;
    margin-top: 0.25rem;
  }

  .label {
    font-size: var(--t-micro);
    color: var(--dim);
    min-width: 2.5rem;
  }

  .value {
    font-family: var(--font-value);
    font-variant-numeric: tabular-nums;
    font-size: var(--t-small);
    color: var(--dim);
    white-space: pre;
  }

  .value.shifted {
    color: var(--readout);
  }

  .why {
    margin: 0.5rem 0 0;
    font-size: var(--t-micro);
    color: var(--dim);
  }

  .context {
    margin: 0;
    font-size: var(--t-micro);
    color: var(--dim);
  }
</style>
