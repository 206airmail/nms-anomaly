<script lang="ts" module>
  import type { LiveEvent } from "./engine";

  /** The first line of a message: crash reports and module lists are many. */
  function firstLine(event: LiveEvent): string {
    const at = event.msg.indexOf("\n");
    return at < 0 ? event.msg : `${event.msg.slice(0, at)} …`;
  }

  /** A level is the one thing in this app that a colour may mean on its own.
   *
   * Amber has one job everywhere else -- marking the value the game will load --
   * and this is the exception rather than a second job: in a log the level *is*
   * the content, not a decoration on it, and every line carries its own word for
   * it as well, so the colour is never the only cue.
   */
  function levelClass(event: LiveEvent): string {
    if (event.lvl >= 3) return "bad";
    if (event.lvl === 2) return "loud";
    if (event.lvl === 1) return "plainlevel";
    return "quietlevel";
  }
</script>

<script lang="ts">
  /** What the game itself said, run by run.
   *
   * Every other tab reasons about mods as files on disk. This one is the only
   * place that knows what the *game* did with them: which files it actually
   * opened, what it complained about, and where it died. That comes from a small
   * DLL living beside `NMS.exe`, so the pane has two jobs and they are drawn in
   * this order:
   *
   *   1. is this working at all -- because a recorder that is not installed
   *      records nothing, and a screen that quietly showed an empty list would
   *      read as "your game is fine";
   *   2. what the last run amounted to, in one line, with the evidence behind it
   *      on demand.
   *
   * The verdict comes first and the log second, the same way the Actions tab
   * puts the verdict before the report. A session log is 60 to 200 KB of text
   * and almost none of it is ever the answer.
   */
  import Awaiting from "./Awaiting.svelte";
  import Button from "./Button.svelte";
  import LoadOrderMeasured from "./LoadOrderMeasured.svelte";
  import Sheet from "./Sheet.svelte";
  import { names } from "./names.svelte";
  import { sessions, spellDuration } from "./sessions.svelte";
  import {
    hookInstall,
    hookUninstall,
    humanBytes,
    saveForget,
    saveRestore,
    savesBackUp,
    observeSession,
    sessionForget,
    sessionText,
    type Observation,
    type SaveCopy,
    type Session,
  } from "./engine";

  interface Props {
    /**
     * Open a mod's record in the Library.
     *
     * This pane names mods constantly — complained about, never loaded, files
     * ignored — and until now naming them was all it could do: the reader was
     * left to remember a title, change tab and find it in a list of sixty.
     * Everything else it would want to know about that mod is one place, so
     * the names lead there.
     */
    onmod: (owner: string) => void;
  }

  let { onmod }: Props = $props();

  const watch = $derived(sessions.state);
  const hook = $derived(sessions.hook);

  let working = $state(false);
  let failure = $state<string | null>(null);
  let said = $state<string | null>(null);

  /** How much of the live feed to show. Defaults to the part worth reading. */
  let show = $state<"loud" | "mods" | "all">("loud");

  const shown = $derived(
    sessions.events
      .filter((e) =>
        show === "all" ? true : show === "mods" ? e.cat === "modfile" : e.lvl >= 2,
      )
      // Newest first: a live feed that grows downwards either scrolls away from
      // the reader or fights them for the scroll position.
      .slice()
      .reverse(),
  );

  /** Seconds the open session has been running, ticking while it is open. */
  let now = $state(Date.now());
  $effect(() => {
    if (!watch.recording) return;
    const timer = setInterval(() => (now = Date.now()), 1000);
    return () => clearInterval(timer);
  });
  const running = $derived(
    watch.recording ? Math.max(0, Math.round((now - watch.since_ms) / 1000)) : 0,
  );

  /** Copies of the save, newest first. */
  const copies = $derived(sessions.copies);
  let copying = $state(false);
  let restoring = $state<string | null>(null);
  let restored = $state<string | null>(null);
  /** The copy a confirmation is about: putting one back overwrites a save. */
  let putting = $state<SaveCopy | null>(null);

  let opened = $state<Session | null>(null);
  let text = $state<string | null>(null);
  let reading = $state(false);
  /** What this session settled about load order; see `LoadOrderMeasured`. */
  let observed = $state<Observation | null>(null);

  async function install(force: boolean) {
    working = true;
    failure = null;
    said = null;
    try {
      await hookInstall(force);
      said = "Installed. The next time the game runs, this program records it.";
    } catch (err) {
      failure = String(err);
    } finally {
      await sessions.refreshHook();
      working = false;
    }
  }

  async function remove() {
    working = true;
    failure = null;
    said = null;
    try {
      await hookUninstall();
      said = "Removed. Nothing of ours is left in the game's folder.";
    } catch (err) {
      failure = String(err);
    } finally {
      await sessions.refreshHook();
      working = false;
    }
  }

  async function copyNow() {
    copying = true;
    failure = null;
    said = null;
    try {
      sessions.copies = await savesBackUp();
      said = "Copied. Anything that had changed since the last copy is kept.";
    } catch (err) {
      failure = String(err);
    } finally {
      copying = false;
    }
  }

  async function putBack(kept: SaveCopy) {
    restoring = kept.path;
    failure = null;
    restored = null;
    try {
      const files = await saveRestore(kept);
      restored = `Put back ${files.length} file${files.length === 1 ? "" : "s"} into ${kept.slot}. The copy it replaced was kept, so this can be undone.`;
      await sessions.reloadCopies();
    } catch (err) {
      failure = String(err);
    } finally {
      restoring = null;
      putting = null;
    }
  }

  async function dropCopy(kept: SaveCopy) {
    try {
      sessions.copies = await saveForget(kept);
    } catch (err) {
      failure = String(err);
    }
  }

  async function open(session: Session) {
    opened = session;
    text = null;
    observed = null;
    reading = true;
    // Two reads of the same file, deliberately. The viewer wants the tail and
    // the measurement wants the whole thing -- the mod files are all opened in
    // the first second of a run, which is the part a tail is missing.
    void measure(session);
    try {
      text = await sessionText(session.log);
    } catch (err) {
      text = String(err);
    } finally {
      reading = false;
    }
  }

  /** Read the load order back out of the log. Failure is quiet on purpose: a
   * session recorded before the game had ever written GCMODSETTINGS has nothing
   * to measure, and a red line about it would imply something broke. */
  async function measure(session: Session) {
    try {
      observed = await observeSession(session.log);
    } catch {
      observed = null;
    }
  }

  async function forget(session: Session) {
    try {
      sessions.list = await sessionForget(session.log);
      opened = null;
    } catch (err) {
      failure = String(err);
    }
  }

  async function reveal(path: string) {
    try {
      const { revealItemInDir } = await import("@tauri-apps/plugin-opener");
      await revealItemInDir(path);
    } catch (err) {
      failure = String(err);
    }
  }

  /** The marks on a session row: only the ones that are not zero. */
  function marks(session: Session): { text: string; kind: string }[] {
    const out: { text: string; kind: string }[] = [];
    if (session.crashed) out.push({ text: "crashed", kind: "badge-alert" });
    const errors = session.counts.error + session.counts.fatal;
    if (errors) out.push({ text: `${errors} error${errors === 1 ? "" : "s"}`, kind: "badge-alert" });
    if (session.counts.warn)
      out.push({ text: `${session.counts.warn} warnings`, kind: "badge-signal" });
    if (session.dialogs.length) out.push({ text: "dialog", kind: "badge-signal" });
    if (session.ignored.length)
      out.push({ text: `${session.ignored.length} files ignored`, kind: "badge-info" });
    return out;
  }

  const trouble = $derived(opened?.mods_in_trouble ?? []);
</script>

<div class="pane">
  <!-- 1. Is anything being recorded. -->
  <section class="panel hero">
    <header>
      <div class="line">
        <h2>Recording</h2>
        {#if watch.recording}
          <span class="badge badge-info"><span class="spin"></span> recording</span>
        {:else if hook?.installed && hook?.ours}
          <span class="badge badge-quiet">ready</span>
        {:else}
          <span class="badge badge-signal">not installed</span>
        {/if}
      </div>
    </header>

    {#if sessions.loading}
      <Awaiting>Looking at the game's folder…</Awaiting>
    {:else if !hook}
      <p class="lede">
        Recording needs the app itself — this pane cannot do anything in a plain
        browser.
      </p>
    {:else if watch.recording}
      <p class="lede">
        The game has been running for <b>{spellDuration(running)}</b>. Everything
        it says is going into a log of its own.
      </p>
      <dl class="readout">
        <div><dt>Errors</dt><dd class:hot={watch.counts.error + watch.counts.fatal > 0}>{watch.counts.error + watch.counts.fatal}</dd></div>
        <div><dt>Warnings</dt><dd>{watch.counts.warn}</dd></div>
        <div><dt>Noted</dt><dd>{watch.counts.info + watch.counts.debug}</dd></div>
        <div><dt>Process</dt><dd>{watch.pid}</dd></div>
      </dl>
      {#if watch.log}
        <p class="hint path">{watch.log}</p>
      {/if}
    {:else if !hook.installed || !hook.ours}
      <p class="lede">
        No Man's Sky writes almost nothing to its own log. A small file beside the
        game — <code>{hook.path ?? "xinput9_1_0.dll"}</code> — records which mod
        files the game actually loads, what it complains about, and where it
        crashes. It is read-only: it watches, and it can be removed at any time.
      </p>
      {#if hook.foreign}
        <p class="warn">
          {hook.foreign}. Installing ours keeps a copy of theirs, and removing
          ours puts it back — but that other tool will not be working in the
          meantime.
        </p>
      {/if}
      {#if hook.blocked}
        <p class="warn">{hook.blocked}</p>
      {/if}
      <div class="verbs">
        <Button
          variant="primary"
          busy={working}
          disabled={!!hook.blocked}
          onclick={() => install(!!hook.foreign)}
        >
          {hook.foreign ? "Install it, keeping a copy of theirs" : "Install the recorder"}
        </Button>
      </div>
    {:else}
      <p class="lede">
        The recorder is in place. The next time the game runs — from here, from
        Steam, from anywhere — this program writes down what it says and sums it
        up when it exits.
      </p>
      {#if !hook.up_to_date}
        <p class="warn">
          What is installed is an older build than this program ships. Installing
          again replaces it.
        </p>
      {/if}
      {#if watch.game_running && !watch.recording && watch.note}
        <p class="warn">{watch.note}</p>
      {:else if watch.note && !watch.game_running}
        <p class="hint">{watch.note}</p>
      {/if}
      <div class="verbs">
        {#if !hook.up_to_date}
          <Button variant="primary" busy={working} onclick={() => install(false)}>
            Install the newer build
          </Button>
        {/if}
        <Button
          variant="ghost"
          busy={working}
          disabled={!!hook.blocked}
          onclick={remove}
        >
          Remove the recorder
        </Button>
        {#if hook.out_dir}
          <Button variant="link" onclick={() => reveal(hook.out_dir!)}>
            Show its own log
          </Button>
        {/if}
      </div>
      {#if hook.blocked && watch.game_running}
        <p class="hint">{hook.blocked}</p>
      {/if}
    {/if}

    {#if said}<p class="ok">{said}</p>{/if}
    {#if failure}<p class="failed">{failure}</p>{/if}
  </section>

  <!-- 2. The live feed, only while there is one. -->
  {#if watch.recording}
    <section class="panel">
      <header>
        <div class="line">
          <h3>As it happens</h3>
          <div class="filters">
            <button class:on={show === "loud"} onclick={() => (show = "loud")}>
              Complaints
            </button>
            <button class:on={show === "mods"} onclick={() => (show = "mods")}>
              Mod files
            </button>
            <button class:on={show === "all"} onclick={() => (show = "all")}>
              Everything
            </button>
          </div>
        </div>
      </header>

      {#if !shown.length}
        <p class="hint">
          {show === "loud"
            ? "Nothing to report so far, which is the good answer."
            : "Nothing of this kind yet."}
        </p>
      {:else}
        <ol class="feed">
          {#each shown.slice(0, 200) as line (line.ts + line.msg)}
            <li class="event {levelClass(line)}">
              <span class="when">{line.at}</span>
              <span class="cat">{line.cat}</span>
              <span class="what">
                {firstLine(line)}
                {#if line.owner}
                  <span class="owner">{names.of(line.owner)}</span>
                {/if}
              </span>
            </li>
          {/each}
        </ol>
        {#if shown.length > 200}
          <p class="hint">
            Showing the newest 200 of {shown.length}. All of it is in the log.
          </p>
        {/if}
      {/if}
    </section>
  {/if}

  <!-- 3. The one thing here that cannot be rebuilt. -->
  <section class="panel">
    <header>
      <div class="line">
        <h3>Copies of your save</h3>
        <span class="hint">
          {copies.kept.length
            ? `${copies.kept.length} kept, ${humanBytes(copies.bytes)}`
            : "none yet"}
        </span>
      </div>
    </header>

    {#if !copies.folders.length}
      <p class="hint">
        No save folder found in your AppData, so there is nothing to copy — which
        is normal if the game has never been run on this machine.
      </p>
    {:else}
      <p class="lede">
        A mod can be downloaded again and a merge rebuilt, but a save cannot. Every
        time the game finishes writing one, the whole slot — the save and its
        manifest together — is copied here, and the last few are kept. This does
        not need the recorder installed.
      </p>
      {#if copies.kept.length}
        <ul class="runs">
          {#each copies.kept.slice(0, 12) as kept (kept.path)}
            <li>
              <div class="copy">
                <span class="when">{kept.at}</span>
                <span class="slot">
                  {kept.slot}
                  {#if kept.before_restore}
                    <span class="badge badge-quiet">replaced</span>
                  {/if}
                </span>
                <span class="size">{humanBytes(kept.bytes)}</span>
                <span class="rowverbs">
                  <Button
                    variant="ghost"
                    busy={restoring === kept.path}
                    onclick={() => (putting = kept)}
                  >
                    Put back
                  </Button>
                  <Button variant="link" onclick={() => dropCopy(kept)}>Delete</Button>
                </span>
              </div>
            </li>
          {/each}
        </ul>
        {#if copies.kept.length > 12}
          <p class="hint">and {copies.kept.length - 12} older copies.</p>
        {/if}
      {:else}
        <p class="hint">
          Nothing copied yet. One is taken as soon as the game writes a save, or
          press the button below.
        </p>
      {/if}
      <div class="verbs">
        <Button variant="ghost" busy={copying} onclick={copyNow}>Copy now</Button>
      </div>
      {#if restored}<p class="ok">{restored}</p>{/if}
    {/if}
  </section>

  <!-- 4. What the runs amounted to. -->
  <section class="panel">
    <header>
      <div class="line">
        <h3>Sessions</h3>
        {#if sessions.list.length}
          <span class="hint">{sessions.list.length} kept</span>
        {/if}
      </div>
    </header>

    {#if sessions.loading}
      <Awaiting>Reading the sessions kept so far…</Awaiting>
    {:else if !sessions.list.length}
      <p class="hint">
        Nothing recorded yet. {hook?.installed && hook?.ours
          ? "The next run will be the first."
          : "Install the recorder above, then play."}
      </p>
    {:else}
      <ul class="runs">
        {#each sessions.list as session (session.log)}
          <li>
            <button class="run" onclick={() => open(session)}>
              <span class="when">{session.started}</span>
              <span class="verdict" class:bad={session.crashed}>{session.verdict}</span>
              <span class="badges">
                {#each marks(session) as mark}
                  <span class="badge {mark.kind}">{mark.text}</span>
                {/each}
              </span>
            </button>
          </li>
        {/each}
      </ul>
    {/if}
  </section>
</div>

{#if putting}
  {@const kept = putting}
  <Sheet title="Put this save back?" danger onclose={() => (putting = null)}>
    <p class="lede">
      This replaces <b>{kept.slot}</b> in the game's save folder with the copy from
      <b>{kept.at}</b> — {kept.files.length} files, {humanBytes(kept.bytes)}.
    </p>
    <p class="hint">
      What is there now is copied aside first, so this can be undone by putting
      that copy back. The game must be closed: it keeps the save in memory and
      would write over anything put back while it runs.
    </p>
    <ul class="plain">
      {#each kept.files as [file, bytes]}
        <li><span class="path">{file}</span> — {humanBytes(bytes)}</li>
      {/each}
    </ul>

    {#snippet verbs()}
      <Button variant="primary" busy={restoring === kept.path} onclick={() => putBack(kept)}>
        Put it back
      </Button>
      <Button variant="link" onclick={() => (putting = null)}>Leave it</Button>
    {/snippet}
  </Sheet>
{/if}

{#if opened}
  {@const session = opened}
  <Sheet title="Session of {session.started}" onclose={() => (opened = null)}>
    <p class="lede">{session.verdict}</p>

    <dl class="readout wide">
      <div><dt>Ran for</dt><dd>{spellDuration(session.seconds)}</dd></div>
      <div><dt>Exit</dt><dd>{session.exit_note}</dd></div>
      <div><dt>Mod files opened</dt><dd>{session.files_opened}</dd></div>
      <div>
        <dt>Events</dt>
        <dd>
          {session.counts.error + session.counts.fatal} errors,
          {session.counts.warn} warnings
        </dd>
      </div>
    </dl>

    {#if session.changed.length}
      <h4>Since your last recorded session</h4>
      <p class="hint">
        Any of these can be why something behaves differently. A list of suspects,
        not a diagnosis.
      </p>
      <ul class="plain">
        {#each session.changed as line}<li>{line}</li>{/each}
      </ul>
    {/if}

    {#if session.saves.length}
      <h4>Saves written</h4>
      <ul class="plain">
        {#each session.saves as save}
          <li class:bad={save.error !== null || save.unfinished}>
            <b>{save.file}</b> —
            {#if save.error !== null}
              failed with Windows error {save.error} after {humanBytes(save.bytes)}
            {:else if save.unfinished}
              the game ended with it still open, {humanBytes(save.bytes)} written
            {:else}
              {humanBytes(save.bytes)} in {save.writes} writes over {save.ms} ms
            {/if}
          </li>
        {/each}
      </ul>
    {/if}

    {#if session.memory}
      {@const memory = session.memory}
      <h4>Memory</h4>
      <p class="plain">
        Working set {humanBytes(memory.first.working_set)} at the first sample,
        {humanBytes(memory.last.working_set)} at the last
        ({memory.last.working_set >= memory.first.working_set ? "+" : "−"}{humanBytes(
          Math.abs(memory.last.working_set - memory.first.working_set),
        )}), peak {humanBytes(memory.last.peak_working_set)}. Handles
        {memory.first.handles} to {memory.last.handles}. The machine had
        {humanBytes(memory.last.system_free)} of
        {humanBytes(memory.last.system_total)} free at the end.
      </p>
    {/if}

    {#if session.crash}
      <h4>Where it crashed</h4>
      <pre class="report">{session.crash}</pre>
    {/if}

    {#if session.dialogs.length}
      <h4>Message boxes the game put up</h4>
      <ul class="plain">
        {#each session.dialogs as dialog}<li>{dialog}</li>{/each}
      </ul>
    {/if}

    {#if trouble.length}
      <h4>Mods the game complained about</h4>
      <ul class="plain">
        {#each trouble as mod}
          <li>
            <button class="who" onclick={() => onmod(mod.folder)}>{names.of(mod.folder)}</button>
            — {mod.errors} errors, {mod.warnings} warnings
            {#each mod.samples as sample}
              <span class="sample">{sample}</span>
            {/each}
          </li>
        {/each}
      </ul>
    {/if}

    {#if session.ignored.length}
      <h4>Files the game ignored</h4>
      <p class="hint">
        The same asset in two formats: the game loaded one and never looked at the
        other, so that copy is doing nothing.
      </p>
      <ul class="plain">
        {#each session.ignored as entry}
          <li>
            <button class="who" onclick={() => onmod(entry.folder)}>
              {names.of(entry.folder)}
            </button>
            — <span class="path">{entry.file}</span>
            ignored; the game used {entry.instead}
          </li>
        {/each}
      </ul>
    {/if}

    {#if session.never_loaded.length}
      <h4>Mods the game opened nothing from</h4>
      <p class="hint">
        Either this run never reached the content they change, or the game no
        longer asks for the files they replace.
      </p>
      <ul class="plain">
        {#each session.never_loaded as folder}
          <li>
            <button class="who" onclick={() => onmod(folder)}>{names.of(folder)}</button>
          </li>
        {/each}
      </ul>
    {/if}

    {#if session.dumps.length}
      <h4>Crash dumps written</h4>
      <ul class="plain">
        {#each session.dumps as dump}<li class="path">{dump}</li>{/each}
      </ul>
    {/if}

    {#if observed}
      <LoadOrderMeasured observation={observed} {onmod} />
    {/if}

    <h4>The log</h4>
    {#if reading}
      <Awaiting>Reading the log…</Awaiting>
    {:else if text}
      <pre class="log">{text}</pre>
    {/if}

    {#snippet verbs()}
      <Button variant="ghost" onclick={() => reveal(session.log)}>
        Show the file
      </Button>
      <Button variant="danger" onclick={() => forget(session)}>
        Delete this session
      </Button>
      <Button variant="link" onclick={() => (opened = null)}>Close</Button>
    {/snippet}
  </Sheet>
{/if}


<style>
  .line {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 0.75rem;
  }

  h2 {
    margin: 0;
    font-size: var(--t-body);
  }

  h3 {
    margin: 0;
    font-size: var(--t-small);
    letter-spacing: 0.02em;
    text-transform: uppercase;
    color: var(--dim);
  }

  h4 {
    margin: 1rem 0 0.375rem;
    font-size: var(--t-small);
    color: var(--readout);
  }

  code {
    font-family: var(--font-value);
    font-size: 0.8em;
    color: var(--dim);
  }

  .path {
    font-family: var(--font-value);
    word-break: break-all;
  }

  /* ---- readouts --------------------------------------------------------- */

  .readout {
    display: flex;
    flex-wrap: wrap;
    gap: 1.25rem;
    margin: 0.875rem 0 0;
  }

  .readout dt {
    font-size: var(--t-micro);
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--faint);
  }

  .readout dd {
    margin: 0.125rem 0 0;
    font-family: var(--font-value);
    font-size: var(--t-body);
    color: var(--readout);
  }

  .readout.wide dd {
    font-size: var(--t-small);
  }

  .readout dd.hot {
    color: var(--contest);
  }

  /* ---- the live feed ---------------------------------------------------- */

  .filters {
    display: flex;
    gap: 0.25rem;
  }

  .filters button {
    padding: 0.15rem 0.5rem;
    border-radius: var(--r-pill);
    color: var(--faint);
    font-size: var(--t-micro);
  }

  .filters button:hover {
    color: var(--readout);
  }

  .filters button.on {
    background: var(--glass-hi);
    color: var(--cyan);
  }

  .feed {
    margin: 0;
    padding: 0;
    list-style: none;
    max-height: 22rem;
    overflow-y: auto;
  }

  .event {
    display: grid;
    grid-template-columns: 5.5rem 5rem 1fr;
    gap: 0.5rem;
    padding: 0.1875rem 0;
    border-bottom: 1px solid var(--rule);
    font-size: var(--t-micro);
  }

  .event:last-child {
    border-bottom: 0;
  }

  .event .when {
    font-family: var(--font-value);
    color: var(--faint);
  }

  .event .cat {
    font-size: 0.625rem;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--faint);
  }

  .event .what {
    color: var(--dim);
    word-break: break-word;
  }

  .event.bad .what {
    color: var(--contest);
  }

  .event.loud .what {
    color: var(--signal);
  }

  .event.plainlevel .what {
    color: var(--readout);
  }

  .owner {
    margin-left: 0.375rem;
    padding: 0 0.3rem;
    border-radius: var(--r-sm);
    background: var(--glass-hi);
    color: var(--faint);
  }

  /* ---- the history ------------------------------------------------------ */

  .runs {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .runs li + li {
    border-top: 1px solid var(--rule);
  }

  .run {
    display: grid;
    grid-template-columns: 11rem 1fr auto;
    align-items: baseline;
    gap: 0.75rem;
    width: 100%;
    padding: 0.5rem 0;
    text-align: left;
  }

  .run:hover .verdict {
    color: var(--cyan);
  }

  .run .when {
    font-family: var(--font-value);
    font-size: var(--t-micro);
    color: var(--faint);
  }

  .run .verdict {
    color: var(--readout);
    font-size: var(--t-small);
  }

  .run .verdict.bad {
    color: var(--contest);
  }

  .badges {
    display: flex;
    flex-wrap: wrap;
    gap: 0.25rem;
    justify-content: flex-end;
  }

  /* ---- copies of the save ----------------------------------------------- */

  .copy {
    display: grid;
    grid-template-columns: 11rem 1fr auto auto;
    align-items: center;
    gap: 0.75rem;
    padding: 0.375rem 0;
  }

  .copy .when {
    font-family: var(--font-value);
    font-size: var(--t-micro);
    color: var(--faint);
  }

  .copy .slot {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    font-size: var(--t-small);
    color: var(--readout);
  }

  .copy .size {
    font-family: var(--font-value);
    font-size: var(--t-micro);
    color: var(--dim);
  }

  .rowverbs {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  /* ---- the details sheet ------------------------------------------------ */

  .plain {
    margin: 0;
    padding: 0;
    list-style: none;
    font-size: var(--t-small);
    color: var(--dim);
  }

  .plain li + li {
    margin-top: 0.375rem;
  }

  /* A save that did not write is the one line in a session worth colouring. */
  .plain li.bad {
    color: var(--contest);
  }

  /* A mod's name, which is also the way to everything else known about it.
     Underlined on hover rather than always: these lists are mostly names, and
     a page of permanent links reads as a page of decoration. */
  .plain .who {
    padding: 0;
    color: var(--readout);
    font-weight: 600;
    text-align: left;
  }

  .plain .who:hover {
    color: var(--cyan);
    text-decoration: underline;
    text-underline-offset: 2px;
  }

  .sample {
    display: block;
    margin-top: 0.1875rem;
    font-family: var(--font-value);
    font-size: var(--t-micro);
    color: var(--faint);
    word-break: break-all;
  }

  .report,
  .log {
    max-height: 18rem;
    margin: 0.375rem 0 0;
    padding: 0.625rem 0.75rem;
    overflow: auto;
    border: 1px solid var(--rule);
    border-radius: var(--r-sm);
    background: var(--inset);
    color: var(--dim);
    font-family: var(--font-value);
    font-size: var(--t-micro);
    line-height: 1.5;
    white-space: pre;
  }

  .log {
    max-height: 26rem;
  }
</style>
