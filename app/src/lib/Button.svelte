<script lang="ts">
  /** Every button in the app that is not part of a bigger grid.
   *
   * The point is not saving markup, it is that there is now one answer to
   * "what does a primary button look like". There used to be fifteen class
   * names for four roles, and they had already drifted apart.
   *
   * `busy` shows the spinner and disables the control, because a button that
   * is working and still pressable gets pressed twice. The *label* stays at
   * the call site: only the caller knows whether the working word is
   * "Cleaning…" or "Asking Nexus…".
   */
  import type { Snippet } from "svelte";

  interface Props {
    /** what this button does to the world */
    variant?: "primary" | "ghost" | "danger" | "link";
    /** running: spinner on, control locked */
    busy?: boolean;
    disabled?: boolean;
    type?: "button" | "submit";
    title?: string;
    onclick?: (event: MouseEvent) => void;
    children: Snippet;
  }

  let {
    variant = "ghost",
    busy = false,
    disabled = false,
    type = "button",
    title,
    onclick,
    children,
  }: Props = $props();
</script>

<button
  class="btn btn-{variant}"
  {type}
  {title}
  {onclick}
  disabled={disabled || busy}
>
  {#if busy}<span class="spin" aria-hidden="true"></span>{/if}
  {@render children()}
</button>
