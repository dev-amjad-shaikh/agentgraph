# Review Reports Index

Seven parallel staff-level reviews of the agentgraph workspace (core crate, server, worker, OTel, SDKs, Studio, docs). Each reviewer used a slightly different severity scale; counts below are normalized:

- **must-fix** = must-fix / High / Major / Critical
- **should-fix** = should-fix / Medium / Minor
- **nit** = nit / Low

| Report | Scope | Must-fix | Should-fix | Nit |
|--------|-------|---------:|-----------:|----:|
| [core-state-graph.md](core-state-graph.md) | `agentgraph/src/{state,node,graph,error}.rs` | 2 | 7 | 16 |
| [core-executor.md](core-executor.md) | `agentgraph/src/{executor,checkpoint,checkpoint_postgres}.rs` | 2 | 6 | 9 |
| [core-llm-tools.md](core-llm-tools.md) | `agentgraph/src/{llm,tool,react,mcp,remote,wasm_node}.rs` + examples | 3 | 7 | 11 |
| [server.md](server.md) | `agentgraph-server/src/` (all 10 files) + `server_demo.rs` | 4 | 8 | 12 |
| [worker-otel.md](worker-otel.md) | `agentgraph-worker/`, `agentgraph-otel/` (src, examples, tests) | 3 | 8 | 6 |
| [sdks-studio.md](sdks-studio.md) | Python + TypeScript SDKs, Studio (`index.html`, `serve.py`) | 0 | 5 | 13 (6 Low + 7 Nit) |
| [documentation.md](documentation.md) | All prose: READMEs, CHANGELOG, CONTRIBUTING, `docs/*.md` | 2 | 7 | 7 |
| **Total** | | **16** | **48** | **74** |

## Headline must-fixes

- **core-state-graph** — nondeterministic fan-in merge order under the executor's `JoinSet` completion order; duplicate edges pass `compile()` and double-activate targets.
- **core-executor** — `Interrupt` silently loses parallel sibling nodes (not re-run after resume); `RunConfig::default()` yields `max_steps == 0` footgun; JSON-file latest-pointer race; Postgres `LIST_SQL` missing tie-break; `get_by_id` not overridden.
- **core-llm-tools** — UTF-8 split-chunk corruption in SSE feed (`llm.rs:714`); API key leaked via derived `Debug`; `unwrap_or(0.0)` silent-zero fallback survives in both examples; uncapped guest-controlled `out_len` host allocation in `wasm_node.rs`; `GraphEvent::Token` unreachable through `create_react_agent`.
- **server** — cross-tenant assistant resolution via unvalidated `assistant_id`; memory-only thread records break durability across restarts; cron `interval_secs` overflow DoS; rollback endpoint bypasses the `Checkpointer` trait; unguarded execute task can wedge a thread forever.
- **worker-otel** — failed OTel re-init mutates global tracer provider and leaks exporter; registry-wide `EnvFilter` also gates OTLP export; `span.enter()` held across `.await` in worker dispatch.
- **sdks-studio** — no High-severity findings; top mediums: TS error surfaces `body.error` kind instead of `body.message`; Python stream timeouts escape untranslated; SSE stream leak on early consumer break; Python SSE parser has zero unit tests; `requires-python >=3.8` vs 3.9-only `resp.status`.
- **documentation** — `agentgraph/CONTRIBUTING.md` frozen at scaffold time (lists shipped code as `todo!()`); `agentgraph-worker/` has no README.
