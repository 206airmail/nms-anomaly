<script lang="ts">
  /** Your mods: the list, the record, and getting more.
   *
   * ---------------------------------------------------------------------------
   * What this pane is now
   * ---------------------------------------------------------------------------
   *
   * Five tabs' worth of screen. The Library listed every mod; Presets listed
   * every managed mod again with a checkbox on each row; Updates listed the
   * ones Nexus had something to say about; Browse was a separate destination
   * for finding more; and a mod's conflicts lived on a sixth tab of their own.
   *
   * None of those were screens. They were one list and several *questions*
   * about the things in it, and this pane is the list plus the answers:
   *
   *   [`ModList`]    one list, with the old tabs as filter chips over it
   *   [`ModRecord`]  everything known about the one mod being read
   *   [`PresetBar`]  which set of mods is on, as a control rather than a tab
   *   [`BrowsePane`] finding more, as the other of two modes rather than a peer
   *
   * ---------------------------------------------------------------------------
   * Two things worth knowing before changing it
   * ---------------------------------------------------------------------------
   *
   * **A mod's folder is not its name.** Rows show `names.of(owner)` — the title
   * from its Nexus page, else the one baked into the archive, else the folder.
   * Everything the engine is *asked* to do still travels by `owner`, because
   * that is what exists on disk.
   *
   * **This pane owns no copy of the library.** It reads [`library`], which the
   * app loads after every scan. Three screens used to fetch the mod list
   * separately and they disagreed with each other; see the note on that store.
   */
  import { open } from "@tauri-apps/plugin-dialog";
  import BrowsePane from "./BrowsePane.svelte";
  import Button from "./Button.svelte";
  import ModList from "./ModList.svelte";
  import ModRecord from "./ModRecord.svelte";
  import ValueEditor from "./ValueEditor.svelte";
  import PresetBar from "./PresetBar.svelte";
  import Sheet from "./Sheet.svelte";
  import { installs } from "./installs.svelte";
  import { library, type Batch } from "./library.svelte";
  import { names } from "./names.svelte";
  import { updates } from "./updates.svelte";
  import { unmergedNote, type Conflict, type SceneDrift } from "./types";
  import type { Action } from "./actions";
  import {
    installPreview,
    modMembers,
    nexusMod,
    removeMod,
    restoreMod,
    repairUndo,
    restoreOriginals,
    setModsEnabled,
    deletePreview,
    deleteMods,
    humanBytes,
    type InstallPlan,
    type ModPage,
    type Removal,
    type DeletePlan,
  } from "./engine";

  interface Props {
    modsDir?: string;
    gameRoot?: string;
    /** the app rescans after anything here changes the mods folder */
    onChanged?: () => void;
    /** false while the user is on another tab, so the webview is parked */
    visible?: boolean;

    /**
     * Installed or finding more.
     *
     * Bindable because the app has to be able to set it: a download started
     * from a Nexus page in any window brings this pane forward, and a mod page
     * asked for from anywhere at all opens the browser — both of which mean
     * putting this pane in the right mode from outside.
     */
    mode?: "installed" | "find";

    /** everything the Actions list wants doing, so a row and a record can say
     *  which mods are named in it */
    actions?: Action[];
    run: (action: Action) => Promise<string>;

    conflicts?: Conflict[];
    drift?: SceneDrift[];
    /** open the evidence over this pane, at a particular asset when there is one */
    onevidence: (conflict: Conflict | null) => void;

    /**
     * A mod some other screen wants read, and the request's number.
     *
     * A counter rather than a bare folder name, because asking twice for the
     * same mod has to work: clicking a name in Sessions, coming back here,
     * scrolling away and clicking the same name again is not a no-op. The
     * counter changes every time even when the name does not.
     */
    wanted?: { owner: string; at: number } | null;

    /** set to open the mod-lists sheet; forwarded to [`PresetBar`], which
     *  clears it through `onListsOpened` once the sheet is up */
    openLists?: boolean;
    onListsOpened?: () => void;
  }

  let {
    modsDir,
    gameRoot,
    onChanged,
    visible = true,
    mode = $bindable("installed"),
    actions = [],
    run,
    conflicts = [],
    drift = [],
    onevidence,
    wanted = null,
    openLists = false,
    onListsOpened,
  }: Props = $props();

  /** The mod being read. One selection, and a click is all it takes. */
  let picked = $state<string | null>(null);

  /**
   * The mod whose values are open for editing, when one is.
   *
   * Over this pane rather than beside the record, for the same reason the
   * evidence is: a property, a box and the game's own value is a wide row, and
   * a hundred of them squeezed into the record's column stop being a column of
   * values to compare at all.
   */
  let editing = $state<string | null>(null);

  // A request from another screen. `served` is a plain `let`, not state: it
  // exists only to remember which request has been answered, and making it
  // reactive would have this effect re-run itself the moment it wrote to it.
  let served = 0;
  $effect(() => {
    if (wanted && wanted.at !== served) {
      served = wanted.at;
      multi = false;
      void show(wanted.owner);
    }
  });
  const selected = $derived(library.row(picked));
  const identity = $derived(library.identity(picked ?? ""));

  // The Nexus page for whichever mod is selected, fetched once and kept.
  let pages = $state<Record<number, ModPage>>({});
  let pageError = $state<string | null>(null);
  let fetching = $state(false);
  let members = $state<string[]>([]);

  let working = $state(false);
  let lastRemoval = $state<Removal | null>(null);
  let actionError = $state<string | null>(null);

  let plan = $state<InstallPlan | null>(null);
  let planFor = $state<string | null>(null);
  let installing = $state(false);

  /** true while the list is picking several mods rather than one */
  let multi = $state(false);
  /** the mods picked in that mode. Meaningless, and empty, outside it. */
  let chosen = $state<Set<string>>(new Set());
  let batching = $state(false);
  let batchError = $state<string | null>(null);
  /**
   * Something that happened to the library that nobody asked for.
   *
   * Deleting a mod takes any combined build made out of it, because that build
   * still holds the deleted mod's edits. Not an error — it is the right thing —
   * but the user would otherwise just find mods back in the game and a combined
   * one gone, with nothing having said so.
   */
  let batchNote = $state<string | null>(null);
  let doomed = $state<DeletePlan[] | null>(null);
  let withArchive = $state(false);

  /** Folders with a newer version on Nexus, so a row can say so. */
  const outdated = $derived(new Set(updates.outdated.map((c) => c.owner)));

  /**
   * Mods named in something worth doing about them.
   *
   * Tidying is left out deliberately. Every cleanable mod is named in a `tidy`
   * action, which on this library is most of them, so a chip counting those
   * would read "Needs attention 27" over a library where nothing is wrong —
   * and a filter that matches almost everything is not a filter.
   */
  const attention = $derived(
    new Set(actions.filter((a) => a.urgency !== "tidy").flatMap((a) => a.mods)),
  );

  /** The findings that name the mod being read, in the same order they rank. */
  const forSelected = $derived.by(() => {
    const owner = picked;
    return owner ? actions.filter((a) => a.mods.includes(owner)) : [];
  });
  const contested = $derived.by(() => {
    const owner = picked;
    return owner ? conflicts.filter((c) => c.mods.includes(owner)) : [];
  });
  const misplaced = $derived.by(() => {
    const owner = picked;
    return owner ? drift.filter((d) => d.mod === owner) : [];
  });

  /** For the preset bar, which reports on the set rather than on any one mod. */
  const managedCount = $derived(library.rows.filter((r) => r.managed).length);
  const onCount = $derived(library.rows.filter((r) => r.managed && r.enabled).length);

  /** Re-read the library, then let the app rescan behind it. */
  async function settle() {
    await library.load(modsDir);
    // Anything deleted elsewhere should not stay picked.
    const alive = new Set(library.rows.map((r) => r.owner));
    if (chosen.size) chosen = new Set([...chosen].filter((o) => alive.has(o)));
    if (picked && !alive.has(picked)) picked = null;
    onChanged?.();
  }

  async function show(owner: string) {
    picked = owner;
    actionError = null;
    pageError = null;
    members = [];

    modMembers(owner, modsDir)
      .then((found) => {
        if (picked === owner) members = found;
      })
      .catch(() => {});

    // Only a mod with files in the game has an Identity, and only an Identity
    // carries the Nexus id. A switched-off mod simply has no page to show.
    const modId = library.identity(owner)?.mod_id ?? null;
    if (modId === null || pages[modId]) return;
    fetching = true;
    try {
      const page = await nexusMod(modId);
      if (page) pages[modId] = page;
    } catch (err) {
      pageError = String(err);
    } finally {
      fetching = false;
    }
  }

  // ---------------------------------------------------------------------
  // Switching, reverting and deleting
  //
  // One verb each, whether it came from the record beside the list or from the
  // batch bar underneath it: the batch bar hands over a list of owners and the
  // record hands over a list of one, so there is no pair of nearly-identical
  // functions to drift apart.
  // ---------------------------------------------------------------------

  async function switchMods(owners: string[], on: boolean) {
    if (!owners.length) return;
    batching = true;
    batchError = null;
    try {
      // One call, so the mods folder is reconciled once rather than per mod.
      await setModsEnabled(owners, on, modsDir);
      await settle();
    } catch (err) {
      batchError = String(err);
    } finally {
      batching = false;
    }
  }

  /**
   * Put mods back to the build their author shipped.
   *
   * One call, one reconcile — the same reason `switchMods` batches. Doing it
   * one mod at a time was once the only way there was, which on a library
   * where everything cleanable has been cleaned is twenty-seven clicks and
   * twenty-seven reconciles to get back to stock.
   */
  async function revert(owners: string[]) {
    if (!owners.length) return;
    batching = true;
    batchError = null;
    batchNote = null;
    try {
      const done = await restoreOriginals(owners, modsDir);
      if (done) {
        const parts: string[] = [];
        if (done.reverted.length) {
          parts.push(`${done.reverted.length} put back to the author's build`);
        }
        if (done.dissolved.length) {
          // Named, because dissolving a merge changes what is in the game for
          // the mods it stood in for, which is more than the person asked about.
          parts.push(
            `${done.dissolved.length === 1 ? "1 merge" : `${done.dissolved.length} merges`} taken apart: ${done.dissolved.map((o) => names.of(o)).join(", ")}`,
          );
        }
        batchNote = parts.length ? `${parts.join(" · ")}.` : "Nothing needed putting back.";
        if (done.problems.length) batchError = done.problems.join(" · ");
      }
      await settle();
    } catch (err) {
      batchError = String(err);
    } finally {
      batching = false;
    }
  }

  /**
   * Put a mended mod back to the build its author shipped.
   *
   * A mend is not a prune, so the clean preview cannot plan its undo and
   * `restore_originals` is the only thing that knows how — which is why this
   * is a call of its own rather than a case inside `revert`.
   */
  async function undoRepair(owner: string) {
    batching = true;
    batchError = null;
    batchNote = null;
    try {
      await repairUndo(owner, modsDir);
      batchNote = `${names.of(owner)} is back to the build its author shipped — the game cannot read one of its files again, so it will show up as a finding.`;
      await settle();
    } catch (err) {
      batchError = String(err);
    } finally {
      batching = false;
    }
  }

  /** What the list's batch bar asked for, performed here. */
  function batch(verb: Batch, owners: string[]) {
    switch (verb) {
      case "activate":
        void switchMods(owners, true);
        return;
      case "deactivate":
        void switchMods(owners, false);
        return;
      case "revert":
        void revert(owners);
        return;
      case "delete":
        void priceDelete(owners);
        return;
    }
  }

  /** The record's Build panel: one mod, and which undo it needs. */
  function revertOne(owner: string) {
    const row = library.row(owner);
    if (row?.variant === "mended") void undoRepair(owner);
    else void revert([owner]);
  }

  /**
   * Work out what deleting these would remove, and show it.
   *
   * Kept as the only way `doomed` is ever set, so the sheet cannot be opened
   * about a set of mods that was never priced — `repriceDelete` reruns it when
   * the download answer changes, and it has to reprice the *same* mods rather
   * than whatever happens to be selected now.
   */
  let pricing = $state<string[]>([]);
  async function priceDelete(owners: string[]) {
    if (owners.length === 0) return;
    pricing = owners;
    batching = true;
    batchError = null;
    try {
      doomed = await deletePreview(owners, withArchive, modsDir, gameRoot);
    } catch (err) {
      batchError = String(err);
    } finally {
      batching = false;
    }
  }

  /**
   * Re-price the deletion when the download answer changes.
   *
   * The box states the destructive act and starts unchecked, rather than
   * stating the safe one and starting checked. The default behaviour is the
   * same either way -- the download is kept -- but "Keep the downloads",
   * pre-ticked, put the cautious choice behind an action the user had to take,
   * and a box you untick to delete something reads as permission rather than
   * as a decision.
   */
  async function repriceDelete(alsoDelete: boolean) {
    withArchive = alsoDelete;
    if (doomed) await priceDelete(pricing);
  }

  async function reallyDelete() {
    if (!doomed) return;
    batching = true;
    batchError = null;
    batchNote = null;
    try {
      const done = await deleteMods(
        doomed.map((d) => d.owner),
        withArchive,
        modsDir,
        gameRoot,
      );
      const trouble = done.flatMap((d) => d.problems);
      batchNote = unmergedNote(
        done.flatMap((d) => d.dissolved),
        "a mod you have just deleted",
      );
      doomed = null;
      chosen = new Set();
      multi = false;
      if (picked && done.some((d) => d.owner === picked)) picked = null;
      await settle();
      if (trouble.length) batchError = trouble.join("   ·   ");
    } catch (err) {
      batchError = String(err);
    } finally {
      batching = false;
    }
  }

  const doomedBytes = $derived((doomed ?? []).reduce((n, d) => n + d.bytes, 0));

  // ---------------------------------------------------------------------
  // Mods we did not install, and mods arriving
  // ---------------------------------------------------------------------

  async function remove() {
    if (!picked) return;
    working = true;
    actionError = null;
    try {
      lastRemoval = await removeMod(picked, modsDir);
      picked = null;
      await settle();
    } catch (err) {
      actionError = String(err);
    } finally {
      working = false;
    }
  }

  async function undoRemoval() {
    if (!lastRemoval) return;
    working = true;
    actionError = null;
    try {
      await restoreMod(lastRemoval.trash, modsDir);
      lastRemoval = null;
      await settle();
    } catch (err) {
      actionError = String(err);
    } finally {
      working = false;
    }
  }

  async function pickArchive() {
    actionError = null;
    // The picker is inside the `try` as well. It was outside it, so a dialog
    // that refused to open rejected into nothing at all -- the button would
    // have looked broken rather than failed, which is the one outcome this
    // pane must never produce.
    try {
      const file = await open({
        multiple: false,
        filters: [{ name: "Mod archive", extensions: ["zip", "rar", "7z"] }],
      });
      if (typeof file !== "string") return;
      planFor = file;
      plan = null;
      plan = await installPreview(file, modsDir);
      if (!plan) {
        // Only reachable outside Tauri, where the engine cannot be called.
        // Still said, rather than leaving the pane blank after a pick.
        planFor = null;
        actionError = "the engine is not available, so nothing can be installed here";
      }
    } catch (err) {
      actionError = String(err);
      planFor = null;
      plan = null;
    }
  }

  async function confirmInstall() {
    if (!planFor) return;
    installing = true;
    actionError = null;
    const archive = planFor;
    const replacing = plan?.collides ?? false;
    // Clear the preview straight away: the strip at the bottom of the window
    // is where progress is reported from here, and it outlives this tab.
    plan = null;
    planFor = null;
    try {
      await installs.fromFile(archive, modsDir, gameRoot, replacing);
      await settle();
    } catch (err) {
      // Said here as well as in the strip along the bottom of the window.
      // The strip is a few pixels high and the install was started from this
      // pane, with the eye on this pane -- a failure reported only down there
      // reads as the button having done nothing at all.
      actionError = String(err);
    } finally {
      installing = false;
    }
  }

  const fileName = $derived(planFor?.split(/[\\/]/).pop() ?? "");

  const MODES: { id: "installed" | "find"; name: string; hint: string }[] = [
    { id: "installed", name: "Installed", hint: "The mods you have" },
    { id: "find", name: "Find more", hint: "Nexus, in a window that installs what you download" },
  ];
</script>

<div class="library">
  <!-- Two modes of one pane, not two tabs. Finding a mod and managing the ones
       you have are the same activity from the user's side -- you arrive at the
       second by finishing the first -- and a mod page opened from anywhere in
       the app lands here either way. -->
  <div class="modes" role="group" aria-label="Installed mods, or find more">
    {#each MODES as item (item.id)}
      <button
        class:on={mode === item.id}
        title={item.hint}
        aria-pressed={mode === item.id}
        onclick={() => (mode = item.id)}
      >
        {item.name}
      </button>
    {/each}
  </div>

  {#if mode === "find"}
    <BrowsePane
      installed={library.rows.map((r) => r.owner)}
      visible={visible && mode === "find"}
    />
  {:else}
    <div class="split">
      <ModList
        rows={library.rows}
        loading={library.loading}
        error={library.error}
        {picked}
        onpick={(owner) => void show(owner)}
        {attention}
        {outdated}
        bind:multi
        bind:chosen
        {batching}
        {batchError}
        {batchNote}
        onbatch={batch}
        onadd={pickArchive}
        adding={installing}
      >
        {#snippet header()}
          <PresetBar {onCount} {managedCount} {openLists} {onListsOpened} onChanged={settle} />
        {/snippet}
      </ModList>

      <div class="pane">
        {#if lastRemoval}
          <section class="panel">
            <p class="headline">
              {names.of(lastRemoval.owner)} is out of the mods folder.
            </p>
            <p class="lede">
              {lastRemoval.moved.length}
              {lastRemoval.moved.length === 1 ? "item" : "items"} moved, nothing deleted.
            </p>
            <p class="wrote path">{lastRemoval.trash}</p>
            {#if lastRemoval.managed_by}
              <p class="warn">
                {lastRemoval.managed_by} deployed this mod and still thinks it owns
                it. It will put the files back on its next deploy unless you remove
                the mod there too.
              </p>
            {/if}
            <div class="verbs">
              <Button onclick={undoRemoval} busy={working}>
                {working ? "Putting it back…" : "Put it back"}
              </Button>
            </div>
          </section>
        {/if}

        {#if plan && planFor}
          <section class="panel">
            <header class="hud-label">Install</header>
            <h2>{plan.owner}</h2>
            <p class="lede">
              {fileName} &mdash; {plan.files}
              {plan.files === 1 ? "file" : "files"}
            </p>
            {#each plan.notes as note}
              <p class="warn">{note}</p>
            {/each}
            {#if plan.collides}
              <!-- Not "a folder called X": this is true of a mod the loadout
                   records even when its folder is gone from the game, and that
                   is the case worth being right about -- it is exactly when
                   the user would go looking in the mods folder and find
                   nothing there. -->
              <p class="warn">
                {plan.owner} is already installed. Installing will replace it,
                including anything the new version no longer ships.
              </p>
            {/if}
            <div class="verbs">
              <Button variant="primary" onclick={confirmInstall} busy={installing}>
                {installing ? "Installing…" : plan.collides ? "Replace it" : "Install it"}
              </Button>
              <Button
                onclick={() => {
                  plan = null;
                  planFor = null;
                }}
              >
                Cancel
              </Button>
            </div>
          </section>
        {/if}

        {#if actionError}
          <p class="failed">{actionError}</p>
        {/if}

        {#if !selected}
          {#if !plan && !lastRemoval}
            <section class="panel">
              <header class="hud-label">Library</header>
              <!-- Never state a count we have not counted yet: "0 mods
                   installed" while the scan is still running reads as an empty
                   library. -->
              <h2>
                {library.loading
                  ? "Reading the mods folder…"
                  : `${library.rows.length} mods installed, ${library.activeCount} in the game.`}
              </h2>
              <p class="lede">
                Pick one to see its page, whether the game is loading it, what
                Nexus has, what it is fighting over and what the game made of it
                last run. To switch or delete several at once, press
                <b>Select</b>.
              </p>
            </section>
          {/if}
        {:else}
          <ModRecord
            row={selected}
            {identity}
            page={identity?.mod_id != null ? (pages[identity.mod_id] ?? null) : null}
            {fetching}
            {pageError}
            {members}
            actions={forSelected}
            {run}
            conflicts={contested}
            drift={misplaced}
            {onevidence}
            busy={batching}
            onswitch={(on) => void switchMods([selected.owner], on)}
            onedit={() => (editing = selected.owner)}
            onrevert={() => revertOne(selected.owner)}
            ondelete={() => void priceDelete([selected.owner])}
            onremove={remove}
            removing={working}
          />
        {/if}
      </div>
    </div>
  {/if}
</div>

<!-- Changing what a mod sets. A wide sheet, and no `verbs` snippet: the editor
     owns its own foot, because which buttons belong there depends on how many
     boxes have been typed in and only it knows that. -->
{#if editing}
  {@const who = editing}
  <Sheet wide title={`Values — ${names.of(who)}`} onclose={() => (editing = null)}>
    <ValueEditor
      owner={who}
      name={names.of(who)}
      {modsDir}
      onchanged={() => {
        void library.load(modsDir);
        onChanged?.();
      }}
      onclose={() => (editing = null)}
    />
  </Sheet>
{/if}

{#if doomed}
  {@const gone = doomed}
  <Sheet
    title={gone.length === 1
      ? `Delete ${names.of(gone[0].owner)}?`
      : `Delete ${gone.length} mods?`}
    danger
    onclose={() => (doomed = null)}
  >
    <p class="lede alarm">
      Takes {gone.length === 1 ? "it" : "them"} out of the game
      <em>and</em> deletes
      {gone.length === 1 ? "it" : "them"} from your library. Deactivating instead
      would keep {gone.length === 1 ? "it" : "them"} on disk. This cannot be undone.
    </p>

    <p class="freeing">Frees {humanBytes(doomedBytes)}</p>

    <!-- A mod we have cleaned or mended exists twice: the copy its author
         shipped and the build we made from it. Both go -- leaving the base
         behind would be a mod nobody asked for, in a folder nobody looks at. -->
    {#if gone.some((d) => d.items.some((i) => i.what === "derived"))}
      <p class="hint">
        Includes both the copy its author shipped and the fixed build made from
        it.
      </p>
    {/if}

    <ul class="what">
      {#each gone as d}
        <li>
          <strong>{names.of(d.owner)}</strong>
          {#if d.items.length === 0}
            <em>nothing left on disk — only the record goes</em>
          {/if}
          {#each d.refused as path}
            <em class="left">left alone, outside the managed folders: {path}</em>
          {/each}
        </li>
      {/each}
    </ul>

    <!-- The four-layer vocabulary -- staged, derived, archive -- is this
         program's own bookkeeping. It is here for anyone who wants to check the
         arithmetic, and folded away because nobody deciding whether to delete a
         mod needs it. -->
    <details class="breakdown">
      <summary>What exactly gets removed</summary>
      <ul>
        {#each gone as d}
          {#each d.items as item}
            <li class="path">
              {item.what} · {item.files} files · {humanBytes(item.bytes)}
              <span class="where">{item.path}</span>
            </li>
          {/each}
        {/each}
      </ul>
    </details>

    <label class="keep">
      <input
        type="checkbox"
        checked={withArchive}
        onchange={(e) => repriceDelete(e.currentTarget.checked)}
      />
      <span>
        <strong>Delete the downloads too</strong>
        <em>
          Off by default: the archives are kept, so reinstalling later costs no
          bandwidth. Tick this to reclaim that space as well.
        </em>
      </span>
    </label>

    {#snippet verbs()}
      <Button onclick={() => (doomed = null)}>Cancel</Button>
      <Button variant="danger" onclick={reallyDelete} busy={batching}>
        {batching ? "Deleting…" : "Delete for good"}
      </Button>
    {/snippet}
  </Sheet>
{/if}

<style>
  .library {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }

  /* Two modes of one pane. A segmented pair rather than two nav items,
     because they are alternatives to each other and not to Actions or
     Settings — the shape says which choice is being made. */
  .modes {
    display: flex;
    gap: 0.25rem;
    flex: none;
    padding: 0.5rem var(--pad);
    border-bottom: 1px solid var(--rule);
  }

  .modes button {
    padding: 0.25rem 0.75rem;
    border: 1px solid transparent;
    border-radius: var(--r-pill);
    color: var(--faint);
    font-size: var(--t-small);
    transition:
      color var(--quick) var(--ease),
      border-color var(--quick) var(--ease),
      background var(--quick) var(--ease);
  }

  .modes button:hover {
    color: var(--readout);
    background: var(--glass-2);
  }

  .modes button.on {
    border-color: var(--rule-hi);
    background: var(--glass-hi);
    color: var(--readout);
  }

  .split {
    display: grid;
    grid-template-columns: 21rem minmax(0, 1fr);
    flex: 1;
    min-height: 0;
  }

  /* ---- the record side ---- */

  .pane {
    padding: var(--pad);
    overflow-y: auto;
  }

  h2 {
    margin: 0.25rem 0 0;
    font-size: var(--t-title);
    font-weight: 600;
    letter-spacing: var(--track-tighter);
  }

  .headline {
    margin: 0;
    font-size: var(--t-lead);
    font-weight: 600;
  }

  .wrote {
    margin: 0.5rem 0 0;
    padding: 0.375rem 0.5625rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-sm);
    background: var(--inset);
    font-size: var(--t-micro);
    color: var(--dim);
    word-break: break-all;
  }

  /* ---- the delete sheet ---- */

  .alarm {
    color: var(--contest);
  }

  .freeing {
    margin: 0.875rem 0 0;
    font-family: var(--font-value);
    font-size: var(--t-lead);
    color: var(--accent);
  }

  .what {
    margin: 0.625rem 0 0;
    padding-left: 1.1rem;
    font-size: var(--t-small);
    color: var(--dim);
  }

  .what strong {
    color: var(--readout);
    font-weight: 500;
  }

  .what em {
    display: block;
    font-size: var(--t-micro);
    font-style: normal;
    color: var(--faint);
  }

  .what em.left {
    color: var(--signal);
  }

  .breakdown {
    margin: 0.875rem 0 0;
    font-size: var(--t-micro);
  }

  .breakdown summary {
    color: var(--faint);
    cursor: pointer;
  }

  .breakdown ul {
    margin: 0.5rem 0 0;
    padding: 0.4375rem 0.625rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-sm);
    background: var(--inset);
    list-style: none;
    color: var(--dim);
  }

  .breakdown .where {
    display: block;
    color: var(--faint);
    word-break: break-all;
  }

  .keep {
    display: flex;
    gap: 0.6rem;
    margin-top: 0.875rem;
    padding: 0.7rem 0 0;
    border-top: 1px solid var(--rule);
    cursor: pointer;
  }

  .keep span {
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
  }

  .keep strong {
    color: var(--readout);
    font-size: var(--t-body);
    font-weight: 500;
  }

  .keep em {
    color: var(--faint);
    font-size: var(--t-micro);
    font-style: normal;
  }
</style>
