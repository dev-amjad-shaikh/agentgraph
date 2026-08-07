/**
 * A tiny, fully deterministic in-browser simulation of Rusty's execution
 * semantics. No backend, no randomness: the scripted ChatModel, the
 * reducers, the super-step loop, the checkpoints, and the interrupt /
 * resume / fork / replay behavior all mirror the real rusty-core model
 * (Pregel/BSP super-steps, AddMessages, versioned checkpoints, HITL
 * interrupts as "transaction aborts with a receipt").
 */

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

export type ScenarioId = "react" | "hitl";
export type Persona = "main" | "branch";

export const PHASES = [
  "plan",
  "parallel",
  "barrier",
  "merge",
  "route",
  "checkpoint",
] as const;
export type Phase = (typeof PHASES)[number];

export const PHASE_LABELS: Record<Phase, string> = {
  plan: "plan",
  parallel: "parallel",
  barrier: "barrier",
  merge: "merge",
  route: "route",
  checkpoint: "checkpoint",
};

export interface ToolCall {
  id: string;
  name: string;
  args: Record<string, unknown>;
}

export interface ChatMessage {
  id: string;
  role: "user" | "assistant" | "tool";
  content: string;
  tool_calls?: ToolCall[];
  tool_call_id?: string;
  name?: string;
}

export type ChannelState = Record<string, unknown>;

export interface Checkpoint {
  checkpoint_id: string;
  thread_id: string;
  /** Super-step index at whose boundary this checkpoint was written. */
  step: number;
  state: ChannelState;
  /** Next-node set scheduled after this boundary. */
  next: string[];
  /** Nodes that executed in the step leading to this boundary. */
  nodes: string[];
  /** Timestamp-ish monotonic counter (a stand-in for created_at). */
  clock: number;
  kind: "step" | "suspension";
}

export type FrameType = "metadata" | "updates" | "values" | "end";

export interface SimFrame {
  /** Per-run monotonically increasing sequence number. */
  seq: number;
  step: number;
  /** SSE frame id: {checkpoint_id}:{step}:{seq}; "-" before the first checkpoint. */
  frameId: string;
  event: FrameType;
  data: unknown;
}

export type ThreadStatus = "idle" | "running" | "paused" | "interrupted" | "done";

export interface ThreadSim {
  id: string;
  scenario: ScenarioId;
  persona: Persona;
  status: ThreadStatus;
  state: ChannelState;
  /** Active set scheduled for the next super-step. */
  next: string[];
  /** Index of the next super-step to execute. */
  step: number;
  clock: number;
  /** SSE sequence counter for the current run attempt. */
  seq: number;
  /** Run attempt counter (attempt 2 = first resume, like the quickstart). */
  attempt: number;
  checkpoints: Checkpoint[];
  frames: SimFrame[];
  interrupt?: unknown;
  forkedFrom?: { thread: string; checkpoint: string };
}

export interface RouteEdge {
  from: string;
  to: string;
  kind: "static" | "conditional" | "command";
}

export interface StepComputation {
  step: number;
  active: string[];
  /** Partial updates per node: node -> channel -> value. */
  updates: Record<string, Record<string, unknown>>;
  mergedState: ChannelState;
  nextSet: string[];
  routes: RouteEdge[];
  checkpoint: Checkpoint;
  updatesFrame: SimFrame;
  valuesFrame: SimFrame | null;
  endFrame: SimFrame | null;
  outcome: "continue" | "done" | "interrupted";
  interrupt?: unknown;
}

// ---------------------------------------------------------------------------
// Scenario metadata (graph layouts, copy, code snippets)
// ---------------------------------------------------------------------------

export interface GraphNodeDef {
  id: string;
  label: string;
  x: number;
  y: number;
  kind: "node" | "end";
}

export interface GraphEdgeDef {
  from: string;
  to: string;
  kind: "static" | "conditional";
  label?: string;
}

export interface ScenarioDef {
  id: ScenarioId;
  graphName: string;
  title: string;
  tagline: string;
  channels: { name: string; reducer: string }[];
  nodes: GraphNodeDef[];
  edges: GraphEdgeDef[];
  entryPoint: string;
  userPrompt?: string;
  resumeSnippet?: string;
}

export const RESUME_SNIPPET = `// The resumable node: check resume_value() FIRST;
// interrupt only when no human decision exists yet.
// On resume it re-runs from the top — so it must be idempotent.
match ctx.resume_value() {
    Some(decision) => Ok(NodeOutput::update("approval", decision.clone())),
    None => Err(ctx.interrupt(json!({"prompt": "Approve this draft?"}))),
}`;

export const SCENARIOS: Record<ScenarioId, ScenarioDef> = {
  react: {
    id: "react",
    graphName: "react_agent",
    title: "ReAct agent",
    tagline:
      "The classic two-node cyclic graph — agent ⇄ tools over one messages channel with the AddMessages reducer. The cycle is super-steps, not recursion.",
    channels: [{ name: "messages", reducer: "AddMessages" }],
    entryPoint: "agent",
    userPrompt:
      "What time is it right now in UTC? Then multiply 128 by 46, and count the words in 'the quick brown fox jumps over the lazy dog'.",
    nodes: [
      { id: "agent", label: "agent", x: 70, y: 60, kind: "node" },
      { id: "tools", label: "tools", x: 250, y: 60, kind: "node" },
      { id: "__end__", label: "End", x: 160, y: 170, kind: "end" },
    ],
    edges: [
      { from: "agent", to: "tools", kind: "conditional", label: "tool_calls?" },
      { from: "tools", to: "agent", kind: "static" },
      { from: "agent", to: "__end__", kind: "conditional", label: "Route::End" },
    ],
  },
  hitl: {
    id: "hitl",
    graphName: "publisher",
    title: "Human approval",
    tagline:
      "draft → approve → publish with a human-in-the-loop interrupt. An interrupt is a transaction abort with a receipt.",
    channels: [
      { name: "draft", reducer: "Overwrite" },
      { name: "approval", reducer: "Overwrite" },
      { name: "published", reducer: "Overwrite" },
    ],
    entryPoint: "draft",
    resumeSnippet: RESUME_SNIPPET,
    nodes: [
      { id: "draft", label: "draft", x: 40, y: 60, kind: "node" },
      { id: "approve", label: "approve", x: 160, y: 60, kind: "node" },
      { id: "publish", label: "publish", x: 280, y: 60, kind: "node" },
      { id: "__end__", label: "End", x: 160, y: 170, kind: "end" },
    ],
    edges: [
      { from: "draft", to: "approve", kind: "static" },
      { from: "approve", to: "publish", kind: "conditional", label: "approved" },
      { from: "approve", to: "__end__", kind: "conditional", label: "rejected" },
    ],
  },
};

// ---------------------------------------------------------------------------
// Deterministic ids and clocks
// ---------------------------------------------------------------------------

/** Pseudo-hex checkpoint ids, deterministic per counter (a94f-style). */
function checkpointId(n: number): string {
  return (0xa11c + n * 0x1f3).toString(16).padStart(4, "0");
}

function latestCheckpointId(t: ThreadSim): string {
  return t.checkpoints.length > 0
    ? t.checkpoints[t.checkpoints.length - 1].checkpoint_id
    : "-";
}

function nextFrame(t: ThreadSim, step: number, event: FrameType, data: unknown): SimFrame {
  const seq = t.seq + 1;
  return {
    seq,
    step,
    frameId: `${latestCheckpointId(t)}:${step}:${seq}`,
    event,
    data,
  };
}

// ---------------------------------------------------------------------------
// AddMessages reducer (ID-aware message upsert)
// ---------------------------------------------------------------------------

function addMessages(existing: ChatMessage[], incoming: ChatMessage[]): ChatMessage[] {
  const out = [...existing];
  for (const msg of incoming) {
    const idx = out.findIndex((m) => m.id === msg.id);
    if (idx >= 0) out[idx] = msg; // upsert by id — replay never duplicates
    else out.push(msg);
  }
  return out;
}

function getMessages(state: ChannelState): ChatMessage[] {
  const m = state["messages"];
  return Array.isArray(m) ? (m as ChatMessage[]) : [];
}

// ---------------------------------------------------------------------------
// The scripted, deterministic ChatModel + tools (no network, no randomness)
// ---------------------------------------------------------------------------

const TOOL_PLAN: { name: string; args: Record<string, unknown> }[] = [
  { name: "get_current_time", args: {} },
  { name: "calculator", args: { a: 128, b: 46, op: "multiply" } },
  {
    name: "word_count",
    args: { text: "the quick brown fox jumps over the lazy dog" },
  },
];

function runTool(call: ToolCall): string {
  switch (call.name) {
    case "get_current_time":
      return "2026-08-06 06:42:52 UTC";
    case "calculator":
      return "128 multiply 46 = 5888";
    case "word_count":
      return JSON.stringify({ characters: 43, lines: 1, words: 9 });
    default:
      return `ERROR: unknown tool ${call.name}`;
  }
}

const FINAL_ANSWER_MAIN =
  "The current time in UTC is August 6, 2026, 06:42:52. 128 multiplied by 46 equals 5888. The phrase 'the quick brown fox jumps over the lazy dog' contains 9 words.";

const FINAL_ANSWER_BRANCH =
  "UTC time: 2026-08-06 06:42:52. 128 × 46 = 5888. Word count: 9 (the forked timeline re-planned the work one tool per super-step).";

/**
 * The scripted model. The main persona batches all three tool calls into one
 * parallel tool batch (the warm-model demo behavior); the branch persona —
 * used on forked timelines — deliberately re-plans one tool per super-step so
 * visitors see two genuinely divergent histories.
 */
function scriptedModelReply(messages: ChatMessage[], persona: Persona): ChatMessage {
  const answered = new Set(
    messages.filter((m) => m.role === "tool").map((m) => m.tool_call_id),
  );
  const assistantCount = messages.filter((m) => m.role === "assistant").length;
  const id = `a-${assistantCount + 1}`;

  const pending = TOOL_PLAN.filter((_, i) => !answered.has(`call-${i}`));

  if (pending.length === 0) {
    return {
      id,
      role: "assistant",
      content: persona === "main" ? FINAL_ANSWER_MAIN : FINAL_ANSWER_BRANCH,
    };
  }

  const batch = persona === "main" ? pending : pending.slice(0, 1);
  return {
    id,
    role: "assistant",
    content:
      persona === "main"
        ? "I'll fire all three tool calls in one parallel batch."
        : "Forked timeline: I'll call tools one at a time, one super-step each.",
    tool_calls: batch.map((t) => ({
      id: `call-${TOOL_PLAN.indexOf(t)}`,
      name: t.name,
      args: t.args,
    })),
  };
}

// ---------------------------------------------------------------------------
// Thread lifecycle
// ---------------------------------------------------------------------------

export function createThread(
  scenario: ScenarioId,
  persona: Persona,
  id: string,
  forkedFrom?: { thread: string; checkpoint: string },
): ThreadSim {
  const def = SCENARIOS[scenario];
  const state: ChannelState =
    scenario === "react"
      ? {
          messages: [
            { id: "u-1", role: "user", content: def.userPrompt ?? "" } satisfies ChatMessage,
          ],
        }
      : {};
  return {
    id,
    scenario,
    persona,
    status: "idle",
    state,
    next: [def.entryPoint],
    step: 0,
    clock: 0,
    seq: 0,
    attempt: 0,
    checkpoints: [],
    frames: [],
    forkedFrom,
  };
}

/** Emit the run's metadata frame (id uses "-" before the first checkpoint). */
export function beginRun(t: ThreadSim): ThreadSim {
  const attempt = t.attempt + 1;
  const started: ThreadSim = { ...t, attempt, seq: 0, status: "running" };
  const frame = nextFrame(started, 0, "metadata", {
    run_id: `run-${started.id}-${attempt}`,
    thread_id: started.id,
    graph: SCENARIOS[started.scenario].graphName,
    attempt,
    metadata: null,
  });
  return { ...started, seq: frame.seq, frames: [...started.frames, frame] };
}

// ---------------------------------------------------------------------------
// One super-step: plan → parallel → barrier → merge → route → checkpoint
// ---------------------------------------------------------------------------

export function computeSuperStep(t: ThreadSim, resumeValue?: unknown): StepComputation {
  const step = t.step;
  const active = t.next;
  const clock = t.clock + 1;
  const updates: Record<string, Record<string, unknown>> = {};
  const routes: RouteEdge[] = [];

  if (t.scenario === "react") {
    const messages = getMessages(t.state);
    const node = active[0];

    if (node === "agent") {
      const reply = scriptedModelReply(messages, t.persona);
      updates["agent"] = { messages: [reply] };
      const merged = addMessages(messages, [reply]);
      const hasToolCalls = (reply.tool_calls?.length ?? 0) > 0;
      const nextSet = hasToolCalls ? ["tools"] : [];
      routes.push(
        hasToolCalls
          ? { from: "agent", to: "tools", kind: "conditional" }
          : { from: "agent", to: "__end__", kind: "conditional" },
      );
      return finishStep(t, step, active, updates, { messages: merged }, nextSet, routes, clock);
    }

    // node === "tools": dispatch the pending tool calls as one batch —
    // results preserve call order, failures would be isolated as ERROR: messages.
    const lastAssistant = [...messages].reverse().find((m) => m.role === "assistant");
    const calls = lastAssistant?.tool_calls ?? [];
    const results: ChatMessage[] = calls.map((c) => ({
      id: `t-${c.id}`,
      role: "tool",
      tool_call_id: c.id,
      name: c.name,
      content: runTool(c),
    }));
    updates["tools"] = { messages: results };
    const merged = addMessages(messages, results);
    routes.push({ from: "tools", to: "agent", kind: "static" });
    return finishStep(t, step, active, updates, { messages: merged }, ["agent"], routes, clock);
  }

  // ------------------------------------------------------------- hitl -----
  const node = active[0];

  if (node === "draft") {
    updates["draft"] = { draft: "Rust agents, one binary." };
    const merged = { ...t.state, draft: "Rust agents, one binary." };
    routes.push({ from: "draft", to: "approve", kind: "static" });
    return finishStep(t, step, active, updates, merged, ["approve"], routes, clock);
  }

  if (node === "approve") {
    if (resumeValue === undefined) {
      // No human decision yet — interrupt. The in-flight step's writes are
      // discarded wholesale and the suspension checkpoint re-schedules the
      // ENTIRE active set. An interrupt is a transaction abort with a receipt.
      const payload = {
        kind: "approval_request",
        prompt: "Approve this draft for publication?",
        draft: (t.state["draft"] as string) ?? null,
      };
      const checkpoint: Checkpoint = {
        checkpoint_id: checkpointId(t.checkpoints.length),
        thread_id: t.id,
        step,
        state: t.state, // pre-step state — nothing merged
        next: [...active],
        nodes: active,
        clock,
        kind: "suspension",
      };
      const updF = nextFrame(t, step, "updates", {
        step,
        updates: { approve: null },
        note: "step discarded — writes aborted at the barrier",
      });
      const endF = nextFrame({ ...t, seq: updF.seq }, step, "end", {
        status: "interrupted",
        interrupt: payload,
        checkpoint_id: checkpoint.checkpoint_id,
      });
      return {
        step,
        active,
        updates: {},
        mergedState: t.state,
        nextSet: [...active],
        routes: [],
        checkpoint,
        updatesFrame: updF,
        valuesFrame: null,
        endFrame: endF,
        outcome: "interrupted",
        interrupt: payload,
      };
    }

    // Resumed: the node re-executes from its start and sees the resume value
    // via ctx.resume_value() — broadcast for the first super-step.
    updates["approve"] = { approval: resumeValue };
    const merged = { ...t.state, approval: resumeValue };
    const approved =
      typeof resumeValue === "object" &&
      resumeValue !== null &&
      (resumeValue as Record<string, unknown>)["approved"] === true;
    const nextSet = approved ? ["publish"] : [];
    routes.push(
      approved
        ? { from: "approve", to: "publish", kind: "conditional" }
        : { from: "approve", to: "__end__", kind: "conditional" },
    );
    return finishStep(t, step, active, updates, merged, nextSet, routes, clock);
  }

  // node === "publish"
  const published = { draft: t.state["draft"] ?? null };
  updates["publish"] = { published };
  const merged = { ...t.state, published };
  return finishStep(t, step, active, updates, merged, [], routes, clock);
}

function finishStep(
  t: ThreadSim,
  step: number,
  active: string[],
  updates: Record<string, Record<string, unknown>>,
  mergedState: ChannelState,
  nextSet: string[],
  routes: RouteEdge[],
  clock: number,
): StepComputation {
  const checkpoint: Checkpoint = {
    checkpoint_id: checkpointId(t.checkpoints.length),
    thread_id: t.id,
    step,
    state: mergedState,
    next: nextSet,
    nodes: active,
    clock,
    kind: "step",
  };
  // Frame ids reference the checkpoint the step started from ({cp}:{step}:{seq});
  // frames before the run's first checkpoint use "-" as the checkpoint component.
  const updatesFrame = nextFrame(t, step, "updates", { step, updates });
  const valuesFrame = nextFrame(
    { ...t, seq: updatesFrame.seq },
    step,
    "values",
    mergedState,
  );
  const done = nextSet.length === 0;
  const endFrame = done
    ? nextFrame({ ...t, seq: valuesFrame.seq }, step, "end", { status: "success" })
    : null;
  return {
    step,
    active,
    updates,
    mergedState,
    nextSet,
    routes,
    checkpoint,
    updatesFrame,
    valuesFrame,
    endFrame,
    outcome: done ? "done" : "continue",
  };
}

// ---------------------------------------------------------------------------
// Time travel: fork_thread + replay
// ---------------------------------------------------------------------------

let forkCounter = 0;

/**
 * fork_thread(src, dst, at_checkpoint_id): copy history (oldest first, up to
 * and including the checkpoint) into a new thread id, ready for a replay run
 * with RunConfig::with_checkpoint_id semantics — the new thread resumes from
 * that checkpoint's state and next-node set.
 */
export function forkThread(src: ThreadSim, at: Checkpoint): ThreadSim {
  forkCounter += 1;
  const id = `${src.id}-fork-${at.step}`;
  const copied = src.checkpoints
    .filter((c) => c.clock <= at.clock)
    .map((c) => ({ ...c, thread_id: id }));
  const forked = createThread(src.scenario, "branch", id, {
    thread: src.id,
    checkpoint: at.checkpoint_id,
  });
  return {
    ...forked,
    state: at.state,
    next: [...at.next],
    step: at.step + 1,
    clock: at.clock,
    checkpoints: copied,
    status: at.kind === "suspension" ? "interrupted" : "paused",
    interrupt: at.kind === "suspension" ? src.interrupt : undefined,
  };
}
