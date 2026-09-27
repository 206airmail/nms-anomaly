<script lang="ts">
  import { onMount } from "svelte";
  import { browser, tidyUrl } from "./browser.svelte";

  interface Props {
    /** false while the user is on another tab, so the webview is parked */
    visible: boolean;
  }

  let { visible }: Props = $props();

  /**
   * The box the native webview should fill.
   *
   * This element draws nothing. It exists to be measured: the webview is a
   * separate surface stacked over the window and knows nothing about our
   * layout, so the only way it lines up is for us to keep telling it where
   * this element is.
   */
  let slot = $state<HTMLDivElement | null>(null);

  function measure() {
    if (!slot || !visible) return;
    const box = slot.getBoundingClientRect();
    if (box.width < 2 || box.height < 2) return;
    void browser.place({
      x: box.left,
      y: box.top,
      width: box.width,
      height: box.height,
    });
  }

  onMount(() => {
    // Anything can move this box: the window resizing, the sidebar, a scroll
    // bar appearing. Watching the element itself catches all of them without
    // having to know which.
    const observer = new ResizeObserver(() => measure());
    if (slot) observer.observe(slot);
    window.addEventListener("resize", measure);
    return () => {
      observer.disconnect();
      window.removeEventListener("resize", measure);
    };
  });

  // Re-measure when the tab comes back, and park the webview when it goes.
  $effect(() => {
    if (visible) {
      // After the layout settles, or the box is measured mid-transition.
      requestAnimationFrame(() => requestAnimationFrame(measure));
    } else {
      void browser.hide();
    }
  });

  // A new page requested while we are already showing: hand it over.
  $effect(() => {
    if (browser.pending && visible) measure();
  });
</script>

<div class="browser" class:showing={visible}>
  <header class="bar">
    <div class="nav">
      <button title="Back" onclick={() => browser.go("back")} aria-label="Back">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
          stroke-linecap="round" stroke-linejoin="round"><path d="M15 18l-6-6 6-6" /></svg>
      </button>
      <button title="Forward" onclick={() => browser.go("forward")} aria-label="Forward">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
          stroke-linecap="round" stroke-linejoin="round"><path d="M9 18l6-6-6-6" /></svg>
      </button>
      <button title="Reload" onclick={() => browser.go("reload")} aria-label="Reload">
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"
          stroke-linecap="round" stroke-linejoin="round">
          <path d="M3 12a9 9 0 1 0 2.6-6.4M3 4v5h5" /></svg>
      </button>
    </div>

    <span class="address path">{tidyUrl(browser.url)}</span>

    <span class="shield" title="Advertising and tracking are blocked here">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.75"
        stroke-linecap="round" stroke-linejoin="round">
        <path d="M12 3l7 3v5c0 4.4-3 8.3-7 10-4-1.7-7-5.6-7-10V6z" />
        <path d="M9 12l2 2 4-4" /></svg>
      <span>Ads blocked</span>
    </span>

    <button class="done" onclick={() => browser.close()}>Close</button>
  </header>

  {#if browser.error}
    <p class="failed">{browser.error}</p>
  {/if}

  <!-- Measured, never painted: the webview covers exactly this. -->
  <div class="slot" bind:this={slot}></div>
</div>

<style>
  .browser {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  .bar {
    display: flex;
    align-items: center;
    gap: 0.625rem;
    padding: 0.5rem var(--pad);
    border-bottom: 1px solid var(--rule);
    background: var(--hull);
  }

  .nav {
    display: flex;
    gap: 0.125rem;
  }

  .nav button {
    display: grid;
    place-items: center;
    width: 1.75rem;
    height: 1.75rem;
    border-radius: var(--r-sm);
    color: var(--dim);
  }

  .nav button:hover {
    color: var(--readout);
    background: var(--card);
  }

  .nav svg {
    width: 1rem;
    height: 1rem;
  }

  .address {
    flex: 1;
    min-width: 0;
    padding: 0.3125rem 0.625rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-pill);
    background: var(--inset);
    font-size: var(--t-micro);
    color: var(--dim);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .shield {
    display: inline-flex;
    align-items: center;
    gap: 0.3125rem;
    flex: none;
    font-size: 0.6875rem;
    font-weight: 600;
    letter-spacing: 0.04em;
    color: var(--accent);
  }

  .shield svg {
    width: 0.875rem;
    height: 0.875rem;
  }

  .done {
    flex: none;
    padding: 0.3125rem 0.75rem;
    border: 1px solid var(--rule-hi);
    border-radius: var(--r-sm);
    font-size: var(--t-micro);
    font-weight: 600;
    color: var(--dim);
  }

  .done:hover {
    color: var(--readout);
    background: var(--cyan-wash);
  }

  .slot {
    flex: 1;
    min-height: 0;
    background: var(--void);
  }

  .failed {
    margin: 0;
    padding: 0.4375rem var(--pad);
    background: var(--contest-wash);
    border-bottom: 1px solid rgba(255, 95, 86, 0.26);
    font-size: var(--t-micro);
    color: var(--readout);
  }
</style>
