<script lang="ts">
  import { names } from "./names.svelte";
  import { byLoadOrder, type Mod } from "./types";

  interface Props {
    mods: Mod[];
    /** mods involved in whatever is selected, highlighted in place */
    involved?: string[];
    /** the one the game will actually apply, of those involved */
    winner?: string | null;
    realOrder: boolean;
  }

  let { mods, involved = [], winner = null, realOrder }: Props = $props();

  const ordered = $derived(byLoadOrder(mods));
</script>

<!--
  This is not navigation. It is the load order itself: the thing that decides
  every winner, kept on screen so a verdict is never unexplained. When a
  conflict is open, its mods light up here and you can see the distance
  between them.
-->
<nav class="rail" aria-label="Mods in load order">
  <header>
    <span>Load order</span>
    <span class="count">{ordered.length}</span>
  </header>

  {#if !realOrder}
    <p class="assumed">
      Assumed alphabetical. Launch the game once with mods and it writes the
      real order.
    </p>
  {/if}

  <ol>
    {#each ordered as mod (mod.name)}
      {@const active = involved.includes(mod.name)}
      <li
        class:active
        class:wins={active && mod.name === winner}
        class:disabled={mod.disabled}
      >
        <span class="pos value">{mod.priority ?? "–"}</span>
        <!-- The title is the folder: this rail is the one place the identifier
             the game actually uses is worth having a hover away. -->
        <span class="name" title={mod.name}>{names.of(mod.name)}</span>
        {#if mod.priority === null}
          <span class="tag" title="The game has not registered this folder yet"
            >new</span
          >
        {/if}
      </li>
    {/each}
  </ol>
</nav>

<style>
  .rail {
    width: var(--rail-w);
    flex: none;
    border-right: 1px solid var(--rule);
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  header {
    display: flex;
    justify-content: space-between;
    align-items: baseline;
    padding: var(--pad) var(--pad) 0.75rem;
    font-size: var(--t-small);
    color: var(--dim);
  }

  .count {
    font-family: var(--font-value);
  }

  .assumed {
    margin: 0 var(--pad) 0.75rem;
    padding-left: 0.625rem;
    border-left: 2px solid var(--rule);
    font-size: var(--t-micro);
    color: var(--dim);
    line-height: 1.45;
  }

  ol {
    margin: 0;
    padding: 0 0 var(--pad);
    list-style: none;
    overflow-y: auto;
    flex: 1;
  }

  li {
    display: flex;
    align-items: baseline;
    gap: 0.625rem;
    padding: 0.1875rem var(--pad);
    border-left: 2px solid transparent;
    color: var(--dim);
    transition: color 160ms ease-out, border-color 160ms ease-out;
  }

  /* Involved in the open conflict: pulled forward, still not shouting. */
  li.active {
    color: var(--readout);
    border-left-color: var(--rule);
    background: var(--hull);
  }

  /* The one that actually loads. The only amber in the rail. */
  li.wins {
    border-left-color: var(--signal);
  }

  li.disabled .name {
    text-decoration: line-through;
    opacity: 0.55;
  }

  .pos {
    min-width: 1.75rem;
    text-align: right;
    font-size: var(--t-micro);
    opacity: 0.7;
  }

  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: var(--t-small);
  }

  .tag {
    font-size: var(--t-micro);
    color: var(--void);
    background: var(--dim);
    padding: 0 0.3125rem;
    border-radius: 2px;
  }

  @media (max-width: 720px) {
    .rail {
      display: none;
    }
  }
</style>
