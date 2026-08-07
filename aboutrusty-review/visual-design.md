No file-write tool (no Bash/Write/Edit) is available in my subagent runtime — only read/search tools. I completed the full read-only review and the report below is ready to save verbatim. **Parent agent: please write this content to `/Users/amjad.shaikh/claude-work/claude-white-papers/05 - RUST/aboutrusty-review/visual-design.md`** (directory does not exist yet).

---

# Visual Design Review — aboutrusty.com (R1_VisualDesign)

## Verdict

The site has a genuinely coherent warm-rust identity: a single rust primary, disciplined banded sections, consistent serif headlines via `.font-display`, and a well-integrated dark-charcoal code block that reads as intentional rather than pasted-in. The main weaknesses are (1) palette drift in the playground where hard-coded emerald/amber/zinc Tailwind colors bypass the token system, (2) a hero that is competently assembled but structurally "default shadcn template," and (3) a handful of low-contrast muted-text usages that will fail WCAG AA at small sizes. None of this is broken; all of it is one focused polish pass away.

## Strengths

- **Disciplined token palette in `src/index.css`** — one rust hue family (primary 16° / accent 28° / secondary 30°), no blue-purple anywhere, warm cream background; exactly the brief.
- **Consistent section rhythm on landing** — every section uses `py-20 sm:py-28` with `mt-12` after `SectionHeading`, and banded `bg-secondary/40 border-y` sections alternate cleanly with plain ones (HowItWorks / ComponentsTable / Limitations).
- **`.font-display` used reliably for h1/h2 and most card titles** — serif editorial voice holds across landing, learn index, articles, and playground header.
- **Repeated icon-chip motif** (`h-9 w-9 rounded-lg bg-accent text-accent-foreground`) in WhyRusty / FeatureGrid / Limitations gives a recognizable brand gesture.
- **CodeBlock chrome is coherent with the theme** — warm charcoal (`--code-bg: 22 16% 11%`), `rounded-xl` matching Card radius, mono title bar; the same chrome is mirrored in EventLog.
- **Radius scale is coherent** — `rounded-xl` (cards, code), `rounded-lg` (chips, small panels), `rounded-md` (inputs, kbd).

## Findings

**1. P0 — Playground status colors bypass the token system and drift the palette**
Location: `src/pages/playground/TransportControls.tsx` (`STATUS_STYLES`, lines 29–35), `src/pages/playground/ResumePanel.tsx` (line 36, `border-amber-600/40 bg-amber-50/60`, plus `text-amber-700` throughout), `src/pages/playground/EventLog.tsx` (`FRAME_COLORS`, lines 10–15), `src/pages/playground/CheckpointTimeline.tsx` (line 66).
Issue: `emerald-700/800`, `amber-600/700/900`, and `zinc-400` are hard-coded raw Tailwind palette colors. Emerald is a cool green hue outside the warm family — it's the one place the site visibly "leaves" its palette — and none of these adapt to the `.dark` token set that `index.css` already defines (amber-50 panels will look wrong if dark mode is ever enabled).
Fix: add semantic tokens in `src/index.css` under `:root` and `.dark`, e.g. `--success: 152 30% 32%` / `--success-foreground` and `--warning: 32 80% 40%` / `--warning-foreground`, extend them in `tailwind.config.js` (`colors.success`, `colors.warning`), then replace `bg-emerald-700/10 text-emerald-800 border-emerald-700/30` → `bg-success/10 text-success border-success/30`, and all `amber-*` playground classes → `warning` equivalents. For `EventLog`'s `FRAME_COLORS`, shift to warm-adjacent hues on charcoal: `metadata: "text-white/40"`, `updates: "text-amber-300"`, `values: "text-orange-300"`, `end: "text-primary"` (or a light rust like `#e8845c`), dropping `emerald-300` and `zinc-400`.

**2. P1 — Hero reads as "default shadcn landing" rather than designed**
Location: `src/sections/landing/Hero.tsx` (lines 22–80).
Issue: Centered badge-row → headline → paragraph → two buttons → centered code block is the stock SaaS-hero skeleton; the only bespoke element is the subtle radial glow. For an "editorial serif" brand the headline gets no editorial treatment, and the four mono badges above it add noise before the value prop.
Fix: (a) Give the headline an accent — wrap "built in Rust." in `<span className="italic text-primary">built in Rust.</span>` so the serif italic carries the brand. (b) Replace the bare badge row with a ruled eyebrow: `<div className="flex items-center gap-3"><span className="h-px w-8 bg-primary/40"/><span className="font-code text-xs uppercase tracking-[0.2em] text-primary">v0.x · MIT OR Apache-2.0 · MSRV 1.86</span><span className="h-px w-8 bg-primary/40"/></div>` (keeps the facts, drops four pill shapes). (c) On `lg`, consider splitting hero into a 2-col grid (headline/CTAs left, Cargo.toml CodeBlock right) — the current max-w-2xl centered code block under a max-w-3xl headline wastes the wide canvas.

**3. P1 — Adjacent feature sections use inconsistent surface + heading voices**
Location: `src/sections/landing/WhyRusty.tsx` (Card-based, `CardTitle className="font-display text-lg font-semibold"`, lines 50–57) vs `src/sections/landing/FeatureGrid.tsx` (borderless, `h3 className="text-sm font-semibold tracking-tight"` sans, line 77).
Issue: Two back-to-back "reasons/features" grids present near-identical content shapes (icon chip + title + muted body) but one is elevated serif cards and the other flat sans text — an abrupt density and voice change mid-page.
Fix: pick one language for both. Recommended: keep WhyRusty cards, and wrap FeatureGrid items in the same surface at `Card className="bg-card"` with `CardTitle className="font-display text-base font-semibold leading-snug"` (slightly smaller than WhyRusty's `text-lg` to preserve hierarchy); alternatively drop cards from WhyRusty and promote FeatureGrid titles to `font-display text-base`. Whichever direction, make icon chip + title type + surface identical in both files.

**4. P1 — `text-muted-foreground/80` fails WCAG AA on small mono text**
Location: `src/pages/learn/LearnIndex.tsx` (line 51, reading-time row) and `src/pages/learn/LearnArticle.tsx` (line 216, same pattern); also `text-muted-foreground/50` on decorative icons (LearnIndex line 45) and `text-primary/35` list numbers (LearnIndex line 34).
Issue: `--muted-foreground` (24 8% 42%) on `--background` (36 33% 97%) is ≈ 5:1 — fine at full opacity. At `/80` opacity over cream the effective contrast drops to ≈ 3.3:1, failing AA 4.5:1 for `text-xs` content (reading time is real information, not decoration). The `text-primary/35` article numbers sit at ≈ 1.7:1, failing even large-text AA (3:1).
Fix: drop the opacity modifiers on informational text — change both reading-time rows to `text-muted-foreground` (remove `/80`). Bump the LearnIndex index numbers to `text-primary/50 group-hover:text-primary` (≈ 2.6:1 is still decorative-leaning; if the numbers are considered meaningful, use `text-muted-foreground` and let hover go to `text-primary`). Keep `/40`–`/50` opacities only on purely decorative icons.

**5. P1 — Article body measure is too wide for comfortable reading**
Location: `src/pages/learn/LearnArticle.tsx` (line 192, `max-w-3xl`) with paragraphs at `leading-7 sm:leading-8` (line 58).
Issue: `max-w-3xl` (768px) minus padding yields ≈ 90 characters per line at base font size — beyond the 65–75ch editorial comfort range this design is clearly aiming for.
Fix: change the article container to `max-w-2xl` (672px → ≈ 78ch) or add `max-w-[68ch]` to the content column specifically (keep the header/prev-next at `max-w-3xl` if the wider nav is desired). Compensate with `leading-8` on `sm:` as already present.

**6. P2 — Nested `<main>` landmarks**
Location: `src/components/layout/SiteLayout.tsx` (line 59, `<main className="flex-1">`) + `src/pages/LandingPage.tsx` (line 12, `<main>`).
Issue: LandingPage renders a `<main>` inside the layout's `<main>` — invalid HTML and duplicate landmark for assistive tech. (All other pages correctly use `<div>`.)
Fix: change `LandingPage.tsx` line 12/22 from `<main>…</main>` to `<div>…</div>` (or a fragment).

**7. P2 — Playground eyebrow tracking doesn't match the shared eyebrow style**
Location: `src/pages/playground/PlaygroundPage.tsx` (line 279, `tracking-widest`) vs `src/sections/landing/SectionHeading.tsx` (line 18, `tracking-[0.2em]`) and `src/pages/learn/LearnIndex.tsx` (line 10, `tracking-[0.2em]`).
Issue: The mono uppercase rust eyebrow is a three-place design element; one instance uses a different letter-spacing (0.1em vs 0.2em), subtly breaking the shared vocabulary.
Fix: change PlaygroundPage line 279 to `tracking-[0.2em]`.

**8. P2 — Elevation language is inconsistent between cards and code blocks**
Location: `src/components/ui/card.tsx` (line 10, `shadow-sm`), `src/components/shared/CodeBlock.tsx` (line 30, `shadow-lg`), `src/pages/playground/EventLog.tsx` (line 30, `shadow-lg`).
Issue: Every code block carries `shadow-lg` while all cards and table wrappers use `shadow-sm`; on the cream background the dark blocks already have strong figure-ground contrast, so the heavy shadow makes them visually louder than any actual content card and slightly "floating."
Fix: reduce CodeBlock and EventLog wrappers to `shadow-md` (keeps a touch of lift for the dark surface, rebalances hierarchy). Keep `border-black/40` as-is.

**9. P2 — White cards on 97%-cream background have weak surface separation**
Location: `src/index.css` (`--card: 0 0% 100%` vs `--background: 36 33% 97%`, lines 8/10) + `--border: 28 18% 86%` (line 24).
Issue: Pure-white cards differ from the cream page by only ~3% lightness and a hue shift; separation rides entirely on a very light border. On low-quality displays cards can read as "holes" in the page rather than surfaces.
Fix: either warm the card — `--card: 36 40% 99%` — or strengthen the border slightly to `28 20% 83%`. One of the two; don't do both. (Also improves the `bg-muted/60` table-header strip inside cards in `Comparison.tsx` line 103 / `ComponentsTable.tsx` line 68, which currently barely registers on white.)

**10. P2 — CodeBlock chrome is duplicated, not shared**
Location: `src/pages/playground/EventLog.tsx` (lines 30–39 re-implement the `bg-code rounded-xl border-black/40` wrapper + title bar that `src/components/shared/CodeBlock.tsx` lines 29–50 define); inline `<pre className="…bg-code…">` chips in `StateInspector.tsx` (line 69) and `ResumePanel.tsx` (line 57) are a third and fourth variant (`rounded-md`/`rounded-lg`, no border/shadow).
Issue: Four hand-rolled dark-code surfaces with three different radii/border treatments; future theme tweaks must be made in four places.
Fix: extract the chrome into CodeBlock (e.g. a `frame?: "full" | "flat"` prop: `full` = current rounded-xl+border+shadow, `flat` = rounded-md no border) and have EventLog pass its scroll area as `children`, and StateInspector/ResumePanel use `frame="flat"`. Visual result: one radius, one border rule, everywhere.

**11. P2 — Ordered-list markers and quote callout rely on faded primary for structure**
Location: `src/pages/learn/LearnArticle.tsx` (line 70, `marker:text-primary/70`; line 74, `marker:text-primary/60`; line 129, quote callout `border-l-4 border-primary bg-accent/50`).
Issue: List markers at `/60`–`/70` primary are ≈ 3.4:1 — acceptable for decoration but they carry enumeration meaning in ordered lists; meanwhile the quote callout's `text-accent-foreground` italic serif on `bg-accent/50` halves the background saturation the token pair was designed for.
Fix: use full `marker:text-primary` for ordered lists (keep `/60` on unordered bullets), and change the quote figure to `bg-accent` (full token) so `accent-foreground` sits on its designed background.

---

**Note to parent:** All scoped files were read (index.css, SiteLayout, CodeBlock, all 9 landing sections, both learn pages, all 8 playground components, card.tsx, App.tsx). Also observed: `src/pages/Home.tsx` + `src/App.css` are unrouted Vite-template leftovers (not a visual issue, but worth deleting). No project files were modified.
