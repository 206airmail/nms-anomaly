<script lang="ts">
  /** Named sets of mods, as a control over the one list.
   *
   * ---------------------------------------------------------------------------
   * Why this is a bar and not a tab
   * ---------------------------------------------------------------------------
   *
   * The Presets tab was a screen whose main content was *a second list of every
   * managed mod*, with a checkbox on each row. The Library already listed those
   * mods, and already switched them — from the record beside the list for one,
   * and from a select mode for several. So there were two lists of the same
   * sixty things offering the same verb in two different shapes, and they
   * disagreed: switching a mod off in one left the other showing it as on until
   * that pane happened to remount.
   *
   * A preset is not a list of mods. It is a *name for which ones are on*, which
   * is one value, and one value belongs in a control rather than a screen. The
   * list it used to duplicate is now the only list, and this sits above it.
   *
   * ---------------------------------------------------------------------------
   * What is kept from the old tab
   * ---------------------------------------------------------------------------
   *
   * Everything except the checkbox column. A preset is a list of which mods are
   * on, not a copy of them, so switching relinks only what actually changed and
   * everything stays installed — switching off is not uninstalling.
   *
   * And every switch is still previewed before it happens. The preview is a real
   * dry run against the game folder rather than a guess, so what it lists is
   * what will change — including a mod the preset names that is no longer
   * installed, which is reported rather than skipped quietly.
   */
  import { open, save } from "@tauri-apps/plugin-dialog";
  import Button from "./Button.svelte";
  import Sheet from "./Sheet.svelte";
  import { names } from "./names.svelte";
  import {
    presetsList,
    presetSave,
    presetDelete,
    presetApply,
    collectionExport,
    collectionWrite,
    collectionOpen,
    collectionPlan,
    collectionImport,
    openModPage,
    type Presets,
    type PresetSwitch,
    type Collection,
    type CollectionPlan,
    type How,
  } from "./engine";
  import { library } from "./library.svelte";

  interface Props {
    /** how many mods are switched on now, for the "save this" line */
    onCount: number;
    /** how many are managed at all: with none, a preset can say nothing */
    managedCount: number;
    /** the library changed on disk, so everything holding a copy should reread */
    onChanged: () => void;
  }

  let { onCount, managedCount, onChanged }: Props = $props();

  let presets = $state<Presets>({ presets: [], active: null });
  let busy = $state(false);
  let failed = $state<string | null>(null);

  /** The rarer verbs — saving, sharing, deleting — live behind one button. */
  let managing = $state(false);
  let newName = $state("");

  let preview = $state<{ name: string; result: PresetSwitch } | null>(null);

  async function load() {
    try {
      presets = await presetsList();
    } catch (e) {
      failed = String(e);
    }
  }

  void load();

  async function saveCurrent() {
    const name = newName.trim();
    if (!name) return;
    busy = true;
    failed = null;
    try {
      const back = await presetSave(name);
      if (back) presets = back;
      newName = "";
    } catch (e) {
      failed = String(e);
    } finally {
      busy = false;
    }
  }

  /** Work out what switching would do, and show it. Nothing is written yet. */
  async function ask(name: string) {
    busy = true;
    failed = null;
    try {
      const result = await presetApply(name, true);
      if (result) preview = { name, result };
    } catch (e) {
      failed = String(e);
    } finally {
      busy = false;
    }
  }

  async function confirm() {
    if (!preview) return;
    busy = true;
    failed = null;
    try {
      await presetApply(preview.name, false);
      preview = null;
      await load();
      onChanged();
    } catch (e) {
      failed = String(e);
    } finally {
      busy = false;
    }
  }

  async function drop(name: string) {
    busy = true;
    try {
      const back = await presetDelete(name);
      if (back) presets = back;
    } catch (e) {
      failed = String(e);
    } finally {
      busy = false;
    }
  }

  /**
   * The select is a *request*, not the switch itself.
   *
   * So it is put straight back to whatever is actually active: the preview
   * sheet is the thing that decides, and a dropdown left showing the preset
   * you were only asking about would claim a switch that has not happened.
   */
  function choose(event: Event) {
    const select = event.currentTarget as HTMLSelectElement;
    const wanted = select.value;
    select.value = presets.active ?? "";
    if (wanted) void ask(wanted);
  }

  // -- sharing a list -------------------------------------------------------
  //
  // A preset names folders in *this* install, and the folder a mod deploys
  // under is whatever its author happened to put inside the archive — so two
  // people running the same mod can have different folder names for it. Sent
  // as a preset, a list would arrive naming nothing. So exporting builds a
  // richer thing (page, title and version per mod) and importing matches on
  // whichever of those the receiving library can answer.

  let sharing = $state(false);
  let incoming = $state<{ list: Collection; plan: CollectionPlan; as: string } | null>(null);

  /** Turn a name into something safe to suggest as a file name. */
  function fileNameFor(name: string): string {
    const clean = name.replace(/[\\/:*?"<>|]/g, " ").trim();
    return `${clean || "mod list"}.nmslist.json`;
  }

  const FILTER = [{ name: "Mod list", extensions: ["json"] }];

  /** Export a preset, or — with no name — whatever is switched on now. */
  async function share(preset?: string) {
    sharing = true;
    failed = null;
    try {
      const list = await collectionExport({ preset });
      if (!list) return;
      const path = await save({
        title: "Save this mod list",
        defaultPath: fileNameFor(list.name),
        filters: FILTER,
      });
      if (!path) return;
      await collectionWrite(list, path);
    } catch (e) {
      failed = String(e);
    } finally {
      sharing = false;
    }
  }

  /**
   * Open a list someone sent, and work out what it means here.
   *
   * Nothing is written and nothing is installed: the plan is shown first,
   * because "these fourteen of yours would switch off" is a thing the user
   * should get to read before it happens rather than after.
   */
  async function receive() {
    sharing = true;
    failed = null;
    try {
      const picked = await open({
        multiple: false,
        directory: false,
        title: "Open a mod list",
        filters: FILTER,
      });
      if (typeof picked !== "string") return;
      const list = await collectionOpen(picked);
      if (!list) return;
      const plan = await collectionPlan(list);
      if (!plan) return;
      incoming = { list, plan, as: plan.name };
    } catch (e) {
      failed = String(e);
    } finally {
      sharing = false;
    }
  }

  /**
   * Keep the open list honest as mods arrive.
   *
   * Getting the missing mods is a trip to the browser and back, once per mod,
   * and the sheet stays open across all of it. Without this it would still say
   * "you do not have these" about mods installed two minutes ago.
   *
   * It re-plans rather than crossing items off locally, because "installed" is
   * a fact about the library and `collection::plan` is the only thing that
   * decides it -- it matches folder, then page, then title, and never on mod_id
   * alone. Re-implementing that here would drift from it silently.
   *
   * The dependency is the library store, which `scan()` refreshes after every
   * nxm install, so this fires on its own. `checking` is separate from `busy`
   * so a background re-plan never disables the buttons under the cursor.
   */
  let checking = $state(false);

  async function recheck() {
    if (!incoming) return;
    checking = true;
    try {
      const fresh = await collectionPlan(incoming.list);
      if (fresh && incoming) incoming.plan = fresh;
    } catch {
      // A failed re-check leaves the previous plan on screen, which is stale
      // but not wrong -- and the manual button is still there to try again.
    } finally {
      checking = false;
    }
  }

  $effect(() => {
    // Read the signals that mean "the library changed" so the effect depends
    // on them; the plan itself is deliberately not read, or writing it would
    // schedule the effect again.
    library.mods.length;
    library.book.entries.length;
    if (incoming) void recheck();
  });

  /** Save the imported list as a preset. Switching to it stays a separate act. */
  async function keepIncoming() {
    if (!incoming) return;
    busy = true;
    failed = null;
    try {
      const back = await collectionImport(incoming.list, incoming.as);
      if (back) presets = back;
      incoming = null;
    } catch (e) {
      failed = String(e);
    } finally {
      busy = false;
    }
  }

  const HOW: Record<How, string> = {
    folder: "same folder",
    page: "same Nexus page",
    title: "matched by name",
  };

  function nothingChanges(r: PresetSwitch): boolean {
    return (
      r.applied.switched_on.length === 0 &&
      r.applied.switched_off.length === 0 &&
      r.applied.missing.length === 0
    );
  }
</script>

{#if managedCount > 0}
  <div class="bar">
    <span class="hud-label">Preset</span>
    <select
      aria-label="Switch to a saved preset"
      value={presets.active ?? ""}
      onchange={choose}
      disabled={busy}
    >
      <!-- Not a preset you can pick: it is the reading when the mods that are
           on match no saved set, which is the ordinary state. -->
      <option value="">{presets.active ? "—" : "not saved"}</option>
      {#each presets.presets as preset (preset.name)}
        <option value={preset.name}>{preset.name}</option>
      {/each}
    </select>
    <button class="btn-link" onclick={() => (managing = true)}>Lists…</button>
  </div>
  {#if failed}<p class="failed">{failed}</p>{/if}
{/if}

<!-- The rare verbs. Behind a button rather than on the bar, for the same
     reason selecting several mods is a mode: saving, sharing and deleting a
     preset are things you do occasionally, and they should not each own a
     control in a column that is mostly a list of sixty mods. -->
{#if managing && !preview && !incoming}
  <Sheet title="Mod lists" onclose={() => (managing = false)}>
    <p class="hint">
      A preset records which mods are on. Switching does not uninstall anything
      &mdash; the files stay staged, so coming back costs only a relink.
    </p>

    <h4>Save what is on now ({onCount})</h4>
    <div class="make">
      <input
        type="text"
        placeholder="Name this set of mods…"
        bind:value={newName}
        onkeydown={(e) => e.key === "Enter" && saveCurrent()}
      />
      <Button variant="primary" onclick={saveCurrent} disabled={busy || !newName.trim()}>
        Save
      </Button>
    </div>

    {#if presets.presets.length}
      <h4>Saved</h4>
      {#each presets.presets as preset (preset.name)}
        <div class="preset" class:on={presets.active === preset.name}>
          <div class="who">
            <strong>{preset.name}</strong>
            <span>{preset.enabled.length} mods</span>
            {#if presets.active === preset.name}<span class="tag">active</span>{/if}
          </div>
          <Button
            onclick={() => {
              managing = false;
              void ask(preset.name);
            }}
            disabled={busy}
          >
            Switch to
          </Button>
          <Button
            onclick={() => share(preset.name)}
            disabled={busy || sharing}
            title="Save this list to a file you can send to another player"
          >
            Share
          </Button>
          <Button variant="danger" onclick={() => drop(preset.name)} disabled={busy}>
            Delete
          </Button>
        </div>
      {/each}
    {:else}
      <p class="hint">None saved yet.</p>
    {/if}

    <h4>Send one to another player</h4>
    <p class="hint">
      A list is a file you can send. It carries each mod's Nexus page, title and
      version &mdash; not its files and none of your settings &mdash; so it works
      even though your mod folders are named differently from theirs. Importing
      saves the list as a preset here and tells you which mods you are missing.
      It never installs, switches or deletes anything on its own.
    </p>
    <div class="verbs">
      <Button onclick={() => share()} disabled={busy || sharing || onCount === 0}>
        {sharing ? "Working…" : `Export what is on now (${onCount})`}
      </Button>
      <Button variant="primary" onclick={receive} disabled={busy || sharing}>
        Import a list…
      </Button>
    </div>

    {#if failed}<p class="failed">{failed}</p>{/if}

    {#snippet verbs()}
      <Button onclick={() => (managing = false)}>Done</Button>
    {/snippet}
  </Sheet>
{/if}

{#if incoming}
  {@const plan = incoming.plan}
  <Sheet title="“{plan.name}”" onclose={() => (incoming = null)}>
    <p class="hint">
      <strong>{plan.have.length}</strong> of {plan.have.length + plan.missing.length}
      mods on this list are installed here.
      {#if plan.exported}Exported {plan.exported}.{/if}
      <!-- The automatic re-check covers installs this program did. One done by
           hand, or in another manager, needs asking. -->
      <Button variant="link" onclick={recheck} disabled={checking}>
        {checking ? "Checking…" : "Check again"}
      </Button>
    </p>
    {#if plan.note}<p class="warn">{plan.note}</p>{/if}

    {#if plan.missing.length}
      <!-- First, because it is the only part that needs the user to go and do
           something. A free Nexus account cannot be handed a download by the
           API, so these open the page rather than pretending to install. -->
      <h4 class="gone">You do not have these ({plan.missing.length})</h4>
      <p class="hint">
        They go on the preset anyway, so it completes itself as you install
        them &mdash; and it says they are missing every time you switch to it
        until you have.
      </p>
      <ul class="gone getlist">
        {#each plan.missing as one (one.owner)}
          <li>
            <span class="lamp" aria-hidden="true"></span>
            <span>{one.name ?? one.owner}</span>
            {#if one.version}<em>{one.version}</em>{/if}
            {#if one.mod_id}
              <Button
                variant="link"
                onclick={() =>
                  openModPage(`https://www.nexusmods.com/nomanssky/mods/${one.mod_id}`)}
              >
                Open on Nexus
              </Button>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}

    {#if plan.have.length}
      <h4>You already have these ({plan.have.length})</h4>
      <ul class="getlist">
        {#each plan.have as one (one.owner)}
          <li>
            <span class="lamp lit" aria-hidden="true"></span>
            <span>{one.name}</span>
            {#if one.differs}
              <em class="differs">
                they have {one.listed.version}, you have {one.version}
              </em>
            {:else if one.how !== "folder"}
              <em>{HOW[one.how]}</em>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}

    {#if plan.extra.length}
      <!-- Named rather than counted. Switching to this list turns these off,
           and a mod going quiet without being named is the failure this
           program exists to prevent. -->
      <h4 class="gone">Yours, not on this list ({plan.extra.length})</h4>
      <p class="hint">
        Switching to this list later would turn these off. Saving it now
        changes nothing.
      </p>
      <ul class="gone">
        {#each plan.extra as name}<li>{name}</li>{/each}
      </ul>
    {/if}

    <h4>Save it as</h4>
    <div class="make">
      <input type="text" bind:value={incoming.as} placeholder="Name this list…" />
    </div>

    {#snippet verbs()}
      <Button onclick={() => (incoming = null)}>Cancel</Button>
      <Button
        variant="primary"
        onclick={keepIncoming}
        busy={busy}
        disabled={!incoming?.as.trim()}
      >
        Save as preset
      </Button>
    {/snippet}
  </Sheet>
{/if}

{#if preview}
  {@const ahead = preview.result}
  <Sheet title="Switch to “{preview.name}”?" onclose={() => (preview = null)}>
    {#if nothingChanges(ahead)}
      <p class="hint">
        Everything is already as this preset wants it. Nothing would change.
      </p>
    {:else}
      {#if ahead.applied.switched_on.length}
        <h4>Switching on ({ahead.applied.switched_on.length})</h4>
        <ul>
          {#each ahead.applied.switched_on as owner}<li>{names.of(owner)}</li>{/each}
        </ul>
      {/if}
      {#if ahead.applied.switched_off.length}
        <h4>Switching off ({ahead.applied.switched_off.length})</h4>
        <ul>
          {#each ahead.applied.switched_off as owner}<li>{names.of(owner)}</li>{/each}
        </ul>
      {/if}
      {#if ahead.applied.missing.length}
        <h4 class="gone">Not installed any more</h4>
        <p class="hint">
          This preset names mods that are no longer installed. They will simply
          be left out.
        </p>
        <ul class="gone">
          {#each ahead.applied.missing as owner}<li>{names.of(owner)}</li>{/each}
        </ul>
      {/if}
      {#if ahead.changes.problems.length}
        <h4 class="gone">Problems</h4>
        <ul class="gone">
          {#each ahead.changes.problems as p}<li>{p}</li>{/each}
        </ul>
      {/if}
    {/if}

    {#snippet verbs()}
      <Button onclick={() => (preview = null)}>Cancel</Button>
      <Button variant="primary" onclick={confirm} busy={busy}>
        {nothingChanges(ahead) ? "Mark as active" : "Switch"}
      </Button>
    {/snippet}
  </Sheet>
{/if}

<style>
  /* One line, above the search box. It is a readout of one value -- which set
     of mods is on -- with the way to change it, so it gets the same shape as
     the chips beneath it rather than a panel of its own. */
  .bar {
    display: flex;
    align-items: center;
    gap: 0.4375rem;
    margin-bottom: 0.5rem;
    padding-bottom: 0.5rem;
    border-bottom: 1px solid var(--rule);
  }

  .bar select {
    flex: 1;
    min-width: 0;
    padding: 0.25rem 0.375rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-sm);
    background: var(--inset);
    color: var(--readout);
    font: inherit;
    font-size: var(--t-micro);
  }

  .bar .btn-link {
    flex: none;
    font-size: var(--t-micro);
  }

  /* ---- inside the sheet ---- */

  h4 {
    margin: 1rem 0 0.375rem;
    font-size: var(--t-small);
    font-weight: 600;
    color: var(--readout);
  }

  h4.gone {
    color: var(--signal);
  }

  .make {
    display: flex;
    gap: 0.5rem;
  }

  .make input {
    flex: 1;
    min-width: 0;
    padding: 0.375rem 0.5625rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-sm);
    background: var(--inset);
    color: var(--readout);
    font: inherit;
    font-size: var(--t-small);
  }

  .preset {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.5rem 0.625rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-sm);
    background: var(--inset);
  }

  .preset + .preset {
    margin-top: 0.375rem;
  }

  .preset.on {
    border-color: var(--accent);
  }

  .preset .who {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    flex: 1;
    min-width: 0;
  }

  .preset .who strong {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 500;
  }

  .preset .who span {
    flex: none;
    color: var(--faint);
    font-size: var(--t-micro);
  }

  .preset .tag {
    padding: 0.05rem 0.3125rem;
    border-radius: var(--r-pill);
    background: var(--accent-wash);
    color: var(--accent);
    font-size: 0.625rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  ul {
    margin: 0.25rem 0 0;
    padding-left: 1.1rem;
    font-size: var(--t-small);
    color: var(--dim);
  }

  ul.gone {
    color: var(--signal);
  }

  .getlist {
    list-style: none;
    padding-left: 0;
  }

  /* The Library's lamp, duplicated rather than made global -- `.lamp` is a
     short name and a global one of those has collided with a component's own
     before. Hollow for "not here yet", filled for "installed", which is the
     same sense it carries in the mod list: an unlit lamp is a real state, not
     a missing value. */
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

  .getlist li {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    padding: 0.1875rem 0;
  }

  .getlist em {
    color: var(--faint);
    font-size: var(--t-micro);
    font-style: normal;
  }

  .getlist em.differs {
    color: var(--signal);
  }
</style>
