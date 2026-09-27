/** Calls into the Rust engine.
 *
 * Everything here degrades to `null` when the page is open in a plain browser
 * (`npm run dev` without Tauri), so the UI can still be worked on with only
 * the fixtures.
 */

import { invoke } from "@tauri-apps/api/core";
import type { Report } from "./types";

export interface Install {
  root: string;
  source: string;
  mods_dir: string;
  has_mods_dir: boolean;
}

function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

/** The install that would be scanned, or null if the game was not found. */
export async function findInstall(): Promise<Install | null> {
  if (!inTauri()) return null;
  try {
    return await invoke<Install | null>("find_install");
  } catch {
    return null;
  }
}

/** Every install found, best source first. */
export async function findInstalls(): Promise<Install[]> {
  if (!inTauri()) return [];
  try {
    return await invoke<Install[]>("find_installs");
  } catch {
    return [];
  }
}

/**
 * Scan and analyse a mod library with the Rust engine.
 *
 * Returns `null` outside Tauri so the page can still be developed against the
 * fixtures in a plain browser. A real failure -- no install, an unreadable
 * folder, a library with nothing in it -- throws with the engine's message,
 * because that is something the user needs to read rather than a blank screen.
 */
export async function analyseLibrary(
  modsDir?: string,
  gameRoot?: string,
): Promise<Report | null> {
  if (!inTauri()) return null;
  return await invoke<Report>("analyse_library", {
    modsDir: modsDir ?? null,
    gameRoot: gameRoot ?? null,
  });
}

/**
 * Combine the mods contesting one asset into a single merged mod.
 *
 * `only` narrows the merge to a subset of the mods contesting the asset, for
 * the case where the rest are better *cleaned* than merged; leave it out to
 * combine everybody. The merge then stands in for exactly the mods named, and
 * the others stay in the game.
 *
 * Returns the path written. Throws with the engine's reason when the copies
 * overlap, or when an edit could not be placed -- a partial merge is never
 * written, because a file that looks like both mods while doing less than
 * either is worse than the conflict.
 */
export async function mergeConflict(
  target: string,
  only?: string[],
  modsDir?: string,
  gameRoot?: string,
): Promise<string | null> {
  if (!inTauri()) return null;
  return await invoke<string>("merge_conflict", {
    target,
    only: only ?? null,
    modsDir: modsDir ?? null,
    gameRoot: gameRoot ?? null,
  });
}

/** One whole-file override and what it would be reduced to. */
export interface CleanPlan {
  /** the mod folder the game loads this from */
  owner: string;
  target: string;
  /** where the file sits inside that folder */
  rel: string;
  /** every property in the file, containers as well as leaves */
  original: number;
  /**
   * properties the mod genuinely changes. **Leaves only.**
   *
   * So this is not `original` minus the dead weight, and `original > edits`
   * does not mean there is dead weight: a nested table counts its
   * `<Property name="Table">` containers in `original`, and a container can
   * never be an edit. Use `dropped`.
   */
  edits: number;
  /** properties cleaning would actually remove. Zero means it would do nothing. */
  dropped: number;
  /**
   * true when this copy replaces the asset outright — a compiled `.MBIN`.
   *
   * Only a whole-file copy can host a merge, and only a whole-file copy reverts
   * the game's own changes wholesale. A `.EXML` planned here is a patch that
   * carries values it does not edit: the same accident at a smaller scale.
   */
  whole_file: boolean;
  /** why this one cannot be cleaned; null when it can */
  refused: string | null;
  /** true when a cleaned build of this file is what the game is loading */
  cleaned: boolean;
}

/**
 * What cleaning every whole-file override in the library would do.
 *
 * Writes nothing. The refusals come back as entries with a reason rather than
 * being dropped, because "this one cannot be cleaned, and here is why" is the
 * more useful answer.
 */
export async function cleanPreview(
  modsDir?: string,
  gameRoot?: string,
): Promise<CleanPlan[]> {
  if (!inTauri()) return [];
  return await invoke<CleanPlan[]>("clean_preview", {
    modsDir: modsDir ?? null,
    gameRoot: gameRoot ?? null,
  });
}

/**
 * Replace one mod's overrides with the edits they actually make.
 *
 * The mod itself is not changed. A cleaned *build* of it is written beside the
 * copy its author shipped and the game is switched over to that, so `cleanUndo`
 * is a switch back rather than a restore and there is nothing that can fail to
 * come back. Returns a line per asset touched.
 */
export async function cleanApply(
  owner: string,
  modsDir?: string,
  gameRoot?: string,
): Promise<string[]> {
  if (!inTauri()) return [];
  return await invoke<string[]>("clean_apply", {
    owner,
    modsDir: modsDir ?? null,
    gameRoot: gameRoot ?? null,
  });
}

/** What cleaning one mod did, or why it did nothing. */
export interface Cleaned {
  owner: string;
  /** a line per asset reduced */
  done: string[];
  /** why this one was left alone; null when it was cleaned */
  failed: string | null;
}

/**
 * Clean several mods at once.
 *
 * One call rather than one per mod, for the same reason `setModsEnabled` takes
 * a list: the expensive work — scanning, locating the tools, planning every
 * override against the game's own copy — is per *library*, and every separate
 * call would end in its own reconcile of the mods folder.
 *
 * A mod that cannot be cleaned comes back with its reason against its own name
 * and the rest still happen.
 */
export async function cleanApplyMany(
  owners: string[],
  modsDir?: string,
  gameRoot?: string,
): Promise<Cleaned[]> {
  if (!inTauri()) return [];
  return await invoke<Cleaned[]>("clean_apply_many", {
    owners,
    modsDir: modsDir ?? null,
    gameRoot: gameRoot ?? null,
  });
}

/** Put a mod back to the build its author shipped. Returns how many changed. */
export async function cleanUndo(
  owner: string,
  modsDir?: string,
  gameRoot?: string,
): Promise<number> {
  if (!inTauri()) return 0;
  return await invoke<number>("clean_undo", {
    owner,
    modsDir: modsDir ?? null,
    gameRoot: gameRoot ?? null,
  });
}

// ---------------------------------------------------------------------------
// Changing a value a mod sets
// ---------------------------------------------------------------------------

/** What kind of value sits at a property, which decides the input drawn. */
export type ValueSort = "bool" | "int" | "float" | "text";

/** One property a mod changes, with everywhere its value comes from. */
export interface EditField {
  /** the property path, e.g. `Table/Row[LAUNCHER]/Cost` */
  path: string;
  /** what the installed game has here. null when the mod introduces it. */
  vanilla: string | null;
  /** what the mod's author ships here */
  author: string;
  /** what you set, when you have set something */
  yours: string | null;
  sort: ValueSort;
  /**
   * The author's value has moved since you set yours.
   *
   * The mod updated and no longer ships the number your edit was a reaction
   * to. Your value still applies — this is the value it replaced.
   */
  author_moved: string | null;
}

/** Every property of one asset that its mod changes. */
export interface EditAsset {
  /** the folder the game loads this from */
  folder: string;
  /** `<folder>/<path inside it>`, which is how a change names its file */
  file: string;
  /** where the file sits inside that folder */
  rel: string;
  /** the canonical asset key */
  target: string;
  /** true when this copy replaces the asset outright — a compiled `.MBIN` */
  whole_file: boolean;
  /** false when the game ships no copy of this asset: the mod adds it, so
   *  nothing in it has a vanilla value */
  in_game: boolean;
  fields: EditField[];
  /** why nothing here can be edited; null when it can */
  refused: string | null;
}

/** One held value the mod can no longer carry. */
export interface StaleValue {
  file: string;
  path: string;
  /** the value still held for it */
  value: string;
  /** why it can no longer be carried */
  why: string;
}

export interface EditSurvey {
  owner: string;
  assets: EditAsset[];
  /**
   * Held values the mod cannot carry any more.
   *
   * An update can take a file away or move a property. The value set on it then
   * stops being applicable *and* stops being visible, so it is named here
   * instead of sitting in `edits.json` for ever.
   */
  stale: StaleValue[];
}

/**
 * Every property one mod changes, with the game's value and yours beside it.
 *
 * Read-only, and per mod on demand: this reads every asset the mod ships,
 * which is more than the start-up scan looks at.
 */
export async function editSurvey(
  owner: string,
  modsDir?: string,
  gameRoot?: string,
): Promise<EditSurvey> {
  if (!inTauri()) return demoSurvey(owner);
  return await invoke<EditSurvey>("edit_survey", {
    owner,
    modsDir: modsDir ?? null,
    gameRoot: gameRoot ?? null,
  });
}

/** One value to set, or to put back to the author's. */
export interface WantedValue {
  file: string;
  path: string;
  /** the new value, or null to go back to the author's */
  value: string | null;
  /** the author's value the box was showing */
  author: string;
}

/** What setting values on a mod produced. */
export interface Applied {
  /** a line per file rewritten */
  done: string[];
  /**
   * Values that could not be applied.
   *
   * A mod that updated may have moved or removed the property a value was set
   * on. The rest are applied and these are named, rather than the build being
   * refused or the loss going unmentioned.
   */
  lost: string[];
}

/**
 * Set values on one mod and switch the game over to the build carrying them.
 *
 * Every change in one call: a rebuild copies the mod, may run MBINCompiler,
 * and relinks the mods folder, so this is not something to do per keystroke.
 */
export async function editApply(
  owner: string,
  changes: WantedValue[],
  modsDir?: string,
  gameRoot?: string,
): Promise<Applied> {
  if (!inTauri()) return { done: [], lost: [] };
  return await invoke<Applied>("edit_apply", {
    owner,
    changes,
    modsDir: modsDir ?? null,
    gameRoot: gameRoot ?? null,
  });
}

/**
 * Forget every value set on one mod and put its own values back.
 *
 * Whichever build was underneath stays: a cleaned mod stays cleaned. Going all
 * the way back to the author's copy is `cleanUndo`. Returns how many values
 * were dropped.
 */
export async function editUndo(
  owner: string,
  modsDir?: string,
  gameRoot?: string,
): Promise<number> {
  if (!inTauri()) return 0;
  return await invoke<number>("edit_undo", {
    owner,
    modsDir: modsDir ?? null,
    gameRoot: gameRoot ?? null,
  });
}

/**
 * Forget held values the mod can no longer carry.
 *
 * Changes nothing the game reads: the build already does not carry them. It only
 * stops them sitting in the record unseen.
 */
export async function editForgetValues(
  owner: string,
  values: StaleValue[],
): Promise<number> {
  if (!inTauri()) return 0;
  return await invoke<number>("edit_forget_values", { owner, values });
}

/**
 * A stand-in survey for running the UI outside Tauri.
 *
 * The editor is the one pane whose whole subject — a property path, its
 * values, a box — cannot be seen at all without data, and the engine returns
 * nothing under `npm run dev`. Small on purpose: enough to show a float, an
 * int, a bool, an enum, an introduced property and a value already set.
 *
 * Exported because the render check draws the editor under Node, where nothing
 * can be awaited, and hands this in as the editor's `initial` survey.
 */
export function demoSurvey(owner: string): EditSurvey {
  const field = (
    path: string,
    vanilla: string | null,
    author: string,
    sort: ValueSort,
    yours: string | null = null,
  ): EditField => ({ path, vanilla, author, yours, sort, author_moved: null });
  return {
    owner,
    stale: [],
    assets: [
      {
        folder: owner,
        file: `${owner}/METADATA/REALITY/TABLES/REWARDTABLE.MBIN`,
        rel: "METADATA/REALITY/TABLES/REWARDTABLE.MBIN",
        target: "METADATA/REALITY/TABLES/REWARDTABLE.MBIN",
        whole_file: true,
        in_game: true,
        refused: null,
        fields: [
          field(
            "Table/GenericTable[R_SCRAPHEAP]/List/Reward[0]/PercentageChance",
            "25.000000",
            "60.000000",
            "float",
            "45.000000",
          ),
          field(
            "Table/GenericTable[R_SCRAPHEAP]/List/Reward[0]/AmountMin",
            "1",
            "4",
            "int",
          ),
          field(
            "Table/GenericTable[R_SCRAPHEAP]/List/Reward[1]/CanBeOverridden",
            "False",
            "True",
            "bool",
          ),
          field(
            "Table/GenericTable[R_SCRAPHEAP]/List/Reward[1]/Id",
            "CATALYST1",
            "TRITIUM",
            "text",
          ),
          // Introduced by the mod: the game has no such row.
          field(
            "Table/GenericTable[R_SALVAGE_NEW]/List/Reward[0]/AmountMin",
            null,
            "2",
            "int",
          ),
        ],
      },
      {
        folder: owner,
        file: `${owner}/GLOBALS/GCAUDIOGLOBALS.GLOBAL.EXML`,
        rel: "GLOBALS/GCAUDIOGLOBALS.GLOBAL.EXML",
        target: "GLOBALS/GCAUDIOGLOBALS.GLOBAL.MBIN",
        whole_file: false,
        in_game: true,
        refused: null,
        fields: [
          field("MusicVolume", "1.000000", "0.400000", "float"),
        ],
      },
    ],
  };
}

/** Who a Nexus API key belongs to. */
export interface NexusAccount {
  name: string;
  /**
   * Non-premium keys can read everything the update check needs but cannot ask
   * the API for a download link, so installing an update from inside the app
   * needs either premium or an `nxm://` handoff from the website.
   */
  is_premium: boolean;
}

/** How one installed mod stands against its Nexus page. */
export type UpdateCheck = {
  owner: string;
  mod_id: number | null;
  recorded_version: string | null;
} & (
  | { state: "current" }
  | {
      state: "outdated";
      latest_version: string;
      latest_name: string;
      latest_file_id: number;
      page: string;
    }
  | { state: "record_stale"; actual_version: string }
  | { state: "withdrawn"; installed_version: string }
  | { state: "unknown"; reason: string }
);

export interface UpdateReport {
  checks: UpdateCheck[];
  budget: { hourly_remaining: number; daily_remaining: number };
}

/** Save a key, after Nexus has confirmed it. Throws if it is rejected. */
export async function nexusSetKey(key: string): Promise<NexusAccount | null> {
  if (!inTauri()) return null;
  return await invoke<NexusAccount>("nexus_set_key", { key });
}

/** Who the stored key belongs to, or `null` when none is stored. */
export async function nexusAccount(): Promise<NexusAccount | null> {
  if (!inTauri()) return null;
  return await invoke<NexusAccount | null>("nexus_account");
}

export async function nexusForgetKey(): Promise<void> {
  if (!inTauri()) return;
  await invoke("nexus_forget_key");
}

/**
 * Ask Nexus about the installed mods. One request per mod page.
 *
 * `owners` narrows it to those mod folders, which is one request rather than
 * sixty — cheap enough to re-ask about a single mod whenever this program's own
 * record of it changes.
 */
export async function checkUpdates(
  modsDir?: string,
  owners?: string[],
): Promise<UpdateReport | null> {
  if (!inTauri()) return null;
  return await invoke<UpdateReport>("check_updates", {
    modsDir: modsDir ?? null,
    owners: owners ?? null,
  });
}

/** What we know about one installed mod without asking anyone. */
export interface Identity {
  /** the mod folder, which is how the game and this program refer to it */
  owner: string;
  /** the title a person would recognise: the page, else the archive, else the
   *  folder. Never empty. See `modNames`. */
  name: string;
  root: string;
  mod_id: number | null;
  version: string | null;
  page: string | null;
  priority: number | null;
  disabled: boolean;
  files: number;
  assets: number;
  /** the mod manager that deployed this, when one did */
  managed_by: string | null;
}

/** A mod's Nexus page, as much of it as the API gives. */
export interface ModPage {
  mod_id: number | null;
  name: string | null;
  version: string | null;
  summary: string | null;
  description: string | null;
  picture_url: string | null;
  author: string | null;
  uploaded_by: string | null;
  uploaded_users_profile_url: string | null;
  endorsement_count: number | null;
  mod_downloads: number | null;
  mod_unique_downloads: number | null;
  updated_time: string | null;
  created_time: string | null;
  status: string | null;
  contains_adult_content: boolean;
  available: boolean;
}

export interface Removal {
  owner: string;
  moved: string[];
  trash: string;
  managed_by: string | null;
}

export interface InstallPlan {
  owner: string;
  files: number;
  collides: boolean;
  notes: string[];
}

export async function libraryList(modsDir?: string): Promise<Identity[]> {
  if (!inTauri()) return [];
  return await invoke<Identity[]>("library_list", { modsDir: modsDir ?? null });
}

/** Mod folder -> the name to show for it. */
export type NameMap = Record<string, string>;

/**
 * The name for every installed mod, from what is already on disk.
 *
 * Instant and offline: archive names plus whatever `resolveNames` has resolved
 * on a previous run. Asked for on start-up so no screen has to render a folder
 * name like `gFreighter Perfect Frigates-2468-6-0-5-0a-1758072172` while
 * waiting for the network.
 */
export async function modNames(modsDir?: string): Promise<NameMap> {
  if (!inTauri()) return {};
  return await invoke<NameMap>("mod_names", { modsDir: modsDir ?? null });
}

/**
 * Ask Nexus for the real title of every mod whose page has never been read.
 *
 * One request per page, only for the ones missing from the cache, and the
 * answers are kept on disk -- so this costs about sixty requests once and
 * nothing on every launch after. Returns the complete map, not just the new
 * entries, so the caller replaces rather than merges.
 */
export async function resolveNames(modsDir?: string): Promise<NameMap> {
  if (!inTauri()) return {};
  return await invoke<NameMap>("resolve_names", { modsDir: modsDir ?? null });
}

/** Throw the resolved names away, so the next resolve asks Nexus again. */
export async function forgetNames(): Promise<void> {
  if (!inTauri()) return;
  await invoke("forget_names");
}

export async function nexusMod(modId: number): Promise<ModPage | null> {
  if (!inTauri()) return null;
  return await invoke<ModPage>("nexus_mod", { modId });
}

/** Which paths in the mods folder belong to one mod. */
export async function modMembers(owner: string, modsDir?: string): Promise<string[]> {
  if (!inTauri()) return [];
  return await invoke<string[]>("mod_members", { owner, modsDir: modsDir ?? null });
}

/** Move a mod out of the mods folder. Nothing is deleted; see `restoreMod`. */
export async function removeMod(owner: string, modsDir?: string): Promise<Removal | null> {
  if (!inTauri()) return null;
  return await invoke<Removal>("remove_mod", { owner, modsDir: modsDir ?? null });
}

export async function restoreMod(trash: string, modsDir?: string): Promise<string[]> {
  if (!inTauri()) return [];
  return await invoke<string[]>("restore_mod", { trash, modsDir: modsDir ?? null });
}

export async function installPreview(
  archive: string,
  modsDir?: string,
): Promise<InstallPlan | null> {
  if (!inTauri()) return null;
  return await invoke<InstallPlan>("install_preview", { archive, modsDir: modsDir ?? null });
}

/**
 * Show a Nexus page in the built-in browser, on the Browse tab.
 *
 * Not an iframe: Nexus forbids framing. Not a second window either, which is
 * what this used to do — browsing is part of using this program, so it
 * belongs in it. `title` is unused now that the page has a tab of its own,
 * and is kept so callers read the same.
 */
export async function openModPage(url: string, _title?: string): Promise<void> {
  if (!inTauri()) {
    window.open(url, "_blank", "noreferrer");
    return;
  }
  const { browser } = await import("./browser.svelte");
  browser.request(url);
}

/** Which step of an install is running. */
export type InstallStage =
  | { step: "resolving" }
  | { step: "downloading"; bytes: number; total: number | null }
  | { step: "extracting" }
  | { step: "deploying" }
  | { step: "done" };

export interface Installed {
  owner: string;
  mod_id: number;
  file_id: number;
  archive: string;
  files: number;
  linked: number;
  copied: number;
  /** combined mods built from the version this install replaced, now removed */
  dissolved: string[];
  notes: string[];
}

/**
 * Download and install what an nxm link points at.
 *
 * Progress arrives on the `install-progress` event, not here: an archive can
 * be tens of megabytes and this only resolves once the mod is deployed.
 */
export async function installFromNxm(
  url: string,
  modsDir?: string,
  gameRoot?: string,
): Promise<Installed | null> {
  if (!inTauri()) return null;
  return await invoke<Installed>("install_from_nxm", {
    url,
    modsDir: modsDir ?? null,
    gameRoot: gameRoot ?? null,
  });
}

/** Install an archive already on disk, through staging and a deploy. */
export async function installArchive(
  archive: string,
  modsDir?: string,
  gameRoot?: string,
  overwrite = false,
): Promise<Installed | null> {
  if (!inTauri()) return null;
  return await invoke<Installed>("install_archive", {
    archive,
    modsDir: modsDir ?? null,
    gameRoot: gameRoot ?? null,
    overwrite,
  });
}

/** Nexus's own curated lists. There is no keyword search in this API. */
export type Listing = "trending" | "latest_added" | "latest_updated";

/** A mod as it appears in a listing. */
export interface Listed extends ModPage {
  mod_id: number | null;
}

export async function nexusBrowse(list: Listing): Promise<Listed[]> {
  if (!inTauri()) return [];
  return await invoke<Listed[]>("nexus_browse", { list });
}

/** Who handles `nxm://` on this machine. */
export interface SchemeOwner {
  ours: boolean;
  /** the registered command line, when it can be read */
  command: string | null;
}

export async function nxmOwner(): Promise<SchemeOwner | null> {
  if (!inTauri()) return null;
  return await invoke<SchemeOwner>("nxm_owner");
}

/**
 * Take over `nxm://` system-wide.
 *
 * Only ever on request. The scheme is global, so claiming it takes downloads
 * for *every* game away from whatever had it — usually the mod manager the
 * user still runs for their other games.
 */
export async function claimNxmScheme(): Promise<SchemeOwner | null> {
  if (!inTauri()) return null;
  return await invoke<SchemeOwner>("claim_nxm_scheme");
}

export async function releaseNxmScheme(): Promise<SchemeOwner | null> {
  if (!inTauri()) return null;
  return await invoke<SchemeOwner>("release_nxm_scheme");
}

// ---------------------------------------------------------------------------
// Settings and presets
// ---------------------------------------------------------------------------

/**
 * Every path the user can override. `null` means "work it out yourself".
 *
 * Two things live in the settings file and are deliberately not here: the
 * Nexus API key and the saved presets. The key is never sent to the screen —
 * connecting and disconnecting go through their own calls, which report the
 * *account* instead — and the presets have their own calls too. Saving this
 * object keeps both: see `settings_save` in the Rust side, which reads them
 * back off disk so a round trip through this form cannot erase them.
 */
export interface Settings {
  game_root: string | null;
  game_exe: string | null;
  mods_dir: string | null;
  staging_dir: string | null;
  archives_dir: string | null;
  check_updates_on_start: boolean;
  show_outdated: boolean;
  block_ads: boolean;
  record_sessions: boolean;
  keep_sessions: number;
  backup_saves: boolean;
  keep_save_backups: number;
  active_preset: string | null;
}

/** One resolved path, and enough about it to explain itself on screen. */
export interface Place {
  path: string | null;
  /** true when the user set this, false when it was detected */
  chosen: boolean;
  exists: boolean;
  problem: string | null;
}

export interface Resolved {
  game_root: Place;
  game_exe: Place;
  mods_dir: Place;
  staging: Place;
  archives: Place;
  detected_from: string | null;
  ready: boolean;
}

export interface Preset {
  name: string;
  enabled: string[];
  note: string | null;
}

export interface Presets {
  presets: Preset[];
  active: string | null;
}

export interface LoadoutEntry {
  owner: string;
  source: string;
  /**
   * Which build of the mod the game is reading.
   *
   * `cleaned` and `mended` are separate on purpose: both are a build sitting
   * beside an untouched staged copy, but they are different operations and this
   * word is shown to the user. One label for both meant a mod whose broken XML
   * had been repaired described itself as "cleaned".
   */
  variant: "original" | "cleaned" | "mended" | "merged";
  replaces: string[];
  deployed: string[];
  enabled: boolean;
  /**
   * true when this build carries values the user set by hand.
   *
   * A flag beside `variant` rather than a fifth variant, because editing is
   * orthogonal to all four: a cleaned mod can carry hand-set values and still
   * be a cleaned mod, and one word could not have said both.
   */
  edited: boolean;
}

export interface Loadout {
  entries: LoadoutEntry[];
}

/** A file more than one enabled mod wanted to own. */
export interface Clash {
  rel: string;
  keeper: string;
  losers: string[];
}

export interface Changes {
  deployed: string[];
  removed: string[];
  unchanged: string[];
  problems: string[];
  clashes: Clash[];
}

export interface PresetSwitch {
  applied: { switched_on: string[]; switched_off: string[]; missing: string[] };
  changes: Changes;
}

/** The defaults, for when the page is open outside Tauri. */
export function blankSettings(): Settings {
  return {
    game_root: null,
    game_exe: null,
    mods_dir: null,
    staging_dir: null,
    archives_dir: null,
    check_updates_on_start: true,
    show_outdated: false,
    block_ads: true,
    record_sessions: true,
    keep_sessions: 25,
    backup_saves: true,
    keep_save_backups: 10,
    active_preset: null,
  };
}

export async function settingsRead(): Promise<Settings> {
  if (!inTauri()) return blankSettings();
  try {
    return await invoke<Settings>("settings_read");
  } catch {
    return blankSettings();
  }
}

export async function settingsResolve(): Promise<Resolved | null> {
  if (!inTauri()) return null;
  try {
    return await invoke<Resolved>("settings_resolve");
  } catch {
    return null;
  }
}

/** Saving reports what the settings now resolve to, so the screen can show
 *  the consequence of a change straight away rather than at next use. */
export async function settingsSave(settings: Settings): Promise<Resolved | null> {
  if (!inTauri()) return null;
  return await invoke<Resolved>("settings_save", { settings });
}

export async function launchGame(): Promise<string | null> {
  if (!inTauri()) return null;
  return await invoke<string>("launch_game");
}

export async function presetsList(): Promise<Presets> {
  if (!inTauri()) return { presets: [], active: null };
  try {
    return await invoke<Presets>("presets_list");
  } catch {
    return { presets: [], active: null };
  }
}

export async function presetSave(name: string, note?: string): Promise<Presets | null> {
  if (!inTauri()) return null;
  return await invoke<Presets>("preset_save", { name, note: note ?? null });
}

export async function presetDelete(name: string): Promise<Presets | null> {
  if (!inTauri()) return null;
  return await invoke<Presets>("preset_delete", { name });
}

export async function presetApply(
  name: string,
  dryRun: boolean,
  modsDir?: string,
): Promise<PresetSwitch | null> {
  if (!inTauri()) return null;
  return await invoke<PresetSwitch>("preset_apply", {
    name,
    dryRun,
    modsDir: modsDir ?? null,
  });
}

// ---------------------------------------------------------------------------
// Mod lists, to send to another player
// ---------------------------------------------------------------------------

/**
 * One mod on a shared list, described every way it can be described.
 *
 * A preset names folders in *this* install, which is useless the moment it
 * leaves the machine: the folder a mod deploys under is whatever was inside
 * the archive its author zipped, so the same mod can sit under different names
 * on two libraries. So a list carries the page and the title as well, and the
 * importing side matches on whichever it can answer.
 */
export interface Listed {
  owner: string;
  name: string | null;
  mod_id: number | null;
  version: string | null;
  archive: string | null;
}

export interface Collection {
  format: string;
  version: number;
  name: string;
  note: string | null;
  exported: string | null;
  game_version: string | null;
  mods: Listed[];
}

/** How a listed mod was recognised here, so the screen can say which. */
export type How = "folder" | "page" | "title";

export interface Have {
  listed: Listed;
  /** the folder name *here*, which is what the preset records */
  owner: string;
  name: string;
  how: How;
  version: string | null;
  /** both sides name a version, and they are not the same one */
  differs: boolean;
  enabled: boolean;
}

export interface CollectionPlan {
  name: string;
  note: string | null;
  exported: string | null;
  game_version: string | null;
  have: Have[];
  missing: Listed[];
  /**
   * Mods switched on here that the list does not mention.
   *
   * Switching to the imported list turns these off. They are named rather than
   * counted because a mod going quiet without being named is the failure this
   * program exists to prevent.
   */
  extra: string[];
}

/** Build a list from a preset, or from what is switched on now. Writes nothing. */
export async function collectionExport(opts: {
  preset?: string;
  name?: string;
  note?: string;
  modsDir?: string;
}): Promise<Collection | null> {
  if (!inTauri()) return null;
  return await invoke<Collection>("collection_export", {
    modsDir: opts.modsDir ?? null,
    preset: opts.preset ?? null,
    name: opts.name ?? null,
    note: opts.note ?? null,
  });
}

export async function collectionWrite(list: Collection, path: string): Promise<void> {
  if (!inTauri()) return;
  await invoke("collection_write", { list, path });
}

export async function collectionOpen(path: string): Promise<Collection | null> {
  if (!inTauri()) return null;
  return await invoke<Collection>("collection_open", { path });
}

/** What a list would mean here. Reads only. */
export async function collectionPlan(
  list: Collection,
  modsDir?: string,
): Promise<CollectionPlan | null> {
  if (!inTauri()) return null;
  return await invoke<CollectionPlan>("collection_plan", {
    list,
    modsDir: modsDir ?? null,
  });
}

/** Save an imported list as a preset. Deliberately does not switch to it. */
export async function collectionImport(
  list: Collection,
  name?: string,
  modsDir?: string,
): Promise<Presets | null> {
  if (!inTauri()) return null;
  return await invoke<Presets>("collection_import", {
    list,
    name: name ?? null,
    modsDir: modsDir ?? null,
  });
}

/** One mod another manager deployed, and whether we can take it over. */
export interface Candidate {
  owner: string;
  source: string;
  deployed: string[];
  files: number;
  /** why it cannot be adopted; null when it can */
  refused: string | null;
  notes: string[];
}

/** What taking over another manager's mods folder would do. */
export interface Survey {
  /** the manager that left a manifest behind, when one did. Null is the
   *  ordinary case: the mods were traced through the links themselves. */
  manager: string | null;
  staging: string | null;
  candidates: Candidate[];
  /** mod folders in the game that nothing in staging accounts for */
  unmanaged: string[];
  /**
   * Mods running a build this program made: cleaned, mended or merged.
   *
   * Their deployed names link into `derived`, not into staging, so they cannot
   * be traced to any mod's source. They are reported rather than adopted: a
   * mod is only in this state because there *was* a loadout, and the answer for
   * it is that loadout rather than a guess at which build it is running.
   */
  ours: string[];
}

/**
 * Look at the mods folder and work out what could be taken over.
 *
 * Read-only. A mod deployed by hardlink keeps its real files in a staging
 * folder and the game gets second names for them — the same arrangement this
 * program uses, so a cutover is bookkeeping rather than a migration. Nothing is
 * copied, moved or deleted.
 *
 * Which staged folder each mod came from is worked out from the links
 * themselves: a deployed file and its staged original are literally the same
 * file, and the file system says so. No other mod manager needs to be
 * installed, and none is asked. `Survey.manager` is null in that ordinary case
 * and names a manager only when one left a manifest behind.
 */
export async function adoptSurvey(modsDir?: string): Promise<Survey | null> {
  if (!inTauri()) return null;
  return await invoke<Survey>("adopt_survey", { modsDir: modsDir ?? null });
}

/** Take over every adoptable mod. Returns how many. */
/** A mod running one of our builds, and the author's copy to put it back on. */
export interface Restorable {
  owner: string;
  /** the staging folder that deploys this name, when exactly one does */
  source: string | null;
  deployed: string[];
  /** why the author's build cannot be found, when it cannot */
  why_not: string | null;
}

export interface Restoration {
  restored: string[];
  /** [mod, why] for each one left alone */
  skipped: [string, string][];
  problems: string[];
}

/**
 * Work out how to put mods running one of our builds back on the author's.
 *
 * Read-only. The answer to `Survey.ours`: those mods cannot be adopted as they
 * stand, because that means guessing which of three builds each is on — but
 * they do not have to be, since the author's copy is still staged and untouched
 * and putting them back on it makes the question disappear.
 */
export async function restoreAuthorsPreview(modsDir?: string): Promise<Restorable[]> {
  if (!inTauri()) return [];
  return await invoke<Restorable[]>("restore_authors_preview", {
    modsDir: modsDir ?? null,
  });
}

/** Do it: relink them to the staged originals, and take them over. */
export async function restoreAuthors(modsDir?: string): Promise<Restoration | null> {
  if (!inTauri()) return null;
  return await invoke<Restoration>("restore_authors", { modsDir: modsDir ?? null });
}

export async function adoptApply(modsDir?: string): Promise<number> {
  if (!inTauri()) return 0;
  return await invoke<number>("adopt_apply", { modsDir: modsDir ?? null });
}

/**
 * The mods this program is responsible for.
 *
 * Takes `modsDir` because the engine heals the record as it reads it: an entry
 * whose builds have all gone from disk — a mod deleted by hand rather than
 * through this program — is dropped, and it needs to know where to look to be
 * sure of that. Without the folder it lists whatever is recorded, unpruned.
 */
export async function loadoutList(modsDir?: string): Promise<Loadout> {
  if (!inTauri()) return { entries: [] };
  try {
    return await invoke<Loadout>("loadout_list", { modsDir: modsDir ?? null });
  } catch {
    return { entries: [] };
  }
}

/** Switch mods on or off. Takes a list so a batch is one reconcile, not many. */
export async function setModsEnabled(
  owners: string[],
  enabled: boolean,
  modsDir?: string,
): Promise<Changes | null> {
  if (!inTauri()) return null;
  return await invoke<Changes>("set_mods_enabled", {
    owners,
    enabled,
    modsDir: modsDir ?? null,
  });
}

export async function deployReconcile(
  dryRun: boolean,
  modsDir?: string,
): Promise<Changes | null> {
  if (!inTauri()) return null;
  return await invoke<Changes>("deploy_reconcile", { dryRun, modsDir: modsDir ?? null });
}

// ---------------------------------------------------------------------------
// Repairing a file the game cannot read
// ---------------------------------------------------------------------------

/** What is wrong with one file, when it is something we can mend. */
export interface Fault {
  unclosed: string[];
  ends_at: number;
  /** true when the file's own layout settles where the missing tag belongs */
  confident: boolean;
  summary: string;
}

export interface Mendable {
  rel_path: string;
  fault: Fault;
}

export interface Mended {
  owner: string;
  fixed: Mendable[];
  /** broken, but not safely mendable: [file, why] */
  refused: [string, string][];
  dest: string;
}

/** What could be mended in a mod. Writes nothing. */
export async function repairPreview(owner: string): Promise<Mended | null> {
  if (!inTauri()) return null;
  return await invoke<Mended>("repair_preview", { owner });
}

/** Write a mended build and switch the game over to it. */
export async function repairApply(
  owner: string,
  modsDir?: string,
  gameRoot?: string,
): Promise<Changes | null> {
  if (!inTauri()) return null;
  return await invoke<Changes>("repair_apply", {
    owner,
    modsDir: modsDir ?? null,
    gameRoot: gameRoot ?? null,
  });
}

/**
 * Put a mod back to the build its author shipped, undoing a repair or a clean.
 *
 * The same switch either way: both are a build sitting beside an untouched
 * staged copy, so there is nothing to restore, only something to point at.
 */
export async function repairUndo(
  owner: string,
  modsDir?: string,
): Promise<Changes | null> {
  if (!inTauri()) return null;
  return await invoke<Changes>("repair_undo", {
    owner,
    modsDir: modsDir ?? null,
  });
}

/** What putting mods back to the build their author shipped did. */
export interface Restored {
  /** [mod folder, the build it was on] */
  reverted: [string, string][];
  /** merges taken apart, which is how their inputs come back */
  dissolved: string[];
  /** already on the author's build, so nothing was done */
  untouched: string[];
  problems: string[];
}

/**
 * Put mods back to the build their author shipped, in one pass.
 *
 * `owners` names them; leaving it out means the whole library. One call rather
 * than one per mod: the expensive part is reconciling the game folder against
 * the loadout, and doing that twenty-five times over is twenty-five times the
 * work for the same answer.
 *
 * A merge is dissolved rather than switched, because it has no author's build
 * behind it — putting one "back" means having the mods it stands in for in the
 * game again.
 */
export async function restoreOriginals(
  owners?: string[],
  modsDir?: string,
): Promise<Restored | null> {
  if (!inTauri()) return null;
  return await invoke<Restored>("restore_originals", {
    owners: owners ?? null,
    modsDir: modsDir ?? null,
  });
}

/** What kind of thing a delete would remove. */
export type What = "staged" | "derived" | "archive";

export interface DeleteItem {
  path: string;
  what: What;
  files: number;
  bytes: number;
}

/** Everything deleting one mod would do, before any of it happens. */
export interface DeletePlan {
  owner: string;
  deployed: string[];
  items: DeleteItem[];
  /** bytes the disk will actually give back */
  bytes: number;
  /** paths outside the managed folders, which are left alone */
  refused: string[];
  warnings: string[];
}

export interface Erased {
  owner: string;
  undeployed: number;
  removed: string[];
  bytes: number;
  /** combined mods built out of this one, which went with it */
  dissolved: string[];
  problems: string[];
}

/** What deleting these mods would remove. Touches nothing. */
export async function deletePreview(
  owners: string[],
  withArchive: boolean,
  modsDir?: string,
  gameRoot?: string,
): Promise<DeletePlan[]> {
  if (!inTauri()) return [];
  return await invoke<DeletePlan[]>("delete_preview", {
    owners,
    withArchive,
    modsDir: modsDir ?? null,
    gameRoot: gameRoot ?? null,
  });
}

/** Delete for good. This cannot be undone. */
export async function deleteMods(
  owners: string[],
  withArchive: boolean,
  modsDir?: string,
  gameRoot?: string,
): Promise<Erased[]> {
  if (!inTauri()) return [];
  return await invoke<Erased[]>("delete_mods", {
    owners,
    withArchive,
    modsDir: modsDir ?? null,
    gameRoot: gameRoot ?? null,
  });
}

/** `1.4 GB`, `812 MB`, `9 KB` — sizes people can weigh a decision against. */
export function humanBytes(n: number): string {
  if (n <= 0) return "nothing";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let at = 0;
  let size = n;
  while (size >= 1024 && at < units.length - 1) {
    size /= 1024;
    at += 1;
  }
  return `${size < 10 && at > 0 ? size.toFixed(1) : Math.round(size)} ${units[at]}`;
}
// ---------------------------------------------------------------------------
// Recording what the game says while it runs
// ---------------------------------------------------------------------------

/** What the recorder's DLL is doing in the game's own folder. */
export interface HookState {
  /** where it goes: beside NMS.exe */
  path: string | null;
  /** our copy, the one an install would write */
  source: string | null;
  installed: boolean;
  /** true when what is installed is ours */
  ours: boolean;
  /** true when it is the build this program ships */
  up_to_date: boolean;
  /** set when another tool owns the same slot */
  foreign: string | null;
  /** a displaced DLL is waiting to be put back */
  backup: boolean;
  game_running: boolean;
  out_dir: string | null;
  /** why installing or removing cannot happen right now */
  blocked: string | null;
}

export interface EventCounts {
  fatal: number;
  error: number;
  warn: number;
  info: number;
  debug: number;
}

/** A mod the game complained about during a session. */
export interface ModTrouble {
  folder: string;
  warnings: number;
  errors: number;
  samples: string[];
}

/** A file the game skipped because it loaded the same asset from another. */
export interface IgnoredFile {
  folder: string;
  file: string;
  instead: string;
}

/** One recorded session, as the list shows it. */
export interface Session {
  log: string;
  started_ms: number;
  started: string;
  seconds: number;
  pid: number;
  hook_version: string;
  exit_code: number | null;
  exit_note: string;
  crashed: boolean;
  stopped_early: boolean;
  counts: EventCounts;
  /** every save the game wrote this session */
  saves: SaveWritten[];
  /** how the game's memory use moved, when the session was sampled at all */
  memory: MemoryTrend | null;
  /** what changed since the session before this one: a suspect list */
  changed: string[];
  files_opened: number;
  mods_in_trouble: ModTrouble[];
  never_loaded: string[];
  ignored: IgnoredFile[];
  crash: string | null;
  dialogs: string[];
  dumps: string[];
  /** one line: what this session amounted to */
  verdict: string;
}

/** One save the game wrote, or tried to. */
export interface SaveWritten {
  /** the file's own name, e.g. `save3.hg` */
  file: string;
  bytes: number;
  writes: number;
  ms: number;
  /** the Windows error, when a write failed */
  error: number | null;
  /** true when the game ended with the file still open */
  unfinished: boolean;
}

/** What the game's memory use looked like at one moment. */
export interface MemorySample {
  at_ms: number;
  working_set: number;
  private: number;
  peak_working_set: number;
  handles: number;
  page_faults: number;
  system_free: number;
  system_total: number;
}

/** How the game's memory use moved over a session. */
export interface MemoryTrend {
  first: MemorySample;
  last: MemorySample;
  samples: number;
}

/** One thing the hook saw, as it arrives while the game runs. */
export interface LiveEvent {
  ts: number;
  /** local clock reading */
  at: string;
  /** 0 debug, 1 info, 2 warn, 3 error, 4 fatal */
  lvl: number;
  cat: string;
  msg: string;
  /** the mod folder this is about, when it is about one */
  owner: string | null;
}

/** What the recorder is doing this second. */
export interface WatchState {
  watching: boolean;
  game_running: boolean;
  pid: number;
  recording: boolean;
  since_ms: number;
  counts: EventCounts;
  log: string | null;
  hook_ready: boolean;
  note: string | null;
  last: Session | null;
}

export function noWatch(): WatchState {
  return {
    watching: false,
    game_running: false,
    pid: 0,
    recording: false,
    since_ms: 0,
    counts: { fatal: 0, error: 0, warn: 0, info: 0, debug: 0 },
    log: null,
    hook_ready: false,
    note: null,
    last: null,
  };
}

export async function hookState(): Promise<HookState | null> {
  if (!inTauri()) return null;
  try {
    return await invoke<HookState>("hook_state");
  } catch {
    return null;
  }
}

/**
 * Put the recorder in the game, so the next run is recorded.
 *
 * `force` answers a question the screen has already asked: another tool owns
 * the same file name, replace it? Its DLL is kept and put back on removal.
 * Throws with the engine's reason -- the game being open, most often, which is
 * something the user can fix and try again.
 */
export async function hookInstall(force = false): Promise<HookState | null> {
  if (!inTauri()) return null;
  return await invoke<HookState>("hook_install", { force });
}

export async function hookUninstall(): Promise<HookState | null> {
  if (!inTauri()) return null;
  return await invoke<HookState>("hook_uninstall");
}

export async function watchState(): Promise<WatchState | null> {
  if (!inTauri()) return null;
  try {
    return await invoke<WatchState>("watch_state");
  } catch {
    return null;
  }
}

/** The events of the session in progress, for a screen opened part-way in. */
export async function watchTail(): Promise<LiveEvent[]> {
  if (!inTauri()) return [];
  try {
    return await invoke<LiveEvent[]>("watch_tail");
  } catch {
    return [];
  }
}

/** Every recorded session, newest first. */
export async function sessionsList(): Promise<Session[]> {
  if (!inTauri()) return [];
  try {
    return await invoke<Session[]>("sessions_list");
  } catch {
    return [];
  }
}

/** The text of one session log. Huge ones come back as their last part. */
export async function sessionText(log: string): Promise<string | null> {
  if (!inTauri()) return null;
  return await invoke<string>("session_text", { log });
}

/** Delete one session log, and get back the list that is left. */
export async function sessionForget(log: string): Promise<Session[]> {
  if (!inTauri()) return [];
  return await invoke<Session[]>("session_forget", { log });
}


// ---------------------------------------------------------------------------
// What the game itself did with the load order
// ---------------------------------------------------------------------------

/** Which way ModPriority ran in the order the game read files. */
export type Direction = "ascending" | "descending" | "mixed" | "untestable";

/** One conclusion the measurement leaves open. */
export interface Reading {
  /** the loader model, in words */
  model: string;
  rule: "first" | "last";
  /** true when this is the rule the analysis uses */
  is_current: boolean;
}

/** The order the game read one asset's copies in. */
export interface LoadSequence {
  target: string;
  /** mod folders, first read first */
  order: string[];
  /** ModPriority per entry, null where the game had not registered the mod */
  priorities: (number | null)[];
}

/** A sequence that ran against the direction the others did. */
export interface OrderException {
  target: string;
  order: string[];
  priorities: number[];
  direction: string;
}

/** What one session settled about load order. */
export interface Measured {
  log: string;
  measured_ms: number;
  direction: Direction;
  readings: Reading[];
  contested: number;
  testable: number;
  agreed: number;
  exceptions: OrderException[];
  loads: number;
}

/** A predicted winner against the two the observed order allows. */
export interface CheckedWinner {
  target: string;
  predicted: string | null;
  read_first: string | null;
  read_last: string | null;
  /** true when the prediction is one of the two ends of the observed order */
  plausible: boolean;
  /** true when the analysis and the game agree on which mods are involved */
  same_mods: boolean;
  order: string[];
}

/** Everything one session log settles. */
export interface Observation {
  measured: Measured;
  /** the paragraph to print: what was measured, and what is still inferred */
  basis: string;
  consistent: boolean;
  applied: LoadSequence[];
  checked: CheckedWinner[];
  implausible: number;
  mismatched: number;
}

/**
 * Measure load order from one recorded session.
 *
 * Reads the whole log rather than the tail [`sessionText`] shows: the game
 * opens every mod file in its first second, which is the part a tail is missing.
 */
export async function observeSession(
  log: string,
  modsDir?: string,
  gameRoot?: string,
): Promise<Observation | null> {
  if (!inTauri()) return null;
  return await invoke<Observation>("observe_session", {
    log,
    modsDir: modsDir ?? null,
    gameRoot: gameRoot ?? null,
  });
}

// ---------------------------------------------------------------------------
// What a game update changed underneath the mods
// ---------------------------------------------------------------------------

/** One game build whose vanilla data is cached on this machine. */
export interface GameBuild {
  key: string;
  first_seen_ms: number;
  paks: number;
  bytes: number;
  /** false when the date came from the folder's timestamp, not from a stamp */
  dated: boolean;
}

/** One property the update moved, and what the mod makes of it. */
export interface Moved {
  path: string;
  before: string | null;
  after: string | null;
  mod_value: string | null;
}

/** How one mod file fared across the update. */
export interface FileImpact {
  owner: string;
  rel_path: string;
  target: string;
  whole_file: boolean;
  severity: "CRITICAL" | "MAJOR" | "MINOR" | "INFO";
  summary: string;
  /** the mod still carries the pre-patch value, so it undoes the patch */
  reverts: Moved[];
  /** the patch removed the property, so the mod's edit lands nowhere */
  dead: Moved[];
  /** the patch added a property this whole-file copy deletes by omission */
  drops: Moved[];
  /** the mod sets its own value and the game's moved underneath it */
  overridden: Moved[];
}

/** Everything one update did to one library. */
export interface UpdateImpact {
  from: GameBuild | null;
  to: GameBuild | null;
  compared: number;
  touched: number;
  /** targets with no cached copy from before the update */
  uncomparable: string[];
  files: FileImpact[];
  /** mods checked and found clear, which is the answer worth having */
  unaffected: string[];
  verdict: string;
  /** why the survey could not run at all */
  blocked: string | null;
}

/** Everything that needs a decision, across the whole survey. */
export function impactHarm(impact: UpdateImpact): number {
  return impact.files.reduce(
    (n, f) => n + f.reverts.length + f.dead.length + f.drops.length,
    0,
  );
}

/** Every cached game build, oldest first. */
export async function vanillaBuilds(): Promise<GameBuild[]> {
  if (!inTauri()) return [];
  try {
    return await invoke<GameBuild[]>("vanilla_builds");
  } catch {
    return [];
  }
}

/**
 * What the latest game update did to the mods.
 *
 * Minutes of work on a first run, so it is never called on a timer or at
 * start-up -- only when asked for.
 */
export async function updateImpact(
  modsDir?: string,
  gameRoot?: string,
): Promise<UpdateImpact | null> {
  if (!inTauri()) return null;
  return await invoke<UpdateImpact>("update_impact", {
    modsDir: modsDir ?? null,
    gameRoot: gameRoot ?? null,
  });
}

/** What preparing the baseline managed to lay down. */
export interface Prepared {
  asked: number;
  cached: number;
  /** targets the game does not ship, i.e. assets the mods invented */
  absent: number;
  unpacked: number;
  error: string | null;
}

/**
 * Cache vanilla copies of everything the library touches, for the next update.
 *
 * The comparison can only judge assets extracted *before* an update, and the
 * old archives are gone once it lands. This is the only way to close that gap,
 * and it has to be asked for.
 */
export async function vanillaPrepare(
  modsDir?: string,
  gameRoot?: string,
): Promise<Prepared | null> {
  if (!inTauri()) return null;
  return await invoke<Prepared>("vanilla_prepare", {
    modsDir: modsDir ?? null,
    gameRoot: gameRoot ?? null,
  });
}
// ---------------------------------------------------------------------------
// Copies of the save
// ---------------------------------------------------------------------------

/** One kept copy of one save slot: both of its files, from one moment. */
export interface SaveCopy {
  /** the account folder it belongs to, e.g. `st_76561198…` */
  account: string;
  /** the slot, as the game names it: `save3`, or `save` for the first */
  slot: string;
  at_ms: number;
  /** local time, spelled out */
  at: string;
  path: string;
  /** the files in it, with their sizes */
  files: [string, number][];
  bytes: number;
  /** true when this copy was taken to make a restore reversible */
  before_restore: boolean;
}

export interface SaveCopies {
  kept: SaveCopy[];
  /** what the copies take up in total */
  bytes: number;
  /** the save folders being watched, one per account */
  folders: string[];
  root: string;
}

export function noCopies(): SaveCopies {
  return { kept: [], bytes: 0, folders: [], root: "" };
}

export async function savesKept(): Promise<SaveCopies> {
  if (!inTauri()) return noCopies();
  try {
    return await invoke<SaveCopies>("saves_kept");
  } catch {
    return noCopies();
  }
}

/** Copy anything that has changed now, rather than waiting for the game to. */
export async function savesBackUp(): Promise<SaveCopies> {
  if (!inTauri()) return noCopies();
  return await invoke<SaveCopies>("saves_back_up");
}

/**
 * Put one kept copy back into the game's save folder.
 *
 * What it replaces is copied aside first, so restoring the wrong moment is undone
 * by restoring the copy this makes. Throws while the game is running: it holds
 * the save in memory and would write over anything put back.
 */
export async function saveRestore(kept: SaveCopy): Promise<string[]> {
  if (!inTauri()) return [];
  return await invoke<string[]>("save_restore", { kept });
}

export async function saveForget(kept: SaveCopy): Promise<SaveCopies> {
  if (!inTauri()) return noCopies();
  return await invoke<SaveCopies>("save_forget", { kept });
}
