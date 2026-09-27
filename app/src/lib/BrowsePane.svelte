<script lang="ts">
  import Awaiting from "./Awaiting.svelte";
  import BrowserView from "./BrowserView.svelte";
  import { browser } from "./browser.svelte";
  import {
    claimNxmScheme,
    nexusBrowse,
    nxmOwner,
    openModPage,
    releaseNxmScheme,
    type Listed,
    type Listing,
    type SchemeOwner,
  } from "./engine";

  interface Props {
    /** folder names already installed, so the list can say so */
    installed?: string[];
    /** false while the user is on another tab */
    visible?: boolean;
  }

  let { installed = [], visible = true }: Props = $props();

  const TABS: { id: Listing; name: string; note: string }[] = [
    { id: "trending", name: "Trending", note: "What people are downloading now" },
    { id: "latest_added", name: "New", note: "Just published" },
    { id: "latest_updated", name: "Updated", note: "Recently changed" },
  ];

  let list = $state<Listing>("trending");
  let cache = $state<Partial<Record<Listing, Listed[]>>>({});
  let loading = $state(false);
  let failure = $state<string | null>(null);
  let search = $state("");

  const mods = $derived(cache[list] ?? []);
  const current = $derived(TABS.find((t) => t.id === list)!);

  async function show(which: Listing) {
    list = which;
    if (cache[which]) return;
    loading = true;
    failure = null;
    try {
      cache = { ...cache, [which]: await nexusBrowse(which) };
    } catch (err) {
      failure = String(err);
    } finally {
      loading = false;
    }
  }

  show("trending");

  // Who gets a download clicked in a web browser, as opposed to in here.
  let owner = $state<SchemeOwner | null>(null);
  let ownerError = $state<string | null>(null);
  nxmOwner()
    .then((o) => (owner = o))
    .catch(() => {});

  async function claim() {
    ownerError = null;
    try {
      owner = await claimNxmScheme();
    } catch (err) {
      ownerError = String(err);
    }
  }

  async function release() {
    ownerError = null;
    try {
      owner = await releaseNxmScheme();
    } catch (err) {
      ownerError = String(err);
    }
  }

  /** Name the other handler from its command line, without the whole path. */
  function other(command: string | null): string {
    if (!command) return "whatever is registered for nxm:// links";
    // The last path segment before `.exe`, so
    // `"C:\Program Files\Vortex\Vortex.exe" -d "%1"` reads as "Vortex".
    const exe = command.match(/([^\\/"]+)\.exe/i);
    return exe ? exe[1] : "another application";
  }

  /**
   * Search runs on the site, in the browser window.
   *
   * The API has no keyword search, and reimplementing one over the curated
   * lists would only ever cover thirty mods. The site's own search is always
   * current and its download button feeds straight back to us.
   */
  function runSearch(event: Event) {
    event.preventDefault();
    const q = search.trim();
    if (!q) return;
    void openModPage(
      `https://www.nexusmods.com/games/nomanssky/mods?keyword=${encodeURIComponent(q)}`,
    );
  }

  function openMod(mod: Listed) {
    if (mod.mod_id === null) return;
    void openModPage(`https://www.nexusmods.com/nomanssky/mods/${mod.mod_id}`);
  }

  const num = (n: number | null | undefined) =>
    n === null || n === undefined ? "—" : n.toLocaleString();

  /** A listing's summary can carry BBCode; show it as text. */
  const plain = (raw: string | null) =>
    (raw ?? "")
      .replace(/<br\s*\/?>/gi, " ")
      .replace(/\[\/?[^\]\n]{1,40}\]/g, "")
      .replace(/&amp;/g, "&")
      .replace(/&#39;/g, "'")
      .replace(/&quot;/g, '"')
      .trim();

  /** Rough: the listing gives no folder name, so match on the mod's title. */
  function alreadyHave(mod: Listed): boolean {
    const name = (mod.name ?? "").toLowerCase();
    return name.length > 3 && installed.some((o) => o.toLowerCase().startsWith(name));
  }
</script>

{#if browser.open || browser.pending}
  <BrowserView visible={visible} />
{:else}
<div class="pane">
  <header class="top">
    <nav>
      {#each TABS as tab}
        <button class:on={list === tab.id} onclick={() => show(tab.id)}>
          {tab.name}
        </button>
      {/each}
    </nav>

    <form onsubmit={runSearch}>
      <input
        type="search"
        bind:value={search}
        placeholder="Search all No Man's Sky mods…"
        spellcheck="false"
      />
      <button class="find" type="submit" disabled={!search.trim()}>Search</button>
    </form>
  </header>

  <p class="note">{current.note}</p>

  {#if loading}
    <Awaiting>Asking Nexus…</Awaiting>
  {:else if failure}
    <p class="failed">{failure}</p>
  {:else}
    <div class="grid">
      {#each mods as mod (mod.mod_id)}
        <article>
          <button class="card" onclick={() => openMod(mod)}>
            {#if mod.picture_url}
              <img src={mod.picture_url} alt="" loading="lazy" />
            {:else}
              <div class="noshot" aria-hidden="true"></div>
            {/if}
            <div class="body">
              <h3>{mod.name}</h3>
              <p class="by">
                {mod.author ?? "unknown"}
                {#if mod.version}<span class="ver">v{mod.version}</span>{/if}
              </p>
              <p class="blurb">{plain(mod.summary)}</p>
              <p class="stats">
                <span>{num(mod.endorsement_count)} endorsements</span>
                <span>{num(mod.mod_downloads)} downloads</span>
              </p>
            </div>
          </button>
          {#if alreadyHave(mod)}
            <span class="have">Installed</span>
          {/if}
        </article>
      {/each}
    </div>

    <section class="how">
      <p>
        Opening a mod shows its real Nexus page. Its <span class="mono"
          >Mod manager download</span
        > button installs it here &mdash; the link is caught inside this window,
        so nothing is registered and your other mod manager keeps handling every
        other game.
      </p>
      {#if owner}
        <p class="owner">
          {#if owner.ours}
            Downloads started in a web browser also come here.
            <button class="btn-link" onclick={release}>Hand that back</button>
          {:else}
            Downloads started in a web browser go to
            <span class="mono">{other(owner.command)}</span>.
            <button class="btn-link" onclick={claim}>Send those here too</button>
          {/if}
        </p>
      {/if}
      {#if ownerError}<p class="failed">{ownerError}</p>{/if}
    </section>
  {/if}
</div>
{/if}

<style>
  .top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
    flex-wrap: wrap;
  }

  nav {
    display: flex;
    gap: 0.25rem;
    padding: 0.1875rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-md);
    background: var(--inset);
  }

  nav button {
    padding: 0.3125rem 0.75rem;
    border-radius: var(--r-sm);
    font-size: var(--t-micro);
    font-weight: 600;
    color: var(--dim);
  }

  nav button:hover {
    color: var(--readout);
  }

  nav button.on {
    background: var(--card-hi);
    color: var(--cyan);
  }

  form {
    display: flex;
    gap: 0.5rem;
    flex: 1;
    min-width: 16rem;
    max-width: 28rem;
  }

  input {
    flex: 1;
    padding: 0.4375rem 0.625rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-sm);
    background: var(--inset);
    color: var(--readout);
    font-size: var(--t-micro);
  }

  input:focus {
    outline: none;
    border-color: var(--rule-hi);
  }

  .find {
    padding: 0.4375rem 0.875rem;
    border: 1px solid var(--rule-hi);
    border-radius: var(--r-sm);
    font-size: var(--t-micro);
    font-weight: 600;
    color: var(--cyan);
  }

  .find:hover:not(:disabled) {
    background: var(--cyan-wash);
  }

  .find:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .note {
    margin: 0.75rem 0 0.875rem;
    font-size: var(--t-micro);
    color: var(--faint);
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(17rem, 1fr));
    gap: 0.75rem;
  }

  article {
    position: relative;
  }

  .card {
    display: block;
    width: 100%;
    text-align: left;
    border: 1px solid var(--rule);
    border-radius: var(--r-lg);
    background: var(--card);
    box-shadow: var(--lip);
    overflow: hidden;
    transition:
      background 140ms ease,
      border-color 140ms ease,
      box-shadow 140ms ease;
  }

  .card:hover {
    background: var(--card-hi);
    border-color: var(--rule-hi);
    box-shadow: var(--lip), var(--shadow-md);
  }

  img,
  .noshot {
    display: block;
    width: 100%;
    height: 8.5rem;
    object-fit: cover;
    background: var(--inset);
    border-bottom: 1px solid var(--rule);
  }

  .body {
    padding: 0.75rem 0.875rem 0.875rem;
  }

  h3 {
    margin: 0;
    font-size: var(--t-body);
    font-weight: 600;
    line-height: 1.3;
    color: var(--readout);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .by {
    margin: 0.1875rem 0 0;
    font-size: var(--t-micro);
    color: var(--faint);
  }

  .ver {
    margin-left: 0.375rem;
    font-family: var(--font-value);
    color: var(--cyan-dim);
  }

  .blurb {
    margin: 0.4375rem 0 0;
    font-size: var(--t-micro);
    color: var(--dim);
    line-height: 1.5;
    /* Three lines, so every card in a row is the same height. */
    display: -webkit-box;
    -webkit-line-clamp: 3;
    line-clamp: 3;
    -webkit-box-orient: vertical;
    overflow: hidden;
    min-height: 3.4rem;
  }

  .stats {
    display: flex;
    justify-content: space-between;
    gap: 0.5rem;
    margin: 0.625rem 0 0;
    padding-top: 0.5rem;
    border-top: 1px solid var(--rule);
    font-size: 0.6875rem;
    color: var(--faint);
    font-family: var(--font-value);
  }

  .have {
    position: absolute;
    top: 0.5rem;
    right: 0.5rem;
    padding: 0.1875rem 0.5rem;
    border: 1px solid rgba(138, 224, 60, 0.3);
    border-radius: var(--r-pill);
    background: var(--accent-wash);
    font-size: 0.625rem;
    font-weight: 600;
    letter-spacing: 0.1em;
    text-transform: uppercase;
    color: var(--accent);
  }

  .how {
    margin: 1rem 0 0;
    padding-top: 0.75rem;
    border-top: 1px solid var(--rule);
  }

  .how p {
    margin: 0;
    font-size: var(--t-micro);
    color: var(--faint);
    max-width: 72ch;
  }

  .owner {
    margin-top: 0.375rem !important;
  }

  .mono {
    font-family: var(--font-value);
    color: var(--dim);
  }

</style>
