<script lang="ts">
  /** What the last game update changed underneath the mods.
   *
   * The Actions tab's other rows come out of the scan, which runs every launch.
   * This one does not, and the reason is worth knowing before changing anything
   * here: the survey decompiles two copies of every asset the library touches,
   * which is minutes on a first run, and the answer only changes when Hello
   * Games ship a patch. Putting it on the launch path is the mistake that once
   * cost this app 76 seconds of start-up.
   *
   * ## Why this is not the "outdated mods" list
   *
   * That list is hidden by default, and rightly: "built for 6.45" is a date, not
   * a fault, and most mods with an old stamp work perfectly. This panel answers
   * the question that list only gestured at — of the properties this mod sets,
   * which ones did *the game itself* change since? A mod still carrying the
   * pre-patch value hands the patch straight back, silently, whether or not
   * anything conflicts with it.
   *
   * ## Two states that must not look alike
   *
   * "Checked, and your mods take nothing back" and "could not check" are
   * opposite answers, and an empty list says both. So `blocked` is drawn as its
   * own thing with the reason in it, and the clean result names how many assets
   * it compared to get there.
   */
  import Awaiting from "./Awaiting.svelte";
  import Button from "./Button.svelte";
  import Sheet from "./Sheet.svelte";
  import UpdateImpactDetail from "./UpdateImpactDetail.svelte";
  import { names } from "./names.svelte";
  import {
    impactHarm,
    updateImpact,
    vanillaPrepare,
    type FileImpact,
    type UpdateImpact,
  } from "./engine";

  interface Props {
    modsDir?: string;
    gameRoot?: string;
    /** open a mod's record, as the rest of the app does */
    onmod?: (folder: string) => void;
  }

  let { modsDir, gameRoot, onmod }: Props = $props();

  let impact = $state<UpdateImpact | null>(null);
  let running = $state(false);
  let preparing = $state(false);
  let failure = $state<string | null>(null);
  let prepared = $state<string | null>(null);
  let opened = $state<FileImpact | null>(null);

  const harm = $derived(impact ? impactHarm(impact) : 0);

  async function check() {
    running = true;
    failure = null;
    prepared = null;
    try {
      impact = await updateImpact(modsDir, gameRoot);
    } catch (err) {
      failure = String(err);
    } finally {
      running = false;
    }
  }

  /** Cache vanilla for everything, so the *next* update can be judged in full. */
  async function prepare() {
    preparing = true;
    failure = null;
    try {
      const done = await vanillaPrepare(modsDir, gameRoot);
      if (done) {
        prepared =
          `Baseline laid down: ${done.cached} of ${done.asked} assets cached` +
          (done.absent
            ? `, ${done.absent} of them not shipped by the game at all (mods invented those).`
            : ".");
        if (done.error) failure = done.error;
      }
    } catch (err) {
      failure = String(err);
    } finally {
      preparing = false;
    }
  }

</script>

<section class="panel">
  <header class="hud-label">This game update</header>

  {#if running}
    <Awaiting>
      Comparing the game's own files before and after the update…
    </Awaiting>
  {:else if !impact}
    <p class="lede">What did the update change underneath your mods?</p>
    <p class="hint">
      Not the version stamp — the values. A mod that ships a whole game file was
      built when that file looked different, so it quietly reverts everything Hello
      Games changed in it since. Nothing errors when that happens, which is why it
      is worth asking.
    </p>
    <div class="verbs">
      <Button variant="primary" onclick={check}>Check what the update changed</Button>
    </div>
  {:else}
    {#if impact.blocked}
      <p class="lede">Not yet answerable</p>
      <p class="warn">{impact.blocked}</p>
      <p class="hint">
        The comparison works by keeping the game's own files from before an update
        and diffing them against the ones after. It needs a baseline on disk
        already, because once a patch lands the old archives are gone and there is
        no recovering them.
      </p>
    {:else}
      <p class="lede">
        {#if harm}
          <span class="badge badge-alert">{harm} values undone</span>
        {:else}
          <span class="badge badge-info">nothing taken back</span>
        {/if}
        {impact.verdict}
      </p>

      {#if impact.from && impact.to}
        <p class="hint">
          Comparing build <span class="key">{impact.from.key.slice(0, 8)}</span>
          against <span class="key">{impact.to.key.slice(0, 8)}</span>;
          {impact.compared} assets your mods touch were in both, and the update moved
          {impact.touched} of them.
          {#if !impact.from.dated}
            The older build's date came from its folder timestamp rather than a
            stamp written at the time, so treat it as approximate.
          {/if}
        </p>
      {/if}

      {#if impact.files.length}
        <ul class="findings">
          {#each impact.files as file (file.owner + file.rel_path)}
            <li>
              <button class="openrow" onclick={() => (opened = file)}>
                <span class="badge {file.severity === 'CRITICAL' ? 'badge-alert' : 'badge-signal'}">
                  {file.severity}
                </span>
                <span class="who">{names.of(file.owner)}</span>
                <span class="says">{file.summary}</span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}

      {#if impact.unaffected.length}
        <p class="hint">
          {impact.unaffected.length} mods were checked against this update and are
          clear. That is the answer the outdated-version list could never give.
        </p>
      {/if}
    {/if}

    {#if impact.uncomparable.length}
      <p class="hint">
        {impact.uncomparable.length} assets could not be judged: nothing was cached
        for them before the update, and the old archives are gone. Laying down a
        full baseline now closes that gap for the next one.
      </p>
    {/if}

    <div class="verbs">
      <Button onclick={check} busy={running}>Check again</Button>
      <Button variant="primary" onclick={prepare} busy={preparing}>
        {preparing ? "Extracting…" : "Prepare the baseline for next time"}
      </Button>
    </div>
  {/if}

  {#if prepared}<p class="ok">{prepared}</p>{/if}
  {#if failure}<p class="failed">{failure}</p>{/if}
</section>

{#if opened}
  {@const file = opened}
  <Sheet title="{names.of(file.owner)} and this update" wide onclose={() => (opened = null)}>
    <UpdateImpactDetail {file} />

    {#snippet verbs()}
      {#if onmod}
        <Button
          variant="primary"
          onclick={() => {
            onmod?.(file.owner);
            opened = null;
          }}
        >
          Open this mod
        </Button>
      {/if}
      <Button variant="link" onclick={() => (opened = null)}>Close</Button>
    {/snippet}
  </Sheet>
{/if}

<style>
  .findings {
    list-style: none;
    margin: 0.75rem 0 0;
    padding: 0;
    display: grid;
    gap: 0.3rem;
  }

  .openrow {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    gap: 0.5rem;
    width: 100%;
    padding: 0.55rem 0.7rem;
    background: var(--card);
    border: 1px solid var(--rule);
    border-radius: var(--r-md);
    color: var(--dim);
    text-align: left;
    cursor: pointer;
    transition: background var(--quick) var(--ease);
  }

  .openrow:hover {
    background: var(--card-hi);
  }

  .who {
    color: var(--readout);
  }

  .says {
    flex: 1 1 18rem;
  }

  .key {
    font-family: var(--font-value);
    font-size: var(--t-micro);
    color: var(--dim);
  }

</style>
