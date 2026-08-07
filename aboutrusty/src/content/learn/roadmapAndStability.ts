import type { Article } from "./types";

export const roadmapAndStability: Article = {
  slug: "roadmap-and-stability",
  title: "Roadmap, versioning, and stability",
  description:
    "Named releases, independently versioned crates, the two surfaces that are stable today, what R1.0 changes — and the integrations Rusty rejected on the record.",
  readingTime: "8 min read",
  blocks: [
    {
      type: "paragraph",
      text: "Rusty's releases are named, but its packages are versioned independently: `rusty-agent-runtime`, `rusty-server`, `rusty-worker`, `rusty-otel`, plus the Python and TypeScript SDKs. The named releases are a branding and history layer — a named release does not imply a shared version number.",
    },
    {
      type: "callout",
      variant: "quote",
      text: "There is no single “Rusty version.”",
    },

    { type: "heading", level: 2, text: "Release timeline" },
    {
      type: "table",
      head: ["Release", "Date", "Codename", "Headline"],
      rows: [
        [
          "rusty-agent-runtime 0.1.0",
          "2026-07-31",
          "R0.1 — Ignition",
          "Execution core, checkpointing, HITL interrupts, LLM & tool layer",
        ],
        [
          "rusty-agent-runtime 0.2.0 + rusty-server 0.1.0",
          "2026-08-05",
          "R0.2 — Persistence",
          "Postgres checkpointer, token streaming, HTTP/SSE server crate",
        ],
        [
          "v0.3.0",
          "2026-08-05",
          "R0.3 — Interop",
          "MCP client, remote nodes + `rusty-worker`, server API completion, tracing",
        ],
        [
          "v0.4.0",
          "2026-08-05",
          "R0.4 — Time Travel",
          "WASM nodes, time travel (fork/replay), Postgres server store, `rusty-otel`, Rusty Studio, CORS",
        ],
        [
          "v0.5.0",
          "2026-08-05",
          "(pre-1.0 cycle)",
          "Python SDK + TypeScript SDK (both v0.1.0), multi-tenant auth, live-LLM validation",
        ],
      ],
    },

    { type: "heading", level: 2, text: "R1.0 — Unleashed" },
    {
      type: "callout",
      variant: "warning",
      title: "Directional, not scheduled",
      text: "R1.0 — Unleashed is the upcoming v1.0 track. It is directional, not scheduled — it has no date.",
    },
    {
      type: "paragraph",
      text: "Three ambitions:",
    },
    {
      type: "list",
      ordered: true,
      items: [
        "**Hosted multi-tenant service** — the server crate operated as a managed platform: tenant isolation, durable queues, autoscaling workers. **Partially started:** v0.5 implemented the tenant-isolation brick (per-tenant API keys, namespaced storage, 404-on-cross-tenant semantics) in `rusty-server` v0.4.0; durable queues and autoscaling remain open.",
        "**WASM target** — run graphs themselves in the browser or edge runtimes (sans native checkpointers).",
        "**Edge deployment** — single-digit-MB agent services on edge runtimes, leaning on Rust's footprint and the static-binary story.",
      ],
    },

    { type: "heading", level: 2, text: "Versioning policy" },
    {
      type: "list",
      items: [
        "**Pre-1.0 SemVer.** All packages are `0.x`. A **minor** bump (`0.x.0 → 0.x+1.0`) may contain breaking changes (each recorded in the CHANGELOG); a **patch** bump is fixes only — no API or wire-format changes.",
        "**The remote-execution wire protocol versions separately.** `PROTOCOL_VERSION` (in `rusty-core/src/remote.rs`) is a single `u32`, currently **`1`**, governing `RemoteNode` ↔ `rusty-worker` (`POST /execute`, `NodeTask` / `TaskResult`). Evolution within v1 is additive-only; workers must reject tasks with an unsupported `protocol_version`; responses are accepted regardless of their version field (newer workers serve older clients). A non-additive change bumps the protocol to 2.",
        "**Server↔SDK compatibility is not yet versioned by a constant** — no numeric protocol version on the HTTP/SSE API today. Rule: an SDK `0.x.y` is tested against the same-cycle server release; cross-cycle pairing may work where overlap is additive but is unvalidated.",
        "**MSRV = Rust 1.86** for all four crates, declared once in `[workspace.package]` (`rust-version = \"1.86\"`) and inherited workspace-wide; enforced in CI per-crate. Pre-1.0, an MSRV bump may land in any minor release.",
      ],
    },
    { type: "heading", level: 3, text: "Current versions (as of 2026-08-06)" },
    {
      type: "table",
      head: ["Package", "Registry", "Source", "Version"],
      rows: [
        ["`rusty-agent-runtime`", "crates.io", "`rusty-core/`", "0.4.0"],
        ["`rusty-server`", "crates.io", "`rusty-server/`", "0.4.0"],
        ["`rusty-worker`", "crates.io", "`rusty-worker/`", "0.1.0"],
        ["`rusty-otel`", "crates.io", "`rusty-otel/`", "0.1.0"],
        ["`@rusty-runtime/client`", "npm", "`sdks/typescript/`", "0.1.0"],
        [
          "`rusty-agent-runtime` (import: `rusty_client`)",
          "PyPI",
          "`sdks/python/`",
          "0.1.0",
        ],
      ],
    },
    {
      type: "paragraph",
      text: "**Name-collision note (by design):** the Rust core crate and the Python SDK are both published as `rusty-agent-runtime` (crates.io and PyPI respectively). Different packages, independent version numbers; the Python SDK is imported as `rusty_client`. Registry publishing for both SDKs is still pending.",
    },

    { type: "heading", level: 2, text: "Stability guarantees" },
    {
      type: "callout",
      variant: "quote",
      text: "This document is a contract, not an aspiration: if something is not listed under “stable”, assume it can change in the next minor release.",
    },
    {
      type: "paragraph",
      text: "Stable today — only two surfaces, treated as protocol-level:",
    },
    {
      type: "list",
      ordered: true,
      items: [
        "**The remote-execution wire protocol (v1)** — additive-only within v1.",
        "**The checkpoint format, within a minor version line** — a checkpoint written by any `rusty-agent-runtime` `0.x.*` release is readable by every other `0.x.*` in that same minor line, including restore, `get_by_id` replay, and `fork_thread` time-travel forks. Across a minor bump the struct may change (the CHANGELOG will say so and ship a migration path where one exists); no cross-minor guarantee in either direction.",
      ],
    },
    {
      type: "paragraph",
      text: "**Not stable** (may change in any 0.x minor release): the Rust API surface of all four crates (pin `=0.x.y` if rebuilds must not break); HTTP request/response JSON fields; SSE event families and payload fields (clients must ignore unknown events/fields; `metadata`, `error`, `end` always emitted; default `stream_mode` is `[\"values\", \"updates\"]`); SDK class/function shapes (`RustyClient` / `RustyError` / `SSEEvent`, `@rusty-runtime/client` exports); Rusty Studio internals; tenant-isolation internals (the `{tenant}/` prefix layout is an implementation detail — but 404-never-403 is intended behavior).",
    },
    {
      type: "paragraph",
      text: "**Deprecation at 0.x** is a CHANGELOG commitment, not a code mechanism; removal lands no sooner than the following minor release where feasible (security/correctness fixes excepted). No `#[deprecated]` lint guarantee — the CHANGELOG is the channel.",
    },
    {
      type: "paragraph",
      text: "**What changes at R1.0 — Unleashed:** full SemVer across crates, HTTP/SSE API, and both SDKs; the HTTP/SSE API becomes a versioned, stable surface (the same-cycle pairing rule goes away); checkpoint migrations guaranteed (a 1.x runtime reads any earlier 1.x checkpoint; migration path across the 0.x → 1.0 boundary); MSRV bumps become minor-release-only events; deprecation gains teeth (`#[deprecated]` warnings for ≥ 1 minor release before removal).",
    },
    {
      type: "callout",
      variant: "quote",
      text: "R1.0 — Unleashed flips the default from “may break” to “must not break” for the public surface.",
    },

    { type: "heading", level: 2, text: "Explicitly rejected" },
    {
      type: "paragraph",
      text: "Two integration paths were considered and rejected, on the record:",
    },
    {
      type: "callout",
      variant: "note",
      title: "napi-rs / PyO3 bindings — REJECTED",
      text: "“They'd freeze a trait surface that's still moving and split maintenance across three ecosystems; the HTTP/SSE server is the polyglot interop layer instead.”",
    },
    {
      type: "callout",
      variant: "note",
      title: "cdylib / C ABI — REJECTED",
      text: "“A C ABI over async tokio graphs leaks runtime-ownership and panic-safety problems across the boundary for near-zero demand; embed the Rust crate directly or talk HTTP.”",
    },
    {
      type: "callout",
      variant: "quote",
      text: "The server is the polyglot interop layer by design.",
    },
  ],
};
