<script lang="ts">
  /** Where everything is, and what the program does on its own.
   *
   * Two things this screen tries to do that a plain form would not.
   *
   * It shows the *consequence* of every path, not just the path: whether it
   * was detected or chosen, whether it is actually there, and — for staging
   * — whether it is on the game's drive, because the wrong drive silently
   * turns hardlinks into copies and doubles the disk used. Saving re-resolves
   * and redraws, so a mistake is visible at the moment it is made.
   *
   * And it never presents a guess as a fact. A detected path is labelled
   * "found", a chosen one "yours", and a missing one says what to do.
   */
  import { open } from "@tauri-apps/plugin-dialog";
  import Button from "./Button.svelte";
  import { names } from "./names.svelte";
  import { ago, updates } from "./updates.svelte";
  import {
    nexusForgetKey,
    nexusSetKey,
    settingsRead,
    settingsResolve,
    settingsSave,
    blankSettings,
    deployReconcile,
    forgetNames,
    adoptSurvey,
    adoptApply,
    restoreAuthorsPreview,
    restoreAuthors,
    type Restorable,
    type Settings,
    type Resolved,
    type Place,
    type Survey,
    type Install,
  } from "./engine";

  interface Props {
    modsDir?: string;
    /** the app rescans after anything here changes the mods folder */
    onChanged?: () => void;

    /**
     * Every copy of the game found on this machine, and which one is in use.
     *
     * Passed in rather than found here: the app has already detected them on
     * start-up, and a second detection pass would be both slower and able to
     * disagree with the one the rest of the program is acting on.
     */
    installs?: Install[];
    activeInstall?: string;
    /** switch to the install at this index, and rescan */
    onInstall?: (index: number) => void;
  }

  let {
    modsDir,
    onChanged,
    installs = [],
    activeInstall,
    onInstall,
  }: Props = $props();

  /** The two kinds of setting, kept apart so a path can never be written
   *  where a flag belongs and the compiler can check every write. */
  type PathField = "game_root" | "game_exe" | "mods_dir" | "staging_dir" | "archives_dir";
  type FlagField =
    | "check_updates_on_start"
    | "show_outdated"
    | "block_ads"
    | "record_sessions"
    | "backup_saves";

  let settings = $state<Settings>(blankSettings());
  let resolved = $state<Resolved | null>(null);
  let busy = $state(false);
  let note = $state<string | null>(null);
  let failed = $state<string | null>(null);

  async function load() {
    settings = await settingsRead();
    resolved = await settingsResolve();
  }

  void load();

  async function save() {
    busy = true;
    failed = null;
    note = null;
    try {
      resolved = await settingsSave(settings);
      note = "Saved.";
    } catch (e) {
      failed = String(e);
    } finally {
      busy = false;
    }
  }

  function setPath(field: PathField, value: string | null) {
    settings = { ...settings, [field]: value };
  }

  /** Pick a folder (or a file) and put it in `field`, then save at once --
   *  a path chosen from a picker needs no second confirmation. */
  async function browse(field: PathField, directory: boolean, title: string) {
    try {
      const picked = await open({ directory, multiple: false, title });
      if (typeof picked !== "string") return;
      setPath(field, picked);
      await save();
    } catch (e) {
      failed = String(e);
    }
  }

  async function clear(field: PathField) {
    setPath(field, null);
    await save();
  }

  /**
   * Put the game folder back in line with the mod list.
   *
   * The engine has always been able to do this and nothing could ask it to.
   * It is the answer when something outside this program has been at the mods
   * folder -- another manager deploying, a hand-deleted folder, a game verify --
   * and without it the only way back was to toggle a mod off and on again to
   * provoke the reconcile as a side effect.
   */
  let fixing = $state(false);
  async function relink() {
    fixing = true;
    failed = null;
    note = null;
    try {
      const changes = await deployReconcile(false, modsDir);
      if (!changes) return;
      const moved = changes.deployed.length + changes.removed.length;
      note = moved
        ? `${changes.deployed.length} put back, ${changes.removed.length} taken out.`
        : "The game folder already matches your mod list.";
      if (changes.problems.length) failed = changes.problems.join("   ·   ");
      if (moved) onChanged?.();
    } catch (e) {
      failed = String(e);
    } finally {
      fixing = false;
    }
  }

  /**
   * Take over a mods folder another manager deployed.
   *
   * The first-run path for anyone arriving with an existing library, and until
   * now the engine could do it and nothing could ask. Surveying is read-only;
   * the survey is shown before anything is written down, because "we now
   * consider these ours" is a claim the user should get to read first.
   */
  let survey = $state<Survey | null>(null);
  let surveying = $state(false);
  let adopting = $state(false);

  const adoptable = $derived((survey?.candidates ?? []).filter((c) => !c.refused));
  const unadoptable = $derived((survey?.candidates ?? []).filter((c) => c.refused));
  /**
   * Putting mods back on the build their author shipped.
   *
   * Previewed before anything moves, like every other thing here that touches
   * the game folder: it says which staged mod each one will be relinked to, and
   * names the ones it cannot place rather than quietly leaving them behind.
   */
  let restorePlan = $state<Restorable[] | null>(null);
  let planning = $state(false);
  let restoring = $state(false);

  async function planRestore() {
    planning = true;
    failed = null;
    note = null;
    try {
      restorePlan = await restoreAuthorsPreview(modsDir);
    } catch (e) {
      failed = String(e);
    } finally {
      planning = false;
    }
  }

  async function doRestore() {
    restoring = true;
    failed = null;
    note = null;
    try {
      const done = await restoreAuthors(modsDir);
      if (done) {
        const parts: string[] = [];
        if (done.restored.length) {
          parts.push(
            `${done.restored.length} ${done.restored.length === 1 ? "mod is" : "mods are"} back on the build their author shipped, and are now yours to manage`,
          );
        }
        if (done.skipped.length) {
          // Named, not counted: which one was left alone is the useful part.
          parts.push(
            `left alone: ${done.skipped.map(([owner, why]) => `${names.of(owner)} — ${why}`).join("   ·   ")}`,
          );
        }
        note = parts.length ? `${parts.join(". ")}.` : "Nothing needed putting back.";
        if (done.problems.length) failed = done.problems.join("   ·   ");
      }
      restorePlan = null;
      // The survey it came from is now out of date in the way that matters.
      survey = null;
      onChanged?.();
    } catch (e) {
      failed = String(e);
    } finally {
      restoring = false;
    }
  }

  /** Adoptable, but matched on content rather than on file identity. */
  const guesses = $derived(
    adoptable
      .filter((c) => c.notes.some((n) => n.includes("strong guess")))
      .map((c) => c.owner),
  );

  async function look() {
    surveying = true;
    failed = null;
    note = null;
    survey = null;
    try {
      survey = await adoptSurvey(modsDir);
    } catch (e) {
      // Not an error worth a red box: "no manifest here" is the ordinary
      // answer for a folder this program already manages.
      note = String(e).replace(/^Error:\s*/, "");
    } finally {
      surveying = false;
    }
  }

  async function takeOver() {
    adopting = true;
    failed = null;
    try {
      const taken = await adoptApply(modsDir);
      note =
        taken === 0
          ? "Nothing new to take over."
          : `${taken} ${taken === 1 ? "mod is" : "mods are"} now managed here — you can switch, clean, mend and delete ${taken === 1 ? "it" : "them"}.`;
      survey = null;
      onChanged?.();
    } catch (e) {
      failed = String(e);
    } finally {
      adopting = false;
    }
  }

  /**
   * Connecting the Nexus account.
   *
   * Through the shared `updates` store rather than local state, so the check
   * that runs on start-up and the Library's update marks see the key the moment
   * it is saved, without a reload.
   *
   * This is now the only place the key is entered. There used to be a second
   * copy of this form, on the Updates tab, which greeted anyone without a key
   * with a full-screen "Nexus needs to know it is you" — including people who
   * had already connected one here.
   */
  let key = $state("");
  let connecting = $state(false);

  async function connect() {
    connecting = true;
    failed = null;
    note = null;
    try {
      updates.account = await nexusSetKey(key);
      key = "";
      note = updates.account ? `Connected as ${updates.account.name}.` : null;
      void updates.check(modsDir, true);
    } catch (e) {
      failed = String(e);
    } finally {
      connecting = false;
    }
  }

  async function disconnect() {
    connecting = true;
    failed = null;
    note = null;
    try {
      await nexusForgetKey();
      updates.forget();
      note = "Disconnected. The key has been removed from this machine.";
    } catch (e) {
      failed = String(e);
    } finally {
      connecting = false;
    }
  }

  /** Forget the resolved Nexus titles, so the next start-up asks again. */
  let renaming = $state(false);
  async function refreshNames() {
    renaming = true;
    failed = null;
    note = null;
    try {
      await forgetNames();
      await names.resolve(modsDir);
      note = "Mod names read again from Nexus.";
    } catch (e) {
      failed = String(e);
    } finally {
      renaming = false;
    }
  }

  const PATHS: {
    field: PathField;
    label: string;
    hint: string;
    directory: boolean;
    of: (r: Resolved) => Place;
  }[] = [
    {
      field: "game_root",
      label: "Game folder",
      hint: "The folder holding GAMEDATA. Found automatically for Steam and GOG installs.",
      directory: true,
      of: (r) => r.game_root,
    },
    {
      field: "game_exe",
      label: "Game program",
      hint: "Used by the Play button, and to read which version you are on.",
      directory: false,
      of: (r) => r.game_exe,
    },
    {
      field: "mods_dir",
      label: "Mods folder",
      hint: "The only folder the game loads mods from. Everything is deployed here.",
      directory: true,
      of: (r) => r.mods_dir,
    },
    {
      field: "staging_dir",
      label: "Staging folder",
      hint: "Where the real mod files live. The game gets hardlinks to these, so this must be on the same drive as the game.",
      directory: true,
      of: (r) => r.staging,
    },
    {
      field: "archives_dir",
      label: "Downloads folder",
      hint: "Where downloaded archives are kept, so reinstalling costs no bandwidth.",
      directory: true,
      of: (r) => r.archives,
    },
  ];

  const FLAGS: { field: FlagField; label: string; hint: string }[] = [
    {
      field: "check_updates_on_start",
      label: "Check for updates when the app opens",
      hint: "Asks Nexus about each installed mod. Uses a small part of the hourly budget.",
    },
    {
      field: "show_outdated",
      label: "List mods built for an older game version",
      hint: "Off by default: most of them still work, so listing them all buries the real problems. Shown under “Also seen” on the Actions list.",
    },
    {
      field: "block_ads",
      label: "Block ads in the built-in browser",
      hint: "Stops ad and tracker requests before they are made, under Library › Find more.",
    },
    {
      field: "record_sessions",
      label: "Record what the game says while it runs",
      hint: "Costs nothing while the game is closed, and does nothing until the recorder is installed — see Sessions.",
    },
    {
      field: "backup_saves",
      label: "Keep copies of your save",
      hint: "A save is the one thing here that cannot be rebuilt. Copies are taken as the game writes it, and the last few of each slot are kept — see Sessions.",
    },
  ];
</script>

<div class="pane settings">
  <header>
    <h2>Settings</h2>
    {#if resolved?.detected_from}
      <span class="found">Install found via {resolved.detected_from}</span>
    {/if}
    <!-- Play used to live here, and only here, which meant the one verb in the
         app that is not about mods was three clicks away in the pane visited
         least. It is in the window chrome now, on screen whatever tab is open,
         so a second copy of it here would only be a second place to look. -->
  </header>

  {#if resolved && !resolved.ready}
    <p class="failed">
      The game folder is not set up yet, so scanning and installing will not work.
      Set the folders below.
    </p>
  {/if}

  <section>
    <h3>Folders</h3>
    <p class="lede">
      Leave a box empty to have it found automatically. Anything you set here wins over
      detection.
    </p>

    <!-- Which install to work on. It used to be a dropdown in the window
         chrome, disguised as a readout labelled "Found via" — a real control
         wearing the clothes of a caption, in a bar that was otherwise three
         facts nobody needed. It belongs with the paths, because that is all it
         is: the coarsest of them.

         Only when there is a choice to make. One install is not a decision, and
         a dropdown with one entry is a control that does nothing. -->
    {#if installs.length > 1}
      <div class="row">
        <div class="head">
          <label for="which-install">Which install</label>
          <span class="tag auto">{installs.length} found</span>
        </div>
        <p class="hint">
          You have more than one copy of the game. Everything in this program —
          scanning, installing, switching — acts on the one chosen here.
        </p>
        <select
          id="which-install"
          class="pick"
          onchange={(e) => onInstall?.(Number(e.currentTarget.value))}
        >
          {#each installs as install, i}
            <option value={i} selected={install.root === activeInstall}>
              {install.source} — {install.root}
            </option>
          {/each}
        </select>
      </div>
    {/if}

    {#each PATHS as row}
      {@const place = resolved ? row.of(resolved) : null}
      <div class="row" class:bad={!!place?.problem}>
        <div class="head">
          <label for={row.field}>{row.label}</label>
          {#if place}
            {#if place.problem}
              <span class="tag wrong">problem</span>
            {:else if place.chosen}
              <span class="tag yours">yours</span>
            {:else if place.path}
              <span class="tag auto">found</span>
            {/if}
          {/if}
        </div>

        <div class="line">
          <input
            id={row.field}
            type="text"
            spellcheck="false"
            placeholder={place?.path ?? "not set"}
            value={settings[row.field] ?? ""}
            oninput={(e) => {
              const text = e.currentTarget.value.trim();
              setPath(row.field, text === "" ? null : text);
            }}
            onchange={save}
          />
          <Button onclick={() => browse(row.field, row.directory, row.label)} disabled={busy}>
            Browse
          </Button>
          {#if settings[row.field]}
            <Button onclick={() => clear(row.field)} disabled={busy}>Reset</Button>
          {/if}
        </div>

        {#if place?.problem}
          <p class="problem">{place.problem}</p>
        {:else if place?.path && !settings[row.field]}
          <p class="resolvedpath">{place.path}</p>
        {:else}
          <p class="hint">{row.hint}</p>
        {/if}
      </div>
    {/each}
  </section>

  <section>
    <h3>Behaviour</h3>
    {#each FLAGS as flag}
      <label class="check">
        <input
          type="checkbox"
          checked={settings[flag.field]}
          onchange={(e) => {
            settings = { ...settings, [flag.field]: e.currentTarget.checked };
            void save();
          }}
        />
        <span>
          <strong>{flag.label}</strong>
          <em>{flag.hint}</em>
        </span>
      </label>
    {/each}
  </section>

  <section>
    <h3>Nexus account</h3>
    <p class="lede">
      A personal API key lets this program ask Nexus which version of each mod
      is current, read mod pages, and take downloads you start in its browser.
      It is read-only, it never leaves this machine except to Nexus, and you can
      disconnect it here at any time.
    </p>

    {#if !updates.settled && !updates.account}
      <p class="hint">Checking for a saved key…</p>
    {:else if updates.account}
      <div class="row">
        <div class="head">
          <label for="nexus-key">Connected as {updates.account.name}</label>
          {#if !updates.account.is_premium}<span class="tag auto">free account</span>{/if}
        </div>
        <p class="hint">
          {#if updates.account.is_premium}
            Downloads can be fetched straight from Nexus.
          {:else}
            A free account cannot be handed a download by the API, so mods are
            still fetched by pressing <em>Mod manager download</em> on the page
            under <em>Library &rsaquo; Find more</em>. Everything else works the
            same.
          {/if}
        </p>
        <div class="verbs">
          <Button variant="danger" onclick={disconnect} busy={connecting}>Disconnect</Button>
        </div>
      </div>

      <!-- The one thing the Updates tab did that was not a duplicate of this
           section: running the check by hand. The *results* are not here —
           they belong against the mods they are about, which is the Library's
           Update chip and each mod's record. This is only the button. -->
      <div class="row">
        <div class="head">
          <label for="recheck">Version check</label>
          {#if updates.report}
            <span class="tag auto">
              {updates.outdated.length
                ? `${updates.outdated.length} behind`
                : "all current"}
            </span>
          {/if}
        </div>
        <p class="hint">
          Asks Nexus about every installed mod, one request per mod page.
          {#if updates.checkedAt}
            Last checked {ago(updates.checkedAt)}.
          {:else}
            Not checked yet this session.
          {/if}
          {#if updates.report}
            {updates.report.budget.hourly_remaining} requests left this hour.
          {/if}
          What it found is marked in your Library, on each mod.
        </p>
        {#if updates.error}<p class="failed">{updates.error}</p>{/if}
        <div class="verbs">
          <Button onclick={() => updates.check(modsDir, true)} busy={updates.checking}>
            {#if !updates.checking}
              Check now
            {:else if updates.progress}
              {updates.progress.done} of {updates.progress.total}…
            {:else}
              Asking Nexus…
            {/if}
          </Button>
        </div>
      </div>
    {:else}
      <div class="row">
        <div class="head">
          <label for="nexus-key">Personal API key</label>
        </div>
        <p class="hint">
          Nexus Mods &rsaquo; Account settings &rsaquo; API keys &rsaquo;
          <em>Personal API key</em>
        </p>
        <form
          class="line"
          onsubmit={(e) => {
            e.preventDefault();
            void connect();
          }}
        >
          <input
            id="nexus-key"
            type="password"
            bind:value={key}
            placeholder="Paste the key"
            autocomplete="off"
            spellcheck="false"
          />
          <Button variant="primary" type="submit" busy={connecting} disabled={!key.trim()}>
            {connecting ? "Checking…" : "Connect"}
          </Button>
        </form>
      </div>
    {/if}

    <div class="row">
      <div class="head">
        <label for="renames">Mod names</label>
      </div>
      <p class="hint">
        Titles are remembered after the first look, so this only matters when an
        author has renamed their page.
      </p>
      <div class="verbs">
        <Button onclick={refreshNames} busy={renaming}>
          {renaming ? "Asking Nexus…" : "Read mod names again"}
        </Button>
      </div>
    </div>
  </section>

  <section>
    <h3>Take over an existing library</h3>
    <p class="lede">
      Writes down which staged folder each mod in the game came from, so they
      can be switched, cleaned, mended and deleted here. Nothing is copied,
      moved or deleted &mdash; the files in the game stay exactly where they
      are.
    </p>
    <p class="hint">
      It works out where each mod came from by reading the mods folder itself: a
      deployed mod and its staged original are <em>the same file</em> under two
      names, and the file system says so. No other mod manager has to be
      installed, and none has to be asked.
    </p>
    <div class="verbs">
      <Button onclick={look} busy={surveying}>
        {surveying ? "Looking…" : "See what can be taken over"}
      </Button>
      {#if adoptable.length}
        <Button variant="primary" onclick={takeOver} busy={adopting}>
          Take over {adoptable.length}
          {adoptable.length === 1 ? "mod" : "mods"}
        </Button>
      {/if}
    </div>

    {#if survey}
      <p class="hint">
        <!-- Named only when a manager actually left a manifest behind. Without
             one we traced the links ourselves, and claiming "another manager
             deployed these" would be inventing a program that may never have
             been on this machine. -->
        {#if survey.manager}
          {survey.manager} deployed {survey.candidates.length}
          {survey.candidates.length === 1 ? "mod" : "mods"} here.
        {:else if survey.candidates.length}
          Traced {survey.candidates.length}
          {survey.candidates.length === 1 ? "mod" : "mods"} back to
          {survey.staging ?? "your staging folder"}.
        {:else}
          Nothing in the mods folder could be traced back to your staging
          folder, so there is nothing to take over.
        {/if}
        {#if survey.unmanaged.length}
          {survey.unmanaged.length}
          {survey.unmanaged.length === 1 ? "folder is" : "folders are"} not in
          your staging folder at all &mdash; installed by hand, or by something
          that keeps its files elsewhere &mdash; so
          {survey.unmanaged.length === 1 ? "it" : "they"} cannot be taken over
          this way.
        {/if}
      </p>
      {#if survey.ours.length}
        <!-- The one state adoption cannot help with as it stands -- but it can
             be got out of, so this offers the way rather than naming thirteen
             mods and leaving the reader to find each one. It used to send them
             to the Library, which cannot show these: they are not in the mod
             list, which is the very reason they are on this list. -->
        <p class="warn">
          {survey.ours.length}
          {survey.ours.length === 1 ? "mod is" : "mods are"} running a build this
          program made &mdash; cleaned, mended or combined. Those load from our own
          folder rather than from the author's, so there is no way to tell which of
          the three {survey.ours.length === 1 ? "it is" : "they are"} on without the
          mod list. Putting
          {survey.ours.length === 1 ? "it" : "them"} back on the author's build settles
          that, and takes
          {survey.ours.length === 1 ? "it" : "them"} over at the same time &mdash; the
          author's copy is still staged and untouched, so nothing is lost but the
          cleaning, which can be done again.
        </p>
        {#if restorePlan}
          {@const ready = restorePlan.filter((r) => r.source)}
          {@const stuck = restorePlan.filter((r) => !r.source)}
          {#if ready.length}
            <ul class="refused plain">
              {#each ready as r (r.owner)}
                <li>{names.of(r.owner)} &rarr; <span class="mono">{r.source}</span></li>
              {/each}
            </ul>
          {/if}
          {#if stuck.length}
            <ul class="refused">
              {#each stuck as r (r.owner)}
                <li><strong>{names.of(r.owner)}</strong> — {r.why_not}</li>
              {/each}
            </ul>
          {/if}
          <div class="verbs">
            {#if ready.length}
              <Button variant="primary" onclick={doRestore} busy={restoring}>
                {restoring
                  ? "Relinking…"
                  : `Use the author's build for ${ready.length} ${ready.length === 1 ? "mod" : "mods"}`}
              </Button>
            {/if}
            <Button onclick={() => (restorePlan = null)} disabled={restoring}>
              Not now
            </Button>
          </div>
        {:else}
          <div class="verbs">
            <Button onclick={planRestore} busy={planning}>
              {planning ? "Looking…" : "See how to put them back"}
            </Button>
          </div>
        {/if}
      {/if}
      {#if guesses.length}
        <!-- A content match is a strong guess, not a fact, and "Take over"
             takes the whole set at once -- so the ones that were guessed at are
             named here as well as in their own note, where someone reading the
             survey before pressing it will see them. -->
        <p class="warn">
          {guesses.length}
          {guesses.length === 1 ? "mod was" : "mods were"} matched by file name and
          size rather than by file identity, because
          {guesses.length === 1 ? "it was" : "they were"} copied into the game rather
          than linked. Worth a look before taking
          {guesses.length === 1 ? "it" : "them"} over: {guesses.join(", ")}.
        </p>
      {/if}
      {#if unadoptable.length}
        <!-- Named rather than quietly skipped: the loadout's whole job is to
             know what is in the game, and an entry that is wrong about that is
             worse than no entry. -->
        <ul class="refused">
          {#each unadoptable as c (c.owner)}
            <li><strong>{c.owner}</strong> — {c.refused}</li>
          {/each}
        </ul>
      {/if}
    {/if}
  </section>

  <section>
    <h3>Repair the mods folder</h3>
    <p class="lede">
      Puts the game folder back in line with your mod list. Use it when something
      outside this program has been at <span class="mono">GAMEDATA\MODS</span> —
      another mod manager deploying, a folder deleted by hand, a game file verify.
      Mods this program did not install are left alone.
    </p>
    <div class="verbs">
      <Button onclick={relink} busy={fixing}>
        {fixing ? "Checking every mod…" : "Put it back in line"}
      </Button>
    </div>
  </section>

  {#if note}<p class="ok">{note}</p>{/if}
  {#if failed}<p class="failed">{failed}</p>{/if}
</div>

<style>
  /* Narrower than a full pane, and a stack rather than a flow: this is a form,
     and a 90-character-wide text field is harder to read a path out of, not
     easier. The shared `.pane` gives it the scroll and the padding. */
  .settings {
    display: flex;
    flex-direction: column;
    gap: 1.5rem;
    max-width: 52rem;
  }

  header {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }
  h2 {
    font-size: var(--t-head);
    margin: 0;
    color: var(--readout);
  }
  .found {
    font-size: var(--t-micro);
    color: var(--faint);
  }
  section {
    border: 1px solid var(--rule);
    border-radius: var(--r-md);
    background: var(--glass);
    padding: var(--pad);
    box-shadow: var(--cast), var(--lip);
  }
  h3 {
    margin: 0 0 0.35rem;
    font-size: var(--t-lead);
    color: var(--readout);
  }
  .lede {
    margin: 0 0 1rem;
    color: var(--dim);
    font-size: var(--t-small);
  }
  .row {
    padding: 0.75rem 0;
    border-top: 1px solid var(--rule);
  }
  .row:first-of-type {
    border-top: 0;
  }
  .head {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    margin-bottom: 0.35rem;
  }
  label {
    color: var(--readout);
    font-size: var(--t-body);
    font-weight: 600;
  }
  .tag {
    font-size: var(--t-micro);
    padding: 0.1rem 0.5rem;
    border-radius: var(--r-pill);
  }
  .auto {
    background: var(--cyan-wash);
    color: var(--cyan);
  }
  .yours {
    background: var(--accent-wash);
    color: var(--accent);
  }
  .wrong {
    background: var(--contest-wash);
    color: var(--contest);
  }
  .line {
    display: flex;
    gap: 0.5rem;
  }
  input[type="text"],
  input[type="password"] {
    flex: 1;
    background: var(--inset);
    border: 1px solid var(--rule);
    border-radius: var(--r-sm);
    color: var(--readout);
    font-family: var(--font-value);
    font-size: var(--t-small);
    padding: 0.45rem 0.6rem;
  }
  input[type="text"]:focus,
  input[type="password"]:focus {
    outline: none;
    border-color: var(--rule-hi);
  }
  .bad input[type="text"] {
    border-color: var(--contest);
  }

  /* Which install to work on. Shaped like the path boxes above it, because
     that is what it is: the coarsest of the paths. */
  .pick {
    width: 100%;
    margin-top: 0.5rem;
    background: var(--inset);
    border: 1px solid var(--rule);
    border-radius: var(--r-sm);
    color: var(--readout);
    font-family: var(--font-value);
    font-size: var(--t-small);
    padding: 0.45rem 0.6rem;
  }

  .pick:focus {
    outline: none;
    border-color: var(--rule-hi);
  }
  .hint,
  .problem,
  .resolvedpath {
    margin: 0.35rem 0 0;
    font-size: var(--t-micro);
  }
  .hint {
    color: var(--faint);
  }
  .problem {
    color: var(--contest);
  }
  .resolvedpath {
    color: var(--dim);
    font-family: var(--font-value);
  }
  .check {
    display: flex;
    gap: 0.6rem;
    padding: 0.6rem 0;
    border-top: 1px solid var(--rule);
    font-weight: 400;
    cursor: pointer;
  }
  .check:first-of-type {
    border-top: 0;
  }
  .check span {
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
  }
  .check strong {
    color: var(--readout);
    font-size: var(--t-body);
    font-weight: 500;
  }
  .check em {
    color: var(--faint);
    font-size: var(--t-micro);
    font-style: normal;
  }
  .mono {
    font-family: var(--font-value);
    color: var(--dim);
  }
  .refused {
    margin: 0.5rem 0 0;
    padding: 0.5rem 0.625rem;
    border: 1px solid var(--rule);
    border-radius: var(--r-sm);
    background: var(--inset);
    list-style: none;
    font-size: var(--t-micro);
    color: var(--dim);
  }
  .refused strong {
    color: var(--readout);
    font-weight: 500;
  }

  /* The same list shape, for rows that are not refusals: what each mod will be
     relinked to. The border says "here is a list"; only the words say whether
     it is good news. */
  .refused.plain li {
    color: var(--readout);
  }

  .refused.plain .mono {
    color: var(--dim);
  }
</style>
