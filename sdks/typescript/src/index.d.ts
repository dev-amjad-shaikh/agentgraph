/**
 * agentgraph-client — TypeScript declarations.
 *
 * Zero-dependency JS/TS client for agentgraph-server (HTTP + SSE).
 * Works in Node.js >= 18 and modern browsers.
 */

/** JSON-serializable value. */
export type JsonValue =
  | string
  | number
  | boolean
  | null
  | JsonValue[]
  | { [key: string]: JsonValue };

/** Client constructor options. */
export interface AgentGraphClientOptions {
  /** Static API key sent as the `X-Api-Key` header. */
  apiKey?: string;
  /**
   * Per-request timeout in milliseconds (default 30000).
   * For `runStream`, covers establishing the stream only. `0` disables.
   */
  timeout?: number;
  /** Custom fetch implementation (defaults to global `fetch`). */
  fetch?: typeof fetch;
  /** Extra headers sent on every request. */
  headers?: Record<string, string>;
}

/** Error thrown for non-2xx responses and network-level failures. */
export class AgentGraphError extends Error {
  /** HTTP status code, or 0 when no response was received. */
  status: number;
  /** Parsed response body (JSON value or raw text), if any. */
  body: unknown;
  /** Request URL. */
  url?: string;
  constructor(
    message: string,
    init?: { status?: number; body?: unknown; url?: string; cause?: unknown },
  );
  static fromResponse(response: Response, url: string): Promise<AgentGraphError>;
}

/** Thrown when a request exceeds the client or per-call timeout. */
export class AgentGraphTimeoutError extends AgentGraphError {
  timeoutMs: number;
  constructor(url: string, timeoutMs: number);
}

/** One registered graph as reported by `GET /info`. */
export interface GraphInfo {
  name: string;
  channels: string[];
}

/** `GET /info` response. */
export interface InfoResponse {
  service: string;
  version: string;
  checkpointer: string;
  server_store?: string;
  store_path: string;
  graphs: GraphInfo[];
}

/** Thread record returned by `POST /threads`. */
export interface Thread {
  thread_id: string;
  graph: string;
  metadata: Record<string, JsonValue> | null;
  created_at: string;
}

/** Checkpoint metadata. */
export interface Checkpoint {
  checkpoint_id: string;
  thread_id: string;
  step: number;
  created_at: string;
}

/** Thread state snapshot: `{ values, next, checkpoint }`. */
export interface ThreadState {
  values: Record<string, JsonValue>;
  next: string[];
  checkpoint: Checkpoint;
}

/** `POST /threads/{id}/fork` response. */
export interface ForkResult {
  thread_id: string;
  checkpoints_copied: number;
}

/** Run-create payload (subset of the LangGraph Platform shape). */
export interface RunPayload {
  /** Input state for the run (e.g. `{ messages: [...] }`). */
  input?: Record<string, JsonValue>;
  /** Human-in-the-loop resume channel. */
  command?: { resume?: JsonValue };
  config?: { recursion_limit?: number } & Record<string, JsonValue>;
  /** Time travel: replay from this checkpoint of the thread. */
  checkpoint?: { checkpoint_id: string };
  metadata?: Record<string, JsonValue>;
  /** SSE frame families for streaming runs. Default `["values", "updates"]`. */
  stream_mode?: Array<
    'values' | 'updates' | 'messages' | 'metadata' | 'error' | 'end' | (string & {})
  >;
  multitask_strategy?: 'enqueue' | 'reject';
  /** Run through a named assistant (must bind the thread's graph). */
  assistant_id?: string;
  [key: string]: unknown;
}

/** Background-run record returned by `POST /threads/{id}/runs`. */
export interface RunRecord {
  run_id: string;
  thread_id: string;
  status: string;
  [key: string]: unknown;
}

/** Terminal result of a blocking run. */
export interface RunTerminal {
  status: 'success' | 'interrupted' | 'error' | (string & {});
  output?: Record<string, JsonValue>;
  interrupt?: JsonValue;
  error?: string;
  checkpoint_id?: string;
  state?: Record<string, JsonValue>;
  run_id?: string;
  thread_id?: string;
  [key: string]: unknown;
}

/** `GET /runs/{run_id}` response. Terminal runs carry output/error/interrupt. */
export interface RunStatus {
  run_id: string;
  thread_id: string;
  graph: string;
  attempt: number;
  status: 'pending' | 'running' | 'success' | 'interrupted' | 'error' | (string & {});
  output?: Record<string, JsonValue>;
  error?: string;
  interrupt?: JsonValue;
  [key: string]: unknown;
}

/** One parsed SSE frame from a streaming run. */
export interface StreamFrame {
  /** Frame family: `metadata`, `updates`, `values`, `messages`, `error`, `end`. */
  event: string;
  /** JSON-parsed payload, or the raw data string when not JSON. */
  data: any;
  /** Frame id: `{checkpoint_id}:{step}:{seq}` (seq is 1-based per run). */
  id?: string;
  /** Server-suggested reconnection delay (ms), when present. */
  retry?: number;
}

/** Assistant record: a named graph + config alias. */
export interface Assistant {
  assistant_id: string;
  name: string;
  graph: string;
  config?: Record<string, JsonValue> | null;
  metadata?: Record<string, JsonValue> | null;
  created_at?: string;
  [key: string]: unknown;
}

/** Cron record. */
export interface Cron {
  cron_id: string;
  graph: string;
  interval_secs?: number;
  cron_expr?: string;
  input?: Record<string, JsonValue> | null;
  metadata?: Record<string, JsonValue> | null;
  on_run_completed?: 'keep' | 'delete';
  runs_fired?: number;
  last_run_at?: string | null;
  [key: string]: unknown;
}

/** One KV-store item. */
export interface StoreItem {
  namespace: string;
  key: string;
  value: JsonValue;
  created_at: string;
  updated_at: string;
}

/** Options accepted by most read methods. */
export interface RequestOptions {
  signal?: AbortSignal;
}

/** Options for {@link AgentGraphClient.runWait}. */
export interface RunWaitOptions extends RequestOptions {
  /** Timeout override (ms) — raise for long-running graphs. */
  timeout?: number;
}

/** Options for {@link AgentGraphClient.runStream}. */
export interface RunStreamOptions {
  /** Resume position sent as the `Last-Event-ID` header. */
  lastEventId?: string | number;
  /** Convenience for `payload.stream_mode`. */
  streamMode?: string[];
  /** snake_case alias of `streamMode` (accepted; `streamMode` wins when both are set). */
  stream_mode?: string[];
  /** Timeout (ms) for establishing the stream; the open stream is not timed. */
  timeout?: number;
  /** Abort the stream at any time. */
  signal?: AbortSignal;
}

/** Options for {@link AgentGraphClient.createThread}. */
export interface CreateThreadOptions extends RequestOptions {
  metadata?: Record<string, JsonValue>;
  /** Explicit thread id (server-generated UUID when omitted). */
  threadId?: string;
}

/** Options for {@link AgentGraphClient.updateState}. */
export interface UpdateStateOptions extends RequestOptions {
  /** Attribute the update to this node. */
  asNode?: string;
  /** Override the next-node set. */
  nextNodes?: string[];
}

/** Options for {@link AgentGraphClient.history}. */
export interface HistoryOptions extends RequestOptions {
  limit?: number;
  /** Only return checkpoints before this checkpoint id. */
  before?: string;
}

/** Options for {@link AgentGraphClient.fork}. */
export interface ForkOptions extends RequestOptions {
  /** Explicit id for the fork (server-generated UUID when omitted). */
  newThreadId?: string;
  /** Copy only up to and including this checkpoint (mid-history fork). */
  checkpointId?: string;
}

/** Input for {@link AgentGraphClient.createAssistant}. */
export interface CreateAssistantInput {
  name: string;
  graph: string;
  config?: Record<string, JsonValue>;
  metadata?: Record<string, JsonValue>;
  assistantId?: string;
}

/** Input for {@link AgentGraphClient.createCron}. Exactly one schedule kind. */
export interface CreateCronInput {
  graph: string;
  /** Fixed interval in seconds (>= 1). */
  intervalSecs?: number;
  /** 5-field cron expression (UTC). */
  cronExpr?: string;
  input?: Record<string, JsonValue>;
  metadata?: Record<string, JsonValue>;
  /** `"delete"` turns the cron into a one-shot. */
  onRunCompleted?: 'keep' | 'delete';
}

export declare class AgentGraphClient {
  constructor(baseUrl: string, options?: AgentGraphClientOptions);

  /** The normalized server base URL. */
  readonly baseUrl: string;

  /** Liveness probe. */
  ok(): Promise<{ ok: boolean }>;

  /** Service metadata: version, checkpointer, store path, registered graphs. */
  info(): Promise<InfoResponse>;

  /** Create a thread bound to a registered graph. */
  createThread(graph: string, opts?: CreateThreadOptions): Promise<Thread>;

  /** Latest checkpoint of a thread. */
  getState(threadId: string, opts?: RequestOptions): Promise<ThreadState>;

  /** Write a new checkpoint (the `update_state` analog). */
  updateState(
    threadId: string,
    values: Record<string, JsonValue>,
    opts?: UpdateStateOptions,
  ): Promise<ThreadState>;

  /** Checkpoint history, newest first. */
  history(threadId: string, opts?: HistoryOptions): Promise<ThreadState[]>;

  /** Fork a thread's checkpoint history into a new thread (time travel). */
  fork(threadId: string, opts?: ForkOptions): Promise<ForkResult>;

  /** Start a background run; resolves with the run record (`run_id`). */
  run(threadId: string, payload?: RunPayload, opts?: RequestOptions): Promise<RunRecord>;

  /** Run to completion; resolves with the terminal JSON. */
  runWait(
    threadId: string,
    payload?: RunPayload,
    opts?: RunWaitOptions,
  ): Promise<RunTerminal>;

  /**
   * Run with SSE streaming. Yields `metadata` / `updates` / `values` /
   * `messages` / `error` / `end` frames.
   */
  runStream(
    threadId: string,
    payload?: RunPayload,
    options?: RunStreamOptions,
  ): AsyncGenerator<StreamFrame, void, void>;

  /** Poll a run's status. */
  runStatus(runId: string, opts?: RequestOptions): Promise<RunStatus>;

  /** Roll back a finished run's checkpoints. */
  deleteRun(threadId: string, runId: string, opts?: RequestOptions): Promise<unknown>;

  /** Create a named graph alias. */
  createAssistant(input: CreateAssistantInput, opts?: RequestOptions): Promise<Assistant>;

  /** List all assistants. */
  listAssistants(opts?: RequestOptions): Promise<Assistant[]>;

  /** Fetch one assistant by id. */
  getAssistant(assistantId: string, opts?: RequestOptions): Promise<Assistant>;

  /** Schedule recurring runs (interval or cron expression). */
  createCron(input: CreateCronInput, opts?: RequestOptions): Promise<Cron>;

  /** List crons with `runs_fired` / `last_run_at` bookkeeping. */
  listCrons(opts?: RequestOptions): Promise<Cron[]>;

  /** Delete a cron (`404` when unknown). */
  deleteCron(cronId: string, opts?: RequestOptions): Promise<unknown>;

  /** Fetch one KV item (`404` when absent). */
  kvGet(namespace: string, key: string, opts?: RequestOptions): Promise<StoreItem>;

  /** Upsert a JSON value in a namespace. */
  kvPut(
    namespace: string,
    key: string,
    value: JsonValue,
    opts?: RequestOptions,
  ): Promise<StoreItem>;

  /** Delete one KV item (`404` when absent). */
  kvDelete(namespace: string, key: string, opts?: RequestOptions): Promise<unknown>;

  /** List a namespace's items, sorted by key. */
  kvList(namespace: string, opts?: RequestOptions): Promise<StoreItem[]>;
}

/**
 * Incrementally parse an SSE byte stream into frames. Exported for reuse and
 * testing; `runStream` uses it internally.
 */
export declare function parseSseStream(
  body: ReadableStream<Uint8Array>,
  signal?: AbortSignal,
): AsyncGenerator<StreamFrame, void, void>;

export default AgentGraphClient;
