No file-writing tool (Write/Edit/Bash) is available in this subagent context, and system tools only cover memory/skills features — so I cannot create the report file myself. Below is the complete report, ready to save verbatim to `/Users/amjad.shaikh/claude-work/claude-white-papers/05 - RUST/aboutrusty-review/learn-content.md` (parent agent: please write it there; the directory does not yet exist).

---

# R4_LearnContent — Learn Section Content Review

## Verdict

The Learn section is unusually strong on fidelity: I spot-checked every factual claim in the architecture, quickstart, studio, and roadmap articles against `docs/architecture.md`, `server-quickstart.md`, `studio.md`, `roadmap.md`, `versioning.md`, and `stability.md`, and found near-verbatim accuracy (frame ids, reducer semantics, 404-never-403, MSRV 1.86, current versions as of 2026-08-06 all check out). The two real wounds are structural: the flagship quickstart article never shows the `main.rs` it tells readers to write, and the architecture article flattens the source doc's diagrams into prose while skipping the entire LLM/remote/WASM/MCP dimension that its own failure-mode table depends on. Hooks, index descriptions, callouts, and reading-time estimates are generally good; most remaining findings are polish.

## Strengths

- **Fidelity is excellent.** Error codes, defaults (`max_steps` 1000, event log 1000 frames), SSE frame ordering, CORS middleware details, and the rejected-integrations quotes all match the source docs word-for-word.
- **Strong hook pattern.** 4 of 5 articles open with a memorable aphorism in a quote callout ("Cargo.toml is the new langgraph.json", "An interrupt is a transaction abort with a receipt").
- **Index descriptions are specific and benefit-led**, not generic ("…the ten failure modes Rusty is built to kill").
- **Failure-modes-as-table** in `architecture` is exactly the right format choice — scannable, name-the-pain-first.
- **Honest v0.x tone** is preserved ("Directional, not scheduled", "Honest limitations" in studio).
- **Ordered-list super-step beats** and the "flow at a glance" time table in the quickstart show good list-vs-prose judgment.

## Findings

### 1. P0 — `server-quickstart` / "Step 2 — Define the graph": the code readers must write is never shown

**Location:** `src/content/learn/serverQuickstart.ts`, heading "Step 2 — Define the graph" (lines 64–83).

**Issue:** Step 2 is a 3-minute "write this code" step, but the article contains no `main.rs` code block — only prose and a bullet list describing the graph. A reader following the article literally cannot complete the quickstart without leaving for the source doc. This breaks the article's core promise ("Zero to a served graph in ten minutes").

**Fix:** Insert a `code` block immediately after the intro paragraph (line 68), before the bullet list. Use the full listing from `docs/server-quickstart.md` §2 verbatim (it is ~75 lines and every line is load-bearing — `StateSpec`, both `add_node` closures including the resume-value pattern, `compile()?`, `GraphRegistry`, `ServerConfig::new`, `serve`). Suggested block:

```ts
{
  type: "code",
  language: "rust",
  title: "src/main.rs — the whole server",
  code: `// paste the full main.rs from docs/server-quickstart.md §2 here, unchanged`
}
```

Then retitle the existing bullet list's role: it becomes the "what to notice in this code" annotation, which is already how it reads (it references `builder.compile()?`, `GraphRegistry`, `ServerConfig::new(...)` — identifiers that currently appear nowhere before they're discussed).

### 2. P1 — `server-quickstart` / Step 1: path-dependency layout assumption dropped (accuracy)

**Location:** `serverQuickstart.ts` Step 1 code blocks (lines 46–62).

**Issue:** The `Cargo.toml` uses `path = "../rusty-core"` / `path = "../rusty-server"`, but the article omits the source doc's caveat (server-quickstart.md line 11): commands assume the new project is a **sibling** of the `rusty-core` and `rusty-server` checkouts. A reader who clones `rusty` elsewhere hits an unexplainable `cargo` error on step 3.

**Fix:** Add a paragraph between the two code blocks:

> "The `path` deps assume you created `agent-server-demo` as a **sibling** of the `rusty-core` and `rusty-server` checkouts — adjust the `path =` values if your layout differs."

This also fixes the current back-to-back code blocks with no connective prose (see finding 10).

### 3. P1 — `server-quickstart` / "The flow at a glance" table promises steps the article doesn't cover

**Location:** `serverQuickstart.ts`, table at lines 21–43.

**Issue:** The overview table lists "4–6. Create thread → run to interrupt → resume over SSE" and "7–8. Time travel", but the article ends at Step 5; steps 6–8 live in the `human-in-the-loop` article. The table sets an expectation the page doesn't fulfill.

**Fix:** Replace the last two rows with continuation-aware copy:

```ts
["4–5. Create thread → run to the interrupt", "~2 min", "All via curl — steps 4–5 below"],
["6–8. Resume over SSE, history, fork & replay", "~4 min", "Continued in **Interrupts, resume, and time travel**"],
```

### 4. P1 — `human-in-the-loop`: `$TID` and `$RUN_ID` appear with no origin note

**Location:** `src/content/learn/humanInTheLoop.ts`, first curl block (line 59) and throughout.

**Issue:** The article's curl commands use `$TID` (and later `$RUN_ID`) that are only ever created in the quickstart article. A reader landing directly from the index hits undefined shell variables and is never told the two articles form a sequence (only the quickstart's closing paragraph points *forward*; nothing points *back*).

**Fix:** Add a callout immediately after the opening paragraphs (after line 27):

```ts
{
  type: "callout",
  variant: "note",
  title: "Continues from the quickstart",
  text: "`$TID` below is the thread created in **Zero to a served graph in ten minutes**, parked at its approval interrupt. Run that first — or substitute any thread id with a parked interrupt of your own.",
}
```

### 5. P1 — `roadmap-and-stability` / "Release timeline" table: version labels contradict the per-crate record (accuracy)

**Location:** `src/content/learn/roadmapAndStability.ts`, table at lines 22–56.

**Issue:** The "Release" column says "v0.3.0", "v0.4.0", "v0.5.0" — but `docs/roadmap.md` records R0.3 as `runtime 0.3.0 + server 0.2.0 + worker 0.1.0`, R0.4 as `runtime 0.4.0 + server 0.3.0 + otel 0.1.0`, and v0.5 as `server 0.4.0 + sdks 0.1.0`. There is no crate that was ever "v0.5.0", and flattening each cycle to one number undermines the article's own opening thesis ("There is no single Rusty version").

**Fix:** Change the column head from `"Release"` to `"Packages (versions)"` and replace the three flattened rows:

```ts
["rusty-agent-runtime 0.3.0 + rusty-server 0.2.0 + rusty-worker 0.1.0", "2026-08-05", "R0.3 — Interop", "MCP client, remote nodes + `rusty-worker`, server API completion, tracing"],
["rusty-agent-runtime 0.4.0 + rusty-server 0.3.0 + rusty-otel 0.1.0", "2026-08-05", "R0.4 — Time Travel", "WASM nodes, time travel (fork/replay), Postgres server store, `rusty-otel`, Rusty Studio, CORS"],
["rusty-server 0.4.0 + Python/TypeScript SDKs 0.1.0", "2026-08-05", "(pre-1.0 cycle)", "Both SDKs, multi-tenant auth, live-LLM validation"],
```

### 6. P1 — `architecture`: the super-step loop and "run, end to end" are flat prose where the source has diagrams

**Location:** `src/content/learn/architecture.ts`, "The super-step loop" (lines 88–119) and "The run, end to end" (lines 227–231).

**Issue:** The source doc carries mermaid sequence diagrams for the run flow, routing decision tree, and fork/replay; the site renders none of them. The six-beat ordered list is a decent fallback, but "The run, end to end" — the article's payoff section — is a single dense 70-word paragraph. The content model (`types.ts`) has no diagram/stepper block, so the most visual material in the source is the least visual on the site.

**Fix:** Two concrete options, cheapest first:
- **(a) No new block type:** replace the "run, end to end" paragraph with a 3-row table — head `["Phase", "What the executor does"]`, rows for *Restore-or-seed*, *Loop `execute_super_step`* (plan → parallel → barrier → merge → route → checkpoint), and *Terminate* (`Done(state)` / `Interrupted { value, state, checkpoint_id }` / `max_steps` error). Keeps the renderer untouched.
- **(b) Better, small component:** add a `steps` block type (`{ type: "steps"; items: string[] }`) rendered by `LearnArticle` as a horizontal flex strip of chips — `font-code text-xs`, numbered circles in `bg-primary/10 text-primary`, `→` separators in `text-muted-foreground/50`, wrapping on mobile. Use it for the six super-step beats. This is the one place a simple visual unambiguously beats prose, and it stays inside the existing warm, low-saturation token set.

Also consider converting the routing prose ("Routing — three kinds…") into a small decision table: head `["Post-barrier state", "Next active set"]` with rows for `Command::goto` present (overrides static edges), static edge (activate target), router returns `Node` / `Send` / `End` — mirroring the source's flowchart at architecture.md §4d.

### 7. P1 — `architecture`: failure-mode table references subsystems the article never introduces

**Location:** `architecture.ts`, "Named failure modes" table (lines 239–283).

**Issue:** Rows 8 and 9 describe WASM guests (`ResourceLimiter`, fuel metering) and MCP frame caps, but the article never mentions `WasmNode`, `RemoteNode`, the `ChatModel` trait, the prebuilt ReAct agent, or MCP anywhere — the source's §4g ("the model is one node, the loop is the graph") and §4h ("three ways code enters a graph") are skipped entirely. Readers hit "A guest WASM module loops forever" with zero context, and the row 3 mention of "LLM and tool errors" likewise assumes a layer never described.

**Fix:** Add one compact section before "Named failure modes" (after "The run, end to end"):

> **Heading (h2):** "One Node trait, three kinds of code"
> **Paragraph:** "Behind the single `Node` trait the engine runs three kinds of code without being able to tell them apart: native async closures; `RemoteNode`, which POSTs the invocation to a `rusty-worker` over a versioned wire protocol (HITL interrupts cross the wire); and `WasmNode` (feature `wasm`), which runs untrusted modules under fuel metering and a memory `ResourceLimiter`, with no WASI and no host functions."
> **Paragraph:** "The LLM is not special-cased either: the prebuilt ReAct agent is just a two-node cycle — `agent → tools → agent` — over a `messages` channel with the `AddMessages` reducer, so it inherits durability, interrupts, and time travel for free. MCP tool servers plug into the same `ToolRegistry` as native tools over stdio, with inbound frames capped at 16 MiB."

(All claims traceable to architecture.md §4g/§4h.) This makes every failure-table row self-explanatory and materially improves the "15 min" value of the article.

### 8. P2 — `architecture`: opening is a definition, not a hook

**Location:** `architecture.ts`, first paragraph (lines 10–13).

**Issue:** "Rusty is the durable agent runtime built in Rust — a full-Rust, LangGraph-style agentic platform" answers *what*, not *why should I care*. The other four articles all open on a tension or aphorism; the longest, most important article opens on taxonomy.

**Fix:** Replace the first paragraph with a pain-led opener (faithful — every pain named is a row in the article's own failure table):

> "Agent systems tend to fail the same few ways: parallel nodes silently clobber shared state, a crash mid-run loses everything, human approval means bespoke glue code, and a runaway loop burns tokens until someone kills the process. Rusty — the durable agent runtime built in Rust, a full-Rust LangGraph-style platform — is built around four primitives that each kill one of those failure classes. Its core mental model fits in one sentence:"

(The existing quote callout "An agent is a graph over shared state, executed in super-steps." then lands with more force.)

### 9. P2 — `architecture`: reducer table rows for `Append` / `DeepMerge` are tautological

**Location:** `architecture.ts`, reducer table (lines 30–41).

**Issue:** "Append — Multi-write reducer." and "DeepMerge — Multi-write reducer." say nothing beyond what the row above's error message already implies; the table wastes its most teachable cells. The `Overwrite` and `AddMessages` rows are good because they anchor to LangGraph equivalents.

**Fix:** Replace the two cells with semantics-differentiating copy (consistent with the source's single-write error text and LangGraph parity, which the doc asserts):

```ts
["`Append`", "Multi-write reducer — parallel writes in one super-step accumulate into the channel instead of conflicting."],
["`DeepMerge`", "Multi-write reducer — parallel partial updates merge into the channel's object instead of conflicting."],
```

### 10. P2 — `server-quickstart` / Step 1: two code blocks back-to-back with no prose between

**Location:** `serverQuickstart.ts`, lines 46–62.

**Issue:** `cargo new` and the `Cargo.toml` listing are adjacent code blocks; the second arrives unannounced. Minor, but it's the only place in the section where a code block is "dumped" rather than introduced.

**Fix:** The paragraph proposed in finding 2 ("Add the dependencies to `Cargo.toml`. The `path` deps assume…") inserted between the two blocks resolves both findings at once.

### 11. P2 — Reading-time estimates: plausible, except `architecture` is optimistic and `server-quickstart` conflates two clocks

**Location:** `readingTime` fields in `architecture.ts` ("15 min read") and `serverQuickstart.ts` ("10 min read").

**Issue:** `architecture` is ~30 blocks with 2 large tables, a glossary, and 5 code listings — a careful read is closer to 18–20 minutes; under-promising slightly erodes the site's otherwise precise tone. `server-quickstart`'s "10 min read" collides with its own "ten minutes, one process" *build-time* promise — the article (once finding 1 lands) is an ~8-minute read that takes ~10 minutes to *do*.

**Fix:** Set `architecture` to `"18 min read"`; set `serverQuickstart` to `"8 min read · 10 min hands-on"` (the index renders the string verbatim, so no renderer change needed).

### 12. P2 — No path from Learn to the playground; index cards could set format expectations

**Location:** `src/pages/learn/LearnIndex.tsx` (cards, lines 26–57) and article footers.

**Issue:** None of the five articles ever mentions `/playground`, so the site's most interactive surface is undiscoverable from its most engaged readers. Separately, the five cards mix formats (concept deep-dive, tutorial, reference) but the index gives no signal of which is which — only a number badge and reading time.

**Fix:** 
- Add a closing note callout to `human-in-the-loop` (the most hands-on article): `{ type: "callout", variant: "note", title: "Try it live", text: "The **playground** runs these same concepts — interrupts, resume, checkpoint history — in the browser. Open /playground when you're done reading." }` (adjust copy to whatever the playground actually demonstrates).
- In `LearnIndex`, add a small `font-code text-[11px] uppercase tracking-wider text-muted-foreground/70` kicker above each title, sourced from an optional `kicker?: string` on `Article` (additive to `types.ts`): `architecture` → "Concepts", `server-quickstart` → "Tutorial", `human-in-the-loop` → "Tutorial", `studio` → "Guide", `roadmap-and-stability` → "Reference". Cheap, and it makes the 01–05 ordering read as a deliberate curriculum.

---

**Summary for orchestrator:** Report complete above (12 findings: 1 P0, 6 P1, 5 P2). All claims were verified against the six source docs; proposed replacement copy is faithful to them. **Action needed from parent:** this subagent has no Write/Bash tool, so please save the report verbatim to `/Users/amjad.shaikh/claude-work/claude-white-papers/05 - RUST/aboutrusty-review/learn-content.md` (create the directory). No project files were modified.
