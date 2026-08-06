# Review: SDKs (Python + TypeScript) & Studio — `review/sdks-studio.md`

**Reviewer:** Reviewer_SDKs_Studio (read-only)
**Scope:** `sdks/python/agentgraph_client/*.py`, `sdks/python/tests/`, `sdks/typescript/src/index.js` + `index.d.ts` + `test/`, `studio/index.html` + `serve.py`, plus READMEs and `pyproject.toml`/`package.json` for docs drift. Server sources (`error.rs`, `routes.rs`) consulted only to verify wire-format claims.

## Findings

| # | Sev | Location | Category | Finding |
|---|---|---|---|---|
| F1 | **Medium** | `sdks/typescript/src/index.js:55-60` | Correctness | `AgentGraphError.fromResponse` builds the detail from `body.error`, but the server's error body is `{"error": <kind>, "message": <detail>}` (`agentgraph-server/src/error.rs:21`). Every error message surfaces the machine kind (`404 not_found`) instead of the human detail (`404 thread … not found`). The Studio's `ApiError` correctly uses `body.message`. Prefer `body.message` (optionally prefixed with the kind). |
| F2 | **Medium** | `sdks/python/agentgraph_client/client.py:131, 156, 578` | Correctness | Read-time socket timeouts escape untranslated. `_open` wraps `URLError`, but `resp.read()` in `_request` and the `for raw in resp` loop in `_iter_sse` raise raw `socket.timeout`/`TimeoutError` on a stalled stream — exactly the `run_stream`-on-slow-graph case the timeout param exists for. This contradicts the module docstring ("raised for any non-2xx response or transport failure"). Wrap body/stream reads and re-raise as `AgentGraphError(status=None)`. |
| F3 | **Medium** | `sdks/typescript/src/index.js:709-712` | Idiom / resource cleanup | `parseSseStream`'s `finally` calls `reader.releaseLock()` but never `reader.cancel()`. A consumer that `break`s out of `for await (… runStream(…))` early leaves the HTTP response body streaming until the server finishes the run. Add `await reader.cancel()` (guarded) in `finally` for the non-abort exit path. |
| F4 | **Medium** | `sdks/python/tests/test_client.py` (whole file) | Test coverage | The hand-rolled Python SSE parser `_iter_sse` has **zero unit tests**: CRLF, multi-line `data:`, comment/keepalive, EOF flush, and id-only blocks are never exercised — the e2e server always emits well-formed LF frames. The JS side has exactly these unit tests (`client.test.js:414-461`); port them. |
| F5 | **Medium** | `sdks/python/pyproject.toml:10` + `client.py:166` | Correctness / compat | `requires-python = ">=3.8"`, but `_request` uses `resp.status` on `urllib.response.addinfourl`, an attribute added in Python 3.9. On 3.8 the JSON-decode-error branch raises `AttributeError` instead of `AgentGraphError`. Narrow path, but either bump `requires-python` to `>=3.9` or use `getattr(resp, "status", None)` / `resp.getcode()`. |
| F6 | Low | `studio/index.html:769-786` | Correctness / robustness | Studio's `parseSseFrame` splits frames on `"\n\n"` only and never strips trailing `\r`. A CRLF-emitting SSE source would never split frames, and `event` values would retain `\r` (`frame.event === "end"` fails). Both SDK parsers are CRLF-tolerant; the Studio isn't. Works against axum's LF-only SSE today, so it's latent — but it's the third copy of this parser and the weakest. Also: leftover bytes in `buf` after `done` are discarded with no EOF flush (both SDKs flush). |
| F7 | Low | `studio/serve.py:4-7` | Docs | Stale premise: the docstring says "agentgraph-server sends no CORS headers" and presents the proxy as required. The server layers `CorsLayer::permissive()` (`agentgraph-server/src/routes.rs:142`), `docs/studio.md:74-91` and the Studio's own error text (`index.html:404`) both say v0.3+ sends permissive CORS, and `docs/studio.md:9` calls serve.py "optional". Update the docstring to match. |
| F8 | Low | `sdks/typescript/src/index.js:203` | Idiom / API consistency | Network-level failures (`fetch` `TypeError`) are rethrown raw, not wrapped in `AgentGraphError` — despite the class JSDoc ("thrown … for network-level failures") and the Python SDK wrapping `URLError` uniformly. Either wrap or narrow the doc. |
| F9 | Low | `sdks/typescript/src/index.js:382` vs `index.d.ts:208-217` | Docs / d.ts drift | Implementation accepts `options.stream_mode` (snake_case alias) in `runStream`; `RunStreamOptions` declares only `streamMode`, so TS users passing the accepted alias get a type error. Also `stream_mode?: Array<… \| string>` and `status: 'success' \| … \| string` unions collapse to `string` — pick one style. |
| F10 | Low | `sdks/python/agentgraph_client/client.py:131` | Correctness / semantics | `timeout or self.timeout` treats `timeout=0` as "use default"; the JS client documents `0` = disable. Python has no way to disable a timeout and the docstring doesn't say so. Use `self.timeout if timeout is None else timeout`. |
| F11 | Low | `sdks/python/agentgraph_client/client.py:582-588, 604-609` | Correctness / spec | `_iter_sse` dispatches a frame when a block has `id:`/`event:` but **no** `data:` lines (yields `data=""`). Per the SSE spec, such blocks update the last-event-id but must not dispatch. The JS parser correctly requires `dataLines.length > 0`. Harmless against this server (every frame has data) but a spec deviation. |
| F12 | Nit | `sdks/python/agentgraph_client/client.py:618` | Idiom | `except (json.JSONDecodeError, ValueError)` — `JSONDecodeError` is a `ValueError` subclass; redundant tuple. |
| F13 | Nit | `sdks/python/agentgraph_client/client.py:103-105` | Idiom | `Content-Type: application/json` is sent on bodiless GET/DELETE requests (JS sets it only when a body exists). Harmless noise. |
| F14 | Nit | `sdks/typescript/test/index.js` | Dead-ish code | One-line re-export of `client.test.js`. Under `node --test test/` default globs only `*.test.js` runs, so this file exists solely to enable plain `node test/index.js`. Keep if intentional, otherwise delete. |
| F15 | Nit | repo root `.gitignore` | Housekeeping | No `__pycache__/` or `.tmp-e2e` entries; `sdks/python/**/__pycache__/*.pyc` artifacts are present in the tree. |
| F16 | Nit | `studio/index.html:646-664` | Correctness / minor | `pollRun` uses `setInterval` with an async callback — overlapping polls if a request exceeds 800 ms. Use recursive `setTimeout`. |
| F17 | Nit (security-note) | `studio/index.html:361, 449` | Security hygiene | API key persisted in plaintext `localStorage` (`ags:conn`). Acceptable for a local debug UI, but worth a one-line warning in the UI or docs. |
| F18 | Nit | `studio/serve.py:53, 33-40` | Robustness | `int(self.headers.get("Content-Length") or 0)` 500s on a malformed header; `/api` (no trailing slash) and `/?query` miss the proxy/index rewrite. Dev-tool severity. |

## AI-generated tells — verdict: **Clean (no material tells)**

- **Over-commenting:** No. Docstrings are dense but carry semantics (endpoint, resume conventions, time-travel behavior), not line narration. `# pragma: no cover - cosmetic`, `# malformed line; ignore`, `# Last-resort cleanup` read as human judgment calls.
- **Marketing adjectives:** Only mild flavor in the Python README "Philosophy" section ("no `pip install` of anything else, ever", "fancy async stack") — opinionated but substantive, acceptable for a philosophy note.
- **Uniform repetitive bodies hiding a missing abstraction:** No. The Python method bodies are uniform, but `_run_body` was genuinely extracted for the three run variants and `_request`/`_open` carry all shared logic; the per-endpoint dict-building differs meaningfully. JS has the equivalent `#request` core.
- **Leftover TODOs / dead code:** None found (`TODO|FIXME|XXX|HACK` grep empty in both trees). Only F14's redundant `test/index.js` approaches dead code.
- Idiosyncratic, clearly deliberate details (Studio fallback path for pre-time-travel servers with JSON-vs-plain 404 discrimination, cron tests using `interval_secs=3600` "must not fire during the suite", `id '-:0:1'` fixtures matching README examples) argue for curated authorship.

## Per-area verdicts

- **Python SDK (`client.py`, `__init__.py`):** PASS with fixes. Idiomatic stdlib code, correct generator cleanup (`finally: resp.close()`), EOF flush present, `_q()` quoting consistent, type hints uniform. Fix F2 (timeout translation), F5 (3.8 compat), F10 (falsy timeout).
- **Python tests:** PASS, gap noted. Excellent live-server e2e hygiene (port probe, build fallback, log-tail on early exit, `tearDownClass` robustness, documented interrupt skip). The hole is F4: no parser unit tests.
- **TypeScript SDK (`index.js`):** PASS with fixes. SSE parser is the most spec-conformant of the three copies (NUL-in-id, `retry`, CRLF, EOF flush). AbortController wiring is careful (caller-signal listener removal in `finally`, raw-handoff clears the timer). Fix F1 (error detail), F3 (cancel on early break), F8 (wrap network errors).
- **`index.d.ts`:** PASS. Signatures match the implementation method-for-method; only drift is F9 (`stream_mode` alias, string-collapsed unions).
- **TypeScript tests:** PASS. Good mix of e2e and no-I/O unit tests; the deterministic fake-fetch timeout test and CRLF/multi-line SSE fixtures are exactly right. Missing: early-break cancellation test (would catch F3) and EOF-flush fixture.
- **Studio (`index.html`):** PASS with caveats. Consistent `escapeHtml` hygiene (no XSS sinks found), correct `{error, message}` handling, thoughtful old-server fallbacks. Caveats: F6 (weakest SSE parser copy), F16, F17.
- **`serve.py`:** PASS. Clean stdlib proxy (hop-by-hop header filtering, SSE flush, broken-pipe handling). Fix the stale docstring (F7).
- **Docs (READMEs):** PASS. API tables match implementations on both sides; cross-SDK examples agree (`-:0:1` ids, fork/replay flow). TS README's CORS claim verified true against `routes.rs:142`.

**Overall verdict:** PASS — ship after the 5 medium fixes; none are architectural.

## Top 5 must-fixes

1. **F1** — JS `fromResponse` reports `body.error` (`not_found`) instead of `body.message`; every user-facing error loses its detail (`src/index.js:55-60`).
2. **F2** — Python stream/body read timeouts escape as raw `socket.timeout`, breaking the "all transport failures → `AgentGraphError`" contract, worst in `run_stream` (`client.py:131,156,578`).
3. **F3** — `parseSseStream` leaks the HTTP stream on consumer early-break; add `reader.cancel()` in `finally` (`src/index.js:709-712`).
4. **F4** — Python SSE parser has no unit tests for CRLF / multi-line data / comments / EOF flush; port the JS fixtures.
5. **F5** — `pyproject.toml` promises Python 3.8 but `resp.status` needs 3.9+; bump `requires-python` or use `getcode()`.

---

**Final summary for orchestrator:**

- **Severity counts:** 0 High · 5 Medium (F1–F5) · 6 Low (F6–F11) · 7 Nit (F12–F18)
- **Top 5 must-fixes:** (1) TS `fromResponse` uses `body.error` kind instead of `body.message`; (2) Python read/stream timeouts not wrapped in `AgentGraphError`; (3) TS `parseSseStream` doesn't `reader.cancel()` on early consumer break (stream leak); (4) Python SSE parser has zero unit tests (CRLF/multi-line/EOF flush untested — JS has them); (5) `requires-python >=3.8` vs `addinfourl.status` (3.9+).
- **Verdicts:** No AI-generation tells (clean, curated code; no TODOs, no dead code, informative comments). All areas PASS with fixes; nothing architectural.
