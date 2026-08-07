/**
 * @rusty-runtime/client — zero-dependency JS/TS client for rusty-server.
 *
 * ESM-only. Works in Node.js >= 18 (global `fetch`, `ReadableStream`,
 * `TextDecoder`) and in modern browsers. No runtime dependencies.
 *
 * The API mirrors the rusty-server HTTP surface (Agent-Protocol-compatible
 * subset): threads, runs (background / blocking / SSE-streamed), checkpoints,
 * fork & replay time travel, assistants, crons, and the cross-thread KV store.
 *
 * @module @rusty-runtime/client
 */

/**
 * Error thrown for any non-2xx response from the server, for network-level
 * failures, and (via the {@link RustyTimeoutError} subclass) for
 * client-side timeouts.
 */
export class RustyError extends Error {
  /**
   * @param {string} message Human-readable message.
   * @param {object} [init]
   * @param {number} [init.status] HTTP status code (0 when no response was received).
   * @param {unknown} [init.body] Parsed response body (JSON value or raw text).
   * @param {string} [init.url] Request URL.
   * @param {unknown} [init.cause] Underlying error, if any.
   */
  constructor(message, init = {}) {
    super(message, init.cause !== undefined ? { cause: init.cause } : undefined);
    this.name = 'RustyError';
    /** @type {number} HTTP status code, or 0 when no response was received. */
    this.status = init.status ?? 0;
    /** @type {unknown} Parsed response body (JSON value or raw text), if any. */
    this.body = init.body;
    /** @type {string|undefined} Request URL. */
    this.url = init.url;
  }

  /**
   * Build an RustyError from a non-ok fetch Response, consuming its body.
   * @param {Response} response
   * @param {string} url
   * @returns {Promise<RustyError>}
   */
  static async fromResponse(response, url) {
    let body;
    const text = await response.text().catch(() => '');
    if (text) {
      try {
        body = JSON.parse(text);
      } catch {
        body = text;
      }
    }
    // The server's error body is {"error": <kind>, "message": <detail>};
    // surface the human-readable message, falling back to the machine kind.
    const detail =
      body && typeof body === 'object' && body !== null
        ? String(
            /** @type {{message?: unknown, error?: unknown}} */ (body).message ??
              /** @type {{message?: unknown, error?: unknown}} */ (body).error ??
              response.statusText,
          )
        : typeof body === 'string' && body
          ? body
          : response.statusText;
    return new RustyError(
      `rusty-server request failed: ${response.status} ${detail}`.trim(),
      { status: response.status, body, url },
    );
  }
}

/**
 * Thrown when a request exceeds the client's (or per-call) timeout.
 * `status` is 0 and `body` is undefined.
 */
export class RustyTimeoutError extends RustyError {
  /**
   * @param {string} url
   * @param {number} timeoutMs
   */
  constructor(url, timeoutMs) {
    super(`rusty-server request timed out after ${timeoutMs} ms`, { status: 0, url });
    this.name = 'RustyTimeoutError';
    /** @type {number} */
    this.timeoutMs = timeoutMs;
  }
}

/**
 * Client for an rusty-server deployment.
 *
 * @example
 * import { RustyClient } from '@rusty-runtime/client';
 *
 * const client = new RustyClient('http://localhost:8100');
 * const { thread_id } = await client.createThread('pipeline');
 * const terminal = await client.runWait(thread_id, {});
 * console.log(terminal.status, terminal.output);
 */
export class RustyClient {
  #baseUrl;
  #apiKey;
  #timeout;
  #fetch;
  #extraHeaders;

  /**
   * @param {string} baseUrl Server base URL, e.g. `"http://localhost:8100"`.
   *   A trailing slash is stripped.
   * @param {object} [options]
   * @param {string} [options.apiKey] Static API key sent as the `X-Api-Key`
   *   header. Required only when the server is configured with one.
   * @param {number} [options.timeout=30000] Per-request timeout in
   *   milliseconds. For `runStream` the timeout covers establishing the
   *   stream (until response headers arrive), not the stream lifetime.
   *   Set to `0` to disable.
   * @param {typeof fetch} [options.fetch] Custom fetch implementation
   *   (defaults to the global `fetch`). Useful for tests and proxies.
   * @param {Record<string, string>} [options.headers] Extra headers sent on
   *   every request.
   */
  constructor(baseUrl, options = {}) {
    if (typeof baseUrl !== 'string' || baseUrl.length === 0) {
      throw new TypeError('RustyClient: baseUrl must be a non-empty string');
    }
    this.#baseUrl = baseUrl.replace(/\/+$/, '');
    this.#apiKey = options.apiKey;
    this.#timeout = options.timeout ?? 30_000;
    this.#fetch = options.fetch ?? globalThis.fetch?.bind(globalThis);
    if (typeof this.#fetch !== 'function') {
      throw new Error(
        'RustyClient: no fetch implementation available. ' +
          'Use Node.js >= 18, a modern browser, or pass { fetch }.',
      );
    }
    this.#extraHeaders = options.headers ?? {};
  }

  /** @returns {string} The normalized server base URL. */
  get baseUrl() {
    return this.#baseUrl;
  }

  /**
   * Perform an HTTP request and parse a JSON response.
   * @param {string} method
   * @param {string} path Path beginning with `/`.
   * @param {object} [opts]
   * @param {unknown} [opts.body] JSON-serializable request body.
   * @param {Record<string, string>} [opts.headers] Per-request headers.
   * @param {number} [opts.timeout] Per-request timeout override (ms).
   * @param {AbortSignal} [opts.signal] Caller-supplied abort signal.
   * @param {boolean} [opts.raw] Return the raw Response instead of parsed JSON.
   * @returns {Promise<any>}
   */
  async #request(method, path, opts = {}) {
    const url = this.#baseUrl + path;
    const controller = new AbortController();
    const timeoutMs = opts.timeout ?? this.#timeout;
    let timedOut = false;
    const timer =
      timeoutMs > 0
        ? setTimeout(() => {
            timedOut = true;
            controller.abort();
          }, timeoutMs)
        : null;
    const callerSignal = opts.signal;
    const onCallerAbort = () => controller.abort();
    if (callerSignal) {
      if (callerSignal.aborted) controller.abort();
      else callerSignal.addEventListener('abort', onCallerAbort, { once: true });
    }

    const headers = { ...this.#extraHeaders, ...(opts.headers ?? {}) };
    if (this.#apiKey != null) headers['x-api-key'] = this.#apiKey;
    if (opts.body !== undefined) headers['content-type'] = 'application/json';

    try {
      const response = await this.#fetch(url, {
        method,
        headers,
        body: opts.body !== undefined ? JSON.stringify(opts.body) : undefined,
        signal: controller.signal,
      });
      if (!response.ok) throw await RustyError.fromResponse(response, url);
      if (opts.raw) {
        // For raw (streaming) responses the timeout only covered connection
        // setup; hand the open stream to the caller.
        if (timer) clearTimeout(timer);
        if (callerSignal) callerSignal.removeEventListener('abort', onCallerAbort);
        return response;
      }
      if (response.status === 204) return null;
      const text = await response.text();
      return text ? JSON.parse(text) : null;
    } catch (err) {
      if (timedOut) throw new RustyTimeoutError(url, timeoutMs);
      if (err instanceof RustyError) throw err;
      if (err instanceof Error && err.name === 'AbortError') {
        throw new RustyError('rusty-server request aborted', {
          status: 0,
          url,
          cause: err,
        });
      }
      if (err instanceof TypeError) {
        // fetch rejects with TypeError for network-level failures (DNS,
        // refused connection, TLS) before any response exists.
        throw new RustyError(`rusty-server network error: ${err.message}`, {
          status: 0,
          url,
          cause: err,
        });
      }
      throw err;
    } finally {
      if (timer) clearTimeout(timer);
      if (callerSignal) callerSignal.removeEventListener('abort', onCallerAbort);
    }
  }

  // ------------------------------------------------------------------ meta

  /**
   * Liveness probe.
   * @returns {Promise<{ok: boolean}>}
   */
  async ok() {
    return this.#request('GET', '/ok');
  }

  /**
   * Service metadata: version, checkpointer kind, store path, and the
   * registered graphs with their state channels.
   * @returns {Promise<import('./index.d.ts').InfoResponse>}
   */
  async info() {
    return this.#request('GET', '/info');
  }

  // --------------------------------------------------------------- threads

  /**
   * Create a thread bound to a registered graph.
   * @param {string} graph Registered graph name.
   * @param {object} [opts]
   * @param {Record<string, unknown>} [opts.metadata] Free-form thread metadata.
   * @param {string} [opts.threadId] Explicit thread id (server-generated
   *   UUID when omitted). Re-using an existing id re-attaches to its
   *   persisted checkpoints.
   * @param {AbortSignal} [opts.signal]
   * @returns {Promise<import('./index.d.ts').Thread>}
   */
  async createThread(graph, opts = {}) {
    const body = { graph };
    if (opts.metadata !== undefined) body.metadata = opts.metadata;
    if (opts.threadId !== undefined) body.thread_id = opts.threadId;
    return this.#request('POST', '/threads', { body, signal: opts.signal });
  }

  /**
   * Latest checkpoint of a thread: `{ values, next, checkpoint }`.
   * @param {string} threadId
   * @param {object} [opts]
   * @param {AbortSignal} [opts.signal]
   * @returns {Promise<import('./index.d.ts').ThreadState>}
   */
  async getState(threadId, opts = {}) {
    return this.#request('GET', `/threads/${encodeURIComponent(threadId)}/state`, {
      signal: opts.signal,
    });
  }

  /**
   * Write a new checkpoint (the `update_state` analog).
   * @param {string} threadId
   * @param {Record<string, unknown>} values Channel values to write.
   * @param {object} [opts]
   * @param {string} [opts.asNode] Attribute the update to this node.
   * @param {string[]} [opts.nextNodes] Override the next-node set.
   * @param {AbortSignal} [opts.signal]
   * @returns {Promise<import('./index.d.ts').ThreadState>} The written checkpoint state.
   */
  async updateState(threadId, values, opts = {}) {
    const body = { values };
    if (opts.asNode !== undefined) body.as_node = opts.asNode;
    if (opts.nextNodes !== undefined) body.next_nodes = opts.nextNodes;
    return this.#request('POST', `/threads/${encodeURIComponent(threadId)}/state`, {
      body,
      signal: opts.signal,
    });
  }

  /**
   * Checkpoint history, newest first.
   * @param {string} threadId
   * @param {object} [opts]
   * @param {number} [opts.limit] Max checkpoints to return.
   * @param {string} [opts.before] Only return checkpoints before this checkpoint id.
   * @param {AbortSignal} [opts.signal]
   * @returns {Promise<import('./index.d.ts').ThreadState[]>}
   */
  async history(threadId, opts = {}) {
    const body = {};
    if (opts.limit !== undefined) body.limit = opts.limit;
    if (opts.before !== undefined) body.before = opts.before;
    return this.#request('POST', `/threads/${encodeURIComponent(threadId)}/history`, {
      body,
      signal: opts.signal,
    });
  }

  /**
   * Fork a thread's checkpoint history into a new thread (time travel).
   * @param {string} threadId Source thread id.
   * @param {object} [opts]
   * @param {string} [opts.newThreadId] Explicit id for the fork
   *   (server-generated UUID when omitted).
   * @param {string} [opts.checkpointId] Copy only up to and including this
   *   checkpoint (mid-history fork). Omit for a full-history fork.
   * @param {AbortSignal} [opts.signal]
   * @returns {Promise<import('./index.d.ts').ForkResult>}
   */
  async fork(threadId, opts = {}) {
    const body = {};
    if (opts.newThreadId !== undefined) body.new_thread_id = opts.newThreadId;
    if (opts.checkpointId !== undefined) body.checkpoint_id = opts.checkpointId;
    return this.#request('POST', `/threads/${encodeURIComponent(threadId)}/fork`, {
      body,
      signal: opts.signal,
    });
  }

  // ------------------------------------------------------------------ runs

  /**
   * Start a background run. Returns immediately with the run record.
   * @param {string} threadId
   * @param {import('./index.d.ts').RunPayload} [payload]
   * @param {object} [opts]
   * @param {AbortSignal} [opts.signal]
   * @returns {Promise<import('./index.d.ts').RunRecord>} Contains `run_id`.
   */
  async run(threadId, payload = {}, opts = {}) {
    return this.#request('POST', `/threads/${encodeURIComponent(threadId)}/runs`, {
      body: payload,
      signal: opts.signal,
    });
  }

  /**
   * Run to completion and return the terminal JSON
   * (`{status, output | interrupt, …}`).
   * @param {string} threadId
   * @param {import('./index.d.ts').RunPayload} [payload]
   * @param {object} [opts]
   * @param {number} [opts.timeout] Timeout override (ms). Defaults to the
   *   client timeout; raise it for long-running graphs.
   * @param {AbortSignal} [opts.signal]
   * @returns {Promise<import('./index.d.ts').RunTerminal>}
   */
  async runWait(threadId, payload = {}, opts = {}) {
    return this.#request('POST', `/threads/${encodeURIComponent(threadId)}/runs/wait`, {
      body: payload,
      timeout: opts.timeout,
      signal: opts.signal,
    });
  }

  /**
   * Run with SSE streaming. Yields one frame per server event
   * (`metadata`, `updates`, `values`, `messages`, `error`, `end`).
   *
   * @param {string} threadId
   * @param {import('./index.d.ts').RunPayload} [payload]
   * @param {object} [options]
   * @param {string|number} [options.lastEventId] Resume position: sent as the
   *   `Last-Event-ID` header; the server replays the run's event-log tail
   *   after this frame id before streaming live frames.
   * @param {string[]} [options.streamMode] Convenience for
   *   `payload.stream_mode` (e.g. `["updates", "values"]`).
   * @param {number} [options.timeout] Timeout (ms) for establishing the
   *   stream; the open stream itself is not timed.
   * @param {AbortSignal} [options.signal] Abort the stream at any time.
   * @returns {AsyncGenerator<import('./index.d.ts').StreamFrame, void, void>}
   *
   * @example
   * for await (const frame of client.runStream(threadId, { input })) {
   *   if (frame.event === 'end') console.log('done:', frame.data.status);
   * }
   */
  async *runStream(threadId, payload = {}, options = {}) {
    const body = { ...payload };
    const streamMode = options.streamMode ?? options.stream_mode;
    if (streamMode !== undefined && body.stream_mode === undefined) {
      body.stream_mode = streamMode;
    }
    const headers = { accept: 'text/event-stream' };
    if (options.lastEventId !== undefined && options.lastEventId !== null) {
      headers['last-event-id'] = String(options.lastEventId);
    }
    const response = await this.#request(
      'POST',
      `/threads/${encodeURIComponent(threadId)}/runs/stream`,
      { body, headers, raw: true, timeout: options.timeout, signal: options.signal },
    );
    if (!response.body) {
      throw new RustyError('rusty-server stream response has no body', {
        status: response.status,
        url: response.url,
      });
    }
    yield* parseSseStream(response.body, options.signal);
  }

  /**
   * Poll a run's status. Terminal runs also carry `output` / `error` /
   * `interrupt` fields.
   * @param {string} runId
   * @param {object} [opts]
   * @param {AbortSignal} [opts.signal]
   * @returns {Promise<import('./index.d.ts').RunStatus>}
   */
  async runStatus(runId, opts = {}) {
    return this.#request('GET', `/runs/${encodeURIComponent(runId)}`, { signal: opts.signal });
  }

  /**
   * Roll back a finished run: delete its checkpoints and re-anchor the thread
   * to the pre-run checkpoint. (`409` while the run is active.)
   * @param {string} threadId
   * @param {string} runId
   * @param {object} [opts]
   * @param {AbortSignal} [opts.signal]
   * @returns {Promise<unknown>}
   */
  async deleteRun(threadId, runId, opts = {}) {
    return this.#request(
      'DELETE',
      `/threads/${encodeURIComponent(threadId)}/runs/${encodeURIComponent(runId)}`,
      { signal: opts.signal },
    );
  }

  // ------------------------------------------------------------ assistants

  /**
   * Create a named graph alias (assistant).
   * @param {object} assistant
   * @param {string} assistant.name Human-readable name.
   * @param {string} assistant.graph Registered graph the assistant binds to.
   * @param {Record<string, unknown>} [assistant.config] Default run config
   *   (e.g. `{ recursion_limit: 25 }`).
   * @param {Record<string, unknown>} [assistant.metadata]
   * @param {string} [assistant.assistantId] Explicit id (UUID when omitted).
   * @param {object} [opts]
   * @param {AbortSignal} [opts.signal]
   * @returns {Promise<import('./index.d.ts').Assistant>}
   */
  async createAssistant(assistant, opts = {}) {
    const body = { name: assistant.name, graph: assistant.graph };
    if (assistant.config !== undefined) body.config = assistant.config;
    if (assistant.metadata !== undefined) body.metadata = assistant.metadata;
    if (assistant.assistantId !== undefined) body.assistant_id = assistant.assistantId;
    return this.#request('POST', '/assistants', { body, signal: opts.signal });
  }

  /**
   * List all assistants.
   * @param {object} [opts]
   * @param {AbortSignal} [opts.signal]
   * @returns {Promise<import('./index.d.ts').Assistant[]>}
   */
  async listAssistants(opts = {}) {
    return this.#request('GET', '/assistants', { signal: opts.signal });
  }

  /**
   * Fetch one assistant by id.
   * @param {string} assistantId
   * @param {object} [opts]
   * @param {AbortSignal} [opts.signal]
   * @returns {Promise<import('./index.d.ts').Assistant>}
   */
  async getAssistant(assistantId, opts = {}) {
    return this.#request('GET', `/assistants/${encodeURIComponent(assistantId)}`, {
      signal: opts.signal,
    });
  }

  // ----------------------------------------------------------------- crons

  /**
   * Schedule recurring runs. Exactly one schedule kind is required:
   * `intervalSecs` (fixed interval, >= 1 s) or `cronExpr` (5-field cron, UTC).
   * @param {object} cron
   * @param {string} cron.graph Registered graph to run.
   * @param {number} [cron.intervalSecs] Fixed interval in seconds.
   * @param {string} [cron.cronExpr] 5-field cron expression (UTC).
   * @param {Record<string, unknown>} [cron.input] Run input for fired runs.
   * @param {Record<string, unknown>} [cron.metadata]
   * @param {'keep'|'delete'} [cron.onRunCompleted] `"delete"` makes the cron
   *   a one-shot: it removes itself after its first fired run terminates.
   * @param {object} [opts]
   * @param {AbortSignal} [opts.signal]
   * @returns {Promise<import('./index.d.ts').Cron>}
   */
  async createCron(cron, opts = {}) {
    const body = { graph: cron.graph };
    if (cron.intervalSecs !== undefined) body.interval_secs = cron.intervalSecs;
    if (cron.cronExpr !== undefined) body.cron_expr = cron.cronExpr;
    if (cron.input !== undefined) body.input = cron.input;
    if (cron.metadata !== undefined) body.metadata = cron.metadata;
    if (cron.onRunCompleted !== undefined) body.on_run_completed = cron.onRunCompleted;
    return this.#request('POST', '/crons', { body, signal: opts.signal });
  }

  /**
   * List crons (with `runs_fired` / `last_run_at` bookkeeping).
   * @param {object} [opts]
   * @param {AbortSignal} [opts.signal]
   * @returns {Promise<import('./index.d.ts').Cron[]>}
   */
  async listCrons(opts = {}) {
    return this.#request('GET', '/crons', { signal: opts.signal });
  }

  /**
   * Delete a cron (`404` when unknown).
   * @param {string} cronId
   * @param {object} [opts]
   * @param {AbortSignal} [opts.signal]
   * @returns {Promise<unknown>}
   */
  async deleteCron(cronId, opts = {}) {
    return this.#request('DELETE', `/crons/${encodeURIComponent(cronId)}`, {
      signal: opts.signal,
    });
  }

  // ------------------------------------------------------------------- kv

  /**
   * Fetch one item from the cross-thread KV store (`404` when absent).
   * @param {string} namespace Namespace segment (`[A-Za-z0-9._-]`, 1–128 chars).
   * @param {string} key Key segment (same character rules).
   * @param {object} [opts]
   * @param {AbortSignal} [opts.signal]
   * @returns {Promise<import('./index.d.ts').StoreItem>}
   */
  async kvGet(namespace, key, opts = {}) {
    return this.#request(
      'GET',
      `/store/${encodeURIComponent(namespace)}/${encodeURIComponent(key)}`,
      { signal: opts.signal },
    );
  }

  /**
   * Upsert a JSON value in a namespace. `201` on create, `200` on replace
   * (the server's `created_at` is preserved on replace).
   * @param {string} namespace
   * @param {string} key
   * @param {unknown} value Any JSON-serializable value.
   * @param {object} [opts]
   * @param {AbortSignal} [opts.signal]
   * @returns {Promise<import('./index.d.ts').StoreItem>}
   */
  async kvPut(namespace, key, value, opts = {}) {
    return this.#request(
      'PUT',
      `/store/${encodeURIComponent(namespace)}/${encodeURIComponent(key)}`,
      { body: value, signal: opts.signal },
    );
  }

  /**
   * Delete one item (`404` when absent).
   * @param {string} namespace
   * @param {string} key
   * @param {object} [opts]
   * @param {AbortSignal} [opts.signal]
   * @returns {Promise<unknown>}
   */
  async kvDelete(namespace, key, opts = {}) {
    return this.#request(
      'DELETE',
      `/store/${encodeURIComponent(namespace)}/${encodeURIComponent(key)}`,
      { signal: opts.signal },
    );
  }

  /**
   * List a namespace's items, sorted by key (empty array for an unwritten
   * namespace).
   * @param {string} namespace
   * @param {object} [opts]
   * @param {AbortSignal} [opts.signal]
   * @returns {Promise<import('./index.d.ts').StoreItem[]>}
   */
  async kvList(namespace, opts = {}) {
    return this.#request('GET', `/store/${encodeURIComponent(namespace)}`, {
      signal: opts.signal,
    });
  }
}

/**
 * Incrementally parse an SSE byte stream into frames.
 *
 * Handles the full SSE field grammar: `event`, multi-line `data` (joined with
 * `\n`), `id`, `retry`, and `:` comment/keepalive lines; both `\n` and `\r\n`
 * line endings. Frame payloads are JSON-parsed when possible, otherwise
 * yielded as raw strings.
 *
 * @param {ReadableStream<Uint8Array>} body
 * @param {AbortSignal} [signal]
 * @returns {AsyncGenerator<import('./index.d.ts').StreamFrame, void, void>}
 */
export async function* parseSseStream(body, signal) {
  const reader = body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';
  let event = 'message';
  /** @type {string[]} */
  let dataLines = [];
  let id;
  let retry;

  const onAbort = () => reader.cancel().catch(() => {});
  if (signal) {
    if (signal.aborted) {
      reader.cancel().catch(() => {});
    } else {
      signal.addEventListener('abort', onAbort, { once: true });
    }
  }

  /** Take one dispatched frame, if the event buffer holds one. */
  const dispatch = () => {
    if (dataLines.length === 0) {
      // Comment-only or empty block: reset and emit nothing.
      event = 'message';
      id = undefined;
      retry = undefined;
      return undefined;
    }
    const raw = dataLines.join('\n');
    /** @type {unknown} */
    let data = raw;
    try {
      data = JSON.parse(raw);
    } catch {
      /* not JSON — yield the raw string */
    }
    const frame = { event, data };
    if (id !== undefined) frame.id = id;
    if (retry !== undefined) frame.retry = retry;
    event = 'message';
    dataLines = [];
    id = undefined;
    retry = undefined;
    return frame;
  };

  /** @type {Array<import('./index.d.ts').StreamFrame|undefined>} */
  const pending = [];
  const processLine = (line) => {
    if (line.endsWith('\r')) line = line.slice(0, -1);
    if (line === '') {
      pending.push(dispatch());
      return;
    }
    if (line.startsWith(':')) return; // comment / keepalive
    const colon = line.indexOf(':');
    const field = colon === -1 ? line : line.slice(0, colon);
    let value = colon === -1 ? '' : line.slice(colon + 1);
    if (value.startsWith(' ')) value = value.slice(1);
    switch (field) {
      case 'event':
        event = value;
        break;
      case 'data':
        dataLines.push(value);
        break;
      case 'id':
        if (!value.includes(String.fromCharCode(0))) id = value; // NUL in id kills the field per spec
        break;
      case 'retry': {
        const n = Number.parseInt(value, 10);
        if (Number.isFinite(n)) retry = n;
        break;
      }
      default:
        break; // unknown fields are ignored per spec
    }
  };

  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      let nl;
      while ((nl = buffer.indexOf('\n')) !== -1) {
        processLine(buffer.slice(0, nl));
        buffer = buffer.slice(nl + 1);
      }
      while (pending.length > 0) {
        const frame = pending.shift();
        if (frame) yield frame;
      }
    }
    buffer += decoder.decode();
    if (buffer.length > 0) processLine(buffer);
    pending.push(dispatch());
    while (pending.length > 0) {
      const frame = pending.shift();
      if (frame) yield frame;
    }
  } finally {
    if (signal) signal.removeEventListener('abort', onAbort);
    // A consumer that breaks out of `for await` early lands here with the
    // HTTP body still streaming; cancel it so the connection is torn down
    // instead of draining until the server finishes the run. (No-op after
    // a normal EOF or an abort, which already cancels via onAbort.)
    await reader.cancel().catch(() => {});
    reader.releaseLock();
  }
}

export default RustyClient;
