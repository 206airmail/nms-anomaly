/** Does `actions.ts` tell the user the right thing to do?
 *
 * Run with `npm run check:actions`. It bundles the module for the server with
 * Vite and executes it under Node -- no browser, no DOM, no test framework.
 *
 * Every case here is a shape the real library produces, and the two that matter
 * most are the ones the old code got wrong: a mergeable conflict where only
 * *some* of the whole-file copies can be cleaned (the merge used to vanish, so
 * the conflict was silently reported as handled), and the ordering of the two
 * remedies, which must never leave a cleanable mod outside a merge that would
 * then override it.
 */

import {
  bandOf,
  buildActions,
  mergePlanFor,
  pending,
  reversible,
  verdict,
  banded,
} from "../src/lib/actions";
import type { CleanPlan } from "../src/lib/engine";

let failures = 0;
function ok(what: string, cond: boolean, extra = "") {
  if (!cond) failures++;
  console.log(`${cond ? "PASS" : "FAIL"}  ${what}${extra ? "  -- " + extra : ""}`);
}

/** A whole-file (.MBIN) copy: replaces the asset outright. */
const plan = (owner: string, target: string, over: Partial<CleanPlan> = {}): CleanPlan => ({
  owner,
  target,
  rel: "GLOBALS/X.MBIN",
  original: 153416,
  edits: 2,
  dropped: 153414,
  whole_file: true,
  refused: null,
  cleaned: false,
  ...over,
});

/** A sparse (.EXML) copy that carries values it does not change. */
const patch = (owner: string, target: string, over: Partial<CleanPlan> = {}): CleanPlan =>
  plan(owner, target, {
    rel: "GLOBALS/X.EXML",
    whole_file: false,
    original: 6183,
    edits: 2790,
    dropped: 3393,
    ...over,
  });

const conflict = (mods: string[], over: any = {}) => ({
  target: "METADATA/REALITY/TABLES/REWARDTABLE.MBIN",
  mods,
  severity: "CRITICAL",
  kind: "field-disagreement",
  benign: false,
  summary: "3 shared properties set to different values",
  predicted_winner: mods[mods.length - 1],
  unique_counts: {},
  declared_only: [],
  notes: [],
  clashes: [],
  mergeable: true,
  overlap: [],
  edit_counts: Object.fromEntries(mods.map((m, i) => [m, (i + 1) * 3])),
  ...over,
});

const report = (over: any = {}) => ({
  roots: ["MODS"],
  winner_rule: "last",
  reference_version: null,
  stats: { mods: 3, files: 9, targets: 5, exml: 4, mbin: 5, dds: 0, lua: 0, decompiled: 2, parse_errors: 0, error_details: [] },
  actionable_count: 0,
  merged_count: 0,
  real_load_order: true,
  manager: null,
  disable_all: false,
  disabled: [],
  unregistered: [],
  mods: [],
  conflicts: [],
  broken: [],
  drift: [],
  tools: { mbincompiler: "7.03", hgpaktool: "1.0", scene_check: true, scene_check_note: null },
  stale: [],
  loc_clashes: [],
  ...over,
});

console.log("\n== 1. every whole-file copy is cleanable: cleaning settles it, no merge ==");
{
  const c = conflict(["Whole A", "Whole B", "Patch C"]);
  const plans = [plan("Whole A", c.target), plan("Whole B", c.target)];
  const p = mergePlanFor(c as any, plans);
  ok("both whole-file copies are offered for cleaning", p.clean.join(",") === "Whole A,Whole B", p.clean.join(","));
  ok("nothing is left to merge", p.merge.length === 0, JSON.stringify(p.merge));

  const acts = buildActions(report({ conflicts: [c] }) as any, plans);
  ok("no merge action is produced", !acts.some((a) => a.verb === "combine"));
  ok("two clean actions are produced", acts.filter((a) => a.urgency === "unblock").length === 2);
  ok("the clean says it ends the conflict", acts.find((a) => a.urgency === "unblock")!.why.includes("outright"));
}

console.log("\n== 2. THE BUG: one whole-file copy cannot be cleaned ==");
{
  const c = conflict(["Whole A", "Stubborn B", "Patch C"]);
  const plans = [
    plan("Whole A", c.target),
    plan("Stubborn B", c.target, { refused: "works by removing nodes", edits: 0 }),
  ];
  const p = mergePlanFor(c as any, plans);
  ok("cleaning is not offered, because it would not settle it", p.clean.length === 0, p.clean.join(","));
  ok("the merge covers everybody", p.merge.join(",") === "Whole A,Stubborn B,Patch C", p.merge.join(","));

  const acts = buildActions(report({ conflicts: [c] }) as any, plans);
  const merge = acts.find((a) => a.verb === "combine");
  ok("a merge action IS produced (this used to vanish entirely)", !!merge);
  ok("it covers all three mods", merge!.mods.length === 3, merge!.mods.join(","));
  ok("the cleanable mod is IN the merge, not left to be overridden", merge!.mods.includes("Whole A"));
  ok("it says why cleaning is not the answer here", merge!.bonus!.includes("Stubborn B") && merge!.bonus!.includes("cannot be cleaned"), merge!.bonus);
  ok("no clean action is offered for this conflict", !acts.some((a) => a.urgency === "unblock"));
}

console.log("\n== 3. nothing is cleanable: whole-conflict merge ==");
{
  const c = conflict(["Whole A", "Whole B"]);
  const p = mergePlanFor(c as any, []);
  ok("everybody merges", p.merge.join(",") === "Whole A,Whole B" && p.clean.length === 0);
  const acts = buildActions(report({ conflicts: [c] }) as any, []);
  const merge = acts.find((a) => a.verb === "combine")!;
  ok("`only` pins the exact set the card showed", merge.only!.join(",") === "Whole A,Whole B");
  ok("nothing is blamed when nothing was examined", merge.bonus === undefined);
}

console.log("\n== 3b. plans not arrived yet: a fix is offered immediately ==");
{
  // The clean comparison is slower than the scan. Before it lands the list must
  // still show something actionable, and refine to the cheaper fix afterwards.
  const c = conflict(["Whole A", "Patch C"]);
  const early = buildActions(report({ conflicts: [c] }) as any, []);
  ok("a merge is offered while the comparison is still running", early.some((a) => a.verb === "combine"));

  const late = buildActions(report({ conflicts: [c] }) as any, [plan("Whole A", c.target)]);
  ok("once it lands, the cheaper clean replaces it", !late.some((a) => a.verb === "combine") && late.some((a) => a.verb === "clean"));
}

console.log("\n== 4. a cleaned copy is no longer a merge host ==");
{
  const c = conflict(["Whole A", "Patch C"]);
  const plans = [plan("Whole A", c.target, { cleaned: true })];
  const p = mergePlanFor(c as any, plans);
  ok("an already-cleaned copy is not offered again", p.clean.length === 0);
  // Once the only whole-file copy has been cleaned, every copy is a patch --
  // and `merge::build` refuses outright with "which the game already merges
  // itself". Offering the button would be offering one that is rejected.
  ok("and no merge is offered, because nothing can host one", p.merge.length === 0);
  const acts = buildActions(report({ conflicts: [c] }) as any, plans);
  ok("it asks for a decision instead", acts.some((a) => a.urgency === "decide"));
  ok("and shows no combine button", !acts.some((a) => a.verb === "combine"));
}

console.log("\n== 4b. all-patch conflicts never offer a merge that would be refused ==");
{
  // Two sparse copies, one carrying dead weight it cannot be rid of. There is
  // no whole document to build a merge on top of.
  const c = conflict(["Carrier A", "Patch B"]);
  const plans = [
    patch("Carrier A", c.target, { refused: "the game ships no copy of this asset" }),
    patch("Patch B", c.target, { original: 610, edits: 609, dropped: 1 }),
  ];
  const p = mergePlanFor(c as any, plans);
  ok("no merge", p.merge.length === 0);
  ok("no clean either — one of them cannot be reduced", p.clean.length === 0);
  ok(
    "so it is a decision",
    buildActions(report({ conflicts: [c] }) as any, plans).some((a) => a.urgency === "decide"),
  );
}

console.log("\n== 4c. a contested .EXML carrying vanilla values is cleanable ==");
{
  // The BetterRewardsCombined shape: sparse in form, not in content. Cleaning
  // every carrier settles the asset exactly as it does for a whole file.
  const c = conflict(["Carrier A", "Patch B"]);
  const plans = [
    patch("Carrier A", c.target, { original: 6183, edits: 2790, dropped: 3393 }),
    patch("Patch B", c.target, { original: 610, edits: 609, dropped: 1 }),
  ];
  const p = mergePlanFor(c as any, plans);
  ok("both are offered for cleaning", p.clean.join(",") === "Carrier A,Patch B", p.clean.join(","));
  ok("and no merge is needed", p.merge.length === 0);
}

console.log("\n== 4d. a patch's tidy card does not claim it replaces whole files ==");
{
  const acts = buildActions(report() as any, [patch("Carrier A", "SOME/ASSET.MBIN")]);
  const card = acts.find((a) => a.id === "clean:Carrier A")!;
  ok("it is offered", !!card);
  ok("it does not say 'replaces whole game files'", !card.why.includes("replaces whole game files"), card.why);
  ok("it names the carried count", card.why.includes("3,393 values"), card.why);

  const whole = buildActions(report() as any, [plan("Fat Mod", "SOME/ASSET.MBIN")]);
  const fat = whole.find((a) => a.id === "clean:Fat Mod")!;
  ok("a whole-file card still says what it does", fat.why.includes("replaces whole game files"), fat.why);
}

console.log("\n== 4e. a patch with nothing to strip is not offered at all ==");
{
  // 609 of 610 properties are real edits in the commonest case. Cleaning such a
  // patch removes nothing, and offering it put "restates 0 values" on a card.
  const tight = patch("Tidy Mod", "SOME/ASSET.MBIN", { original: 1, edits: 1, dropped: 0 });
  const acts = buildActions(report() as any, [tight]);
  ok("no clean card", !acts.some((a) => a.id === "clean:Tidy Mod"), JSON.stringify(acts.map((a) => a.id)));

  // And it is not a carrier, so it cannot block a conflict from being settled
  // by cleaning the copies that *do* carry.
  const c = conflict(["Whole A", "Tidy Mod"]);
  const p = mergePlanFor(c as any, [
    plan("Whole A", c.target),
    patch("Tidy Mod", c.target, { original: 4, edits: 4, dropped: 0 }),
  ]);
  ok("cleaning the real carrier settles it", p.clean.join(",") === "Whole A", p.clean.join(","));
  ok("and no merge is needed", p.merge.length === 0);

  // A whole file always carries, even when the comparison failed and the counts
  // came back as zero.
  const unknown = plan("Opaque", "SOME/ASSET.MBIN", {
    original: 0,
    edits: 0,
    dropped: 0,
    refused: "could not be decompiled",
  });
  const blocked = mergePlanFor(conflict(["Opaque", "Patch B"]) as any, [unknown]);
  ok("an unmeasurable whole file still blocks the clean", blocked.clean.length === 0);
}

console.log("\n== 4f. a nested patch whose every leaf is an edit is not offered ==");
{
  // The LaunchThrustersReworked shape, and the one the old rule got wrong. Its
  // leaves are all changes, so there is nothing in it to take away -- but
  // `original` counts the containers those leaves hang off and `edits` cannot,
  // so `original > edits` read as dead weight. The card was offered; cleaning
  // copied the same bytes into `derived/`, reported success and recorded the
  // mod as cleaned; and the card came straight back, because the next preview
  // found the same nothing to remove. Two mods sat in that loop.
  const nested = patch("Launch Thrusters", "SOME/ASSET.MBIN", {
    original: 70,
    edits: 26,
    dropped: 0,
  });
  const acts = buildActions(report() as any, [nested]);
  ok(
    "no clean card",
    !acts.some((a) => a.id === "clean:Launch Thrusters"),
    JSON.stringify(acts.map((a) => a.id)),
  );

  // And the arrow on a card that IS offered compares like with like: both sides
  // counted off `original`, never `original` against a count of leaves.
  const real = buildActions(report() as any, [patch("Carrier A", "SOME/ASSET.MBIN")]);
  const card = real.find((a) => a.id === "clean:Carrier A")!;
  ok("the metric is original -> what survives", card.metric === "6,183 → 2,790 properties", card.metric);
}

console.log("\n== 5. one mod, two assets, a different remedy for each ==");
{
  const settled = conflict(["Whole A", "Patch C"], { target: "A/ONE.MBIN" });
  const blocked = conflict(["Whole A", "Stubborn B"], { target: "A/TWO.MBIN" });
  const plans = [
    plan("Whole A", "A/ONE.MBIN"),
    plan("Whole A", "A/TWO.MBIN"),
    plan("Stubborn B", "A/TWO.MBIN", { refused: "no vanilla copy", edits: 0 }),
  ];
  const acts = buildActions(report({ conflicts: [settled, blocked] }) as any, plans);

  const clean = acts.find((a) => a.id === "unblock:Whole A")!;
  ok("the clean is offered, for the asset it settles", !!clean);
  ok("and names only that asset", clean.metric === "ONE.MBIN", clean.metric);
  ok("its wording is singular", clean.why.includes("ends the conflict outright"), clean.why.slice(-60));

  ok("the other asset gets a merge", acts.some((a) => a.verb === "combine" && a.target === "A/TWO.MBIN"));
  ok("the settled asset does not", !acts.some((a) => a.verb === "combine" && a.target === "A/ONE.MBIN"));
  ok(
    "and that merge includes Whole A, so cleaning cannot strand it",
    acts.find((a) => a.target === "A/TWO.MBIN" && a.verb === "combine")!.mods.includes("Whole A"),
  );
}

console.log("\n== 5b. plural wording when one clean settles several assets ==");
{
  const one = conflict(["Whole A", "Patch C"], { target: "A/ONE.MBIN" });
  const two = conflict(["Whole A", "Patch D"], { target: "A/TWO.MBIN" });
  const plans = [plan("Whole A", "A/ONE.MBIN"), plan("Whole A", "A/TWO.MBIN")];
  const clean = buildActions(report({ conflicts: [one, two] }) as any, plans).find(
    (a) => a.id === "unblock:Whole A",
  )!;
  ok("one card for both", clean.metric === "ONE.MBIN   TWO.MBIN", clean.metric);
  ok("plural reads correctly", clean.why.includes("ends those conflicts outright"), clean.why.slice(-60));
  ok("it names both other mods", clean.mods.includes("Patch C") && clean.mods.includes("Patch D"));
}

console.log("\n== 6. non-mergeable conflicts still ask for a decision ==");
{
  const c = conflict(["A", "B"], { mergeable: false, overlap: ["Rate", "Cost"] });
  const acts = buildActions(report({ conflicts: [c] }) as any, [plan("A", c.target)]);
  ok("it is a decide, not a merge", acts.some((a) => a.urgency === "decide") && !acts.some((a) => a.verb === "combine"));
}

console.log("\n== 7. mergeable === null (no vanilla baseline) is not treated as safe ==");
{
  const c = conflict(["A", "B"], { mergeable: null });
  const acts = buildActions(report({ conflicts: [c] }) as any, []);
  ok("unknown is asked about, never merged", acts.some((a) => a.urgency === "decide") && !acts.some((a) => a.verb === "combine"));
}

console.log("\n== 8. bands and the verdict ==");
{
  const clean = report();
  ok("a clean library answers the heading", verdict(buildActions(clean as any, [])) === "None.");

  const broken = report({ broken: [{ mod: "M", rel_path: "A.EXML", error: "mismatched tag" }] });
  const acts = buildActions(broken as any, []);
  ok("a broken mod reads as needing fixing", verdict(acts) === "1 thing needs fixing.", verdict(acts));
  const groups = banded(acts);
  ok("it lands in the critical band", groups[0].id === "critical" && groups[0].rows.length === 1);
  ok("empty bands are not rendered", groups.length === 1);

  const both = report({
    broken: [{ mod: "M", rel_path: "A.EXML", error: "x" }],
    conflicts: [conflict(["A", "B"], { mergeable: false, overlap: ["R"] })],
  });
  ok(
    "critical and recommended are counted apart",
    verdict(buildActions(both as any, [])) === "1 thing needs fixing, and 1 more thing to look at.",
    verdict(buildActions(both as any, [])),
  );

  const tidyOnly = report();
  const tidyActs = buildActions(tidyOnly as any, [plan("Fat Mod", "SOME/OTHER.MBIN")]);
  ok("a tidy-only library says nothing is wrong", verdict(tidyActs) === "Nothing is wrong.", verdict(tidyActs));
  ok("and it is in the optional band", banded(tidyActs)[0].id === "optional");
}

console.log("\n== 8b. an already-cleaned mod offers the way back ==");
{
  // `clean_preview` adds these from the loadout by diffing the two builds; a
  // cleaned mod ships no override for the scan to find, so without them it
  // would drop off this list entirely and take its undo with it.
  const done = [plan("Fat Mod", "A/ONE.MBIN", { cleaned: true, original: 0, edits: 0 })];
  const acts = buildActions(report() as any, done);
  const undo = acts.find((a) => a.verb === "undo")!;
  ok("an undo action is offered", !!undo);
  ok("it is optional, not something wrong", bandOf(undo.urgency) === "optional");
  ok("singular reads correctly", undo.why.includes("1 file replaced with the edits it actually makes"), undo.why);
  ok("and it does not claim a backup exists", !undo.why.includes("outside the mods folder"));

  const two = [
    plan("Fat Mod", "A/ONE.MBIN", { cleaned: true }),
    plan("Fat Mod", "A/TWO.MBIN", { cleaned: true }),
  ];
  const plural2 = buildActions(report() as any, two).find((a) => a.verb === "undo")!;
  ok("plural reads correctly", plural2.why.includes("2 files replaced with the edits they actually make"), plural2.why);
}

console.log("\n== 8c. an undo is not work outstanding ==");
{
  // A library where everything cleanable has been cleaned. The undos are worth
  // keeping -- they are the only place the way back is offered -- but they are
  // not a to-do list, and counting them as one had the screen say "Nothing is
  // wrong" under an Optional heading promising to make the library tidier,
  // above twenty-five rows of work already finished.
  const done = [
    plan("Fat Mod", "A/ONE.MBIN", { cleaned: true, original: 0, edits: 0, dropped: 0 }),
  ];
  const acts = buildActions(report() as any, done);

  ok("there is still an undo to offer", reversible(acts).length === 1);
  ok("but nothing is being asked for", pending(acts).length === 0);
  ok("so the verdict is None", verdict(acts) === "None.", verdict(acts));
  ok("and no band is rendered at all", banded(pending(acts)).length === 0);

  // With real work outstanding as well, the Optional band comes back and the
  // undo stays out of it.
  const mixed = buildActions(report() as any, [...done, plan("Fat Two", "A/TWO.MBIN")]);
  ok("the verdict notices the outstanding work", verdict(mixed) === "Nothing is wrong.", verdict(mixed));
  const optional = banded(pending(mixed)).find((b) => b.id === "optional")!;
  ok("the optional band holds only the clean", optional.rows.length === 1, JSON.stringify(optional.rows.map((r) => r.id)));
  ok("and the undo is still offered separately", reversible(mixed).length === 1);
}

console.log("\n== 8d. a mended mod is listed and can be put back ==");
{
  // A mend produces no clean plan — plans are about pruning — so without being
  // passed separately a mended mod appeared in this tab nowhere at all, while
  // the Library said it was running a mended build and offered the way back.
  const acts = buildActions(report() as any, [], undefined, ["Broken Mod"]);
  const undo = acts.find((a) => a.subject === "Broken Mod")!;
  ok("it is offered", !!undo);
  ok("as a way back, not as work", undo.verb === "undo" && reversible(acts).length === 1);
  ok("nothing is being asked for", pending(acts).length === 0);
  ok("it says a mend, not a clean", undo.why.includes("could not read"), undo.why);
  ok("and does not claim to have reduced anything", !undo.why.includes("edits they actually make"));

  // A mod that is both cleaned and mended is one card, not two: the ids collide
  // deliberately, and the clean's wording wins because it carries the counts.
  const both = buildActions(
    report() as any,
    [plan("Broken Mod", "A/ONE.MBIN", { cleaned: true, original: 0, edits: 0, dropped: 0 })],
    undefined,
    ["Broken Mod"],
  );
  ok("one card, not two", both.filter((a) => a.subject === "Broken Mod").length === 1);
}

console.log("\n== 9. ordering: critical first, tidy last ==");
{
  const r = report({
    broken: [{ mod: "M", rel_path: "A.EXML", error: "x" }],
    conflicts: [conflict(["A", "B"], { mergeable: false, overlap: ["R"] })],
    drift: [{ mod: "D", rel_path: "S.MBIN", target: "S.MBIN", severity: "MAJOR", reference: "vanilla", added: 0, removed: 0, moved: [{ path: "N", kind: "LOCATOR", before: [0, 0, 0], after: [1, 0, 0], distance: 1, axis: "X", contents_held: true, rotated_only: false }] }],
  });
  const order = buildActions(r as any, [plan("Fat", "Z/Z.MBIN")]).map((a) => a.urgency);
  ok("ranked broken, wrong, decide, tidy", order.join(">") === "broken>wrong>decide>tidy", order.join(">"));
}

// Thrown rather than `process.exit`: an uncaught throw already fails the script
// with a non-zero status, and reaching for `process` would drag `@types/node`
// into a project that has deliberately done without it.
if (failures > 0) {
  throw new Error(`${failures} of these checks failed; see the FAIL lines above`);
}
console.log("\nALL PASS");
