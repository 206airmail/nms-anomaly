<script lang="ts">
  /** We have asked for something and it has not arrived.
   *
   * There were four phrasings of this in three different markup shapes --
   * "Reading the mods folder…", "Asking Nexus…", "Asking Nexus about every
   * installed mod…", "Checking for a saved key…" -- some with a spinner, some
   * without, and the ones without looked like a finished sentence about
   * nothing rather than like waiting.
   *
   * The words stay at the call site, because naming what is being waited for
   * is the whole value of the line. The spinner is not optional: a waiting
   * message with no moving part is indistinguishable from a stuck one.
   */
  import type { Snippet } from "svelte";

  interface Props {
    children: Snippet;
  }

  let { children }: Props = $props();
</script>

<p class="awaiting" aria-live="polite">
  <span class="spin" aria-hidden="true"></span>
  {@render children()}
</p>

<style>
  .awaiting {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin: 0.875rem 0 0;
    color: var(--faint);
    font-size: var(--t-small);
  }
</style>
