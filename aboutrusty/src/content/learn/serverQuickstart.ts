import type { Article } from "./types";

export const serverQuickstart: Article = {
  slug: "server-quickstart",
  title: "Zero to a served graph in ten minutes",
  description:
    "Cargo.toml is the new langgraph.json: scaffold, define, serve, and drive a graph over HTTP — ten minutes, one process, no Docker, no database.",
  readingTime: "10 min read",
  blocks: [
    {
      type: "callout",
      variant: "quote",
      text: "Cargo.toml is the new langgraph.json.",
    },
    {
      type: "paragraph",
      text: "There is no config file declaring your graphs — the declaration is your `main.rs`, checked by the compiler. **Prerequisites:** a Rust toolchain (`rustup`), `curl`, and ~10 minutes. No Docker, no database, no Redis — everything runs in one process.",
    },

    { type: "heading", level: 2, text: "The flow at a glance" },
    {
      type: "table",
      head: ["Step", "Time", "What happens"],
      rows: [
        ["1. Create the project", "1 min", "`cargo new agent-server-demo`; add path deps"],
        [
          "2. Define the graph",
          "3 min",
          "Two-node graph `draft → approve` with a human-in-the-loop interrupt",
        ],
        [
          "3. Run it",
          "1 min",
          "`cargo run`; server on `localhost:8080`, dev mode, no API key",
        ],
        [
          "4–6. Create thread → run to interrupt → resume over SSE",
          "~4 min",
          "All via curl",
        ],
        ["7–8. Time travel", "2 min", "Checkpoint history, rollback, fork & replay"],
      ],
    },

    { type: "heading", level: 2, text: "Step 1 — Create the project" },
    {
      type: "code",
      language: "bash",
      title: "terminal",
      code: `cargo new agent-server-demo
cd agent-server-demo`,
    },
    {
      type: "code",
      language: "toml",
      title: "Cargo.toml",
      code: `[dependencies]
rusty-agent-runtime = { path = "../rusty-core" }
rusty-server = { path = "../rusty-server" }
tokio = { version = "1", features = ["full"] }
serde_json = "1"`,
    },

    { type: "heading", level: 2, text: "Step 2 — Define the graph" },
    {
      type: "paragraph",
      text: "The quickstart graph is two nodes — `draft` and `approve` — over two channels, `draft` and `approval`, both `Reducer::Overwrite`. Each channel here has exactly one writer node, so Overwrite (last-value) semantics are correct everywhere.",
    },
    {
      type: "list",
      items: [
        "**The canonical interrupt/resume pattern:** check `ctx.resume_value()` FIRST; only interrupt when there is none. Phase 1: no decision → `ctx.interrupt(...)` suspends the run and the payload surfaces to the HTTP caller as the run's `interrupt` value. Phase 2: the decision arrives via `command.resume`.",
        "`builder.compile()?` — validates topology before serving anything.",
        "`GraphRegistry` is the Rust analog of langgraph.json's `graphs` map. A name plus the two things the executor needs — Graph + StateSpec.",
        "`ServerConfig::new(\"0.0.0.0:8080\".parse()?, \"./data/checkpoints\")` — bind address and checkpoint store directory are code; a `JsonFileCheckpointer` rooted at `store_path` is wired in automatically. `serve(registry, config).await?` blocks on the axum/tokio runtime.",
      ],
    },
    {
      type: "callout",
      variant: "note",
      title: "The whole server API",
      text: "The server crate adds exactly three names you call directly: `GraphRegistry`, `ServerConfig`, `serve` (plus `router` if you want to embed the routes in a larger axum app). Everything else is the same `rusty-agent-runtime` API as the library examples.",
    },

    { type: "heading", level: 2, text: "Step 3 — Run it" },
    {
      type: "code",
      language: "bash",
      title: "terminal",
      code: `cargo run
# listening on http://localhost:8080 (dev mode: no API key set)

curl localhost:8080/ok
# {"ok":true}

curl localhost:8080/info
# {"service":"rusty-server","version":"0.4.0","checkpointer":"json_file",
#  "store_path":"./data/checkpoints",
#  "graphs":[{"channels":["approval","draft"],"name":"publisher"}]}`,
    },
    {
      type: "paragraph",
      text: "To turn auth on: `ServerConfig::new(…).with_api_key(\"secret\")` — then add `-H \"X-Api-Key: secret\"` to every request.",
    },

    { type: "heading", level: 2, text: "Step 4 — Create a thread" },
    {
      type: "paragraph",
      text: "A thread binds to one registered graph at creation:",
    },
    {
      type: "code",
      language: "bash",
      title: "terminal",
      code: `curl -s -X POST localhost:8080/threads \\
  -H 'Content-Type: application/json' \\
  -d '{"graph": "publisher", "metadata": {"owner": "quickstart"}}'
# 201 {"thread_id": "3f2b9c4e-…", "graph": "publisher",
#      "metadata": {"owner": "quickstart"}, "created_at": "2026-08-05T…Z"}

TID=<paste the thread_id here>`,
    },

    { type: "heading", level: 2, text: "Step 5 — Run to the interrupt" },
    {
      type: "paragraph",
      text: "`POST /threads/{id}/runs/wait` blocks until the run finishes — or suspends:",
    },
    {
      type: "code",
      language: "bash",
      title: "terminal",
      code: `curl -s -X POST localhost:8080/threads/$TID/runs/wait \\
  -H 'Content-Type: application/json' \\
  -d '{"input": {"draft": "Rust agents, one binary."}}'`,
    },
    {
      type: "code",
      language: "json",
      title: "Terminal response",
      code: `{
  "run_id": "7c1e…",
  "thread_id": "3f2b9c4e-…",
  "status": "interrupted",
  "interrupt": {
    "kind": "approval_request",
    "prompt": "Approve this draft for publication?",
    "draft": "Rust agents, one binary."
  },
  "checkpoint_id": "a94f…",
  "state": { "draft": "Rust agents, one binary." }
}`,
    },
    {
      type: "callout",
      variant: "note",
      title: "Durable by default",
      text: "The run suspended inside `approve`, and the executor persisted a checkpoint for the thread — this is durable, so you could restart the server right now and lose nothing. (Thread records live in memory; checkpoints are durable on disk — re-create the thread with the same `thread_id` after a restart.)",
    },
    {
      type: "paragraph",
      text: "Inspect the parked state:",
    },
    {
      type: "code",
      language: "bash",
      title: "terminal",
      code: `curl -s localhost:8080/threads/$TID/state
# {"values": {"draft": "Rust agents, one binary."}, "next": ["approve"],
#  "checkpoint": {"checkpoint_id": "a94f…", "thread_id": "3f2b…", "step": 1,
#                 "created_at": "…"}}`,
    },
    {
      type: "callout",
      variant: "quote",
      text: "`next: [\"approve\"]` tells you exactly where the run is parked.",
    },
    {
      type: "paragraph",
      text: "Next: resume this run over SSE, then rewind and branch its timeline — covered in **Interrupts, resume, and time travel**.",
    },
  ],
};
