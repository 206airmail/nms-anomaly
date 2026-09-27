<script lang="ts">
  /** Everything this program knows about one mod, in one column.
   *
   * ---------------------------------------------------------------------------
   * What this replaces
   * ---------------------------------------------------------------------------
   *
   * Six screens, each holding one fact about the same mod. To find out what
   * was true of `BetterRewards` you had to visit the Actions tab (it needs
   * cleaning), the Updates tab (Nexus has a newer file), the Evidence tab (it
   * contests a reward table and loses), the Library tab (its page, whether it
   * is on, which build is running), the Presets tab (whether it is on, again)
   * and the Sessions tab (the game complained about it last run) — and match
   * folder names by eye across all six.
   *
   * Those are not six screens. They are six *sections*, and this is the order
   * they are asked in: what is it, what should I do about it, is it in the
   * game, whose build is it running, is it current, is anything fighting it,
   * what did the game make of it, and how do I get rid of it.
   *
   * ---------------------------------------------------------------------------
   * The rule the sections follow
   * ---------------------------------------------------------------------------
   *
   * A section that would say "nothing" is not drawn. A mod with no findings,
   * no conflicts and no session trouble is four panels, not nine empty ones —
   * the absence of a heading is the answer, and a column of "None." is the
   * dashboard this app was specifically not going to be.
   *
   * The exception is **In the game**, which is drawn always. "Is this actually
   * loading" is the question the record is most often opened for, and an
   * answer that appears only sometimes is not an answer.
   */
  import ActionRow from "./ActionRow.svelte";
  import Button from "./Button.svelte";
  import ModPageView from "./ModPageView.svelte";
  import { names } from "./names.svelte";
  import { sessions } from "./sessions.svelte";
  import { ago, updates } from "./updates.svelte";
  import type { Action } from "./actions";
  import type { Row } from "./library.svelte";
  import { openModPage, type Identity, type ModPage } from "./engine";
  import { assetName, mergeAssetName, type Conflict, type SceneDrift } from "./types";

  interface Props {
    /** the mod as the list knows it: on or off, ours or not */
    row: Row;
    /** the mod as the game's folder holds it, when it is in the game at all.
     *  A switched-off mod has none, which is why this can be null. */
    identity: Identity | null;
    page: ModPage | null;
    fetching: boolean;
    pageError: string | null;
    /** the folders this mod puts in the game */
    members: string[];

    /** everything the Actions list wants doing that names this mod */
    actions: Action[];
    run: (action: Action) => Promise<string>;

    /** contested assets this mod has a claim on */
    conflicts: Conflict[];
    /** scene findings against this mod */
    drift: SceneDrift[];
    onevidence: (conflict: Conflict | null) => void;

    busy: boolean;
    onswitch: (on: boolean) => void;
    /** open the value editor over this pane */
    onedit: () => void;
    onrevert: () => void;
    ondelete: () => void;
    onremove: () => void;
    removing: boolean;
  }

  let {
    row,
    identity,
    page,
    fetching,
    pageError,
    members,
    actions,
    run,
    conflicts,
    drift,
    onevidence,
    busy,
    onswitch,
    onedit,
    onrevert,
    ondelete,
    onremove,
    removing,
  }: Props = $props();

  let confirming = $state(false);

  // A different mod is being read: the confirmation belongs to the last one.
  // Without this, picking a hand-installed mod while another one's "remove"
  // confirmation was open showed that confirmation, already answered, for the
  // new mod. `confirmingFor` is a plain `let` rather than state -- it only
  // remembers which mod the open confirmation was for, and making it reactive
  // would have this effect re-run itself the moment it wrote to it.
  let confirmingFor: string | null = null;
  $effect(() => {
    if (confirmingFor !== row.owner) {
      confirmingFor = row.owner;
      confirming = false;
    }
  });

  /**
   * What Nexus last said about this mod.
   *
   * Read straight from the store rather than passed in, because the store is
   * already the one place that filters out verdicts about mods that have since
   * been deleted — see `Updates.#here`. A second copy of that filtering here
   * is how the nav count and the row badge came to disagree once before.
   */
  const check = $derived(updates.checks.find((c) => c.owner === row.owner) ?? null);

  /**
   * What the game itself made of this mod, last time it ran.
   *
   * This is the one fact in the record that does not come from the files on
   * disk, and it is the only one that can contradict them: a mod can be
   * present, enabled, current and uncontested, and still never be opened by
   * the game. The recorder's folder names come back matched to the real
   * folders by `sessionlog`, so this is a plain key lookup.
   */
  const session = $derived(sessions.latest);
  const trouble = $derived(
    session?.mods_in_trouble.find((m) => m.folder === row.owner) ?? null,
  );
  const neverLoaded = $derived(session?.never_loaded.includes(row.owner) ?? false);
  const ignored = $derived(
    (session?.ignored ?? []).filter((entry) => entry.folder === row.owner),
  );
  const saidSomething = $derived(trouble !== null || neverLoaded || ignored.length > 0);
</script>

<ModPageView
  identity={identity ?? {
    // A switched-off mod is not in the game, so the folder scan never saw it
    // and there is no Identity to hand over. The record still has to draw:
    // what it knows is the name and the folder, which is enough of a heading.
    owner: row.owner,
    name: row.name,
    root: "",
    mod_id: null,
    version: null,
    page: null,
    priority: null,
    disabled: !row.enabled,
    files: 0,
    assets: 0,
    managed_by: null,
  }}
  {page}
  variant={row.variant}
  {fetching}
  {pageError}
  {members}
/>

<!-- Everything the Actions list wants doing that names this mod, in the same
     cards, run by the same code. Here as well as there because the two
     questions are different: the Actions tab asks "what should I do next",
     and this asks "what is wrong with the thing I am looking at". -->
{#if actions.length}
  <h3 class="band">
    <span class="hud-label">Needs doing</span>
    <span class="note">
      {actions.length === 1 ? "this mod is named in one finding" : `this mod is named in ${actions.length} findings`}
    </span>
  </h3>
  {#each actions as action (action.id)}
    <ActionRow {action} {run} inspect={() => onevidence(conflicts.find((c) => c.target === action.target) ?? null)} />
  {/each}
{/if}

<!-- Switching one mod belongs here, beside the mod. Selecting several is a
     mode in the list, and having to enter it to turn off the one thing you
     are already looking at would be a silly walk. -->
<!-- There is exactly one way to take a mod out of the game, and which one it
     is depends on whether we put it there.

     These used to be offered side by side, described in almost the same words,
     and the pairing was not merely confusing: `remove_mod` moves files out of
     the mods folder without touching the loadout, so using it on a mod we
     manage leaves the record saying "deployed". The next reconcile -- any
     activate, any preset switch -- would see the files missing and put them
     straight back, while the moved copy sat in the removed folder for good. So
     a managed mod is deactivated, and only a mod we did not install is
     removed. -->
<section class="panel">
  <header class="hud-label">In the game</header>
  <p class="state">
    <span class="lamp" class:lit={row.active} aria-hidden="true"></span>
    {#if row.mergedInto}
      <!-- Named the way the list names it, because this sentence is an
           instruction to go and find that row. -->
      Held out of the game by the combined build
      <b>{mergeAssetName(row.mergedInto)}</b>, which carries its edits. Switch
      that off in the list to get this back.
    {:else if !row.managed}
      In the mods folder, but this program did not put it there.
    {:else if row.enabled}
      Active — the game is loading this mod.
    {:else}
      Not active — out of the game, still in your library.
    {/if}
  </p>

  {#if row.managed}
    <div class="verbs">
      <Button
        onclick={() => onswitch(!row.enabled)}
        busy={busy}
        disabled={row.mergedInto !== null}
      >
        {row.enabled ? "Deactivate" : "Activate"}
      </Button>
    </div>
    <p class="hint">
      Deactivating only takes a mod out of the game; it stays in your library,
      so bringing it back costs a relink. Deleting takes it out of the game
      <em>and</em> removes it from your library.
    </p>
  {/if}
</section>

<!-- Which build of this mod the game is reading, and the way back. Only shown
     when there *is* a way back, so an ordinary mod does not carry a control
     for undoing something that never happened. -->
<!-- Change a value this mod sets. Its own section rather than a verb inside
     "Build", because the two answer different questions: Build says whose copy
     of the mod is running, and this is the one place in the program where what
     is *inside* that copy can be changed.

     Offered for every managed mod, without first counting what it changes: the
     count costs a decompile of every asset the mod ships against the game's own
     copies, which is the editor's own opening read and not something a record
     drawn for every click should pay for. A merge is excluded because it has no
     author's copy behind it — its values come from the mods it was made of. -->
{#if row.managed && row.variant !== "merged"}
  <section class="panel">
    <header class="hud-label">Values</header>
    <p class="lede">
      {#if row.edited}
        This build carries values you set by hand. The mod as its author shipped
        it is untouched, so putting its own values back costs nothing.
      {:else}
        Every property this mod changes can be changed again — each one with the
        value the game has without it, so you can see what the mod is doing before
        you move it.
      {/if}
    </p>
    <div class="verbs">
      <Button onclick={onedit} disabled={busy}>
        {row.edited ? "Edit values" : "Change a value"}
      </Button>
    </div>
  </section>
{/if}

{#if row.managed && (row.variant || row.edited)}
  <section class="panel">
    <header class="hud-label">Build</header>
    <p class="lede">
      {#if row.variant === "cleaned"}
        This program is running a <b>cleaned</b> build: the whole game files this
        mod shipped have been reduced to the edits they actually make, so it stops
        reverting other mods.
      {:else if row.variant === "mended"}
        This program is running a <b>mended</b> build: a file the game could not
        read has been put right, so the mod actually runs.
      {:else if row.variant === "merged"}
        This is a <b>combined</b> build, made out of several mods.
      {:else}
        This program is running an <b>edited</b> build.
      {/if}
      <!-- Said separately, because it is true alongside any of the four rather
           than instead of one: a cleaned mod carrying values you set is both,
           and folding them into one sentence meant picking one to leave out. -->
      {#if row.edited && row.variant}
        It also carries values you set by hand.
      {/if}
      {#if row.variant !== "merged"}
        The copy its author shipped is untouched, so going back costs nothing but
        the relink.
      {/if}
    </p>
    {#if row.variant !== "merged"}
      <div class="verbs">
        <Button onclick={onrevert} busy={busy}>Use the author's build</Button>
      </div>
    {/if}
  </section>
{/if}

<!-- What Nexus has. One line when it is current, because "up to date" is worth
     saying and not worth a paragraph; the whole of what the Updates tab used
     to say about this mod when it is not. -->
{#if check}
  {@const said = check}
  <section class="panel">
    <header class="hud-label">Version</header>
    {#if said.state === "current"}
      <p class="state">
        <span class="lamp lit" aria-hidden="true"></span>
        Up to date{said.recorded_version ? ` — ${said.recorded_version}` : ""}.
      </p>
    {:else if said.state === "outdated"}
      <p class="headline">A newer version is on Nexus.</p>
      <p class="lede">
        You have <span class="mono">{said.recorded_version}</span>. Nexus offers
        <span class="mono">{said.latest_version}</span> &mdash; {said.latest_name}
      </p>
      <div class="verbs">
        <!-- Our own browser window, not the system one. A download clicked in
             an outside browser goes to whatever owns `nxm://` there, which is
             usually a different mod manager; clicked in here it installs. -->
        <Button variant="primary" onclick={() => void openModPage(said.page)}>
          Open on Nexus
        </Button>
      </div>
    {:else if said.state === "record_stale"}
      <p class="lede">
        Nothing to do. Your mod manager still records this as
        <span class="mono">{said.recorded_version}</span>, but the folder holds
        <span class="mono">{said.actual_version}</span>, which is current. That
        happens when an update is installed over an existing entry.
      </p>
    {:else if said.state === "withdrawn"}
      <p class="lede">
        You have <span class="mono">{said.installed_version}</span> and Nexus no
        longer offers it. The author pulled the file without publishing a
        replacement; yours still works.
      </p>
    {:else}
      <p class="lede">{said.reason}</p>
    {/if}
    <p class="hint">Checked {ago(updates.checkedAt)}.</p>
  </section>
{/if}

<!-- Which assets this mod is fighting over, and with whom. The claim stack
     itself is a wide thing and does not belong in a column, so this names the
     assets and the evidence opens over the whole window. -->
{#if conflicts.length || drift.length}
  <section class="panel">
    <header class="hud-label">Contested</header>
    <p class="lede">
      {#if conflicts.length}
        {conflicts.length === 1
          ? "One game asset is claimed by this mod and at least one other."
          : `${conflicts.length} game assets are claimed by this mod and at least one other.`}
      {:else}
        This mod replaces a scene file wholesale.
      {/if}
    </p>
    <ul class="targets">
      {#each conflicts as conflict (conflict.target)}
        <li>
          <button class="target" onclick={() => onevidence(conflict)}>
            <span class="file">{assetName(conflict.target)}</span>
            <span class="against">
              {#if conflict.predicted_winner === row.owner}
                this one wins
              {:else if conflict.predicted_winner}
                {names.of(conflict.predicted_winner)} wins
              {:else}
                no winner worked out
              {/if}
            </span>
          </button>
        </li>
      {/each}
      {#each drift as entry (entry.rel_path)}
        <li>
          <button class="target" onclick={() => onevidence(null)}>
            <span class="file">{assetName(entry.target)}</span>
            <span class="against">
              {entry.moved.length}
              {entry.moved.length === 1 ? "node moved" : "nodes moved"}
            </span>
          </button>
        </li>
      {/each}
    </ul>
  </section>
{/if}

<!-- The one section that is not about the files on disk. It can contradict
     every other section on this screen, which is exactly why it is worth
     having: a mod can be installed, enabled, current and uncontested, and
     still never be opened by the game. -->
{#if saidSomething && session}
  <section class="panel">
    <header class="hud-label">Last run</header>
    {#if neverLoaded}
      <p class="headline">The game never opened a file from this mod.</p>
      <p class="lede">
        It was in the mods folder and the game did not read any of it. That is
        usually load order — another mod claimed the same files first — or a
        folder the game has not registered yet.
      </p>
    {:else if trouble}
      <p class="headline">
        The game complained about this mod {trouble.errors +
          trouble.warnings === 1
          ? "once"
          : `${trouble.errors + trouble.warnings} times`}.
      </p>
      <p class="lede">
        {trouble.errors}
        {trouble.errors === 1 ? "error" : "errors"}, {trouble.warnings}
        {trouble.warnings === 1 ? "warning" : "warnings"}.
      </p>
      <ul class="said">
        {#each trouble.samples.slice(0, 3) as line}
          <li>{line}</li>
        {/each}
      </ul>
    {/if}

    {#if ignored.length}
      <p class="lede">
        {ignored.length}
        {ignored.length === 1 ? "file was" : "files were"} skipped because the game
        had already loaded the same asset from another mod.
      </p>
      <ul class="said">
        {#each ignored.slice(0, 3) as entry}
          <li><span class="mono">{entry.file}</span> — loaded {names.of(entry.instead)} instead</li>
        {/each}
      </ul>
    {/if}

    <p class="hint">From the session that started {session.started}.</p>
  </section>
{/if}

{#if row.managed}
  <section class="panel">
    <header class="hud-label">Delete</header>
    <p class="lede">
      Removes {row.name} from the game and from your library, and frees the disk
      it uses. You are shown exactly what goes before anything happens.
    </p>
    <div class="verbs">
      <Button variant="danger" onclick={ondelete} busy={busy}>Delete…</Button>
    </div>
  </section>
{:else}
  <section class="panel">
    <header class="hud-label">Remove</header>
    {#if !confirming}
      <p class="lede">
        This program did not install {row.name}, so there is no staged copy of it
        to switch on and off. The only way out is to move its files out of the
        mods folder — nothing is deleted, and they can be put straight back.
      </p>
      <div class="verbs">
        <Button variant="danger" onclick={() => (confirming = true)}>
          Remove from the game
        </Button>
      </div>
    {:else}
      <p class="lede">
        This will move {members.length || "its"}
        {members.length === 1 ? "item" : "items"} out of the mods folder:
      </p>
      <ul class="members">
        {#each members as member}
          <li class="path">{member}</li>
        {/each}
      </ul>
      {#if identity?.managed_by}
        <p class="warn">
          {identity.managed_by} deployed this mod. It will redeploy it unless you
          remove it there too.
        </p>
      {/if}
      <div class="verbs">
        <Button variant="danger" onclick={onremove} busy={removing}>
          {removing ? "Removing…" : "Yes, remove it"}
        </Button>
        <Button onclick={() => (confirming = false)}>Keep it</Button>
      </div>
    {/if}
  </section>
{/if}

<style>
  /* A version string, a file name: something the user will compare character
     by character. Local rather than global -- `.mono` is a short name, and a
     global one of those has silently restyled a component's own class before
     (see the note at the top of `components.css`). */
  .mono {
    font-family: var(--font-value);
    font-size: 0.95em;
    color: var(--readout);
  }

  .headline {
    margin: 0;
    font-size: var(--t-lead);
    font-weight: 600;
  }

  .state {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin: 0.3125rem 0 0;
    color: var(--dim);
    font-size: var(--t-small);
  }

  .state ~ .hint {
    margin-top: 0.75rem;
  }

  /* The same lamp the list rows carry, so "active" looks the same in both
     places. Duplicated rather than made global: `.lamp` is a short name and a
     global one of those has collided with a component's own before. */
  .lamp {
    flex: none;
    width: 0.4375rem;
    height: 0.4375rem;
    border: 1px solid var(--faint);
    border-radius: 50%;
    background: none;
  }

  .lamp.lit {
    border-color: var(--accent);
    background: var(--accent);
    box-shadow: 0 0 6px rgba(138, 224, 60, 0.6);
  }

  /* The heading over the findings, matching the bands on the Actions tab --
     they are the same cards and should not arrive under a different shape. */
  .band {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    margin: 1.25rem 0 0.5rem;
  }

  .band .note {
    color: var(--faint);
    font-size: var(--t-micro);
    font-weight: 400;
  }

  /* ---- contested ---- */

  .targets {
    list-style: none;
    margin: 0.625rem 0 0;
    padding: 0;
  }

  .targets li + li {
    margin-top: 0.25rem;
  }

  .target {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 0.75rem;
    width: 100%;
    padding: 0.4375rem 0.625rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-sm);
    background: var(--inset);
    text-align: left;
    font-size: var(--t-small);
    color: var(--readout);
    transition:
      border-color var(--quick) var(--ease),
      background var(--quick) var(--ease);
  }

  .target:hover {
    border-color: var(--cyan);
    background: var(--cyan-wash);
  }

  .target .file {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .target .against {
    flex: none;
    color: var(--faint);
    font-size: var(--t-micro);
  }

  /* ---- what the game said ---- */

  .said {
    list-style: none;
    margin: 0.5rem 0 0;
    padding: 0.4375rem 0.625rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-sm);
    background: var(--inset);
    font-size: var(--t-micro);
    color: var(--dim);
  }

  .said li + li {
    margin-top: 0.25rem;
  }

  .said li {
    word-break: break-word;
  }

  /* ---- removing a mod we did not install ---- */

  .members {
    list-style: none;
    margin: 0.5rem 0 0;
    padding: 0.4375rem 0.625rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-sm);
    background: var(--inset);
    font-size: var(--t-micro);
    color: var(--dim);
    max-height: 9rem;
    overflow-y: auto;
  }

  .members li {
    word-break: break-all;
  }
</style>
