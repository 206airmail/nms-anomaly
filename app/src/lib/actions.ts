/** Findings, restated as things to do.
 *
 * The report answers "what is true about this library". Almost nobody wants
 * that. They want "what should I do", which is a different list: ranked by
 * what it costs them to leave alone, phrased as an instruction, and carrying
 * the button that performs it where one exists.
 *
 * Nothing here is new information. It is the same report, addressed to a
 * person instead of to an engineer.
 *
 * Mods arrive here as folder names, because that is what the engine works in
 * and what every action has to be performed against. They are *addressed* by
 * the name a person recognises: every function that writes a sentence takes a
 * `Naming` and puts the title in the prose while `mods`, `subject` and `target`
 * keep the folder. Get that the wrong way round and the button acts on a mod
 * that does not exist.
 */

import type { CleanPlan } from "./engine";
import { assetName, type Conflict, type Report } from "./types";

/**
 * How much it costs to leave a thing alone, which is the only ranking worth
 * having. Highest first.
 *
 * `unblock` is the one that is not obvious. A mod that replaces a whole game
 * file overrides every other mod touching it -- and if that file is contested,
 * *cleaning* the offender removes the reason the conflict existed, which is
 * better than merging, because a merge is a third mod to keep track of. So
 * cleaning outranks merging wherever it is on offer.
 *
 * It does not always finish the job, though, and that is what [`mergePlanFor`]
 * is for: cleaning settles a conflict only when the copies left behind are all
 * sparse patches the game merges itself. Where a whole-file copy remains that
 * cannot be cleaned, both a clean *and* a merge of what is left are needed, and
 * both are listed -- they are two halves of one fix, not two competing
 * suggestions.
 *
 * The same clean, on a mod that is contesting nothing, is the *lowest* thing
 * here. It is the identical operation; what differs is entirely what it buys.
 */
export type Urgency =
  | "broken"
  | "wrong"
  | "unblock"
  | "merge"
  | "decide"
  | "tidy";

/** Folder name -> the title to show for it. See `names.svelte.ts`. */
export type Naming = (owner: string) => string;

/** When nothing better is known, a mod's folder is its name. */
const asItself: Naming = (owner) => owner;

/** What pressing the button on an action does. */
export type Verb = "combine" | "clean" | "undo" | "repair";

export interface Action {
  id: string;
  urgency: Urgency;
  /** imperative, in the user's words */
  title: string;
  /** one sentence: what it costs to leave this alone */
  why: string;
  mods: string[];
  target?: string;
  /** a number worth seeing, e.g. "153,416 → 2 properties" */
  metric?: string;
  /** absent when no button can do it and the person must decide */
  verb?: Verb;
  /** the argument the verb needs: an asset target, or a mod name */
  subject?: string;
  /**
   * For `combine`: exactly which mods to put in the merge.
   *
   * Absent means "all of them", which is the ordinary case. It is present when
   * some of the mods contesting the asset are better *cleaned* than merged:
   * those are left out, so pressing this does not build a merge around a mod
   * that is about to stop contesting anything. See [`mergePlanFor`].
   */
  only?: string[];
  /** a second-order consequence worth knowing before pressing */
  bonus?: string;
}

/** Order of the list. Everything else is detail. */
const RANK: Record<Urgency, number> = {
  broken: 0, // the mod is not running at all
  wrong: 1, // the game is misbehaving where you can see it
  unblock: 2, // one mod is overriding another, and cleaning it ends that
  merge: 3, // one mod is overriding another; both can be kept
  decide: 4, // the same, but nobody can choose for you
  tidy: 5, // nothing is wrong; it could be tidier
};

/**
 * How serious a thing is, in the three words the screen groups by.
 *
 * Three steps, because more would be a lie, and *named* rather than merely
 * coloured: "critical" and "optional" are the answer to the question the list
 * is opened with, and a colour on its own does not say either of them.
 *
 * This is also the only severity vocabulary. There used to be a second,
 * identically-partitioned one -- alarm/loss/quiet -- used for the colours
 * while these words were used for nothing, so the card's hue and the card's
 * heading could drift apart while both claimed to rank the same list.
 */
export type Band = "critical" | "recommended" | "optional";

const BAND: Record<Urgency, Band> = {
  broken: "critical", // you installed a mod and it is doing nothing
  wrong: "critical", // the game is running wrong where you can see it
  unblock: "recommended", // a mod's edits are being thrown away
  merge: "recommended",
  decide: "recommended",
  tidy: "optional", // nothing is being lost
};

export function bandOf(urgency: Urgency): Band {
  return BAND[urgency];
}

/** The heading each band gets, and what belonging to it means. */
export const BANDS: { id: Band; name: string; means: string }[] = [
  {
    id: "critical",
    name: "Critical",
    means: "a mod is not doing what it says, or the game is running wrong",
  },
  {
    id: "recommended",
    name: "Recommended",
    means: "nothing is broken, but one mod is throwing away another's edits",
  },
  {
    id: "optional",
    name: "Optional",
    means: "nothing is wrong; these only make the library tidier and safer",
  },
];

/** Split a built list into the three bands, in order, dropping empty ones. */
export function banded(actions: Action[]): { id: Band; name: string; means: string; rows: Action[] }[] {
  return BANDS.map((band) => ({
    ...band,
    rows: actions.filter((a) => bandOf(a.urgency) === band.id),
  })).filter((band) => band.rows.length > 0);
}

/**
 * The actions that ask for something, which is not all of them.
 *
 * An `undo` is the way back out of a fix that has already been applied. It is
 * worth keeping on screen -- it is the only place the way back is offered --
 * but it is not work outstanding, and counting it as work is what had a library
 * with nothing left to do saying "Nothing is wrong" under an Optional heading
 * promising things that "make the library tidier and safer", above twenty-five
 * rows that were already tidy. The bands, the verdict and the Overweight
 * readout all take this; [`reversible`] takes the rest.
 */
export function pending(actions: Action[]): Action[] {
  return actions.filter((a) => a.verb !== "undo");
}

/** The other half of [`pending`]: fixes already applied, and how to undo them. */
export function reversible(actions: Action[]): Action[] {
  return actions.filter((a) => a.verb === "undo");
}

const n = (v: number) => v.toLocaleString();

/** `1 mod`, `12 mods`, `2 properties`. Exported: this module owns the phrasing
 *  the user reads, and the screens that report what a button did need it too. */
export function plural(count: number, one: string, many = one + "s"): string {
  return `${n(count)} ${count === 1 ? one : many}`;
}

/**
 * Copies of `target` that write values they do not edit, cleanable or not.
 *
 * These are what make a mergeable conflict *harmful*: a copy carrying the
 * game's own value at a property somebody else changed reverts that change at
 * load, by accident. A `.MBIN` does it wholesale; a `.EXML` does it for the
 * properties it happens to restate, which is a smaller version of the same
 * accident and is why the engine plans contested patches too.
 */
function carriersFor(plans: CleanPlan[], target: string): CleanPlan[] {
  return plans.filter((p) => p.target === target && !p.cleaned && carries(p));
}

/**
 * Does this copy write anything it does not change?
 *
 * A whole file always does, by construction -- it is the entire table, so every
 * property the mod did not touch travels inside it, and that stays true even
 * when the comparison failed and the counts are unknown.
 *
 * A patch only does when it demonstrably restates something. Most do not: 609
 * of 610 properties are real edits in the commonest case, and a patch with
 * nothing to strip reverts nobody. Offering to clean one is offering work with
 * no effect -- it put "restates 0 values they do not change" on a card and
 * pushed the library's Overweight count from 9 to 36 with findings that buy
 * nothing.
 *
 * **It is `dropped`, never `original > edits`.** Those two count different
 * things: `original` counts every property including the containers a nested
 * edit hangs off, `edits` counts changed leaves. `LaunchThrustersReworked`
 * ships four patches in which every single leaf is an edit and still reads 70
 * against 26, because 44 of the 70 are `<Property name="Table">` wrappers. It
 * was offered for cleaning, cleaning copied the same bytes into `derived/`,
 * reported success, marked it cleaned -- and the card came straight back,
 * because the next preview found the same nothing to remove.
 */
function carries(plan: CleanPlan): boolean {
  return plan.whole_file || plan.dropped > 0;
}

/**
 * Copies of `target` that could host a merge.
 *
 * A merge is built on top of one copy's whole document, so it needs a copy that
 * *is* the whole document. `merge::build` refuses outright when every copy is a
 * patch — "which the game already merges itself" — so offering the button
 * without one is offering a button that fails.
 */
function hostsFor(plans: CleanPlan[], target: string): CleanPlan[] {
  return carriersFor(plans, target).filter((p) => p.whole_file);
}

/** Copies of `target` that this tool could reduce to only their edits. */
function cleanableFor(plans: CleanPlan[], target: string): CleanPlan[] {
  return carriersFor(plans, target).filter((p) => !p.refused && p.edits > 0);
}

/**
 * How one mergeable conflict should be settled: clean, or merge.
 *
 * A mergeable conflict exists *because* somebody ships a whole game file. The
 * copies do not disagree — one is carrying the game's own values along and
 * reverting the others by accident. There are exactly two remedies, and which
 * one applies is decided here rather than in each screen that shows it.
 *
 * **Cleaning** reduces the whole-file copies to the edits they make. Once every
 * copy of the asset is a sparse patch the game merges them itself and the
 * conflict stops existing — no third mod to keep track of. This is the better
 * remedy, so it wins wherever it is available.
 *
 * **Merging** builds one asset holding everybody's edits and holds the inputs
 * out of the game. It always works, and it costs a derived mod.
 *
 * The condition used to be wrong, and this is the bug worth naming: it offered
 * cleaning whenever *any* copy was cleanable and then suppressed the merge. But
 * cleaning only settles the asset when *every* whole-file copy can be cleaned.
 * Leave one behind — refused because it works by deleting nodes, or because the
 * game ships no baseline for it — and it goes on replacing the file and
 * reverting the patches the clean just produced, while the merge that would
 * have settled it was never offered at all.
 *
 * ## Why the merge is not simply narrowed to "everyone except the cleanable"
 *
 * That is the obvious shape and it does not work. A merge is a whole-file asset,
 * and `merge::folder_for` deliberately names it `zzz_…` so that it beats any
 * *third* mod touching the same file by load order. A cleanable mod left out of
 * the merge is exactly such a third mod: after cleaning it is a sparse patch,
 * the merge carries the game's own values where its edits were, and the merge
 * wins. Excluding it to "clean it instead" would therefore throw away the very
 * edits both operations exist to preserve. So where a merge is needed at all, it
 * covers everybody.
 *
 * Empty `plans` — the clean comparison is slower than the scan and arrives
 * afterwards — means "nothing known to be cleanable", which resolves to the
 * merge. That is the right way round: the list shows a fix immediately and
 * refines it to the cheaper one when the comparison lands.
 */
export interface MergePlan {
  /** mods to clean, in the conflict's own order; empty when merging is the fix */
  clean: string[];
  /** mods to merge; empty when cleaning settles the asset on its own */
  merge: string[];
}

export function mergePlanFor(conflict: Conflict, plans: CleanPlan[]): MergePlan {
  const mine = (found: CleanPlan[]) => found.filter((p) => conflict.mods.includes(p.owner));
  const carriers = mine(carriersFor(plans, conflict.target));
  const cleanable = mine(cleanableFor(plans, conflict.target));

  // Every carrier has to go, or the one that stays reverts the rest.
  const settles = cleanable.length > 0 && cleanable.length === carriers.length;
  if (settles) {
    return {
      clean: conflict.mods.filter((mod) => cleanable.some((p) => p.owner === mod)),
      merge: [],
    };
  }

  // Cleaning will not settle it, so a merge is the fix -- but only if one of
  // these copies is a whole file for it to be built on. When none is, the
  // engine would refuse, so there is no button to offer and the conflict is
  // one for the user to settle by load order.
  //
  // `plans` is empty until the clean comparison lands, a second or two after
  // the scan. That reads as "no host known", which would suppress every merge
  // on the first paint -- so an unexamined conflict keeps the merge and the
  // answer sharpens rather than appearing from nothing.
  const examined = plans.some((p) => p.target === conflict.target);
  const mergeable = !examined || hostsFor(plans, conflict.target).length > 0;
  return { clean: [], merge: mergeable ? conflict.mods : [] };
}

/**
 * The merge action for one conflict.
 *
 * `stubborn` is the whole-file copies that cannot be cleaned, when there are
 * any: they are the reason this is a merge rather than the cheaper clean, and
 * saying so is the difference between a recommendation and an instruction.
 */
function combineAction(conflict: Conflict, stubborn: string[], name: Naming): Action {
  const mods = conflict.mods;
  return {
    id: `combine:${conflict.target}`,
    urgency: "merge",
    title: `Combine ${plural(mods.length, "mod")} editing ${assetName(conflict.target)}`,
    why:
      "None of them edits the same property, so nothing has to be chosen — but only one of them loads, and the rest of their edits are discarded.",
    mods,
    target: conflict.target,
    metric: Object.entries(conflict.edit_counts)
      .sort((a, b) => b[1] - a[1])
      .map(([mod, count]) => `${name(mod)} ${n(count)}`)
      .join("   "),
    verb: "combine",
    subject: conflict.target,
    // Always the exact mods this card showed. The engine rescans before it
    // merges, so without this a library that changed in between would quietly
    // merge a different set of mods than the one the user agreed to.
    only: mods,
    bonus: stubborn.length
      ? `Cleaning would be tidier, but ${stubborn.map(name).join(", ")} cannot be cleaned — it replaces the whole file and would go on reverting the others. Combining is the fix that holds here.`
      : undefined,
  };
}

/** The conflict nobody can settle for you. */
function decideAction(conflict: Conflict, name: Naming): Action {
  return {
    id: `decide:${conflict.target}`,
    urgency: "decide",
    title: `Choose which mod wins ${assetName(conflict.target)}`,
    why: conflict.overlap.length
      ? `${plural(conflict.overlap.length, "property", "properties")} are set to different values by more than one mod, so this one is a real disagreement.`
      : "These cannot be combined automatically.",
    mods: conflict.mods,
    target: conflict.target,
    metric: conflict.predicted_winner
      ? `${name(conflict.predicted_winner)} currently wins`
      : undefined,
  };
}

/**
 * Build the ranked list.
 *
 * `plans` may be empty: the clean comparison is slower than the scan and
 * arrives afterwards, so the list is built twice and simply grows.
 *
 * `mended` are the mods running a build this program repaired. They cannot come
 * from `plans`, which are about pruning and know nothing about a mend, so they
 * are passed separately — off the loadout, which is the only record of them.
 * Without this a mended mod appeared nowhere in this tab: the Library said it
 * was running a mended build and offered the way back, and the Actions list,
 * which is where every other fix is listed and undone, had never heard of it.
 */
export function buildActions(
  report: Report,
  plans: CleanPlan[],
  name: Naming = asItself,
  mended: string[] = [],
): Action[] {
  const out: Action[] = [];

  // A file the game cannot read means that mod is doing nothing at all, and
  // says so nowhere. Nothing below this outranks it.
  for (const file of report.broken ?? []) {
    out.push({
      id: `broken:${file.mod}:${file.rel_path}`,
      urgency: "broken",
      title: `${name(file.mod)} is not running`,
      why: `The game cannot read one of its files, and reports nothing when it gives up. ${file.error}`,
      mods: [file.mod],
      metric: file.rel_path,
      // Offered for every broken file. Whether it is *actually* mendable is
      // decided by the engine, which looks at the file rather than the error
      // message; pressing it checks first and says so if it cannot.
      verb: "repair",
      subject: file.mod,
    });
  }

  for (const drift of report.drift ?? []) {
    if (drift.severity === "INFO") continue;
    const anchors = drift.moved.filter((m) => m.contents_held);
    out.push({
      id: `drift:${drift.mod}:${drift.target}`,
      urgency: "wrong",
      title: `${name(drift.mod)} has moved ${plural(anchors.length, "anchor")} in ${assetName(drift.target)}`,
      why: "The scene still draws correctly, so nothing looks wrong until the game reads one of these as a position and finds it somewhere else.",
      mods: [drift.mod],
      target: drift.target,
      metric: anchors
        .slice(0, 2)
        .map((m) => `${m.path} ${m.distance.toFixed(1)}u`)
        .join("   "),
    });
  }

  // Conflicts: each one gets exactly one remedy, chosen by `mergePlanFor`.
  //
  // One conflict, one card. A mod that is cleanable for several assets collects
  // them into a single clean action, because that is one press of one button and
  // listing it five times would bury everything else.
  const unblocks = new Map<string, { owners: Set<string>; assets: Set<string> }>();
  for (const conflict of report.conflicts.filter((c) => !c.benign)) {
    if (conflict.mergeable !== true) {
      out.push(decideAction(conflict, name));
      continue;
    }

    const plan = mergePlanFor(conflict, plans);
    if (plan.clean.length === 0) {
      // Neither remedy applies: nothing here can be cleaned and there is no
      // whole-file copy to build a merge on. Saying so beats a button that
      // would be refused by the engine the moment it was pressed.
      if (plan.merge.length === 0) {
        out.push(decideAction(conflict, name));
        continue;
      }
      // The copies that carry values they do not edit and cannot be reduced:
      // they are why this is a merge rather than the cheaper clean.
      const stubborn = carriersFor(plans, conflict.target)
        .filter((p) => p.refused || p.edits === 0)
        .map((p) => p.owner)
        .filter((owner) => conflict.mods.includes(owner));
      out.push(combineAction(conflict, stubborn, name));
      continue;
    }

    for (const owner of plan.clean) {
      const held = unblocks.get(owner) ?? { owners: new Set(), assets: new Set() };
      for (const mod of conflict.mods) if (mod !== owner) held.owners.add(mod);
      held.assets.add(conflict.target);
      unblocks.set(owner, held);
    }
  }

  for (const [owner, held] of unblocks) {
    const others = [...held.owners];
    const assets = [...held.assets];
    out.push({
      id: `unblock:${owner}`,
      urgency: "unblock",
      title: `Clean ${name(owner)} to settle ${plural(assets.length, "conflict")}`,
      why: `It writes values it does not change, so it silently takes back ${others.length ? `${others.map(name).join(", ")}'s edits` : "other mods' edits"} wherever they overlap. Cutting it back to the changes it actually makes ends ${assets.length === 1 ? "the conflict" : "those conflicts"} outright — no merged mod to keep track of afterwards.`,
      mods: [owner, ...others],
      target: assets[0],
      metric: assets.map(assetName).join("   "),
      verb: "clean",
      subject: owner,
    });
  }

  // Cleaning is not a fix for anything broken; it is the thing that stops the
  // next conflict happening. It ranks last and says why it is worth doing.
  const byMod = new Map<string, CleanPlan[]>();
  for (const plan of plans) {
    byMod.set(plan.owner, [...(byMod.get(plan.owner) ?? []), plan]);
  }
  for (const [owner, files] of byMod) {
    const cleaned = files.filter((f) => f.cleaned);
    if (cleaned.length) {
      out.push({
        id: `undo:${owner}`,
        urgency: "tidy",
        title: `${name(owner)} is cleaned`,
        why: `${plural(cleaned.length, "file")} replaced with the edits ${cleaned.length === 1 ? "it actually makes" : "they actually make"}. The mod as its author shipped it was never touched, so putting it back costs nothing.`,
        mods: [owner],
        verb: "undo",
        subject: owner,
      });
      continue;
    }

    // Already recommended above, for a better reason than tidiness.
    if (unblocks.has(owner)) continue;

    // `carries` is what makes this worth a card: a patch that is already all
    // edits has nothing to strip, and a button that removes nothing is noise in
    // the one list that exists to be short.
    const doable = files.filter((f) => !f.refused && f.edits > 0 && carries(f));
    if (!doable.length) continue;

    const shipped = doable.reduce((sum, f) => sum + f.original, 0);
    // What cleaning removes, and what it leaves. Both counted off `original`,
    // so the two sides of the arrow are the same kind of thing -- `edits` is a
    // count of leaves and belongs on neither side of it.
    const carried = doable.reduce((sum, f) => sum + f.dropped, 0);
    const kept = shipped - carried;

    // Which other mods this one is stepping on, across every file it replaces.
    const victims = new Set<string>();
    for (const file of doable) {
      const clash = report.conflicts.find((c) => c.target === file.target);
      for (const mod of clash?.mods ?? []) if (mod !== owner) victims.add(mod);
    }

    // Two different things wear this button, and they do not deserve the same
    // sentence. A whole-file copy reverts *everything* the game has changed
    // since the mod was built. A patch only writes what it names, so what it
    // reverts is exactly the values it restates and nothing else -- true, much
    // smaller, and saying the louder thing about it would be wrong.
    const replaces = doable.some((f) => f.whole_file);

    out.push({
      id: `clean:${owner}`,
      urgency: "tidy",
      title: `Clean ${name(owner)}`,
      why: replaces
        ? victims.size
          ? `It replaces whole game files, so it reverts ${plural(victims.size, "other mod")}' edits and every change the game has made since it was built — none of which it meant to touch.`
          : "It replaces whole game files, so it reverts every change the game has made since it was built — and will fight any mod that touches the same file."
        : `Its patches restate ${plural(carried, "value")} they do not change, and those get written to the game like any other edit${victims.size ? `, so they quietly take back whatever ${[...victims].map(name).join(", ")} does to them` : " — so any mod that edits one of them can be reverted by accident"}.`,
      mods: [owner, ...victims],
      metric: `${n(shipped)} → ${n(kept)} properties`,
      verb: "clean",
      subject: owner,
      bonus: victims.size
        ? `Stops it overriding ${[...victims].map(name).join(", ")}.`
        : undefined,
    });
  }

  // Mods running a build this program repaired. Same card as a cleaned mod's,
  // for the same reason — it is a fix of ours and this is where fixes of ours
  // are listed and undone — but it must say the right thing about itself: a
  // mend puts right a file the game could not read, and reduces nothing.
  for (const owner of mended) {
    if (out.some((a) => a.id === `undo:${owner}`)) continue;
    out.push({
      id: `undo:${owner}`,
      urgency: "tidy",
      title: `${name(owner)} is mended`,
      why: "A file the game could not read was put right, so the mod actually runs. The mod as its author shipped it was never touched, so putting it back costs nothing.",
      mods: [owner],
      verb: "undo",
      subject: owner,
    });
  }

  return out.sort((a, b) => RANK[a.urgency] - RANK[b.urgency]);
}

/**
 * One line for the top of the screen.
 *
 * Critical is named separately rather than folded into a count of "things",
 * because "3 things to look at" reads the same whether the game is running
 * wrong or three mods could be tidier, and those are not the same news.
 *
 * It reads [`pending`] rather than the list it is handed, so that a library
 * whose only remaining rows are undos is reported as having nothing to do —
 * which it has.
 */
export function verdict(all: Action[]): string {
  const actions = pending(all);
  const critical = actions.filter((a) => bandOf(a.urgency) === "critical").length;
  const advised = actions.filter((a) => bandOf(a.urgency) === "recommended").length;

  if (critical === 0 && advised === 0) {
    // Answering the heading above it, which reads "Recommended actions". Not
    // "Nothing is wrong", which is a claim about the library rather than an
    // answer, and which was shown over a screen full of optional work.
    return actions.length === 0 ? "None." : "Nothing is wrong.";
  }
  if (critical === 0) {
    return `${plural(advised, "mod is", "mods are")} losing edits.`;
  }
  const rest = advised ? `, and ${plural(advised, "more thing", "more things")} to look at` : "";
  return `${plural(critical, "thing needs", "things need")} fixing${rest}.`;
}

/** The supporting line: what was actually examined. */
export function scope(report: Report): string {
  return `${plural(report.stats.mods, "mod")}, ${n(report.stats.targets)} game assets checked.`;
}
