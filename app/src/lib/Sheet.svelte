<script lang="ts">
  /** A decision that has to be answered before anything else happens.
   *
   * Replaces two hand-built copies -- the delete confirmation and the preset
   * switch -- which between them had a `role="button"` backdrop you could tab
   * into, no focus trap, and Escape wired to the veil rather than to the
   * dialog. Both were reasonable guesses at what the platform already does.
   *
   * So this uses a real `<dialog>` opened with `showModal()`, which brings the
   * top layer, the backdrop, focus containment, restoring focus on close and
   * Escape, none of which we then have to maintain. `cancel` is intercepted
   * only to route Escape through `onclose`, so closing by any means takes the
   * same path.
   *
   * Clicking the backdrop closes it. That is right for a preview -- the safe
   * answer is "no" -- and `<dialog>` gives no backdrop click event, so the
   * click is read off the dialog's own box: a press whose coordinates fall
   * outside it landed on the backdrop.
   */
  import type { Snippet } from "svelte";

  interface Props {
    /** the question, as a heading */
    title: string;
    /** true when this decision destroys something: frames it in the alert hue */
    danger?: boolean;
    /**
     * Give it most of the window rather than a column.
     *
     * For the one thing in here that is not a question: the evidence behind a
     * verdict. A claim stack is a table of who claims what, beside the load
     * order that decides between them, and squeezed into 38rem it stops being
     * readable as a comparison at all — which is the only thing it is for.
     */
    wide?: boolean;
    onclose: () => void;
    children: Snippet;
    /** the buttons. Primary verb first, the way out after it. */
    verbs?: Snippet;
  }

  let { title, danger = false, wide = false, onclose, children, verbs }: Props = $props();

  let dialog = $state<HTMLDialogElement | null>(null);

  $effect(() => {
    dialog?.showModal();
  });

  function outside(event: MouseEvent) {
    if (!dialog) return;
    const box = dialog.getBoundingClientRect();
    const inside =
      event.clientX >= box.left &&
      event.clientX <= box.right &&
      event.clientY >= box.top &&
      event.clientY <= box.bottom;
    if (!inside) onclose();
  }
</script>

<dialog
  bind:this={dialog}
  class:danger
  class:wide
  onclick={outside}
  oncancel={(event) => {
    event.preventDefault();
    onclose();
  }}
>
  <h3>{title}</h3>
  <div class="body">
    {@render children()}
  </div>
  {#if verbs}
    <div class="verbs">{@render verbs()}</div>
  {/if}
</dialog>

<style>
  dialog {
    width: min(38rem, 92vw);
    max-height: 82vh;
    padding: var(--pad);
    border: 1px solid var(--rule-hi);
    border-radius: var(--r-lg);
    background: var(--hull);
    color: var(--readout);
    box-shadow: var(--shadow-md);
  }

  /* A destructive decision is framed, not coloured throughout: the red says
     "this one is different", and the text stays readable. */
  dialog.danger {
    border-color: rgba(255, 95, 86, 0.4);
  }

  /* See `wide`. The body grows with it, because the thing being widened is a
     list that also has to be scrolled through.

     `[open]` matters: the user agent hides a dialog with `display: none` until
     it is opened, so a bare `display: flex` here would put an unopened one on
     the page. */
  dialog.wide[open] {
    display: flex;
    flex-direction: column;
    width: min(76rem, 94vw);
    height: 86vh;
    max-height: 86vh;
  }

  dialog.wide .body {
    flex: 1;
    min-height: 0;
    max-height: none;
  }

  dialog::backdrop {
    background: rgba(2, 6, 6, 0.72);
    backdrop-filter: blur(2px);
  }

  h3 {
    margin: 0 0 0.75rem;
    font-size: var(--t-title);
    font-weight: 600;
    letter-spacing: var(--track-tighter);
  }

  .body {
    max-height: 56vh;
    overflow-y: auto;
  }

  /* Pinned under the body so a long list scrolls behind the answer rather
     than pushing it off the bottom of the window. */
  .verbs {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    margin-top: 1rem;
    padding-top: 0.875rem;
    border-top: 1px solid var(--rule);
  }
</style>
