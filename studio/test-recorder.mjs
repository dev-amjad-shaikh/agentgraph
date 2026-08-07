#!/usr/bin/env node
/* Node unit tests for the Flight Recorder timeline helpers embedded in
 * studio/index.html. The <script> block is extracted verbatim, the final
 * browser bootstrap (`init();`) is stripped, and the pure helpers are
 * exercised under `vm` — no browser, no dependencies.
 *
 *   node studio/test-recorder.mjs
 */
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";
import vm from "node:vm";

const here = path.dirname(fileURLToPath(import.meta.url));
const html = readFileSync(path.join(here, "index.html"), "utf8");

const match = html.match(/<script>([\s\S]*?)<\/script>/);
if (!match) { console.error("FAIL: no <script> block found in index.html"); process.exit(1); }
const src = match[1].replace(/\ninit\(\);\s*$/, "\n");
if (/\ninit\(\);/.test(src)) { console.error("FAIL: bootstrap init() was not stripped cleanly"); process.exit(1); }

const sandbox = {};
vm.createContext(sandbox);
vm.runInContext(src + `
globalThis.__rec = {
  recSortEvents, recGroups, recLanes, recLaneOf, recCausalChain,
  recKindColor, recKindShort, recPayloadHtml, recDetailHtml, recMarkerHtml,
  recEffectHtml, recFormatUsd, recFormatTokens, REC_KIND_COLORS, REC_EFFECT_INFO,
  recReplayBannerHtml, recApiErrorBannerHtml, recTotalsHtml,
  recCompareRows, recCmpEventHtml, recCompareHtml,
};`, sandbox, { filename: "index.html<script>" });

const R = sandbox.__rec;

/* -- fixture: a journaled two-step run in the golden RunEvent wire shape -- */

const RUN = "019157c4-6f1f-7a3b-8c2d-9e4f5a6b7c8d";
const ev = (seq, kind, extra = {}) => ({
  id: `${RUN}:${seq}`, run_id: RUN, thread_id: "thread-42", node_id: null,
  seq, kind, effect: "pure", input: null, output: null,
  latency_ms: null, tokens: null, cost_usd: null, status: "ok",
  parent: null, recorded_at: "2026-08-07T10:00:00Z", ...extra,
});

const journal = [
  ev(0, "resume", { input: { kind: "inline", value: { checkpoint_id: "cp-9" } } }),
  ev(1, "super_step_start", { input: { kind: "inline", value: { activated: ["first"] } } }),
  ev(2, "node_input", { node_id: "first", parent: `${RUN}:1`,
    input: { kind: "inline", value: { log: [] } } }),
  ev(3, "node_output", { node_id: "first", parent: `${RUN}:2`, latency_ms: 4,
    output: { kind: "inline", value: { log: ["first"] } } }),
  ev(4, "routing_decision", { parent: `${RUN}:1`,
    output: { kind: "inline", value: { next: ["second"] } } }),
  ev(5, "checkpoint_written", { parent: `${RUN}:4`,
    output: { kind: "inline", value: { checkpoint_id: "cp-10", step: 1 } } }),
  ev(6, "super_step_end", { parent: `${RUN}:1` }),
  ev(7, "super_step_start", { input: { kind: "inline", value: { activated: ["agent"] } } }),
  ev(8, "node_input", { node_id: "agent", parent: `${RUN}:7` }),
  ev(9, "model_call", { node_id: "agent", effect: "non_idempotent", parent: `${RUN}:8`,
    latency_ms: 137, tokens: { prompt_tokens: 128, completion_tokens: 32, total_tokens: 160 },
    cost_usd: 0.00042,
    input: { kind: "inline", value: { messages: [{ role: "user", content: "ping" }], tools: [] } },
    output: { kind: "artifact", value: { sha256: "9b71d224bd62f3785d96d46ad3ea3d73319bfbc2890caadae2dff72519673ca7", bytes: 8192 } } }),
  ev(10, "tool_call", { node_id: "tools", effect: "non_idempotent", parent: `${RUN}:9`, latency_ms: 11,
    input: { kind: "inline", value: { name: "echo", args: { text: "<b>ping</b>" } } },
    output: { kind: "inline", value: { text: "ping" } } }),
  ev(11, "node_output", { node_id: "agent", parent: `${RUN}:8`, latency_ms: 152 }),
  ev(12, "routing_decision", { parent: `${RUN}:7` }),
  ev(13, "checkpoint_written", { parent: `${RUN}:12`, status: "error",
    output: { kind: "inline", value: { error: "disk full" } } }),
  ev(14, "super_step_end", { parent: `${RUN}:7` }),
  // Defensive case: a partial server may omit every optional field.
  { kind: "interrupt", status: "interrupted" },
];

/* -- tiny assert harness -------------------------------------------------- */

let passed = 0, failed = 0;
function check(name, cond, detail) {
  if (cond) { passed++; console.log(`ok   ${name}`); }
  else { failed++; console.log(`FAIL ${name}${detail ? " — " + detail : ""}`); }
}
function eq(name, got, want) {
  check(name, JSON.stringify(got) === JSON.stringify(want),
    `got ${JSON.stringify(got)}, want ${JSON.stringify(want)}`);
}

/* -- ordering -------------------------------------------------------------- */

{
  const shuffled = [journal[9], journal[0], journal[5], journal[2]];
  eq("recSortEvents orders by seq", R.recSortEvents(shuffled).map((e) => e.seq), [0, 2, 5, 9]);
  const sorted = R.recSortEvents(journal);
  eq("recSortEvents keeps seqless event last (array-order fallback)",
    sorted[sorted.length - 1].kind, "interrupt");
  check("recSortEvents tolerates non-array input", R.recSortEvents(null).length === 0);
}

/* -- lanes ----------------------------------------------------------------- */

{
  eq("recLanes: run lane first, nodes in first-appearance order",
    R.recLanes(R.recSortEvents(journal)), ["run", "first", "agent", "tools"]);
  eq("recLaneOf maps missing node_id to the run lane",
    R.recLaneOf({ kind: "interrupt" }), "run");
}

/* -- super-step grouping ---------------------------------------------------- */

{
  const groups = R.recGroups(R.recSortEvents(journal));
  eq("recGroups: prelude + two super-steps", groups.map((g) => g.n), [0, 1, 2]);
  eq("recGroups: prelude holds the resume event",
    groups[0].events.map((e) => e.kind), ["resume"]);
  check("recGroups: super_step_start opens its own group",
    groups[1].events[0].kind === "super_step_start" && groups[1].start.seq === 1);
  eq("recGroups: group 2 spans the agent step",
    groups[2].events.map((e) => e.kind),
    ["super_step_start", "node_input", "model_call", "tool_call",
     "node_output", "routing_decision", "checkpoint_written", "super_step_end", "interrupt"]);
}

/* -- causal chain ----------------------------------------------------------- */

{
  const sorted = R.recSortEvents(journal);
  const chain = R.recCausalChain(sorted, `${RUN}:10`);
  check("causal chain of tool_call walks up to its super-step",
    chain.has(`${RUN}:10`) && chain.has(`${RUN}:9`) && chain.has(`${RUN}:8`) && chain.has(`${RUN}:7`));
  check("causal chain excludes unrelated events", !chain.has(`${RUN}:3`) && !chain.has(`${RUN}:13`));
  const cyclic = [
    { id: "a", parent: "b", kind: "node_input" },
    { id: "b", parent: "a", kind: "node_output" },
  ];
  const cyc = R.recCausalChain(cyclic, "a");
  check("causal chain survives a parent cycle", cyc.has("a") && cyc.has("b") && cyc.size === 2);
  check("causal chain of unknown id is empty", R.recCausalChain(sorted, "nope:0").size === 0);
}

/* -- markers ---------------------------------------------------------------- */

{
  const marker = R.recMarkerHtml(journal[9]);
  check("marker carries data-eid", marker.includes(`data-eid="${RUN}:9"`));
  check("marker is colored by kind", marker.includes(`--kcol:${R.REC_KIND_COLORS.model_call}`));
  const errMarker = R.recMarkerHtml(journal[13]);
  check("error status marks the chip", errMarker.includes("st-error"));
  const seqless = R.recMarkerHtml({ kind: "interrupt", status: "interrupted" });
  check("seqless event renders without crashing",
    seqless.includes("st-interrupted") && seqless.includes("intr"));
  check("unknown kind falls back to the faint color",
    R.recKindColor("future_kind") === "#6b6252");
  check("every golden kind has a color and a short label",
    ["super_step_start","super_step_end","node_input","node_output","model_call","tool_call",
     "remote_call","wasm_call","interrupt","resume","routing_decision","checkpoint_written"]
      .every((k) => R.REC_KIND_COLORS[k] && typeof R.recKindShort(k) === "string" && R.recKindShort(k).length <= 6));
}

/* -- payloads --------------------------------------------------------------- */

{
  const inline = R.recPayloadHtml(journal[10].input);
  check("inline payload renders the value", inline.includes("echo"));
  check("inline payload escapes HTML", inline.includes("&lt;b&gt;") && !inline.includes("<b>"));
  const artifact = R.recPayloadHtml(journal[9].output);
  check("artifact ref shows sha256 and bytes",
    artifact.includes("9b71d224bd62f3785d96d46ad3ea3d73319bfbc2890caadae2dff72519673ca7") &&
    artifact.includes("8192"));
  check("missing payload renders a dash", R.recPayloadHtml(null).includes("—"));
  check("unknown future tag renders raw JSON",
    R.recPayloadHtml({ kind: "external", value: { uri: "s3://x" } }).includes("s3://x"));
}

/* -- detail panel ------------------------------------------------------------- */

{
  const detail = R.recDetailHtml(journal[9]);
  check("detail shows the effect badge", detail.includes("eff-non_idempotent"));
  check("detail shows the effect blurb", detail.includes("never silently retried"));
  check("detail renders the parent as a jump link", detail.includes(`data-eid="${RUN}:8"`));
  check("detail shows tokens", detail.includes("128 prompt + 32 completion = 160 total"));
  check("detail shows cost", detail.includes("$0.00042"));
  check("detail shows latency", detail.includes("137 ms"));
  const noParent = R.recDetailHtml(journal[1]);
  check("parentless event renders a dash", noParent.includes("—"));
  const unknownEff = R.recEffectHtml("future_effect");
  check("unknown effect falls back to a neutral badge", unknownEff.includes("badge pending"));
  check("every frozen effect class has a badge tone",
    ["pure","read_only","idempotent","compensatable","non_idempotent"]
      .every((e) => R.REC_EFFECT_INFO[e] && R.REC_EFFECT_INFO[e].tone === `eff-${e}`));
}

/* -- formatting --------------------------------------------------------------- */

eq("recFormatUsd micro-cost", R.recFormatUsd(0.00042), "$0.00042");
eq("recFormatUsd larger cost", R.recFormatUsd(1.5), "$1.5000");
eq("recFormatUsd missing", R.recFormatUsd(null), "—");
eq("recFormatTokens missing", R.recFormatTokens(null), "—");

/* -- replay banner (POST /runs/replay) --------------------------------------
 * Response shape: {run_id, verified, expected_events, actual_events,
 * first_divergence}. Fixture-shaped JSON — the server wave has not landed in
 * this workspace, so these fixtures are the verification stand-in. */

{
  const ok = R.recReplayBannerHtml({
    run_id: RUN, verified: true, expected_events: 16, actual_events: 16, first_divergence: null,
  });
  check("replay banner verified: ok tone + count", ok.includes("rec-banner ok") && ok.includes("16"));
  check("replay banner verified: no jump link", !ok.includes("data-jump-seq"));

  const bad = R.recReplayBannerHtml({
    run_id: RUN, verified: false, expected_events: 16, actual_events: 11, first_divergence: 9,
  });
  check("replay banner mismatch: err tone + both counts",
    bad.includes("rec-banner err") && bad.includes("16") && bad.includes("11"));
  check("replay banner mismatch: divergence is a jump link", bad.includes('data-jump-seq="9"'));

  const noDiv = R.recReplayBannerHtml({ verified: false, expected_events: 3, actual_events: 3 });
  check("replay banner mismatch without divergence: no jump link", !noDiv.includes("data-jump-seq"));

  const partial = R.recReplayBannerHtml({});
  check("replay banner tolerates an empty/partial response",
    partial.includes("rec-banner err") && partial.includes("?"));
}

/* -- replay/diff error mapping (404 / 409 / 422 / route-missing / other) ----- */

{
  const noRoute = R.recApiErrorBannerHtml("POST /runs/replay", 404, null);
  check("non-JSON 404 is the route-missing note",
    noRoute.includes("rec-banner warn") && noRoute.includes("POST /runs/replay"));

  const unknown = R.recApiErrorBannerHtml("POST /runs/replay", 404,
    { error: "not_found", message: "run `abc` not found" });
  check("JSON 404 is the unknown-run note",
    unknown.includes("Unknown run (404)") && unknown.includes("run `abc` not found") && !unknown.includes("route yet"));

  const noJournal = R.recApiErrorBannerHtml("GET /runs/diff", 409,
    { error: "conflict", message: "run `abc` has no persisted journal" });
  check("409 is the no-persisted-journal note",
    noJournal.includes("No persisted journal (409)") && noJournal.includes("no persisted journal"));

  const noGraph = R.recApiErrorBannerHtml("POST /runs/replay", 422,
    { error: "unprocessable", message: "graph `react_agent` is not registered" });
  check("422 is the graph-not-registered note",
    noGraph.includes("Graph not registered (422)") && noGraph.includes("not registered"));

  const boom = R.recApiErrorBannerHtml("POST /runs/replay", 500, { error: "internal_error", message: "boom" });
  check("other statuses render the message verbatim", boom.includes("boom"));

  const net = R.recApiErrorBannerHtml("GET /runs/diff", 0, { error: "network", message: "connection refused" });
  check("network failure renders its message", net.includes("connection refused"));

  const escaped = R.recApiErrorBannerHtml("POST /runs/replay", 404,
    { error: "not_found", message: "run `<script>` not found" });
  check("error messages are HTML-escaped", escaped.includes("&lt;script&gt;") && !escaped.includes("<script>`"));
}

/* -- fork compare: row alignment (BranchDiff shape from rusty-core replay.rs) */

const cev = (seq, kind, extra = {}) => ({
  id: `cmp:${seq}`, seq, kind, node_id: null, status: "ok", ...extra,
});
const CMP_BASE = [
  cev(0, "super_step_start"), cev(1, "node_input", { node_id: "first" }),
  cev(2, "node_output", { node_id: "first" }), cev(3, "super_step_end"),
  cev(4, "super_step_start"), cev(5, "model_call", { node_id: "agent" }),
];
const CMP_BRANCH = [
  cev(0, "super_step_start"), cev(1, "node_input", { node_id: "first" }),
  cev(2, "node_output", { node_id: "first" }), cev(3, "super_step_end"),
  cev(4, "super_step_start"),               // same seq as base, different evidence past it
  cev(6, "tool_call", { node_id: "tools" }), // seq 6 exists only on the branch
];
const CMP_DIFF = {
  first_divergent_seq: 4,
  added: [CMP_BRANCH[4], CMP_BRANCH[5]],
  removed: [CMP_BASE[4], CMP_BASE[5]],
  step_diffs: [],
  base_totals: { events: 6, tokens: { prompt_tokens: 128, completion_tokens: 32, total_tokens: 160 }, cost_usd: 0.00042 },
  branch_totals: { events: 6, tokens: { prompt_tokens: 64, completion_tokens: 16, total_tokens: 80 }, cost_usd: 0.00021 },
};

{
  const rows = R.recCompareRows(CMP_BASE, CMP_BRANCH, CMP_DIFF);
  eq("compare rows: one row per seq in the union",
    rows.map((r) => r.seq), [0, 1, 2, 3, 4, 5, 6]);
  eq("compare rows: identical prefix is dimmed",
    rows.slice(0, 4).map((r) => r.cls), ["same", "same", "same", "same"]);
  eq("compare rows: shared seq at the fork is divergent", rows[4].cls, "divergent");
  eq("compare rows: base-only seq is removed", rows[5].cls, "removed");
  eq("compare rows: branch-only seq is added", rows[6].cls, "added");
  check("compare rows: removed row carries only the base event",
    rows[5].base && rows[5].base.kind === "model_call" && rows[5].branch === null);
  check("compare rows: added row carries only the branch event",
    rows[6].branch && rows[6].branch.kind === "tool_call" && rows[6].base === null);
}

{
  const identical = R.recCompareRows(CMP_BASE, CMP_BASE, { first_divergent_seq: null });
  check("compare rows: identical branches are all 'same'",
    identical.every((r) => r.cls === "same"));
}

{
  // Partial diff: no first_divergent_seq — divergence derived from presence.
  const rows = R.recCompareRows(CMP_BASE, CMP_BRANCH, {});
  eq("partial diff: presence-derived classes",
    rows.map((r) => r.cls), ["same", "same", "same", "same", "same", "removed", "added"]);
  const noDiff = R.recCompareRows(CMP_BASE, CMP_BRANCH, null);
  eq("null diff: same presence-derived classes",
    noDiff.map((r) => r.cls), ["same", "same", "same", "same", "same", "removed", "added"]);
  const seqless = R.recCompareRows([cev(0, "resume"), { kind: "interrupt" }], [cev(0, "resume")], null);
  check("seqless events do not crash and align by position",
    seqless.length === 2 && seqless[1].cls === "removed" && seqless[1].base.kind === "interrupt");
}

{
  const html = R.recCompareHtml("base-run-0001", "branch-run-0002", CMP_BASE, CMP_BRANCH, CMP_DIFF);
  check("compare html: both run ids in the header",
    html.includes("base-run-0001") && html.includes("branch-run-0002"));
  check("compare html: per-branch totals from base_totals/branch_totals",
    html.includes("128 prompt + 32 completion = 160 total") && html.includes("$0.00042") &&
    html.includes("64 prompt + 16 completion = 80 total") && html.includes("$0.00021"));
  check("compare html: divergence is marked once at seq 4",
    html.includes("first divergence — seq 4") && html.indexOf("first divergence") === html.lastIndexOf("first divergence"));
  check("compare html: removed and added tags", html.includes(">removed</span>") && html.includes(">added</span>"));
  check("compare html: prefix cells are dimmed", html.includes('cmp-side same'));

  const sameHtml = R.recCompareHtml("a", "b", CMP_BASE, CMP_BASE, { first_divergent_seq: null });
  check("compare html: identical branches say so", sameHtml.includes("logically identical"));

  const xssBase = [cev(0, "node_input", { node_id: "<img src=x onerror=alert(1)>" })];
  const xssHtml = R.recCompareHtml("a", "b", xssBase, [], { first_divergent_seq: 0 });
  check("compare html escapes node ids", !xssHtml.includes("<img") && xssHtml.includes("&lt;img"));

  const noTotals = R.recCompareHtml("a", "b", CMP_BASE, CMP_BRANCH, {});
  check("compare html: missing totals degrade to a note", noTotals.includes("totals unavailable"));
}

{
  const totals = R.recTotalsHtml(CMP_DIFF.base_totals);
  check("totals line: events + tokens + cost",
    totals.includes("6 event(s)") && totals.includes("160 total") && totals.includes("$0.00042"));
  eq("totals line: missing value", R.recTotalsHtml(undefined).includes("unavailable"), true);
}

console.log(`\n${passed} passed, ${failed} failed`);
process.exit(failed ? 1 : 0);
