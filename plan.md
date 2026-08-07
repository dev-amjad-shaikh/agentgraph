# Plan — aboutrusty.com landing site

## Goal
A landing + learning + interactive experience site for **Rusty** — the durable, LangGraph-style agent runtime built in Rust (this repo). Three pillars:
1. **Landing** — hero, why-Rusty, features, honest comparison vs LangGraph, install/quickstart, components table.
2. **Learn** — structured docs section (architecture deep-dive, server quickstart, studio, roadmap, versioning/stability).
3. **Experience** — interactive playground: animated super-step executor visualization with checkpoints, interrupt/resume, fork/replay — the concepts that make Rusty unique.

## Stack
React + TypeScript + Vite + Tailwind + shadcn/ui (skill: webapp-building, base `0-origin` project).
Design: rust-toned warm palette (low saturation), dark code blocks, ample whitespace. No blue-purple gradients.

## Stage 1 — Research (parallel explore agents)
- R1_Architecture: distill `docs/architecture.md` + `docs/roadmap.md` → content brief for Learn pages.
- R2_Quickstart: distill `docs/server-quickstart.md` + `docs/studio.md` + `docs/live-demo-transcript.md` → brief.
- R3_RepoFacts: distill CHANGELOG.md, docs/versioning.md, docs/stability.md, sdks/, rusty-core/README.md → brief.
Output: `aboutrusty-content/*.md` briefs (concise, factual, landing-site-ready).

## Stage 2 — Scaffold (orchestrator)
- `init-webapp.sh aboutrusty "About Rusty"` (base project, npm install).
- Orchestrator creates: theme tokens (rust palette), App.tsx routes (`/`, `/learn`, `/learn/:slug`, `/playground`), shared Layout/Nav/Footer shell.
- Gate: scaffold builds.

## Stage 3 — Build (parallel coder agents, non-overlapping files)
- W1_Landing: `src/sections/landing/*` + `src/pages/LandingPage.tsx` — hero, why, features, comparison table, quickstart code, components, CTA.
- W2_Learn: `src/pages/learn/*` + content data file — docs index + article pages rendered from R-stage briefs (fidelity to source docs).
- W3_Playground: `src/pages/playground/*` — interactive run simulator: graph view, super-step timeline, checkpoint history, interrupt/resume, fork/replay, state inspector (self-contained simulated engine in TS).
Gate: each worker reports files written; orchestrator runs `npm run build`, fixes integration issues.

## Stage 4 — Validate & deliver
- `npm run build` clean; quick dev-server smoke test then stop it.
- Deliver preview link per Kimi Work rules (localhost:7100 logical port).
