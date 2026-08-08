//! The effect journal: the Flight Recorder's append-only run evidence, plus
//! the determinism seams ([`Clock`], [`RngSource`]) that let a recorded run
//! be re-driven exactly.
//!
//! A [`Journal`] is created per run (one `Executor::run` call) and receives
//! every effect the executor observes: super-step boundaries, node
//! inputs/outputs, interrupts, resumes, routing decisions, and checkpoint
//! writes. Nodes, models, and tools record their own calls (model, tool,
//! remote, WASM) through the same journal when handed a clone — the
//! [`EventDraft`] builder is the public recording API.
//!
//! Two properties make the journal evidence rather than logs:
//!
//! - **Causal order.** Every event carries a monotonic `seq` and the id of
//!   its causal parent, assigned by the journal at record time. The journal
//!   — not wall time — defines the run's order.
//! - **Tamper-evident head.** Recording chains a SHA-256 head hash over the
//!   canonical serialization of each event. Checkpoints stamp the head as a
//!   [`crate::record::JournalRef`], binding state and evidence together.
//!
//! # Determinism seams
//!
//! The executor sources every timestamp and every random id through
//! [`Clock`] and [`RngSource`], configured per run via
//! `RunConfig::with_clock` / `RunConfig::with_rng`. Defaults
//! ([`Clock::System`], [`RngSource::System`]) are byte-identical to the
//! pre-R0.5 behavior. A [`Clock::Logical`] + [`RngSource::Seeded`] pair
//! makes event timestamps, latencies, event ids, run ids, and checkpoint ids
//! reproducible — the precondition for exact replay.
//!
//! The in-memory journal is the v1 store; [`JournalSnapshot`] is its
//! serde-complete export form (events plus the artifact payloads they
//! reference), the unit portable replay fixtures are built from.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::llm::Usage;
use crate::record::{
    sha256_hex, ArtifactRef, Effect, EffectReceipt, EventStatus, PayloadRef, RunEvent,
    RunEventKind, INLINE_PAYLOAD_MAX_BYTES,
};

/// The `NodeConfig::extra` key under which the executor passes the
/// node-input journal event id of the current invocation.
///
/// Node code that records its own effects (model calls, tool calls) reads
/// this key to parent them correctly: a model call's causal parent is the
/// invocation that made it. Reserved by the runtime — applications must not
/// set it themselves, and it travels in `NodeConfig::extra` over the worker
/// wire protocol so remote nodes can parent their evidence the same way.
pub const PARENT_EVENT_KEY: &str = "rusty.parent_event";

/// The run's time source. All executor timestamps — event `recorded_at`,
/// measured latencies, checkpoint `created_at` — are read through it.
///
/// `Clone` is cheap (a shared handle), so the same clock can serve the
/// executor, every spawned node task, and an attached journal.
#[derive(Debug, Clone, Default)]
pub enum Clock {
    /// Wall-clock UTC (`chrono::Utc::now`). The default; identical to
    /// pre-R0.5 behavior.
    #[default]
    System,

    /// A deterministic logical clock. Starts at `start_ms` (epoch millis)
    /// and advances by `tick_ms` on **every read**. Two runs with the same
    /// parameters observe the same timestamp sequence, which is what makes
    /// recorded evidence reproducible.
    ///
    /// The tick-on-read semantics mean timestamps reflect how many clock
    /// reads happened, not how much wall time passed — that is the point:
    /// the journal's logical order is defined by `seq`, and timestamps
    /// become a deterministic attribute rather than a second, unreliable
    /// clock.
    Logical(LogicalClock),
}

impl Clock {
    /// A logical clock starting at `start_ms` (epoch millis), advancing
    /// `tick_ms` per read.
    pub fn logical(start_ms: u64, tick_ms: u64) -> Self {
        Clock::Logical(LogicalClock {
            state: Arc::new(Mutex::new(LogicalClockState {
                now_ms: start_ms,
                tick_ms,
            })),
        })
    }

    /// The current time. Under [`Clock::Logical`], every call advances the
    /// clock by its tick.
    pub fn now(&self) -> DateTime<Utc> {
        match self {
            Clock::System => Utc::now(),
            Clock::Logical(logical) => {
                let mut state = logical.lock();
                let millis = state.now_ms;
                state.now_ms = state.now_ms.saturating_add(state.tick_ms);
                // Epoch millis are always representable; the logical clock
                // is the only caller-controlled input and saturates.
                DateTime::from_timestamp_millis(millis.min(i64::MAX as u64) as i64)
                    .unwrap_or(DateTime::UNIX_EPOCH)
            }
        }
    }

    /// The current logical clock value in milliseconds (the value stamped
    /// into [`crate::record::CheckpointHeader::logical_clock`]). Reads
    /// through the same seam as [`Clock::now`], so logical clocks tick.
    pub fn now_ms(&self) -> u64 {
        self.now().timestamp_millis().max(0) as u64
    }
}

/// Shared state of a [`Clock::Logical`] handle.
#[derive(Debug, Clone)]
pub struct LogicalClock {
    state: Arc<Mutex<LogicalClockState>>,
}

#[derive(Debug)]
struct LogicalClockState {
    now_ms: u64,
    tick_ms: u64,
}

impl LogicalClock {
    fn lock(&self) -> MutexGuard<'_, LogicalClockState> {
        // Poison means a recording path panicked mid-read; the clock value
        // itself is plain data and stays coherent, so recovering is safe.
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The run's randomness source. Checkpoint ids and run ids are minted
/// through it — the only entropy the executor consumes.
#[derive(Debug, Clone, Default)]
pub enum RngSource {
    /// Operating-system entropy (`uuid::Uuid::new_v4`). The default;
    /// identical to pre-R0.5 behavior.
    #[default]
    System,

    /// A deterministic ChaCha8 stream seeded from a `u64`. Two runs with the
    /// same seed mint the same id sequence, so journals and checkpoints of a
    /// re-driven run reproduce the original's ids.
    Seeded(SeededRng),
}

impl RngSource {
    /// A seeded RNG drawing from one ChaCha8 stream per handle clone.
    pub fn seeded(seed: u64) -> Self {
        RngSource::Seeded(SeededRng {
            rng: Arc::new(Mutex::new(ChaCha8Rng::seed_from_u64(seed))),
        })
    }

    /// Mint a UUID (v4 layout in both modes: variant/version bits are set
    /// explicitly for seeded draws, so downstream consumers cannot
    /// distinguish the source from the bits).
    pub fn uuid(&self) -> uuid::Uuid {
        match self {
            RngSource::System => uuid::Uuid::new_v4(),
            RngSource::Seeded(seeded) => {
                use rand::RngCore as _;
                let mut bytes = [0u8; 16];
                let mut rng = seeded.rng.lock().unwrap_or_else(|e| e.into_inner());
                rng.fill_bytes(&mut bytes);
                uuid::Builder::from_random_bytes(bytes).into_uuid()
            }
        }
    }

    /// Convenience: [`RngSource::uuid`] as a string.
    pub fn uuid_string(&self) -> String {
        self.uuid().to_string()
    }
}

/// A shared ChaCha8 stream behind a mutex (interior mutability lets the
/// executor pass `&self` while node tasks draw ids).
#[derive(Debug, Clone)]
pub struct SeededRng {
    rng: Arc<Mutex<ChaCha8Rng>>,
}

/// A not-yet-sequenced event: everything about a [`RunEvent`] except the
/// fields only the journal can assign (`id`, `run_id`, `thread_id`, `seq`,
/// `recorded_at`). Build via [`EventDraft::new`] plus the builder methods,
/// then hand to [`Journal::record`].
#[derive(Debug, Clone)]
pub struct EventDraft {
    kind: RunEventKind,
    effect: Effect,
    node_id: Option<String>,
    input: Option<Value>,
    output: Option<Value>,
    latency_ms: Option<u64>,
    tokens: Option<Usage>,
    cost_usd: Option<f64>,
    status: EventStatus,
    parent: Option<String>,
}

impl EventDraft {
    /// A draft of kind `kind` produced by an effect of class `effect`.
    pub fn new(kind: RunEventKind, effect: Effect) -> Self {
        Self {
            kind,
            effect,
            node_id: None,
            input: None,
            output: None,
            latency_ms: None,
            tokens: None,
            cost_usd: None,
            status: EventStatus::Ok,
            parent: None,
        }
    }

    /// The node this event is about.
    pub fn node(mut self, node_id: impl Into<String>) -> Self {
        self.node_id = Some(node_id.into());
        self
    }

    /// The input payload. Large values are content-addressed by the journal
    /// at record time (see [`INLINE_PAYLOAD_MAX_BYTES`]).
    pub fn input(mut self, value: Value) -> Self {
        self.input = Some(value);
        self
    }

    /// The output payload; same large-value handling as [`EventDraft::input`].
    pub fn output(mut self, value: Value) -> Self {
        self.output = Some(value);
        self
    }

    /// The measured latency of the recorded operation, in milliseconds.
    pub fn latency_ms(mut self, latency_ms: u64) -> Self {
        self.latency_ms = Some(latency_ms);
        self
    }

    /// Token usage reported for a model call.
    pub fn tokens(mut self, usage: Usage) -> Self {
        self.tokens = Some(usage);
        self
    }

    /// Monetary cost of the recorded operation in USD.
    pub fn cost_usd(mut self, cost: f64) -> Self {
        self.cost_usd = Some(cost);
        self
    }

    /// The outcome status (defaults to [`EventStatus::Ok`]).
    pub fn status(mut self, status: EventStatus) -> Self {
        self.status = status;
        self
    }

    /// The causal parent: the id of the event that caused this one.
    pub fn parent(mut self, parent: impl Into<String>) -> Self {
        self.parent = Some(parent.into());
        self
    }
}

/// The journal's mutable interior, behind one mutex. All fields advance
/// together on every append, so poisoning cannot leave them inconsistent —
/// recovery on poison is safe.
#[derive(Debug)]
struct JournalInner {
    events: Vec<RunEvent>,
    /// Content-addressed payloads too large to inline, keyed by SHA-256 hex.
    /// `BTreeMap` so snapshot serialization order is canonical.
    artifacts: BTreeMap<String, Value>,
    /// Running chained head hash: `H_0 = sha256("")`,
    /// `H_n = sha256(H_{n-1} || canonical_json(event_n))`.
    head_hash: String,
}

/// An append-only, in-memory journal of one run's [`RunEvent`]s.
///
/// Cheap to clone: clones share the same journal (one `Arc` layer inside),
/// which is how node closures record model/tool calls into the same journal
/// the executor is writing. Thread-safe; `Send + Sync`.
///
/// The journal stamps every event with its `run_id`/`thread_id`, the next
/// sequence number, and a `recorded_at` read from its own [`Clock`] — so
/// events recorded from inside nodes (whose code cannot see the executor's
/// clock) are still timestamped through the run's determinism seam.
#[derive(Debug, Clone)]
pub struct Journal {
    run_id: String,
    thread_id: String,
    clock: Clock,
    inner: Arc<Mutex<JournalInner>>,
}

impl Journal {
    /// A fresh journal for `run_id` / `thread_id`, timestamping events from
    /// `clock`.
    pub fn new(run_id: impl Into<String>, thread_id: impl Into<String>, clock: Clock) -> Self {
        Self {
            run_id: run_id.into(),
            thread_id: thread_id.into(),
            clock,
            inner: Arc::new(Mutex::new(JournalInner {
                events: Vec::new(),
                artifacts: BTreeMap::new(),
                head_hash: sha256_hex(b""),
            })),
        }
    }

    /// Rebuild a journal from a [`JournalSnapshot`] (e.g. a replay fixture).
    ///
    /// The head hash is recomputed from the events rather than trusted from
    /// the snapshot, so a corrupted or edited fixture is detected here
    /// instead of deep inside a replay. Returns `None`-style failure as a
    /// [`crate::error::RustyError::Serialization`] when an event fails to
    /// serialize (not realistically possible for well-formed snapshots).
    pub fn from_snapshot(snapshot: JournalSnapshot, clock: Clock) -> crate::error::Result<Self> {
        let mut head_hash = sha256_hex(b"");
        for event in &snapshot.events {
            head_hash = chained_hash(&head_hash, event)?;
        }
        if head_hash != snapshot.head_hash {
            return Err(crate::error::RustyError::Serialization(
                serde_json::Error::io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!(
                        "journal snapshot head hash mismatch: events recompute to \
                         {head_hash}, snapshot claims {}",
                        snapshot.head_hash
                    ),
                )),
            ));
        }
        let run_id = snapshot.run_id.clone();
        let thread_id = snapshot.thread_id.clone();
        Ok(Self {
            run_id,
            thread_id,
            clock,
            inner: Arc::new(Mutex::new(JournalInner {
                events: snapshot.events,
                artifacts: snapshot.artifacts,
                head_hash,
            })),
        })
    }

    /// The run this journal records.
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// The thread the run belongs to.
    pub fn thread_id(&self) -> &str {
        &self.thread_id
    }

    /// The clock events are timestamped from.
    pub fn clock(&self) -> &Clock {
        &self.clock
    }

    fn lock(&self) -> MutexGuard<'_, JournalInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Append one event and return its id (`{run_id}:{seq}`).
    ///
    /// The journal assigns the sequence number and timestamp, promotes
    /// oversized payloads into the artifact map (replacing them with
    /// [`PayloadRef::Artifact`] references), and chains the head hash over
    /// the canonical serialization of the appended event.
    pub fn record(&self, draft: EventDraft) -> String {
        let recorded_at = self.clock.now();
        let mut inner = self.lock();
        let seq = inner.events.len() as u64;
        let id = format!("{}:{seq}", self.run_id);

        let input = draft
            .input
            .map(|value| store_payload(&mut inner.artifacts, value));
        let output = draft
            .output
            .map(|value| store_payload(&mut inner.artifacts, value));

        let event = RunEvent {
            id: id.clone(),
            run_id: self.run_id.clone(),
            thread_id: self.thread_id.clone(),
            node_id: draft.node_id,
            seq,
            kind: draft.kind,
            effect: draft.effect,
            input,
            output,
            latency_ms: draft.latency_ms,
            tokens: draft.tokens,
            cost_usd: draft.cost_usd,
            status: draft.status,
            parent: draft.parent,
            recorded_at,
        };

        // Hash chaining over the canonical serialization: serde_json's map
        // order is deterministic (BTreeMap-backed), so the hash is stable
        // for identical event content. Serialization of a just-constructed
        // event cannot realistically fail; if it somehow does, the event is
        // still recorded — evidence must not be lost to a hashing defect —
        // and the head chains over a fixed marker instead.
        match serde_json::to_vec(&event) {
            Ok(bytes) => {
                inner.head_hash = sha256_hex(&[inner.head_hash.as_bytes(), &bytes].concat());
            }
            Err(e) => {
                tracing::warn!(error = %e, "failed to serialize event for head hash");
                inner.head_hash =
                    sha256_hex(&[inner.head_hash.as_bytes(), b"!unhashable"].concat());
            }
        }
        inner.events.push(event);
        id
    }

    /// The recorded events in sequence order.
    pub fn events(&self) -> Vec<RunEvent> {
        self.lock().events.clone()
    }

    /// Number of recorded events.
    pub fn len(&self) -> usize {
        self.lock().events.len()
    }

    /// `true` when nothing has been recorded.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The current head hash (chained SHA-256 over all recorded events).
    pub fn head_hash(&self) -> String {
        self.lock().head_hash.clone()
    }

    /// The journal head as a [`crate::record::JournalRef`], for stamping
    /// into checkpoints.
    pub fn head_ref(&self) -> crate::record::JournalRef {
        let inner = self.lock();
        crate::record::JournalRef {
            events: inner.events.len() as u64,
            sha256: inner.head_hash.clone(),
        }
    }

    /// A serde-complete export of the journal: every event plus the artifact
    /// payloads they reference. The unit a portable replay fixture is built
    /// from.
    pub fn snapshot(&self) -> JournalSnapshot {
        let inner = self.lock();
        JournalSnapshot {
            run_id: self.run_id.clone(),
            thread_id: self.thread_id.clone(),
            events: inner.events.clone(),
            artifacts: inner.artifacts.clone(),
            head_hash: inner.head_hash.clone(),
        }
    }

    /// Resolve a [`PayloadRef`] to its value, looking through the artifact
    /// map. `None` for artifact references whose bytes are absent (a
    /// truncated snapshot — detectable via [`Journal::from_snapshot`]'s hash
    /// check).
    pub fn resolve(&self, payload: &PayloadRef) -> Option<Value> {
        match payload {
            PayloadRef::Inline(value) => Some(value.clone()),
            PayloadRef::Artifact(reference) => {
                self.lock().artifacts.get(&reference.sha256).cloned()
            }
        }
    }

    /// Journal an effect receipt (R0.6 Durable Work): the effect's own
    /// confirmation of an `Idempotent` side effect, recorded as a
    /// [`RunEventKind::EffectReceipt`] event with the receipt as its output
    /// payload and `parent` as its causal parent — the task lifecycle event
    /// that completed the effect once task lifecycle journaling lands; until
    /// then, the run's journal head is the honest parent (the receipt was
    /// caused by everything the run did before the task settled).
    ///
    /// Recording here — one canonical shape, as with model/tool calls — is
    /// what lets [`JournalSnapshot::find_effect_receipt`] serve the receipt
    /// during replay instead of re-sending the effect.
    pub fn record_effect_receipt(&self, receipt: &EffectReceipt, parent: Option<String>) -> String {
        let mut draft = EventDraft::new(RunEventKind::EffectReceipt, Effect::Idempotent)
            // Serialization of a just-built receipt cannot realistically
            // fail; if it somehow does, record the payload the event must
            // not lose rather than drop the evidence.
            .output(serde_json::to_value(receipt).unwrap_or_else(
                |_| serde_json::json!({ "idempotency_key": receipt.idempotency_key }),
            ));
        if let Some(parent) = parent {
            draft = draft.parent(parent);
        }
        self.record(draft)
    }
}

/// Store `value` inline or — when its serialized size exceeds
/// [`INLINE_PAYLOAD_MAX_BYTES`] — in the artifact map, returning the
/// reference to record on the event.
fn store_payload(artifacts: &mut BTreeMap<String, Value>, value: Value) -> PayloadRef {
    let Ok(bytes) = serde_json::to_vec(&value) else {
        // A Value that fails to serialize is a contradiction; keep it inline
        // rather than drop evidence.
        return PayloadRef::Inline(value);
    };
    if bytes.len() <= INLINE_PAYLOAD_MAX_BYTES {
        return PayloadRef::Inline(value);
    }
    let reference = ArtifactRef {
        sha256: sha256_hex(&bytes),
        bytes: bytes.len() as u64,
    };
    artifacts.entry(reference.sha256.clone()).or_insert(value);
    PayloadRef::Artifact(reference)
}

/// `sha256(prev || canonical_json(event))` — the head-hash chain step,
/// shared by [`Journal::record`] and [`Journal::from_snapshot`].
fn chained_hash(prev: &str, event: &RunEvent) -> crate::error::Result<String> {
    let bytes = serde_json::to_vec(event)?;
    Ok(sha256_hex(&[prev.as_bytes(), &bytes].concat()))
}

/// The serde-complete export form of a [`Journal`]: run identity, the full
/// event sequence, the artifact payloads referenced by events, and the head
/// hash binding them. Round-trips through JSON unchanged; load with
/// [`Journal::from_snapshot`] to re-verify the head.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalSnapshot {
    /// The recorded run's id.
    pub run_id: String,

    /// The thread the run belongs to.
    pub thread_id: String,

    /// Every recorded event, in sequence order.
    pub events: Vec<RunEvent>,

    /// Content-addressed payloads referenced by events (SHA-256 hex → value).
    pub artifacts: BTreeMap<String, Value>,

    /// The chained head hash over `events`.
    pub head_hash: String,
}

impl JournalSnapshot {
    /// The effect-receipt replay lookup (R0.6 Durable Work): the journaled
    /// [`EffectReceipt`] for `idempotency_key`, when one exists.
    ///
    /// This is the queue-dispatched analogue of the seq-ordered serving
    /// cursor exact replay uses for model and tool calls: a task's effect
    /// lands outside the run's super-step order, so receipts are matched by
    /// the idempotency key the effect ran under, not by sequence. A
    /// re-driven run (or a replayed activity) asks "did this key already
    /// land?" and serves the receipt instead of re-sending the effect.
    ///
    /// Malformed receipt payloads (a hand-edited journal) are skipped rather
    /// than fatal — integrity failures of the snapshot itself are caught by
    /// [`Journal::from_snapshot`]'s hash check at load time.
    pub fn find_effect_receipt(&self, idempotency_key: &str) -> Option<EffectReceipt> {
        self.events
            .iter()
            .filter(|event| event.kind == RunEventKind::EffectReceipt)
            .find_map(|event| {
                let value = match event.output.as_ref()? {
                    PayloadRef::Inline(value) => value.clone(),
                    PayloadRef::Artifact(reference) => {
                        self.artifacts.get(&reference.sha256)?.clone()
                    }
                };
                let receipt: EffectReceipt = serde_json::from_value(value).ok()?;
                (receipt.idempotency_key == idempotency_key).then_some(receipt)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn journal() -> Journal {
        Journal::new("run-1", "thread-1", Clock::System)
    }

    #[test]
    fn record_assigns_sequence_and_deterministic_ids() {
        let j = journal();
        let a = j.record(EventDraft::new(RunEventKind::SuperStepStart, Effect::Pure));
        let b = j.record(
            EventDraft::new(RunEventKind::NodeInput, Effect::Pure)
                .node("n")
                .parent(a.clone()),
        );
        assert_eq!(a, "run-1:0");
        assert_eq!(b, "run-1:1");
        let events = j.events();
        assert_eq!(events[1].seq, 1);
        assert_eq!(events[1].parent.as_deref(), Some("run-1:0"));
        assert_eq!(events[1].node_id.as_deref(), Some("n"));
        assert_eq!(j.len(), 2);
        assert!(!j.is_empty());
    }

    #[test]
    fn large_payloads_are_content_addressed_and_resolvable() {
        let j = journal();
        let big = json!({"blob": "x".repeat(INLINE_PAYLOAD_MAX_BYTES)});
        j.record(
            EventDraft::new(RunEventKind::ModelCall, Effect::NonIdempotent).output(big.clone()),
        );
        let event = &j.events()[0];
        let reference = match event.output.as_ref() {
            Some(PayloadRef::Artifact(reference)) => reference.clone(),
            other => panic!("expected artifact reference, got {other:?}"),
        };
        assert_eq!(
            reference.bytes as usize,
            serde_json::to_vec(&big).unwrap().len()
        );
        assert_eq!(j.resolve(event.output.as_ref().unwrap()), Some(big));
    }

    #[test]
    fn head_hash_advances_and_binds_content() {
        let j1 = journal();
        let j2 = journal();
        assert_eq!(j1.head_hash(), j2.head_hash());
        let draft = || EventDraft::new(RunEventKind::SuperStepStart, Effect::Pure);
        j1.record(draft());
        assert_ne!(j1.head_hash(), j2.head_hash());
        j2.record(draft());
        // Same event content (modulo timestamps — system clock here, so the
        // hashes may differ; only the advance is asserted).
        assert_ne!(j1.head_hash(), j2.head_hash());
    }

    #[test]
    fn snapshot_roundtrip_reverifies_head() {
        let j = journal();
        j.record(
            EventDraft::new(RunEventKind::SuperStepStart, Effect::Pure).input(json!({"step": 0})),
        );
        j.record(
            EventDraft::new(RunEventKind::NodeInput, Effect::Pure)
                .node("a")
                .output(json!({"blob": "y".repeat(INLINE_PAYLOAD_MAX_BYTES)})),
        );
        let snapshot = j.snapshot();
        let wire = serde_json::to_string(&snapshot).unwrap();
        let parsed: JournalSnapshot = serde_json::from_str(&wire).unwrap();
        let rebuilt = Journal::from_snapshot(parsed, Clock::System).unwrap();
        assert_eq!(rebuilt.head_hash(), j.head_hash());
        assert_eq!(rebuilt.events(), j.events());

        // A tampered snapshot is rejected on load.
        let mut tampered = j.snapshot();
        tampered.events[0].status = EventStatus::Error;
        assert!(Journal::from_snapshot(tampered, Clock::System).is_err());
    }

    #[test]
    fn logical_clock_ticks_deterministically() {
        let clock = Clock::logical(1_000_000, 5);
        let a = clock.now();
        let b = clock.now();
        assert_eq!(b.timestamp_millis() - a.timestamp_millis(), 5);
        assert_eq!(a.timestamp_millis(), 1_000_000);

        let same = Clock::logical(1_000_000, 5);
        assert_eq!(same.now(), a);
        assert_eq!(same.now(), b);
    }

    #[test]
    fn seeded_rng_mints_reproducible_v4_uuids() {
        let a = RngSource::seeded(42);
        let b = RngSource::seeded(42);
        assert_eq!(a.uuid(), b.uuid());
        assert_eq!(a.uuid(), b.uuid());
        let id = a.uuid();
        assert_eq!(id.get_version(), Some(uuid::Version::Random));
        // Different seeds diverge.
        assert_ne!(RngSource::seeded(1).uuid(), RngSource::seeded(2).uuid());
    }

    #[test]
    fn system_defaults_match_pre_r05_shapes() {
        assert!(matches!(Clock::default(), Clock::System));
        assert!(matches!(RngSource::default(), RngSource::System));
    }

    fn receipt(key: &str) -> EffectReceipt {
        EffectReceipt {
            provider: "stripe".into(),
            provider_id: "ch_3PKd".into(),
            idempotency_key: key.into(),
            task_id: Some("task-9".into()),
        }
    }

    #[test]
    fn effect_receipt_records_with_parentage_and_is_findable_by_key() {
        let j = journal();
        let step = j.record(EventDraft::new(RunEventKind::SuperStepStart, Effect::Pure));
        let event_id = j.record_effect_receipt(&receipt("run-1:charge:7"), Some(step.clone()));

        let events = j.events();
        let recorded = events.last().unwrap();
        assert_eq!(recorded.id, event_id);
        assert_eq!(recorded.kind, RunEventKind::EffectReceipt);
        // The receipt is an Idempotent effect's confirmation — declared as
        // such so the replay/retry policies classify it like any other event.
        assert_eq!(recorded.effect, Effect::Idempotent);
        assert_eq!(recorded.parent.as_deref(), Some(step.as_str()));

        // The lookup serves the receipt by idempotency key, and only it.
        let snapshot = j.snapshot();
        assert_eq!(
            snapshot.find_effect_receipt("run-1:charge:7"),
            Some(receipt("run-1:charge:7"))
        );
        assert_eq!(snapshot.find_effect_receipt("run-1:charge:8"), None);

        // The journaled snapshot survives the integrity-verified reload with
        // the receipt still servable (the crash boundary the receipt exists
        // to cross).
        let rebuilt = Journal::from_snapshot(snapshot, Clock::System).unwrap();
        let snapshot = rebuilt.snapshot();
        assert_eq!(
            snapshot.find_effect_receipt("run-1:charge:7"),
            Some(receipt("run-1:charge:7"))
        );
    }

    #[test]
    fn effect_receipt_records_without_a_parent() {
        let j = journal();
        j.record_effect_receipt(&receipt("k"), None);
        let event = j.events().pop().unwrap();
        assert_eq!(event.parent, None);
    }
}
