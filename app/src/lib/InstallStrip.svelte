<script lang="ts">
  import { describe, fraction, installs } from "./installs.svelte";

  // Shown above the status bar rather than inside a tab, because a download
  // can be started from the browser window and has to stay visible wherever
  // the user goes next.
  const jobs = $derived(installs.jobs);
  const busy = $derived(installs.running.length > 0);
</script>

{#if jobs.length}
  <section class="strip" aria-live="polite">
    <header>
      <span class="hud-label">{busy ? "Installing" : "Recent installs"}</span>
      {#if !busy}
        <button onclick={() => installs.clear()}>Clear</button>
      {/if}
    </header>

    {#each jobs as job (job.id)}
      {@const done = fraction(job)}
      <article class:failed={job.outcome === "failed"} class:ok={job.outcome === "installed"}>
        <div class="line">
          <span class="name">{job.label}</span>
          <span class="state">{describe(job)}</span>
        </div>
        {#if job.outcome === null}
          <div class="bar" class:unknown={done === null}>
            <div class="fill" style={done === null ? "" : `width: ${done * 100}%`}></div>
          </div>
        {/if}
        {#each job.notes as note}
          <p class="note">{note}</p>
        {/each}
      </article>
    {/each}
  </section>
{/if}

<style>
  .strip {
    padding: 0.625rem var(--pad);
    border-top: 1px solid var(--rule);
    background: var(--hull);
    max-height: 13rem;
    overflow-y: auto;
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 0.375rem;
  }

  header button {
    font-size: var(--t-micro);
    color: var(--faint);
    padding: 0.125rem 0.375rem;
    border-radius: var(--r-sm);
  }

  header button:hover {
    color: var(--cyan);
  }

  article {
    padding: 0.4375rem 0;
  }

  .line {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 1rem;
    font-size: var(--t-small);
  }

  .name {
    color: var(--readout);
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .state {
    flex: none;
    font-size: var(--t-micro);
    color: var(--dim);
    font-family: var(--font-value);
  }

  .ok .state {
    color: var(--accent);
  }

  .failed .state {
    color: var(--contest);
    font-family: var(--font-ui);
    white-space: normal;
    text-align: right;
    max-width: 44ch;
  }

  .bar {
    height: 3px;
    margin-top: 0.375rem;
    border-radius: var(--r-pill);
    background: var(--inset);
    overflow: hidden;
  }

  .fill {
    height: 100%;
    width: 0;
    border-radius: var(--r-pill);
    background: var(--accent);
    box-shadow: var(--accent-glow);
    transition: width 200ms ease-out;
  }

  /* No content-length from the server: show motion rather than a lie. */
  .bar.unknown .fill {
    width: 35%;
    animation: sweep 1.1s ease-in-out infinite;
  }

  @keyframes sweep {
    0% {
      transform: translateX(-100%);
    }
    100% {
      transform: translateX(300%);
    }
  }

  .note {
    margin: 0.25rem 0 0;
    font-size: var(--t-micro);
    color: var(--signal);
    max-width: 62ch;
  }
</style>
