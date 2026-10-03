<script lang="ts">
  import Awaiting from "./Awaiting.svelte";
  import Button from "./Button.svelte";
  import { openModPage } from "./engine";
  import { updates } from "./updates.svelte";
  import type { Identity, ModPage } from "./engine";

  interface Props {
    identity: Identity;
    page: ModPage | null;
    /** "cleaned" / "mended" / "merged" when this is a build we made, else null */
    variant?: string | null;
    fetching: boolean;
    pageError: string | null;
    members: string[];
  }

  let { identity, page, variant = null, fetching, pageError, members }: Props = $props();

  /**
   * The newest version Nexus offers, which is not the page's own `version`.
   * That one is a field the author types by hand and often forgets: page 3718
   * says 1.4.0 while it offers 1.4.2 and archives 1.4.0. The update check
   * walks the files themselves, so its answer is used whenever there is one.
   */
  const check = $derived(updates.checks.find((c) => c.owner === identity.owner) ?? null);
  const newest = $derived(
    check?.state === "outdated"
      ? check.latest_version
      : check?.state === "current"
        ? check.recorded_version
        : (page?.version ?? null),
  );

  let opening = $state(false);
  let openError = $state<string | null>(null);

  async function browse() {
    if (!identity.page) return;
    opening = true;
    openError = null;
    try {
      await openModPage(identity.page);
    } catch (err) {
      openError = String(err);
    } finally {
      opening = false;
    }
  }

  /**
   * Nexus descriptions are BBCode with `<br />` for line breaks, written by
   * mod authors. Rendering them as HTML would be running strangers' markup in
   * a window that can call the engine, so the tags are stripped and the text
   * is shown as text.
   */
  function readable(raw: string | null): string {
    if (!raw) return "";
    return raw
      .replace(/<br\s*\/?>/gi, "\n")
      .replace(/\[\/?[^\]\n]{1,40}\]/g, "")
      .replace(/&amp;/g, "&")
      .replace(/&lt;/g, "<")
      .replace(/&gt;/g, ">")
      .replace(/&quot;/g, '"')
      .replace(/&#39;/g, "'")
      .replace(/\n{3,}/g, "\n\n")
      .trim();
  }

  const blurb = $derived(readable(page?.summary ?? null));
  const body = $derived(readable(page?.description ?? null));

  let expanded = $state(false);
  const shown = $derived(expanded ? body : body.slice(0, 600));

  function when(iso: string | null | undefined): string {
    if (!iso) return "—";
    const d = new Date(iso);
    return Number.isNaN(d.getTime())
      ? "—"
      : d.toLocaleDateString(undefined, {
          year: "numeric",
          month: "short",
          day: "numeric",
        });
  }

  const num = (n: number | null | undefined) =>
    n === null || n === undefined ? "—" : n.toLocaleString();
</script>

<section class="panel">
  <div class="head">
    {#if page?.picture_url}
      <img src={page.picture_url} alt="" />
    {/if}
    <div class="titles">
      <span class="hud-label">
        {identity.mod_id === null ? "Not from Nexus" : `Nexus ${identity.mod_id}`}
      </span>
      <!-- The page's own title when we have it, else the best name the
           engine could work out. Never the folder, unless that is all there is. -->
      <h2>{page?.name ?? identity.name}</h2>
      {#if page?.author}
        <p class="by">by {page.author}</p>
      {/if}
      {#if blurb}
        <p class="blurb">{blurb}</p>
      {/if}
    </div>
  </div>

  {#if identity.page}
    <div class="verbs">
      <Button variant="primary" onclick={browse} busy={opening}>
        {opening ? "Opening…" : "Open on Nexus"}
      </Button>
    </div>
    {#if openError}<p class="failed">{openError}</p>{/if}
  {/if}

  {#if fetching}
    <Awaiting>Asking Nexus about this mod…</Awaiting>
  {:else if pageError}
    <p class="failed">{pageError}</p>
  {:else if identity.mod_id === null}
    <p class="quiet">
      No mod manager recorded a Nexus archive for this folder, so there is no
      page to show. It was most likely installed by hand.
    </p>
  {/if}

  <dl class="facts">
    <div>
      <dt>Installed</dt>
      <dd class="path">{identity.version ?? "—"}</dd>
    </div>
    {#if variant}
      <!-- The version above is the author's, not ours: a build we made carries
           no version of its own, and it is still that release of their mod that
           updates are checked against. Saying which build is running stops the
           version reading as a contradiction. -->
      <div>
        <dt>Running</dt>
        <dd class="path">our {variant} build</dd>
      </div>
    {/if}
    {#if newest}
      <div>
        <dt>On Nexus</dt>
        <dd class="path">{newest}</dd>
      </div>
    {/if}
    <div>
      <dt>Load order</dt>
      <dd class="path">{identity.priority ?? "not registered"}</dd>
    </div>
    <div>
      <dt>Assets</dt>
      <dd class="path">{identity.assets}</dd>
    </div>
    <div>
      <dt>Files</dt>
      <dd class="path">{identity.files}</dd>
    </div>
    {#if page}
      <div>
        <dt>Updated</dt>
        <dd class="path">{when(page.updated_time)}</dd>
      </div>
      <div>
        <dt>Endorsements</dt>
        <dd class="path">{num(page.endorsement_count)}</dd>
      </div>
      <div>
        <dt>Downloads</dt>
        <dd class="path">{num(page.mod_downloads)}</dd>
      </div>
    {/if}
    {#if identity.managed_by}
      <div>
        <dt>Deployed by</dt>
        <dd class="path">{identity.managed_by}</dd>
      </div>
    {/if}
  </dl>

  {#if members.length}
    <details>
      <summary>
        What it puts in the mods folder ({members.length})
      </summary>
      <ul class="members">
        {#each members as member}
          <li class="path">{member}</li>
        {/each}
      </ul>
    </details>
  {/if}

  {#if body}
    <div class="about">
      <span class="hud-label">About</span>
      <p class="body">{shown}{!expanded && body.length > 600 ? "…" : ""}</p>
      {#if body.length > 600}
        <button class="btn-link more" onclick={() => (expanded = !expanded)}>
          {expanded ? "Show less" : "Show all"}
        </button>
      {/if}
    </div>
  {/if}
</section>

<style>
  .head {
    display: flex;
    gap: 1rem;
    align-items: flex-start;
  }

  img {
    flex: none;
    width: 8.5rem;
    height: 4.75rem;
    object-fit: cover;
    border: 1px solid var(--rule);
    border-radius: var(--r-md);
    background: var(--inset);
  }

  .titles {
    min-width: 0;
  }

  h2 {
    margin: 0.125rem 0 0;
    font-size: var(--t-title);
    font-weight: 600;
    letter-spacing: -0.01em;
    line-height: 1.25;
  }

  .by {
    margin: 0.1875rem 0 0;
    font-size: var(--t-micro);
    color: var(--faint);
  }

  .blurb {
    margin: 0.4375rem 0 0;
    font-size: var(--t-small);
    color: var(--dim);
    max-width: 62ch;
  }

  .facts {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(8.5rem, 1fr));
    gap: 0.5rem;
    margin: 0.875rem 0 0;
    padding: 0.75rem 0.875rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-sm);
    background: var(--inset);
  }

  .facts dt {
    font-size: 0.625rem;
    font-weight: 600;
    letter-spacing: 0.14em;
    text-transform: uppercase;
    color: var(--cyan-dim);
  }

  .facts dd {
    margin: 0.125rem 0 0;
    font-size: var(--t-small);
    color: var(--readout);
  }

  details {
    margin-top: 0.875rem;
  }

  summary {
    font-size: var(--t-micro);
    color: var(--dim);
    cursor: pointer;
  }

  summary:hover {
    color: var(--cyan);
  }

  .members {
    list-style: none;
    margin: 0.4375rem 0 0;
    padding: 0.4375rem 0.625rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-sm);
    background: var(--inset);
    font-size: var(--t-micro);
    color: var(--dim);
    max-height: 11rem;
    overflow-y: auto;
  }

  .members li {
    word-break: break-all;
  }

  .about {
    margin-top: 0.875rem;
    padding-top: 0.75rem;
    border-top: 1px solid var(--rule);
  }

  .body {
    margin: 0.375rem 0 0;
    font-size: var(--t-small);
    color: var(--dim);
    line-height: 1.55;
    max-width: 68ch;
    white-space: pre-wrap;
  }

  /* Not an error and not a warning: an explanation of why a panel that
     usually holds a Nexus page is holding nothing. */
  .quiet {
    margin: 0.875rem 0 0;
    font-size: var(--t-small);
    color: var(--dim);
    max-width: 62ch;
  }

  .more {
    margin-top: 0.5rem;
  }
</style>
