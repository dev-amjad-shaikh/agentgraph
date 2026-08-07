I have completed the full review, but this subagent context does not include any file-writing tool (no Write/Bash/Edit; system tools offer none either), so I could not create `aboutrusty-review/R2_LandingEngagement.md` myself. **Please write the report below verbatim to `/Users/amjad.shaikh/claude-work/claude-white-papers/05 - RUST/aboutrusty-review/R2_LandingEngagement.md`.** No project files were modified. All findings below are grounded in `src/pages/LandingPage.tsx`, all eight files in `src/sections/landing/`, and the README as factual source.

---

## Verdict

The landing page is unusually strong for a v0.x developer product: the copy is factual, the limitations section is a genuine trust asset, and the "what → why → how → proof → honesty → action" arc is intact. Its main weaknesses are density, not accuracy — the hero subline packs three propositions into one 45-word sentence, the comparison table is a 9×5 wall with no visual hierarchy, and the checkpointing primitive is re-explained three times in near-identical words. The biggest missed opportunity is engagement: the primary CTA promises a playground the page never shows, and the super-step loop — the product's core idea — is described only in prose.

## Strengths

- Hero headline ("The durable agent runtime, built in Rust.") passes the 5-second test — category + differentiator in eight words.
- Fact badges (v0.x, license, MSRV 1.86, tokio) pre-empt the exact questions a Rust dev asks first.
- Copy is sourced and honest throughout; the dated comparison footnote and the "not verified by us" convention are real credibility signals.
- `Limitations` is the best section on the page — "We would rather you know them now than discover them in production" converts honesty into voice.
- `FinalCta` gives two runnable paths (local script / docker) instead of a hollow "Get started" button.
- Section order is correct: no developer is asked to act before being given proof and caveats.

## Findings

**1. P1 — Hero subline is one overloaded sentence**
Location: `src/sections/landing/Hero.tsx`, the `<p>` under the `<h1>` (lines 46–52).
Issue: One 45-word sentence carries state model + execution model + checkpointing + three deployment modes. On a first scan, the eye slides off it; the strongest proof point ("versioned checkpoint at every step boundary") is buried mid-sentence.
Proposed fix: Split into a two-sentence subline with the durability payoff as its own sentence:

> Define an agent as a graph over schema-declared JSON state; the engine runs it in transactional super-steps and writes a versioned checkpoint at every step boundary. Runs survive crashes, pause for human approval, and replay from any point in history — embedded in your process, behind one static binary, or across remote and WASM nodes.

All clauses are sourced from README lines 9 and 17. Keep `max-w-2xl`; optionally drop to `sm:text-base` since the second sentence adds length.

**2. P1 — Primary CTA sells a playground the page never shows**
Location: `Hero.tsx` button "Try the Playground" (lines 55–60); also the only mention of the playground anywhere on the landing.
Issue: The top-of-funnel action asks for a click into an unknown surface. A developer doesn't know whether the playground is a REPL, a canned demo, or a marketing form, so the CTA's click-through depends entirely on curiosity.
Proposed fix: Add a one-line caption directly under the button row (same style as the install caveat note, `text-xs text-muted-foreground`), e.g. "In-browser: define a graph, watch super-steps and checkpoints stream live — no install." (adjust to whatever the playground actually does — this must be verified against the real /playground route before shipping). If feasible later, upgrade to a static screenshot or a small auto-playing trace of a run under the hero code block.

**3. P1 — Comparison section is a 9×5 wall with no takeaway**
Location: `src/sections/landing/Comparison.tsx` — `ROWS` (9 rows × 4 competitor columns, `min-w-[900px]`).
Issue: The table is factually excellent but visually flat; the Rusty column differs only by `text-foreground` vs `text-muted-foreground`. A scanner leaves with no remembered fact, and on mobile it becomes a horizontal scroll chore.
Proposed fix: (a) Add a stat-callout strip above the table — three items, using existing accent styling:

> **Checkpoint at every super-step boundary** · **Single static binary; library or server** · **MIT OR Apache-2.0**

Each as a small centered block (`font-display text-lg font-semibold` value, `text-xs uppercase tracking` label). (b) Give the Rusty header cell and column cells a subtle tint (`bg-accent/50`) so the eye anchors there. (c) Consider cutting the "Language" and "License" rows (low-information) to tighten to 7 rows. Keep the dated footnote — it's a differentiator.

**4. P1 — The checkpointing primitive is explained three times in near-identical words**
Location: `WhyRusty.tsx` card 1 ("Every super-step boundary is checkpointed — resume after a crash, suspend for human approval, fork and replay any historical step"), `HowItWorks.tsx` step 03, and `FeatureGrid.tsx` card 3 ("A versioned checkpoint at every super-step boundary…").
Issue: Repetition of the core idea is fine; repetition of the same sentence structure three times reads as padding and trains the reader to skim. Each section should add a *new* angle.
Proposed fix: Keep `WhyRusty` card 1 as-is (it's the thesis). Rewrite `FeatureGrid` card 3 to add the backend/ops angle not mentioned in WhyRusty:

> body: "Memory, JSON-file, or Postgres backends behind one Checkpointer trait — the core checkpoints only when you attach one, so dev stays in-memory and prod goes to Postgres with a feature flag."

And tighten `HowItWorks` step 03 to the mechanics ("one primitive behind resume, interrupts, and fork & replay" — already close; just delete "A versioned checkpoint is written at every step boundary" since WhyRusty said it):

> body: "One primitive behind resume after a crash, human-in-the-loop interrupts, and fork & replay time travel — written at every step boundary, never mid-node."

The "never mid-node" phrase also seeds the Limitations idempotency contract.

**5. P2 — FeatureGrid cards 2 and 3 overlap; titles lead with mechanism, not benefit**
Location: `src/sections/landing/FeatureGrid.tsx`, cards "Super-step executor" and "Checkpoints".
Issue: Both bodies open with Pregel/BSP super-step language ("Pregel/BSP execution — plan → parallel → barrier…" / "A versioned checkpoint at every super-step boundary"), so adjacent cards blur. "State channels & 4 reducers" also front-loads a count ("4") that means nothing until you read the body.
Proposed fix: Differentiate by audience concern. Card 2 keeps the mechanism (it's the "how"), card 3 becomes the ops story per finding 4. Retitle card 1 to "Versioned state channels" and move the reducer list fully into the body (it already is). Optionally reorder so execution-flow cards (channels → executor → checkpoints → interrupts → time travel) precede topology cards (RemoteNode, WasmNode, MCP) — currently correct, keep it.

**6. P2 — HowItWorks describes the super-step loop in prose but never draws it**
Location: `src/sections/landing/HowItWorks.tsx`, step 02 body and section layout.
Issue: "plan → parallel over an immutable snapshot → barrier → merge via reducers → route" is the product's central mental model, and it exists only as an inline arrow string inside a paragraph. This is the natural place for the one visual on the page.
Proposed fix: Add a simple horizontal cycle diagram above the three steps (between `SectionHeading` and the grid): five `font-code` chips (`plan → parallel → barrier → merge → route`) with a curved "repeat" arrow back to `plan`, built with flexbox + lucide `ArrowRight`/`Repeat` icons — no animation library needed; a CSS-only pulse on the active chip would suffice. Reuse the exact labels from the step 02 body so diagram and prose reinforce each other.

**7. P2 — Only the embedded usage path is shown as code; server and SDK paths are prose-only**
Location: `HowItWorks.tsx` (single Rust `CodeBlock`) and `ComponentsTable.tsx` (SDKs as a table row).
Issue: The page claims "the same compiled graph runs embedded, behind the HTTP/SSE server, or across remote and WASM nodes" but only ever shows the embedded path. Developers who'd adopt via the server + Python/TS SDK never see their own workflow.
Proposed fix: Convert the HowItWorks code block into a three-tab component (tabs: "Embedded (Rust)" / "Server (HTTP/SSE)" / "Python SDK"). Tab 1 = existing snippet. Tab 2 = a curl or the `./scripts/dev.sh` line from README "Try it in one command". Tab 3 = a minimal `rusty_client` call (README confirms zero-dependency `rusty_client` exists; pull the exact API from `sdks/python/` before writing it — do not invent method names). A lightweight local tab state (`useState`) inside HowItWorks is enough; no new dependency.

**8. P2 — Two section headings are flat or generic**
Location: `HowItWorks.tsx` title "One execution model, end to end." and `Comparison.tsx` title "How Rusty compares."
Issue: "One execution model, end to end" is generic SaaS cadence — it could headline any orchestration product. "How Rusty compares." is a label, not a claim; the eyebrow "Honest comparison" already says it.
Proposed fix: HowItWorks → "Plan, parallel, barrier, merge, route." (the loop itself as the headline — memorable and unique to this product; pairs with the diagram in finding 6). Comparison → "Rusty vs. LangGraph, stated plainly." (names the competitor — developers search this comparison — and "stated plainly" echoes the Limitations voice "Production readiness, stated plainly"). Keep eyebrows unchanged.

**9. P2 — ComponentsTable interrupts the conviction arc**
Location: `src/pages/LandingPage.tsx` — `ComponentsTable` sits between `FeatureGrid` and `Comparison`.
Issue: The page builds emotional/technical conviction (why → how → features), then pauses for a 6-row reference table, then asks the reader to re-engage for the comparison. Reference material belongs after the persuasive peak.
Proposed fix: Move `<ComponentsTable />` to after `<Comparison />` and before `<Limitations />` — the flow becomes features → external proof (comparison) → packaging facts (components) → honesty → CTA. No content changes needed; one-line reorder in `LandingPage.tsx`.

**10. P2 — Hero surfaces no live project-health signal**
Location: `Hero.tsx` `FACT_BADGES`.
Issue: All four badges are static facts. The README leads with a CI badge, and for a v0.x project "CI passing" is a stronger trust signal than "tokio" (which the headline already implies by saying "built in Rust" and the WhyRusty title repeats).
Proposed fix: Replace the `"tokio"` badge with a CI badge — either a shields.io `<img>` (matches README line 5) or, to keep the current Badge styling, a secondary badge reading "CI passing" with a `CheckCircle2` icon, linking to the GitHub Actions workflow. Keep the other three.

**11. P2 — Final CTA hierarchy is inconsistent with the hero's**
Location: `FinalCta.tsx` buttons — primary "Learn the architecture", outline "View on GitHub".
Issue: The hero's primary action is "Try the Playground" (doing), but the bottom of the funnel — the moment of maximum conviction, right after Limitations — demotes action and promotes more reading. A developer who just read the limitations and stayed is ready to clone, not to read docs.
Proposed fix: Make "View on GitHub" the primary button (it's the highest-intent developer action and matches the install/clone code blocks directly above it) and demote "Learn the architecture" to `variant="outline"`. Optionally add a third `variant="ghost"` link back to `/playground` for consistency with the hero.

**Ambiguity for the parent:** findings 2 and 7 require facts I could not verify from the files in scope — what the `/playground` route actually renders, and the exact `rusty_client` Python API. The proposed copy for those two is marked as conditional; verify against `src/pages/` playground code and `sdks/python/` before implementing. All other replacement copy is sourced directly from README lines 9, 17–20, 26, 40, 84–89, and 95–107.
