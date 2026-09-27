<script lang="ts">
  import ClaimStack from "./ClaimStack.svelte";
  import Button from "./Button.svelte";
  import { mergeConflict, type CleanPlan } from "./engine";
  import { mergePlanFor } from "./actions";
  import { names } from "./names.svelte";
  import { assetFolder, assetName, combinedNote, type Conflict } from "./types";

  interface Props {
    conflict: Conflict;
    realOrder: boolean;
    /** the clean comparison, so this pane offers the same fix the Actions tab
     *  does. Without it Evidence offered a plain "Combine them" for conflicts
     *  Actions had already decided were better cleaned, and the two screens
     *  contradicted each other about the same asset. */
    plans?: CleanPlan[];
    onMerged?: () => void;
  }

  let { conflict, realOrder, plans = [], onMerged }: Props = $props();

  let merging = $state(false);
  let merged = $state<string | null>(null);
  let mergeError = $state<string | null>(null);

  // Clean or merge, decided in exactly one place for both screens.
  const plan = $derived(mergePlanFor(conflict, plans));
  const cleanInstead = $derived(plan.clean.length > 0);

  async function combine() {
    merging = true;
    mergeError = null;
    try {
      merged = await mergeConflict(conflict.target, conflict.mods);
      if (merged) onMerged?.();
    } catch (err) {
      mergeError = String(err);
    } finally {
      merging = false;
    }
  }

  const SHOWN = 8;
  let expanded = $state(false);
  const visible = $derived(
    expanded ? conflict.clashes : conflict.clashes.slice(0, SHOWN),
  );
  const hidden = $derived(conflict.clashes.length - visible.length);
</script>

<article>
  <!-- The bracket is the one NMS-derived device kept, and it earns its place:
       it frames claims that are in contention. It appears nowhere else. -->
  <div class="frame">
    <h2>
      <span class="file">{assetName(conflict.target)}</span>
      <span class="folder path">{assetFolder(conflict.target)}</span>
    </h2>
    <!-- The count is a raw value comparison, so it fires even when one mod is
         only carrying the game's own values along. Saying that in red, above
         a verdict of "these can be combined", reads as self-contradiction. -->
    <p class="summary" class:reconciled={conflict.mergeable}>
      {conflict.summary}
    </p>
    <p class="who">
      {conflict.mods.map((mod) => names.of(mod)).join("  ⇄  ")}
    </p>
  </div>

  {#if conflict.predicted_winner}
    <p class="basis">
      {names.of(conflict.predicted_winner)} wins on
      {realOrder ? "ModPriority" : "assumed alphabetical order"}.
    </p>
  {/if}

  {#if conflict.mergeable}
    <!-- The remedy, not just the diagnosis. Telling someone to pick a winner
         here would be wrong: nobody disagrees, one copy simply overwrites
         edits it never meant to touch. -->
    <div class="merge">
      <p class="headline">These can be combined. Nothing has to be chosen.</p>
      <p class="detail">
        The values above differ, but no property is <em>edited</em> by more
        than one of them. Where they differ, one mod is carrying the game's own
        value along inside a file it replaces wholesale &mdash; so the other's
        edit is reverted by accident, not by disagreement.
      </p>
      <ul class="edits">
        {#each Object.entries(conflict.edit_counts).sort((a, b) => b[1] - a[1]) as [mod, n]}
          <li>
            <span class="n">{n.toLocaleString()}</span>
            <span class="who">{names.of(mod)}</span>
            {#if plan.clean.includes(mod)}
              <span class="aside">carries values it does not change — clean it</span>
            {/if}
          </li>
        {/each}
      </ul>

      {#if cleanInstead}
        <!-- Every whole-file copy here can be reduced to a patch, and once they
             all are the game merges the rest itself. So there is a cheaper fix
             than a merge and this pane must not offer the merge instead: the
             Actions tab is where the button for it lives. -->
        <p class="detail">
          There is a better fix than combining these:
          {plan.clean.map((mod) => names.of(mod)).join(", ")}
          {plan.clean.length === 1 ? "writes values it does not" : "write values they do not"}
          change, and {plan.clean.length === 1 ? "it" : "they"} can be cut back to
          the edits {plan.clean.length === 1 ? "it makes" : "they make"}. Once
          {plan.clean.length === 1 ? "it is" : "they are"} cleaned nothing here
          restates anybody else's value, the game applies all of these itself, and
          there is no combined mod to keep track of. See <b>Clean</b> on the Actions
          tab.
        </p>
      {:else if plan.merge.length === 0}
        <!-- Nothing here can be cleaned and no copy is a whole file to build a
             merge on, so `merge::build` would refuse. Saying so beats a button
             that is rejected the moment it is pressed. -->
        <p class="detail">
          These cannot be combined automatically: a combined asset has to be built
          on top of one mod's whole copy of the file, and every copy here is a
          patch. Load order decides, and the rail shows who wins.
        </p>
      {:else if merged}
        <p class="outcome">{combinedNote(conflict.mods)}</p>
        <p class="wrote path">{merged}</p>
      {:else}
        <div class="verbs">
          <Button variant="primary" onclick={combine} busy={merging}>
            {merging ? "Combining…" : "Combine them"}
          </Button>
        </div>
        {#if mergeError}
          <p class="outcome bad">{mergeError}</p>
        {/if}
      {/if}
    </div>
  {:else if conflict.mergeable === false && conflict.overlap.length}
    <p class="basis">
      {conflict.overlap.length.toLocaleString()} propert{conflict.overlap
        .length === 1
        ? "y is"
        : "ies are"} changed by more than one mod &mdash; those need a decision.
    </p>
  {/if}

  {#if conflict.clashes.length}
    <section aria-label="Contested properties">
      {#each visible as clash, i (clash.path)}
        <ClaimStack {clash} winner={conflict.predicted_winner} index={i} />
      {/each}
    </section>

    {#if hidden > 0}
      <button class="btn-link more" onclick={() => (expanded = true)}>
        Show {hidden} more contested {hidden === 1 ? "property" : "properties"}
      </button>
    {:else if expanded && conflict.clashes.length > SHOWN}
      <button class="btn-link more" onclick={() => (expanded = false)}>
        Show fewer
      </button>
    {/if}
  {/if}

  {#each conflict.notes as note}
    <p class="note">{note}</p>
  {/each}
</article>

<style>
  article {
    padding-bottom: 0;
  }

  /* Amber is reserved for "this is what actually loads"; a merge verdict is
     the same kind of statement, so it carries the same bar. */
  .merge {
    margin: 0.5rem 0 1rem;
    padding: 0.5rem 0 0.5rem 0.875rem;
    border-left: 2px solid var(--settled);
  }

  .merge .headline {
    margin: 0;
    font-size: var(--t-body);
    color: var(--readout);
  }

  .merge .detail {
    margin: 0.25rem 0 0;
    font-size: var(--t-micro);
    color: var(--dim);
    max-width: 44rem;
  }

  .edits {
    list-style: none;
    margin: 0.5rem 0 0;
    padding: 0;
  }

  .edits li {
    display: flex;
    align-items: baseline;
    gap: 0.75rem;
    font-size: var(--t-micro);
  }

  .edits .n {
    font-family: var(--font-value);
    font-variant-numeric: tabular-nums;
    min-width: 4rem;
    text-align: right;
    color: var(--readout);
  }

  .edits .who {
    color: var(--dim);
  }

  /* Names the row that is being left out of the merge, beside the count it is
     being left out on account of. */
  .edits .aside {
    color: var(--cyan-dim);
  }

  .outcome {
    margin: 0.5rem 0 0;
    font-size: var(--t-micro);
    color: var(--readout);
  }

  .outcome.bad {
    color: var(--contest);
    max-width: 44rem;
  }

  .wrote {
    margin: 0.125rem 0 0;
    font-size: var(--t-micro);
    color: var(--dim);
    word-break: break-all;
  }

  /* Corner brackets, drawn as two L-shapes rather than a full border, so the
     frame reads as "these claims are in contention" and not as a card. */
  .frame {
    position: relative;
    padding: 0.875rem 1rem;
  }

  .frame::before,
  .frame::after {
    content: "";
    position: absolute;
    width: 1.5rem;
    height: 1.5rem;
    pointer-events: none;
  }

  .frame::before {
    top: 0;
    left: 0;
    border-top: 1px solid var(--rule);
    border-left: 1px solid var(--rule);
  }

  .frame::after {
    bottom: 0;
    right: 0;
    border-bottom: 1px solid var(--rule);
    border-right: 1px solid var(--rule);
  }

  h2 {
    margin: 0;
    font-size: var(--t-lead);
    font-weight: 600;
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0.625rem;
  }

  .folder {
    font-size: var(--t-micro);
    font-weight: 400;
    color: var(--dim);
    word-break: break-all;
  }

  .summary {
    margin: 0.375rem 0 0;
    color: var(--contest);
    font-size: var(--t-small);
  }

  /* Nothing is in contention here, so it does not get the contention colour. */
  .summary.reconciled {
    color: var(--dim);
  }

  .who {
    margin: 0.25rem 0 0;
    color: var(--dim);
    font-size: var(--t-small);
  }

  .basis {
    margin: 0.875rem 0 0.25rem;
    font-size: var(--t-small);
    color: var(--dim);
  }

  .more {
    margin-top: 0.875rem;
    font-size: var(--t-small);
  }

  .note {
    margin: 0.75rem 0 0;
    font-size: var(--t-micro);
    color: var(--dim);
    max-width: 68ch;
  }
</style>
