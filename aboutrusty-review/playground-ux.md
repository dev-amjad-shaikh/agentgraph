I have read all nine playground files in full. One important limitation to flag: this subagent environment gives me read-only file tools plus Blueprint tools — there is no `Write`/`Edit`/`Bash` tool available to me, so **I cannot physically create `/aboutrusty-review/playground-ux.md`**. The complete report follows verbatim so the orchestrator can save it to that path unchanged.

---

```markdown
# R5_PlaygroundUX — Interactive UX Review: /playground

## Verdict
The playground is pedagogically strong — the phase-by-phase captions, the motivating amber ResumePanel, and the scripted fork divergence show real teaching craft. Its two biggest gaps are the cold start (nothing moves, nothing prompts the first click, and the seeded user prompt is hidden behind an empty state) and status-blind chrome (captions and disabled buttons that don't explain themselves once a run finishes or interrupts). Fork/replay is implemented brilliantly in the engine but its payoff — visible divergence — is nearly impossible to compare in the UI.

## Strengths
- TransportControls' per-phase caption line ("all-or-nothing: writes become visible only here") glosses barrier/merge/route inline — best learnability feature on the page.
- ResumePanel is genuinely motivating: interrupt payload, the idempotent-node Rust snippet, reviewer input, and explicit Approve/Reject buttons.
- CheckpointTimeline's "Fork first, replay on the fork" callout teaches the non-obvious semantics at the exact moment of intent.
- Empty states exist everywhere (StateInspector, EventLog, CheckpointTimeline) with actionable copy.
- Keyboard hints (Kbd R / Kbd S) printed directly on the buttons — discoverable, not buried in a help modal.
- Engine design: the branch persona deliberately re-plans one-tool-per-step plus a verify call, so forks *do* diverge — the raw material for a great demo is there.
- Responsive touches already present: timeline meta hidden below `sm:`, flex-wrap on phase chips and scenario badges.

## Findings

1. **P0 — No obvious first action; the page is static on load.**
   Location: `PlaygroundPage.tsx` header + `TransportControls.tsx` Run button.
   Issue: In the first 10 seconds nothing moves and nothing points at the Run button. The header paragraph is dense prose; the two empty states that say "press Run or Step" live below the fold in the default State tab. A visitor who doesn't read carefully may bounce without ever seeing a super-step.
   Proposed fix: Don't auto-run (it would rob the "press it yourself" moment), but (a) add a `animate-pulse` / `ring-2 ring-primary/50` treatment to the Run button until `hasRun` becomes true (clear it in `handleRun`/`handleStep`), and (b) add a compact step strip under the header: `1 Run the graph → 2 Watch the barrier merge state → 3 Fork a checkpoint and replay` as three `font-code text-xs` chips in `PlaygroundPage.tsx` after the intro `<p>`.

2. **P0 — The seeded user prompt is invisible before the first run.**
   Location: `PlaygroundPage.tsx` (`hasRun` gate) + `StateInspector.tsx` empty state.
   Issue: `createThread` seeds the ReAct scenario with a scripted user message ("What time is it right now in UTC? Then multiply 128 by 46…"), but `hasRun = frames.length > 0 || checkpoints.length > 0` is false at load, so the inspector shows "No state yet" and the visitor never sees *what* the agent will do — the single best motivator to press Run.
   Proposed fix: In `StateInspector`, drop the `hasRun` requirement when `liveState` has content: render the channels with a `<Badge variant="outline">input seed</Badge>` and caption "seeded input — press Run to watch reducers grow it". Alternatively surface `def.userPrompt` as a chat-style quote card directly above the Transport in `PlaygroundPage.tsx`.

3. **P1 — Status caption lies when the run is done or interrupted.**
   Location: `TransportControls.tsx`, the `phase ? … : \`super-step ${step} — press Run or Step\`` line.
   Issue: After completion or interrupt, `phase` is null and the caption still reads "super-step N — press Run or Step" while both buttons are disabled — contradictory guidance at exactly the moment the user wonders "what now?"
   Proposed fix: Make the idle caption status-aware: `status === "done"` → "run complete — Reset to replay, or open the Checkpoints tab and fork a boundary"; `status === "interrupted"` → "parked at an interrupt — answer in the amber panel below"; `status === "paused"` → "paused — Run continues, Step advances one super-step".

4. **P1 — Disabled Run/Step buttons give no reason.**
   Location: `TransportControls.tsx` Run/Step `<Button disabled={!canRun}>` / `disabled={!canStep}`.
   Issue: When interrupted or done, the primary action is silently dead; on touch devices there is no hover tooltip at all.
   Proposed fix: Add a `title` attribute ("Interrupted — resume from the panel below" / "Run finished — Reset to run again") and, better, a one-line inline hint under the button row (`text-[11px] text-muted-foreground font-code`) rendered whenever `!canRun`, since the disabled state needs no hover to be explained.

5. **P1 — Event log autoscroll is rude and jittery at 2×.**
   Location: `EventLog.tsx` `useEffect` → `scrollIntoView({ behavior: "smooth" })` on every `frames.length` change.
   Issue: The log yanks to the bottom on every frame even if the user scrolled up to read an earlier `updates` frame, and at 2× speed a smooth scroll fires roughly every 150–300 ms, producing continuous motion sickness-grade jitter.
   Proposed fix: Track a `pinnedRef` — on the container's `onScroll`, set `pinned = scrollTop + clientHeight >= scrollHeight - 24`; only autoscroll when pinned, use `behavior: "auto"` while `speed === 2` (pass speed as a prop or use a threshold on frame rate), and render a small floating `↓ latest` button (`absolute bottom-3 right-3`, `bg-secondary font-code text-[10px]`) when unpinned that re-pins on click.

6. **P1 — Fork divergence payoff is invisible; main vs fork can't be compared.**
   Location: `BranchesPanel.tsx` + `CheckpointTimeline.tsx` `onFork` flow.
   Issue: The engine guarantees a divergent replay (branch persona: one tool per super-step + an extra verify call, more checkpoints), but nothing tells the user *that this will happen*, and after running both timelines the only comparison aid is the tiny "N cp · status" text — you must click back and forth and memorize.
   Proposed fix: (a) When a fork is created, show a one-line hint banner above the Graph card: "Forked at step {n} — press Run: this replay re-plans from the checkpoint and diverges from main." (b) In `BranchesPanel`, append a delta to each fork row: `+2 cp vs main` computed as `t.checkpoints.length - main.checkpoints.length`. (c) Stretch goal: a `Compare` toggle that renders the two checkpoint timelines side-by-side (`grid sm:grid-cols-2`) inside the Checkpoints tab, matching rows by `step` and highlighting rows that differ with `border-primary/40`.

7. **P1 — Jargon gaps: Pregel/BSP, Overwrite, SSE/Last-Event-ID, "persona" behavior.**
   Location: `PlaygroundPage.tsx` header; scenario picker cards; Event-stream `CardDescription`; engine's branch persona (unexplained in UI).
   Issue: "barrier" and "reducers" are glossed well in the Transport/State cards, but the header's "Pregel/BSP super-step loop", the scenario badges' `Overwrite` reducer (only `AddMessages` gets an inline gloss, and only in StateInspector), the Event card's "SSE … Last-Event-ID dedupes", and — most confusing — the fork replay behaving *differently* with no stated reason all assume architecture-doc knowledge.
   Proposed fix: (a) Header: append a parenthetical — "Pregel/BSP (bulk-synchronous: nodes run in parallel rounds separated by a barrier)". (b) Scenario card badges: add `title="Overwrite reducer: last write wins"` / `title="AddMessages reducer: upsert messages by id"`. (c) Event card description: "SSE (Server-Sent Events — the real engine's HTTP stream); Last-Event-ID lets a reconnected client skip frames it already saw." (d) For the persona: the fork hint banner from Finding 6 covers it — explicitly say "the re-plan from checkpoint state is free to choose a different path."

8. **P2 — Keyboard shortcut R silently no-ops when interrupted/done.**
   Location: `PlaygroundPage.tsx` `onKey` handler → `handlersRef.current.runToggle()`.
   Issue: Pressing R while `status === "interrupted"` calls `handleRun`, which early-returns with zero feedback — feels like a broken page for keyboard-first users (the exact audience).
   Proposed fix: In the key handler, when the action is a no-op due to status, flash the status Badge (add a `key`-bump or transient `ring-2 ring-amber-600/60` class for ~400 ms) or set a transient caption line ("interrupted — resume below") in the Transport.

9. **P2 — Scenario switch mid-run wipes the run without warning.**
   Location: `PlaygroundPage.tsx` `handleScenario` → `resetAll`.
   Issue: Clicking the other scenario card while running (or after a careful interrupt setup) instantly destroys all threads, forks, and checkpoints — irreversible and surprising.
   Proposed fix: Disable scenario cards while `busy` (`disabled={busy}` + `opacity-60 cursor-not-allowed`), and when `!busy && threads.length > 1` (forks exist) or a run is in progress, gate the switch behind a lightweight confirm (`window.confirm("Switch scenario? This discards the current run and its forks.")` is acceptable for v0.x; a shadcn AlertDialog is nicer).

10. **P2 — EventLog React keys collide across run attempts.**
    Location: `EventLog.tsx` `key={\`${threadId}-${f.seq}\`}`.
    Issue: `beginRun` resets `seq` to 1 on every attempt, so attempt 2's metadata frame reuses keys (`main-1`, `main-2`, …) from attempt 1 — duplicate React keys in the same list, risking stale/mis-reused DOM rows in the terminal view and console warnings.
    Proposed fix: Key on the frame id plus a monotonic tiebreaker, e.g. `key={\`${threadId}-${f.frameId}-${f.seq}\`}` (frameId embeds checkpoint+step so collisions vanish), or carry a never-reset `frameSeq` counter on the thread.

11. **P2 — `values` frames dump full state as one long wrapped line.**
    Location: `EventLog.tsx` `JSON.stringify(f.data)` with `break-all`.
    Issue: Each `values` frame repeats the entire channel state (all messages) on a single wrapped line; after a few super-steps the log is mostly duplicated noise and the interesting `updates` frames scroll away — the density problem of the page.
    Proposed fix: Render `data:` for `values` frames truncated to ~140 chars with a `+ expand` toggle (`font-code text-[10px] text-white/40 hover:text-white/70`), or pretty-print only `updates`/`end` payloads and collapse `values` to `∑ full state @ step N (click to expand)`.

12. **P2 — Mobile: the transport scrolls away while tab content is long.**
    Location: `PlaygroundPage.tsx` two-rail grid collapse (`lg:grid-cols-12`) + `EventLog` `h-80` / `StateInspector` `max-h-64`.
    Issue: On phones the order (transport → graph → timelines → tabs) is sensible, but once a run produces a long event log or state, the Transport and phase strip are a full scroll away — you can't watch phases *and* the log, which is the core demo. Two-rail comparison (Finding 6) is also impossible on mobile.
    Proposed fix: Make the phase chip strip (not the whole Transport card) `sticky top-14 z-10 bg-background/95 backdrop-blur` on viewports below `lg` while `busy`, so the six-beat loop stays visible during scroll; keep the full compare view (Finding 6c) desktop-only with the per-row `+N cp vs main` delta as the mobile fallback.
```

---

**Summary for the orchestrator:**

- **Reviewed:** all 9 files under `src/pages/playground/` (page, engine, and 7 components).
- **Could not write the report file:** this subagent has no `Write`/`Edit`/`Bash` tool (only Read/Grep/Glob/search + Blueprint tools), and an attempted system-tool fallback timed out. **Please save the Markdown block above verbatim to `/Users/amjad.shaikh/claude-work/claude-white-papers/05 - RUST/aboutrusty-review/playground-ux.md`.**
- **Headline findings:** (P0) no obvious first action / hidden seeded prompt on cold start; (P1) status-blind transport caption and unexplained disabled buttons, rude autoscroll in EventLog, invisible fork divergence with no main-vs-fork comparison, jargon gaps (Pregel/BSP, Overwrite, SSE); (P2) silent keyboard no-ops, unguarded scenario-switch reset, React key collisions across run attempts, dense `values` frame dumps, mobile sticky-phase-strip fix.
