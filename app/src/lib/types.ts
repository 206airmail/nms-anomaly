/** The shape the Rust engine emits.
 *
 * This is the contract between the analysis engine and this UI, produced by
 * `engine::report::to_json` and served by the `analyse_library` command.
 */

export type Severity = "CRITICAL" | "MAJOR" | "MINOR" | "INFO";

export interface Clash {
  /** Property path, e.g. `GenericTable[R_SCRAPHEAP]/List/.../PercentageChance` */
  path: string;
  /** mod name -> the value that mod claims. `null` means the mod omits it. */
  values: Record<string, string | null>;
  /** mods carrying an AMUMSS change marker, i.e. provably the author */
  attributed: string[];
}

export interface Conflict {
  target: string;
  mods: string[];
  severity: Severity;
  kind: string;
  /** true when the game merges these copies cleanly and nothing is lost */
  benign: boolean;
  summary: string;
  predicted_winner: string | null;
  unique_counts: Record<string, number>;
  declared_only: string[];
  notes: string[];
  clashes: Clash[];
  /** true when no two mods change the same property, so they can be combined
   *  rather than chosen between. null when the vanilla baseline could not be
   *  reached and the question was not answered -- never guessed. */
  mergeable: boolean | null;
  /** property paths more than one mod changes: the real decisions */
  overlap: string[];
  /** mod name -> how many properties that mod changes from vanilla */
  edit_counts: Record<string, number>;
}

export interface Mod {
  name: string;
  root: string;
  files: number;
  targets: string[];
  declared_targets: string[];
  amumss_version: string | null;
  source: string | null;
  disabled: boolean;
  max_version: string | null;
  min_version: string | null;
  /** ModPriority from the game. null = the game has not registered it yet. */
  priority: number | null;
}

export interface Stale {
  mod: string;
  version: string;
  reference: string;
  file_count: number;
  severity: Severity;
  examples: string[];
}

/** A file the game cannot read, so the mod silently does nothing. */
export interface BrokenFile {
  mod: string;
  rel_path: string;
  error: string;
}

/** One node whose world position differs from the game's own copy. */
export interface MovedNode {
  /** path from the scene root, e.g. `FREIGHTERBASE/BaseBuildingData` */
  path: string;
  /** LOCATOR / MESH / REFERENCE / ... */
  kind: string;
  before: [number, number, number];
  after: [number, number, number];
  distance: number;
  /** "X" | "Y" | "Z" for an axis-aligned move, "" otherwise */
  axis: string;
  /**
   * The node moved but everything underneath it stayed put.
   *
   * This is the signature of a rebake rather than an edit, and the only shape
   * worth raising: the scene still draws correctly, so nothing looks wrong
   * until the game reads the node as an anchor and finds it elsewhere.
   */
  contents_held: boolean;
  /** only the rotation/scale basis changed; the origin held */
  rotated_only: boolean;
}

/** A wholesale scene replacement that shifts nodes it did not mean to. */
export interface SceneDrift {
  mod: string;
  rel_path: string;
  target: string;
  severity: Severity;
  /** what it was compared against; currently always "vanilla" */
  reference: string;
  added: number;
  removed: number;
  moved: MovedNode[];
}

/** Which external tools the run had, and what it therefore could not check.
 *
 * Without this, a run that could not check anything and a genuinely clean
 * library produce the same empty result. The UI has to be able to tell them
 * apart, so it can say "not checked" instead of "nothing wrong".
 */
export interface ToolStatus {
  mbincompiler: string | null;
  hgpaktool: string | null;
  scene_check: boolean;
  scene_check_note: string | null;
}

export interface Stats {
  mods: number;
  files: number;
  targets: number;
  exml: number;
  mbin: number;
  dds: number;
  lua: number;
  decompiled: number;
  parse_errors: number;
  error_details: string[];
}

export interface Report {
  roots: string[];
  winner_rule: "first" | "last";
  reference_version: string | null;
  stats: Stats;
  actionable_count: number;
  merged_count: number;
  real_load_order: boolean;
  manager: string | null;
  disable_all: boolean;
  disabled: string[];
  unregistered: string[];
  mods: Mod[];
  conflicts: Conflict[];
  broken: BrokenFile[];
  drift: SceneDrift[];
  tools: ToolStatus;
  stale: Stale[];
  loc_clashes: { id: string; mods: string[] }[];
}

/** Conflicts that need a decision. Benign overlaps are not among them. */
export function actionable(report: Report): Conflict[] {
  return report.conflicts.filter((c) => !c.benign);
}

/** Scene changes that need a decision, as opposed to a note. */
export function seriousDrift(report: Report): SceneDrift[] {
  return (report.drift ?? []).filter((d) => d.severity !== "INFO");
}

/** The moves within one finding that are worth showing: the rebakes. */
export function rebaked(drift: SceneDrift): MovedNode[] {
  return drift.moved.filter((m) => m.contents_held);
}

/** What one rescan changed, so a refresh reports rather than just redraws. */
export interface ScanDelta {
  addedMods: string[];
  removedMods: string[];
  conflicts: number;
  broken: number;
  drift: number;
}

/** Compare two scans. Returns null when nothing worth mentioning moved. */
export function diffReports(prev: Report, next: Report): ScanDelta | null {
  const before = new Set(prev.mods.map((m) => m.name));
  const after = new Set(next.mods.map((m) => m.name));
  const delta: ScanDelta = {
    addedMods: [...after].filter((n) => !before.has(n)).sort(),
    removedMods: [...before].filter((n) => !after.has(n)).sort(),
    conflicts: actionable(next).length - actionable(prev).length,
    broken: (next.broken ?? []).length - (prev.broken ?? []).length,
    drift: seriousDrift(next).length - seriousDrift(prev).length,
  };
  const quiet =
    delta.addedMods.length === 0 &&
    delta.removedMods.length === 0 &&
    delta.conflicts === 0 &&
    delta.broken === 0 &&
    delta.drift === 0;
  return quiet ? null : delta;
}

/** One line summarising a rescan, in the order a reader cares about. */
export function describeDelta(d: ScanDelta): string {
  const parts: string[] = [];
  const list = (names: string[], verb: string) =>
    names.length <= 2
      ? `${verb} ${names.join(" and ")}`
      : `${verb} ${names.length} mods`;
  if (d.addedMods.length) parts.push(list(d.addedMods, "added"));
  if (d.removedMods.length) parts.push(list(d.removedMods, "removed"));

  const moved = (n: number, one: string, many: string) =>
    n > 0
      ? `${n} new ${n === 1 ? one : many}`
      : `${-n} fewer ${-n === 1 ? one : many}`;
  if (d.broken !== 0) parts.push(moved(d.broken, "broken file", "broken files"));
  if (d.drift !== 0) parts.push(moved(d.drift, "misplaced anchor", "misplaced anchors"));
  if (d.conflicts !== 0) parts.push(moved(d.conflicts, "conflict", "conflicts"));
  return parts.join(", ");
}

/** Mods in the order the game applies them - the spine of the whole UI. */
export function byLoadOrder(mods: Mod[]): Mod[] {
  return [...mods].sort((a, b) => {
    if (a.priority === null && b.priority === null)
      return a.name.localeCompare(b.name);
    if (a.priority === null) return 1; // unregistered folders sink to the end
    if (b.priority === null) return -1;
    return a.priority - b.priority;
  });
}

/**
 * Order a clash's claims so the losers read first and the winner lands last.
 *
 * The winner is the point of the stack, so it sits at the bottom where the eye
 * stops, the way a total sits under a column of figures.
 */
export function orderedClaims(
  clash: Clash,
  winner: string | null,
): { mod: string; value: string | null; wins: boolean; authored: boolean }[] {
  return Object.entries(clash.values)
    .map(([mod, value]) => ({
      mod,
      value,
      wins: mod === winner,
      authored: clash.attributed.includes(mod),
    }))
    .sort((a, b) => Number(a.wins) - Number(b.wins) || a.mod.localeCompare(b.mod));
}

/** Short asset name for a heading; the full path stays available underneath. */
export function assetName(target: string): string {
  return target.split("/").pop() ?? target;
}

/**
 * A merge's folder name, said as the asset it settles.
 *
 * `merge::folder_for` builds the folder out of the asset path, so the name
 * reads back: the last segment is the file that was merged. Worth doing,
 * because `zzz_nmscheck_METADATA_REALITY_TABLES_REWARDTABLE` is a name the
 * user never chose and would not recognise.
 */
const MERGE_PREFIX = "zzz_nmscheck_";

export function mergeAssetName(owner: string): string {
  // Only ours can be read back. Anything else keeps its whole name: taking the
  // last underscore-separated word off a folder we did not build turns
  // "not_one_of_ours" into "ours", which is worse than saying nothing clever.
  if (!owner.startsWith(MERGE_PREFIX)) return owner;
  const last = owner.slice(MERGE_PREFIX.length).split("_").filter(Boolean).pop();
  return last ?? owner;
}

/**
 * What just happened when a merge was built, in the user's words.
 *
 * Shared because it was written twice -- once on the action card and once in
 * the evidence pane -- and both copies said "the two it was made from"
 * regardless of how many there were. A three-way merge reported itself wrongly
 * in both places.
 */
export function combinedNote(inputs: string[]): string {
  const one = inputs.length === 1;
  const them = one ? "the mod" : `the ${inputs.length === 2 ? "two" : inputs.length} mods`;
  return (
    `Combined into its own mod, and ${them} it was made from ${one ? "is" : "are"} now held out ` +
    `of the game so ${one ? "it cannot" : "they cannot"} override it. Nothing of ${one ? "its" : "theirs"} ` +
    `was changed or deleted — deactivate the merge in your Library to get ${one ? "it" : "them"} back.`
  );
}

/**
 * Why a combined mod disappeared, in the user's words.
 *
 * A merge is a single built asset with its inputs' edits already inside it, so
 * changing or removing one of those inputs makes it a stand-in for something
 * that no longer exists -- it has to go, and the mods it was holding back come
 * out of its shadow. That is not a failure and not an error, but it *is* a
 * thing that happened to the library without being asked for, so it is said
 * plainly rather than left for the user to notice a mod had come back.
 *
 * `because` completes "it was built from …".
 */
export function unmergedNote(merges: string[], because: string): string | null {
  if (!merges.length) return null;
  const names = merges.map(mergeAssetName).join(", ");
  const one = merges.length === 1;
  return (
    `Combined ${names} removed — ${one ? "it was" : "they were"} built from ${because}. ` +
    `The mods ${one ? "it stood" : "they stood"} in for are back in the game; ` +
    `combine them again whenever you like.`
  );
}

export function assetFolder(target: string): string {
  const parts = target.split("/");
  parts.pop();
  return parts.join("/");
}
