//! End-to-end test: a graph mixing **local** nodes with **remote** nodes
//! (`RemoteNode` → in-process worker), run by the real `Executor` — including
//! an interrupt → resume round trip across the wire.

use std::sync::Arc;

use agentgraph::prelude::*;
use agentgraph::remote::RemoteNode;
use agentgraph_worker::{router, WorkerRegistry};
use serde_json::json;

/// Start a worker with the test handlers on an ephemeral port; returns the
/// base URL to point `RemoteNode`s at.
async fn start_worker() -> String {
    let registry = WorkerRegistry::new()
        // A normal remote handler: doubles `state.n` into `doubled`.
        .with("doubler", |ctx: NodeContext| async move {
            let n = ctx
                .state()
                .get("n")
                .and_then(|v| v.as_i64())
                .expect("state channel `n` must be set by an upstream node");
            Ok(NodeOutput::update("doubled", json!(n * 2)))
        })
        // A HITL remote handler: interrupts until the run is resumed.
        .with("approval_gate", |ctx: NodeContext| async move {
            match ctx.resume_value() {
                Some(value) => Ok(NodeOutput::update("approved", value.clone())),
                None => Err(ctx.interrupt(json!({
                    "question": "approve deployment?",
                    "node": "approval_gate",
                }))),
            }
        });

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router(registry))
            .await
            .expect("worker server");
    });
    format!("http://{addr}")
}

/// The mixed local/remote graph used by both tests:
///
/// ```text
/// seed (local) -> double (REMOTE) -> gate (REMOTE, HITL) -> final (local)
/// ```
fn build_graph(worker_url: &str) -> Graph {
    let mut builder = GraphBuilder::new();

    builder.add_node("seed", |_ctx: NodeContext| async {
        Ok(NodeOutput::update("n", json!(21)))
    });
    builder.add_node("double", RemoteNode::new("doubler", worker_url));
    builder.add_node("gate", RemoteNode::new("approval_gate", worker_url));
    builder.add_node("final", |ctx: NodeContext| async move {
        let approved = ctx.state().get("approved").cloned().unwrap_or(json!(false));
        Ok(NodeOutput::update(
            "log",
            json!(format!("finished with approved={approved}")),
        ))
    });

    builder.add_edge("seed", "double");
    builder.add_edge("double", "gate");
    builder.add_edge("gate", "final");
    builder.set_entry_point("seed");
    builder.compile().expect("graph compiles")
}

fn state_spec() -> StateSpec {
    StateSpec::new()
        .channel("n", Reducer::Overwrite)
        .channel("doubled", Reducer::Overwrite)
        .channel("approved", Reducer::Overwrite)
        .channel("log", Reducer::Append)
}

#[tokio::test]
async fn remote_node_executes_end_to_end_alongside_local_nodes() {
    let worker_url = start_worker().await;

    // Liveness probe: the worker reports its protocol + handlers.
    let ok: serde_json::Value = http_get_json(&format!("{worker_url}/ok")).await;
    assert_eq!(ok["status"], json!("ok"));
    assert_eq!(
        ok["protocol_version"],
        json!(agentgraph::remote::PROTOCOL_VERSION)
    );
    assert_eq!(ok["nodes"], json!(["approval_gate", "doubler"]));

    // Graph that skips the HITL gate: seed -> double -> final.
    let mut builder = GraphBuilder::new();
    builder.add_node("seed", |_ctx: NodeContext| async {
        Ok(NodeOutput::update("n", json!(21)))
    });
    builder.add_node("double", RemoteNode::new("doubler", &worker_url));
    builder.add_node("final", |ctx: NodeContext| async move {
        let doubled = ctx.state().get("doubled").cloned().unwrap_or(json!(0));
        Ok(NodeOutput::update(
            "log",
            json!(format!("doubled={doubled}")),
        ))
    });
    builder.add_edge("seed", "double");
    builder.add_edge("double", "final");
    builder.set_entry_point("seed");
    let graph = builder.compile().unwrap();

    let outcome = Executor::new()
        .run(
            &graph,
            &state_spec(),
            State::new(),
            RunConfig::new("t-plain"),
        )
        .await
        .expect("run succeeds");

    match outcome {
        ExecutionOutcome::Done(state) => {
            assert_eq!(state.get("n"), Some(&json!(21)));
            // Written by the REMOTE node, merged by the local executor.
            assert_eq!(state.get("doubled"), Some(&json!(42)));
            assert_eq!(state.get("log"), Some(&json!(["doubled=42"])));
        }
        other => panic!("expected Done, got {other:?}"),
    }
}

#[tokio::test]
async fn interrupt_resume_round_trip_across_the_wire() {
    let worker_url = start_worker().await;
    let graph = build_graph(&worker_url);
    let spec = state_spec();
    let checkpointer = Arc::new(InMemoryCheckpointer::new());

    // ---- Run 1: seed -> double -> gate INTERRUPTS (remotely) ----
    let executor = Executor::with_checkpointer(checkpointer.clone());
    let outcome = executor
        .run(&graph, &spec, State::new(), RunConfig::new("t-hitl"))
        .await
        .expect("run succeeds (suspends)");

    let checkpoint_id = match outcome {
        ExecutionOutcome::Interrupted {
            value,
            state,
            checkpoint_id,
        } => {
            // The interrupt payload came back across the wire from the worker.
            assert_eq!(
                value,
                json!({"question": "approve deployment?", "node": "approval_gate"})
            );
            // Work up to the suspension point survived: the remote `double`
            // node's update was merged before the gate ran.
            assert_eq!(state.get("n"), Some(&json!(21)));
            assert_eq!(state.get("doubled"), Some(&json!(42)));
            assert!(!checkpoint_id.is_empty());
            checkpoint_id
        }
        other => panic!("expected Interrupted, got {other:?}"),
    };

    // The suspension point was persisted and schedules the gate node.
    let stored = checkpointer
        .get_latest("t-hitl")
        .await
        .unwrap()
        .expect("checkpoint exists");
    assert_eq!(stored.id, checkpoint_id);
    assert_eq!(stored.next_nodes, vec!["gate".to_string()]);

    // ---- Run 2: resume; the value crosses the wire into the gate ----
    let executor = Executor::with_checkpointer(checkpointer);
    let outcome = executor
        .run(
            &graph,
            &spec,
            State::new(),
            RunConfig::new("t-hitl").with_resume(json!(true)),
        )
        .await
        .expect("resume succeeds");

    match outcome {
        ExecutionOutcome::Done(state) => {
            // The gate re-ran remotely with config.resume = true and wrote
            // `approved`; `final` then ran locally.
            assert_eq!(state.get("approved"), Some(&json!(true)));
            assert_eq!(state.get("doubled"), Some(&json!(42)));
            assert_eq!(
                state.get("log"),
                Some(&json!(["finished with approved=true"]))
            );
        }
        other => panic!("expected Done after resume, got {other:?}"),
    }
}

/// Minimal GET helper (the worker crate intentionally has no HTTP client
/// dependency): one raw HTTP/1.1 request over a tokio TCP stream.
async fn http_get_json(url: &str) -> serde_json::Value {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let authority = url
        .strip_prefix("http://")
        .expect("test worker URLs are plain http");
    let (host, path) = match authority.split_once('/') {
        Some((host, path)) => (host, format!("/{path}")),
        None => (authority, "/".to_string()),
    };
    let mut stream = tokio::net::TcpStream::connect(host)
        .await
        .expect("connect to worker");
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nhost: {host}\r\nconnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await.unwrap();
    let raw = String::from_utf8(raw).unwrap();
    let body = raw.split("\r\n\r\n").nth(1).expect("HTTP body present");
    serde_json::from_str(body).expect("valid JSON from /ok")
}
