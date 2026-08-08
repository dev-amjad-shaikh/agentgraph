//! Agent Fabric contracts (R0.7, wave 1): durable agent identity, the
//! versioned capability manifest, and the state-scope taxonomy.
//!
//! This module freezes the wire shapes the agent fabric builds on — the
//! server's agent registry and activation leases, the agent hosts (a later
//! wave), and the SDKs must agree on them byte-for-byte, exactly the rule
//! [`crate::durable`] set for the R0.6 queue before it. Nothing here
//! performs I/O, scheduling, or supervision; these are pure data contracts
//! plus the addressing grammar both sides of a mailbox must share.
//!
//! The three pillars:
//!
//! - [`AgentId`] — stable, tenant-namespaced agent identity. Identity is
//!   names and records, not processes: it survives redeploys and crashes
//!   because the registry record, the thread, and the mailbox all derive
//!   from the one id. [`AgentId::mailbox_recipient`] is the addressing
//!   discipline the R0.6 queue carries mailbox traffic under.
//! - [`CapabilityManifest`] — the versioned declaration of what an agent
//!   runs, which message kinds its mailbox accepts, which [`StateScope`]s
//!   it may touch, and its budget ceiling. The exact-match
//!   `manifest_version` pin is the agent-level form of R0.6's worker
//!   version pinning: a team started against one manifest never has its
//!   semantics changed mid-flight by a redeploy.
//! - [`StateScope`] — the closed taxonomy of state an agent may read and
//!   write, mapped onto stores that already exist (the agent's checkpoint
//!   log, a shared team thread, the server KV namespaces).
//!
//! Golden-file tests under `tests/golden/` pin the serialized shapes; any
//! accidental contract drift fails CI. To bless an intentional change,
//! re-run with `UPDATE_GOLDEN=1` and review the diff.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::durable::ArtifactContract;

/// The address prefix distinguishing mailbox recipients from worker pools
/// on the task queue: a `recipient` of `agent:{agent_id}` is mailbox
/// traffic, drained one message at a time by the agent's activation; any
/// other recipient is ordinary queue work. Server-side, recipient-addressed
/// tasks are excluded from the pool claim path — a pool worker must never
/// steal a mailbox message out from under the turn-serialization
/// discipline.
pub const AGENT_RECIPIENT_PREFIX: &str = "agent:";

/// A stable agent identity (R0.7).
///
/// Tenant-namespaced like every other server id (`{tenant}/researcher-7`
/// inherits the v0.5 isolation model unchanged — a cross-tenant agent
/// resolves to nothing). Transparent over `String`: the wire shape is the
/// id itself. The newtype — not convention — is what keeps agent ids
/// distinct from run ids, task ids, and pool names at every construction
/// site, the same discipline [`crate::record::PolicyVersion`] applies to
/// policy pins.
///
/// The constructor does not validate: the server's route layer enforces the
/// id grammar (non-empty, bounded, no path separators) at the boundary,
/// the same division of labor every other server id follows.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentId(pub String);

impl AgentId {
    /// Wrap an id string.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The id string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The queue recipient addressing this agent's mailbox:
    /// `agent:{agent_id}`. A mailbox is an addressing discipline on the
    /// existing task queue — not a new queue — so mailbox messages inherit
    /// the R0.6 durability, retry, dead-letter, and cancellation machinery
    /// unchanged.
    pub fn mailbox_recipient(&self) -> String {
        format!("{AGENT_RECIPIENT_PREFIX}{}", self.0)
    }

    /// The thread holding this agent's private state: `agent:{agent_id}` —
    /// a naming convention over the existing checkpointer, not a new store,
    /// so checkpoints, time travel, and fork-on-replay work on agent state
    /// unmodified. The checkpoint log *is* the private state; restart is
    /// re-driving it.
    pub fn thread_id(&self) -> String {
        format!("{AGENT_RECIPIENT_PREFIX}{}", self.0)
    }
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Strip the mailbox prefix from a recipient string: `Some(agent_id)` when
/// `recipient` is mailbox traffic (`agent:{agent_id}`), `None` for ordinary
/// pool/pinned-worker recipients. The server uses this to keep mailbox
/// messages off the pool claim path.
pub fn agent_id_from_recipient(recipient: &str) -> Option<&str> {
    recipient.strip_prefix(AGENT_RECIPIENT_PREFIX)
}

/// A state scope an agent may read and write (R0.7): the closed taxonomy
/// naming where agent state lives, declared per agent in its
/// [`CapabilityManifest::scopes`].
///
/// Each scope maps onto a store that already exists — the contract is the
/// name and the declaration, not a new mechanism. Access is checked against
/// the manifest's declaration: an undeclared access fails fast before any
/// I/O, the same shape as a write to an undeclared channel failing at the
/// barrier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateScope {
    /// The agent's own thread (`agent:{agent_id}`). The checkpoint log is
    /// the private state: per-turn writes land in channels, the boundary
    /// checkpoint persists them, restart restores them.
    Private,

    /// A thread shared by a team's members (`team:{team_id}`), written only
    /// through mailbox-driven turns so every mutation has a journaled
    /// author. Shared mutable team state outside the turn discipline is not
    /// offered — that is the shared-state bug class the channel/reducer
    /// model was built to kill.
    Team,

    /// The server KV store under a `user:{user_id}` namespace (inside the
    /// tenant's `{tenant}/` isolation prefix).
    User,

    /// The server KV store's tenant namespace itself — configuration and
    /// reference data shared by every agent and run in the tenant.
    Tenant,
}

/// An agent-level budget ceiling (R0.7): the whole-activity bound across
/// turns, the way [`crate::durable::TaskBudget`] bounds one task across its
/// attempts.
///
/// Every field is optional and omitted from the wire when unset, so a
/// manifest carrying no budget — or a partial one — produces no shape
/// change for older readers. `None` means unbounded within the tenant's
/// own quotas, never an invented default.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct AgentBudget {
    /// Token ceiling across all of the agent's turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,

    /// Cost ceiling in USD across all turns. Evidence-grade `f64`, matching
    /// [`crate::record::RunEvent::cost_usd`]: the ledger aggregates
    /// elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,

    /// Wall-clock deadline for the whole agent activity, across turns.
    /// Expiry is cancellation by clock one level up from
    /// [`crate::durable::TaskEnvelope::deadline`] — enforced with the
    /// supervision wave (R0.7 wave 2); wave 1 declares the shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<DateTime<Utc>>,
}

/// The versioned capability manifest of an agent (R0.7 wave 1): what the
/// agent runs, what its mailbox accepts, which state scopes it may touch,
/// and its budget ceiling.
///
/// Registration pins the manifest in the server's agent registry; a team
/// started against `researcher/1.4.0` pins its mailbox traffic to that
/// manifest, so a mid-team redeploy never changes semantics under an
/// in-flight coordination. `manifest_version` is an *exact* version string,
/// deliberately: semver ranges make a team's behavior depend on the version
/// grammar's rules rather than on what the team actually recorded — the
/// same reason R0.6's worker version pin is exact match only.
///
/// Every field past `agent_kind` / `manifest_version` is additive and
/// omitted from the wire when empty, so manifests written against later
/// waves keep deserializing here and a minimal manifest stays minimal on
/// the wire. The supervision policy fields (restart / intensity / period,
/// R0.7 wave 2) land the same additive way.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapabilityManifest {
    /// What the agent runs: a graph/assistant identity (a graph plus
    /// config, as the server's assistant registry already models it).
    pub agent_kind: String,

    /// The exact manifest version string (e.g. `researcher/1.4.0`). Exact
    /// match, surviving retries and redeploys — see the type docs.
    pub manifest_version: String,

    /// The message kinds the mailbox accepts: message kind → the
    /// [`ArtifactContract`] its payload must satisfy. A send naming a kind
    /// outside this map fails fast at submission
    /// ([`crate::durable::ErrorClass::InvalidInput`] semantics — never
    /// retried) instead of dead-lettering after the attempt budget.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub accepts: BTreeMap<String, ArtifactContract>,

    /// The declared [`StateScope`]s the agent may read and write. Journaled
    /// with the spawn event ([`crate::record::RunEventKind::AgentSpawn`]),
    /// checked at every access.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<StateScope>,

    /// The agent-level budget ceiling. `None` (the default) means no
    /// agent-level bound; tenant quotas still apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<AgentBudget>,
}

impl CapabilityManifest {
    /// A minimal manifest: the agent kind and its exact version, accepting
    /// no message kinds and declaring no scopes. Set the optional fields
    /// directly; they are public.
    pub fn new(agent_kind: impl Into<String>, manifest_version: impl Into<String>) -> Self {
        Self {
            agent_kind: agent_kind.into(),
            manifest_version: manifest_version.into(),
            accepts: BTreeMap::new(),
            scopes: Vec::new(),
            budget: None,
        }
    }

    /// The contract the manifest declares for message `kind`, or `None`
    /// when the mailbox refuses that kind outright.
    pub fn accepts_kind(&self, kind: &str) -> Option<&ArtifactContract> {
        self.accepts.get(kind)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_manifest() -> CapabilityManifest {
        let mut manifest = CapabilityManifest::new("researcher", "researcher/1.4.0");
        manifest.accepts.insert(
            "summarize".into(),
            ArtifactContract {
                kind: "application/json".into(),
                max_bytes: Some(65_536),
                schema: Some(json!({
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "type": "object",
                    "properties": {"topic": {"type": "string"}},
                    "required": ["topic"],
                })),
            },
        );
        manifest.scopes = vec![StateScope::Private, StateScope::Team];
        manifest.budget = Some(AgentBudget {
            max_tokens: Some(250_000),
            max_cost_usd: Some(1.50),
            deadline: DateTime::<Utc>::from_timestamp_millis(1_800_000_000_000),
        });
        manifest
    }

    #[test]
    fn agent_id_derives_mailbox_and_thread_addresses() {
        let id = AgentId::new("acme/researcher-7");
        assert_eq!(id.as_str(), "acme/researcher-7");
        assert_eq!(id.mailbox_recipient(), "agent:acme/researcher-7");
        // The thread convention mirrors the mailbox address — one name, two
        // views (queue addressing, checkpoint log).
        assert_eq!(id.thread_id(), "agent:acme/researcher-7");
        // Transparent wire shape: the bare string, no wrapper object.
        assert_eq!(
            serde_json::to_value(&id).unwrap(),
            json!("acme/researcher-7")
        );
        assert_eq!(id.to_string(), "acme/researcher-7");
    }

    #[test]
    fn recipient_parsing_strips_only_the_agent_prefix() {
        assert_eq!(
            agent_id_from_recipient("agent:researcher-7"),
            Some("researcher-7")
        );
        assert_eq!(agent_id_from_recipient("pool-default"), None);
        assert_eq!(agent_id_from_recipient("agent:"), Some(""));
        // A prefix alone is not a mailbox address ("agents:x" is not one).
        assert_eq!(agent_id_from_recipient("agents:researcher-7"), None);
    }

    #[test]
    fn state_scope_serde_names_are_snake_case() {
        for (scope, name) in [
            (StateScope::Private, "private"),
            (StateScope::Team, "team"),
            (StateScope::User, "user"),
            (StateScope::Tenant, "tenant"),
        ] {
            assert_eq!(serde_json::to_value(scope).unwrap(), json!(name));
            assert_eq!(
                serde_json::from_value::<StateScope>(json!(name)).unwrap(),
                scope
            );
        }
        assert!(serde_json::from_value::<StateScope>(json!("global")).is_err());
    }

    #[test]
    fn manifest_serde_roundtrip_and_sparse_wire_shape() {
        let manifest = sample_manifest();
        let back: CapabilityManifest =
            serde_json::from_str(&serde_json::to_string(&manifest).unwrap()).unwrap();
        assert_eq!(manifest, back);

        // A minimal manifest stays minimal on the wire: only the two
        // required keys, nothing null or empty — pre-R0.7-shaped readers see
        // no new keys at all.
        let minimal = CapabilityManifest::new("researcher", "researcher/1.4.0");
        let value = serde_json::to_value(&minimal).unwrap();
        assert_eq!(
            value,
            json!({"agent_kind": "researcher", "manifest_version": "researcher/1.4.0"})
        );

        // And the minimal shape — what the smallest client writes — loads
        // with honest defaults: nothing accepted, no scopes, no budget.
        let back: CapabilityManifest = serde_json::from_value(value).unwrap();
        assert_eq!(back, minimal);
        assert!(back.accepts_kind("summarize").is_none());
    }

    #[test]
    fn manifest_lookup_finds_only_declared_kinds() {
        let manifest = sample_manifest();
        let contract = manifest.accepts_kind("summarize").expect("declared kind");
        assert_eq!(contract.kind, "application/json");
        assert!(contract.schema.is_some());
        assert!(manifest.accepts_kind("delete_everything").is_none());
    }

    #[test]
    fn artifact_contract_schema_field_is_additive() {
        // A pre-R0.7 contract — exactly the shape the R0.6 golden pins —
        // deserializes with the schema unset.
        let r06_shape = json!({"kind": "application/json", "max_bytes": 65536});
        let contract: ArtifactContract = serde_json::from_value(r06_shape).unwrap();
        assert_eq!(contract.schema, None);
        // Unset: absent from the wire, so R0.6 readers see no shape change.
        assert_eq!(
            serde_json::to_value(&contract).unwrap(),
            json!({"kind": "application/json", "max_bytes": 65536})
        );

        // Set: carried as the schema document itself, surviving the
        // round-trip.
        let with_schema = ArtifactContract {
            kind: "application/json".into(),
            max_bytes: None,
            schema: Some(json!({"type": "object"})),
        };
        let back: ArtifactContract =
            serde_json::from_str(&serde_json::to_string(&with_schema).unwrap()).unwrap();
        assert_eq!(with_schema, back);
    }

    #[test]
    fn agent_event_kinds_stay_additive_to_the_closed_enum() {
        use crate::record::RunEventKind;
        // The R0.7 variants serialize snake_case like every event kind
        // before them, and pre-R0.7 kinds deserialize unchanged — old
        // journals keep loading (the rule R0.6's EffectReceipt followed).
        for (kind, name) in [
            (RunEventKind::AgentSpawn, "agent_spawn"),
            (RunEventKind::AgentExit, "agent_exit"),
            (RunEventKind::MailboxSend, "mailbox_send"),
            (RunEventKind::MailboxReceive, "mailbox_receive"),
            (RunEventKind::SupervisionEvent, "supervision_event"),
            (RunEventKind::CoordinationStart, "coordination_start"),
            (RunEventKind::CoordinationEnd, "coordination_end"),
        ] {
            assert_eq!(serde_json::to_value(kind).unwrap(), json!(name));
            assert_eq!(
                serde_json::from_value::<RunEventKind>(json!(name)).unwrap(),
                kind
            );
        }
        assert_eq!(
            serde_json::from_value::<RunEventKind>(json!("model_call")).unwrap(),
            RunEventKind::ModelCall
        );
        assert!(serde_json::from_value::<RunEventKind>(json!("agent_magic")).is_err());
    }
}
