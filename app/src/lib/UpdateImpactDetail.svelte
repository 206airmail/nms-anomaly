<script lang="ts">
  /** Everything one game update did to one mod file, as a table.
   *
   * Its own component rather than markup inside the sheet that opens it, for two
   * reasons. The four verdicts are the engine's vocabulary and each needs a
   * sentence saying what it means, which is more prose than a sheet body should
   * carry inline. And a component can be rendered on its own, so `check:ui` can
   * assert that all four still reach the screen -- a verdict renamed in Rust and
   * not here would otherwise just stop appearing, silently.
   */
  import type { FileImpact, Moved } from "./engine";

  interface Props {
    file: FileImpact;
    /** rows past this are counted rather than drawn */
    most?: number;
  }

  let { file, most = 40 }: Props = $props();

  /** The four verdicts, worst first, each with what it means in words. */
  const rows = $derived(
    [
      {
        label: "Takes back",
        means:
          "the mod still carries the value the game had before the update, so loading it undoes the change. The author did not choose this: they agreed with the game, and the game moved.",
        moves: file.reverts,
      },
      {
        label: "Deletes",
        means:
          "the update added these and this copy of the whole file does not contain them, so loading it takes them away again.",
        moves: file.drops,
      },
      {
        label: "Does nothing",
        means:
          "the update removed these, so the mod's edit has nothing left to land on. Whatever it was for is gone.",
        moves: file.dead,
      },
      {
        label: "Deliberate",
        means:
          "the mod sets its own value here and the game's moved underneath it. Not a fault — but the author picked that number against an older game.",
        moves: file.overridden,
      },
    ].filter((row) => row.moves.length > 0),
  );

  function shorten(value: string | null): string {
    if (value === null || value === "") return "—";
    return value.length > 40 ? `${value.slice(0, 39)}…` : value;
  }

  function key(move: Moved): string {
    return move.path;
  }
</script>

<p class="lede">{file.summary}</p>
<p class="assetpath">{file.rel_path}</p>
<p class="hint">
  {#if file.whole_file}
    This mod ships the whole compiled file, so everything below applies whether or
    not another mod touches the same asset. Cleaning it — rewriting it as a patch
    holding only its real edits — makes all of it go away at once.
  {:else}
    This is a sparse patch, so only the properties it names get written. It is
    still carrying values it never meant to set.
  {/if}
</p>

{#each rows as row (row.label)}
  <h4>{row.label} ({row.moves.length})</h4>
  <p class="hint">{row.means}</p>
  <table class="moves">
    <thead>
      <tr>
        <th>property</th>
        <th>game, before</th>
        <th>game, now</th>
        <th>this mod writes</th>
      </tr>
    </thead>
    <tbody>
      {#each row.moves.slice(0, most) as move (key(move))}
        <tr>
          <td class="assetpath">{move.path}</td>
          <td class="was">{shorten(move.before)}</td>
          <td class="now">{shorten(move.after)}</td>
          <td class="mine">{shorten(move.mod_value)}</td>
        </tr>
      {/each}
    </tbody>
  </table>
  {#if row.moves.length > most}
    <p class="hint">…and {row.moves.length - most} more of the same kind.</p>
  {/if}
{/each}

<style>
  h4 {
    margin: 1.25rem 0 0.25rem;
    font-size: var(--t-small);
    color: var(--readout);
  }

  .assetpath {
    font-family: var(--font-value);
    font-size: var(--t-micro);
    color: var(--dim);
    word-break: break-all;
  }

  .moves {
    width: 100%;
    margin: 0.5rem 0 0;
    border-collapse: collapse;
    font-family: var(--font-value);
    font-size: var(--t-micro);
  }

  .moves th {
    text-align: left;
    padding: 0.3rem 0.5rem;
    border-bottom: 1px solid var(--rule);
    color: var(--faint);
    font-weight: 600;
  }

  .moves td {
    padding: 0.25rem 0.5rem;
    border-bottom: 1px solid var(--film);
    vertical-align: top;
  }

  .was {
    color: var(--faint);
  }

  .now {
    color: var(--dim);
  }

  /* Amber, and only here: of the three values on the row this is the one the
     game actually loads. That is the colour's single job. */
  .mine {
    color: var(--signal);
  }
</style>
