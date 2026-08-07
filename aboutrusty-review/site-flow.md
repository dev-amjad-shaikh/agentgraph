I completed the full read-through (App.tsx, SiteLayout.tsx, all landing sections, LearnIndex, LearnArticle, PlaygroundPage + TransportControls, learn content model). 

**Important limitation:** this subagent runtime has no file-write tool (no Bash/Write/Edit), so I could not create `/aboutrusty-review/site-flow.md` myself. The complete report is below — please save it verbatim to `/Users/amjad.shaikh/claude-work/claude-white-papers/05 - RUST/aboutrusty-review/site-flow.md`.

---

# R3_SiteFlow — UX Flow & Information Architecture Review

## Verdict
The skeleton of the journey is right — Hero → Playground CTA → Learn → GitHub all exist, nav is minimal, and the Learn article template (breadcrumb, prev/next) is solid. But the journey silently dead-ends at three points: landing sections never link *into* the Learn articles they summarize, Learn articles structurally cannot contain links (and never mention the Playground), and the Playground's exit path is a plain-text caption instead of a link. Add a missing catch-all 404 route and fix the mobile stacking of the resume panel, and the loop closes.

## Strengths
- Hero CTA hierarchy is correct: primary "Try the Playground", secondary "Learn the architecture" (`src/sections/landing/Hero.tsx:54-64`).
- Learn index is a well-ordered, numbered 01–05 path with reading times (`src/pages/learn/LearnIndex.tsx`).
- LearnArticle has a real breadcrumb (`Learn › {title}`) and styled prev/next cards (`src/pages/learn/LearnArticle.tsx:194-274`).
- Unknown `/learn/:slug` is handled gracefully with a "Back to Learn" link (`LearnArticle.tsx:171-187`).
- Comparison table already does mobile scrolling right (`overflow-x-auto` + `min-w-[900px]`, `src/sections/landing/Comparison.tsx:100-101`).
- Playground transport has a built-in idle hint ("super-step 0 — press Run or Step") and keyboard shortcuts (`TransportControls.tsx:158`).
- GitHub is reachable from nav icon, FinalCta button, and footer.

## Findings

**1. P0 — Learn articles are a structural dead end: no link support, zero Playground references.**
Location: `src/pages/learn/LearnArticle.tsx` (`renderInline`, lines 19-41) and `src/content/learn/types.ts:4`.
Issue: The content model only supports `**bold**` and `` `code` `` inline — there is no link syntax, so no article can link to the Playground, to sibling articles, or to GitHub. Grep confirms the word "playground" appears in zero learn articles. A reader finishing "The anatomy of a run" has only prev/next — the single best conversion moment (reader → hands-on user) is wasted.
Proposed fix: (a) Add link support to `renderInline`: extend the token regex to `(\*\*[^*]+\*\*|\`[^\`]+\`|\[[^\]]+\]\([^)]+\))` and render `[text](/path)` as `<Link className="text-primary underline-offset-4 hover:underline">` (internal) or `<a target="_blank" rel="noreferrer">` for `https?://`. (b) Add a persistent end-of-article CTA in `LearnArticle.tsx`, between the content blocks and the prev/next nav (~line 230): a bordered panel — `rounded-xl border border-primary/25 bg-accent/30 p-6` — with copy "Want to feel this instead of reading it?" + `<Button asChild><Link to="/playground">Try it in the Playground</Link></Button>`. Optionally make it contextual per slug via a `playgroundCta?: string` field on `Article`.

**2. P0 — Landing sections never cross-link into the Learn articles they summarize.**
Location: `src/sections/landing/Limitations.tsx`, `FeatureGrid.tsx`, `ComponentsTable.tsx`, `HowItWorks.tsx`.
Issue: The Limitations panel discusses "open R1.0 items" but doesn't link to `/learn/roadmap-and-stability`, which covers exactly that. Feature cards (super-steps, checkpoints, interrupts) don't link to `/learn/architecture` or `/learn/human-in-the-loop`. The components table mentions Rusty Studio without linking to `/learn/studio`. A visitor who wants depth must manually re-find the topic via the nav — most won't.
Proposed fix: add one inline link per section, using `text-sm font-medium text-primary underline-offset-4 hover:underline` with an `ArrowRight size={14}` icon:
- `Limitations.tsx` after the "Deliberately rejected" paragraph (line ~72): "The full stability contract and R1.0 roadmap → `/learn/roadmap-and-stability`".
- `FeatureGrid.tsx` under the grid (after line 85): "The execution model behind all eight — The anatomy of a run → `/learn/architecture`".
- `ComponentsTable.tsx` under the table (after line 90): "Studio in depth → `/learn/studio` · Serve your first graph → `/learn/server-quickstart`".
- `HowItWorks.tsx` under the code-caption (line ~85): link "OpenAiCompatibleClient" explanation onward to `/learn/server-quickstart`.

**3. P1 — No catch-all route: any unmatched URL renders a blank page.**
Location: `src/App.tsx:10-17`.
Issue: `/learn/nonexistent` is handled, but `/anything-else`, `/playgound`, `/learn/architecture/` (trailing-slash variants depending on server config), etc. render only header + footer with an empty `<main>` — worse than a 404, it looks like a broken load.
Proposed fix: add `<Route path="*" element={<NotFound />} />` inside the `SiteLayout` route. Create `src/pages/NotFound.tsx`: centered `font-display` "Page not found", one muted sentence, and two buttons — `<Button asChild><Link to="/">Back to Overview</Link></Button>` and outline `<Link to="/learn">Browse Learn</Link>`. Reuse the existing "Article not found" layout in `LearnArticle.tsx:172-186` as the template.

**4. P1 — Playground exit path is plain text, not a link.**
Location: `src/pages/playground/PlaygroundPage.tsx:456-459`.
Issue: The closing caption — "Simulation, not a live server — to run the real engine: cargo run --example server_demo, then open studio/index.html" — is the natural handoff to the docs, but it's unlinked dead text. This is the end of the land → try → learn loop, and it stops.
Proposed fix: convert to a small footer CTA row: keep the caption, then add two inline links below it — "Run the real server → `/learn/server-quickstart`" and "Open Rusty Studio → `/learn/studio`" — styled as `text-xs font-medium text-primary underline-offset-4 hover:underline` with `ArrowRight size={12}`.

**5. P1 — Mobile (375px): the resume panel — the one required action — is buried below the entire left rail.**
Location: `src/pages/playground/PlaygroundPage.tsx:331-453` (grid order) and the `ResumePanel` mount at lines 376-385.
Issue: On `< lg` the grid stacks left rail (Transport → Graph → Branches) then right rail (ResumePanel → Tabs). When a run interrupts on a phone, the user must scroll past the graph and branches to reach the approval panel that unblocks everything — the interrupt state looks frozen.
Proposed fix: lift the interrupt panel out of the right rail and render it full-width *above* the debugger grid when active: move the `status === "interrupted"` block to just before `<div className="mt-8 grid gap-6 lg:grid-cols-12">`, wrapped in `<div className="mt-6">`. On desktop it reads equally well as an attention banner; on mobile it's the first thing under the scenario picker.

**6. P1 — Playground first 10 seconds: informative, but no guided first action.**
Location: `src/pages/playground/PlaygroundPage.tsx:278-294` (intro) and `TransportControls.tsx`.
Issue: The intro paragraph is accurate but five lines of dense prose; the only "what do I do now" cue is a `text-[11px]` hint inside the Transport card. Nothing tells a newcomer the intended first loop: run → get interrupted → resume → fork.
Proposed fix: add a compact numbered hint strip between the scenario picker and the debugger grid (`PlaygroundPage.tsx` ~line 329): `flex flex-wrap items-center gap-x-4 gap-y-1.5 font-code text-[11px] text-muted-foreground` with items "① Pick a scenario — ② Press Run (R) — ③ Approve the interrupt — ④ Open Checkpoints and Fork the timeline". Optionally highlight the Run button until the first run: pass `hasRun` into `TransportControls` and add `!hasRun && canRun ? "animate-pulse ring-2 ring-primary/40" : ""` to the Run button's className.

**7. P2 — FinalCta's primary button duplicates the Hero's secondary instead of advancing the journey.**
Location: `src/sections/landing/FinalCta.tsx:29-46`.
Issue: After two setup code blocks ("run your first graph in ten minutes"), the primary button is "Learn the architecture" → `/learn` — the same destination the Hero already offered as secondary. The natural terminal actions here are GitHub (they just read `git clone …`) or the Playground; the landing page never re-offers the Playground after the Hero.
Proposed fix: make the primary button `<a href="https://github.com/dev-amjad-shaikh/rusty">` "View on GitHub" (with the `Github` icon), demote "Learn the architecture" to `variant="outline"`, and add a third `variant="ghost"` link "or try the Playground first →" → `/playground`.

**8. P2 — Learn index has no onward affordance at the bottom and no "start here" cue.**
Location: `src/pages/learn/LearnIndex.tsx` (after line 58).
Issue: The numbered list implies a reading order but never says so, and the page ends at article 05 with no pointer to the Playground — the other half of the learn↔try loop.
Proposed fix: append after the list: `<p className="mt-10 text-center text-sm text-muted-foreground">Read in order, or skip ahead — then <Link to="/playground" className="font-medium text-primary underline-offset-4 hover:underline">feel the super-step loop in the Playground</Link>.</p>`.

**9. P2 — Components table and Learn article tables can clip on narrow screens; inconsistent with the Comparison table.**
Location: `src/sections/landing/ComponentsTable.tsx:65` and `src/pages/learn/LearnArticle.tsx:93`.
Issue: Both wrap tables in `overflow-hidden rounded-xl border` with no `overflow-x-auto` and no `min-w` — cells use default/`whitespace-normal` so they mostly wrap, but the fixed `w-40`/`w-64` header columns at 343px content width produce a cramped ~100px description column, and any long unbreakable `font-code` path silently clips.
Proposed fix: mirror the Comparison pattern: change the wrapper to `overflow-x-auto rounded-xl border bg-card shadow-sm` and give the inner `<Table>` `className="min-w-[560px]"` (ComponentsTable) / `min-w-[480px]` (LearnArticle table case). Horizontal scroll on a code-heavy table beats clipped text.

**10. P2 — Breadcrumb lacks position context; header nav is tight (but OK) at 375px.**
Location: `src/pages/learn/LearnArticle.tsx:194-206`; `src/components/layout/SiteLayout.tsx:29-55`.
Issue: (a) The breadcrumb shows `Learn › {title}` but not where the article sits in the five-part series. (b) At 375px the header fits (logo + 3 items + GitHub icon ≈ 360px), but with only ~8px of slack — any future nav item breaks it; the GitHub icon has an `aria-label` but no visible affordance marking it external.
Proposed fix: (a) In `LearnArticle`, derive the index (`articles.findIndex(a => a.slug === article.slug)`) and append to the breadcrumb: `<span className="ml-auto font-code text-xs text-muted-foreground/70">{i + 1} / {articles.length}</span>`. (b) In `SiteLayout`, reduce nav item padding on small screens (`px-2 sm:px-3`) and shrink the GitHub link to `p-1.5 sm:p-2`; this buys ~24px of headroom without a hamburger menu.

**11. P2 — Footer is decorative where it could be navigational.**
Location: `src/components/layout/SiteLayout.tsx:63-119`.
Issue: The "Site" column just repeats the header nav; the "Project" column has GitHub but no Issues link; the five Learn articles — the site's deepest content — are unreachable from the footer.
Proposed fix: replace the "Site" column's contents with the article list ("Docs": links to all five `/learn/{slug}` titles, `text-sm text-muted-foreground hover:text-foreground`), keep Overview/Learn/Playground as a slim third row or merge into "Project", and add `<a href="https://github.com/dev-amjad-shaikh/rusty/issues">Report an issue</a>` under the GitHub link.

**12. P2 — Dead Vite starter files ship in the bundle tree.**
Location: `src/pages/Home.tsx` (entire file) and `src/App.css` (imported only by `Home.tsx`).
Issue: `Home.tsx` is the unmodified "Vite + React" counter scaffold, routed nowhere. Harmless at runtime but confusing to contributors and a stale-content hazard.
Proposed fix: delete `src/pages/Home.tsx` and `src/App.css`; verify no other import of `App.css` remains (`Grep "App.css"` should return zero hits after deletion).

---

**Summary for parent:** Review complete across routing, layout, all 8 landing sections, learn index/article renderer + content model, and playground page/transport. Report delivered above in full (12 findings: 2 P0, 4 P1, 6 P2). **Action needed from parent:** save the Markdown above to `/Users/amjad.shaikh/claude-work/claude-white-papers/05 - RUST/aboutrusty-review/site-flow.md` (create the directory first) — this subagent had no file-write capability. The two P0s (Learn articles can't link anywhere; landing sections never link into Learn) are the highest-leverage fixes and are cheap to implement.
