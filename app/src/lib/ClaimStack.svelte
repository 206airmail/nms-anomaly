<script lang="ts">
  import { names } from "./names.svelte";
  import { orderedClaims, type Clash } from "./types";

  interface Props {
    clash: Clash;
    winner: string | null;
    /** stagger index, so a long stack arrives in order rather than all at once */
    index?: number;
  }

  let { clash, winner, index = 0 }: Props = $props();

  const claims = $derived(orderedClaims(clash, winner));
  const bothEdited = $derived(clash.attributed.length >= 2);
</script>

<!--
  A conflict here is not two sides of a diff. Several mods each *claim* a value
  for one property and exactly one claim survives, decided by load order. So
  the claims stack, the losers are struck through, and the survivor carries the
  amber bar - the only thing on screen that means "this is what the game runs".
-->
<div class="clash" style="--stagger: {Math.min(index, 8) * 30}ms">
  <p class="path" title={clash.path}>{clash.path}</p>

  <ul>
    {#each claims as claim (claim.mod)}
      <li class:wins={claim.wins}>
        <span class="mod" title={claim.mod}>{names.of(claim.mod)}</span>
        <span class="value" class:absent={claim.value === null}>
          {claim.value ?? "not set"}
        </span>
        <span class="verdict">
          {#if claim.wins}
            <span class="mark" aria-hidden="true">◆</span> loads
          {:else}
            <span class="mark" aria-hidden="true">✕</span> overridden
          {/if}
        </span>
      </li>
    {/each}
  </ul>

  {#if bothEdited}
    <p class="note">
      Both mods deliberately changed this, so neither value is an accident of an
      old baseline.
    </p>
  {/if}
</div>

<style>
  .clash {
    padding: 0.875rem 0;
    border-top: 1px solid var(--rule);
    animation: arrive 220ms ease-out backwards;
    animation-delay: var(--stagger);
  }

  /* The one orchestrated motion in the app: claims arriving when a conflict is
     opened. Nothing else moves on its own. */
  @keyframes arrive {
    from {
      opacity: 0;
      transform: translateY(3px);
    }
  }

  .path {
    margin: 0 0 0.5rem;
    font-size: var(--t-micro);
    color: var(--dim);
    word-break: break-all;
  }

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li {
    display: grid;
    grid-template-columns: minmax(6rem, 14rem) 1fr auto;
    gap: 1rem;
    align-items: baseline;
    padding: 0.25rem 0 0.25rem 0.75rem;
    border-left: 2px solid transparent;
    color: var(--dim);
  }

  li.wins {
    border-left-color: var(--signal);
    color: var(--readout);
  }

  .mod {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .value {
    font-size: var(--t-small);
    text-decoration: line-through;
    text-decoration-color: var(--dim);
    opacity: 0.75;
  }

  li.wins .value {
    text-decoration: none;
    opacity: 1;
    color: var(--signal);
    font-weight: 500;
  }

  .value.absent {
    font-style: italic;
    text-decoration: none;
  }

  /* The shape carries the meaning too, so colour is never the only signal. */
  .verdict {
    font-size: var(--t-micro);
    white-space: nowrap;
  }

  .mark {
    font-size: 0.6875rem;
  }

  li.wins .verdict {
    color: var(--signal);
  }

  .note {
    margin: 0.5rem 0 0 0.75rem;
    font-size: var(--t-micro);
    color: var(--dim);
  }
</style>
