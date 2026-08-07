import type { Article } from "./types";

export const humanInTheLoop: Article = {
  slug: "human-in-the-loop",
  title: "Interrupts, resume, and time travel",
  description:
    "An interrupt is a transaction abort with a receipt: resume values, SSE frame streams, rollback, and the fork-first rule for replaying history.",
  readingTime: "9 min read",
  blocks: [
    {
      type: "callout",
      variant: "quote",
      text: "An interrupt is a transaction abort with a receipt.",
    },
    {
      type: "paragraph",
      text: "A node suspends the run by returning `Err(ctx.interrupt(payload))` (`NodeContext::interrupt`). The suspension is **run-wide**: the in-flight step's writes are discarded — **including writes from sibling nodes that already completed** — still-running siblings are aborted, and the suspension checkpoint **re-schedules the entire active set** of the step, not just the interrupting node.",
    },
    {
      type: "callout",
      variant: "quote",
      text: "Anything less would silently lose the siblings' discarded work.",
    },
    {
      type: "paragraph",
      text: "The caller receives `ExecutionOutcome::Interrupted { value, state, checkpoint_id }` — the receipt. The thread id is the resume handle; the checkpoint makes the suspension durable.",
    },

    { type: "heading", level: 2, text: "The resume-value pattern" },
    {
      type: "paragraph",
      text: "Resume is the same `thread_id` plus `RunConfig::with_resume(value)`. Every node of the suspended set re-executes from its start; the resume value is **broadcast to all of them for the first super-step**. So a resumable node checks `ctx.resume_value()` **first**, and must be idempotent in everything it did before interrupting:",
    },
    {
      type: "code",
      language: "rust",
      title: "Check resume_value() FIRST — interrupt only when there is none",
      code: `builder.add_node("approve", |ctx: NodeContext| async move {
    match ctx.resume_value() {
        Some(decision) => Ok(NodeOutput::update("approval", decision.clone())),
        None => Err(ctx.interrupt(json!({"prompt": "Approve this draft?"}))),
    }
});`,
    },
    {
      type: "paragraph",
      text: "On resume the node re-runs from the top, finds `Some(decision)` waiting, and completes instead of interrupting again.",
    },

    { type: "heading", level: 2, text: "Resume, streamed over SSE" },
    {
      type: "paragraph",
      text: "Over HTTP, the human's decision goes back via `command.resume` (`-N` disables curl buffering — required for SSE):",
    },
    {
      type: "code",
      language: "bash",
      title: "terminal",
      code: `curl -N -X POST localhost:8080/threads/$TID/runs/stream \\
  -H 'Content-Type: application/json' \\
  -d '{"command": {"resume": {"approved": true, "reviewer": "alice"}},
       "stream_mode": ["updates", "values"]}'`,
    },
    {
      type: "paragraph",
      text: "The frames arrive in a fixed order — `metadata` → `updates` → `values` → `end`:",
    },
    {
      type: "code",
      language: "text",
      title: "The resume frame stream",
      code: `event: metadata
id: -:0:1
data: {"run_id": "…", "thread_id": "3f2b9c4e-…", "graph": "publisher",
       "attempt": 2, "metadata": null}

event: updates
id: a94f…:2:2
data: {"step": 2, "updates": {"approve": {"approval": {"approved": true, "reviewer": "alice"}}}}

event: values
id: a94f…:2:3
data: {"draft": "Rust agents, one binary.", "approval": {"approved": true, "reviewer": "alice"}}

event: end
id: a94f…:2:4
data: {"status": "success"}`,
    },
    {
      type: "callout",
      variant: "note",
      title: "What actually happens on resume",
      text: "The executor restored the checkpoint, re-executed `approve` from its start with `ctx.resume_value()` set (this is why node logic must be idempotent), and the run completed.",
    },

    { type: "heading", level: 2, text: "Frame ids and resumable streams" },
    {
      type: "paragraph",
      text: "Every SSE frame carries `id: {checkpoint_id}:{step}:{seq}` where `seq` is a per-run monotonically increasing sequence number; frames before the run's first checkpoint use `-` as the checkpoint component. A reconnecting client sends `Last-Event-ID`; the server skips frames already seen, replaying the run's in-memory event-log tail (capacity: `ServerConfig::event_log_capacity`, **default 1000 frames**) before streaming live frames.",
    },
    {
      type: "code",
      language: "bash",
      title: "terminal",
      code: `curl -N -X POST localhost:8080/threads/$TID/runs/stream \\
  -H 'Content-Type: application/json' \\
  -H 'Last-Event-ID: a94f…:2:2' \\
  -d '{"command": {"resume": {"approved": true}}, "stream_mode": ["updates", "values"]}'`,
    },

    { type: "heading", level: 2, text: "Time travel — history, rollback, fork & replay" },
    {
      type: "paragraph",
      text: "Checkpoint history comes back newest first:",
    },
    {
      type: "code",
      language: "bash",
      title: "terminal",
      code: `curl -s -X POST localhost:8080/threads/$TID/history \\
  -H 'Content-Type: application/json' \\
  -d '{"limit": 10}'`,
    },
    {
      type: "paragraph",
      text: "Rollback undoes a finished run — it deletes the run's checkpoints and re-anchors the thread to the pre-run checkpoint:",
    },
    {
      type: "code",
      language: "bash",
      title: "terminal",
      code: `curl -s -X DELETE localhost:8080/threads/$TID/runs/$RUN_ID
# {"run_id": "…", "thread_id": "…", "deleted_checkpoints": 2,
#  "remaining_checkpoints": 1}`,
    },
    {
      type: "callout",
      variant: "quote",
      text: "Rollback rewinds a thread; fork branches it.",
    },
    {
      type: "code",
      language: "bash",
      title: "terminal",
      code: `# Pick an earlier checkpoint from the history listing
CP_ID=<a checkpoint_id from the history above>

# Fork the thread at that checkpoint (omit checkpoint_id for a full-history fork)
curl -s -X POST localhost:8080/threads/$TID/fork \\
  -H 'Content-Type: application/json' \\
  -d '{"new_thread_id": "branch-a", "checkpoint_id": "'$CP_ID'"}'
# 201 {"thread_id": "branch-a", "checkpoints_copied": 1}

# Replay the run from the same checkpoint, on the fork
curl -s -X POST localhost:8080/threads/branch-a/runs/wait \\
  -H 'Content-Type: application/json' \\
  -d '{"checkpoint": {"checkpoint_id": "'$CP_ID'"}}'`,
    },
    {
      type: "paragraph",
      text: "**Error semantics:** `404` unknown thread/checkpoint id, `400` source thread has no checkpoints to copy, `409` `new_thread_id` already taken.",
    },
    {
      type: "callout",
      variant: "warning",
      title: "The safe pattern",
      text: "The safe pattern is fork first, replay on the fork: the branch gets its own thread id and its own history, while replaying on the original thread appends new checkpoints on top of the old timeline (supported, but rarely what you want).",
    },
  ],
};
