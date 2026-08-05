//! Shared state, channels, and reducers.
//!
//! In `agentgraph` (as in LangGraph) **every state key is a channel** and the
//! channel's [`Reducer`] defines how node updates merge into the shared
//! state. Nodes never call each other; they publish partial updates to
//! channels, and the engine applies the per-channel reducer at the super-step
//! barrier.
//!
//! Channel semantics modeled here:
//!
//! | Reducer        | LangGraph analog            | Multi-write per super-step? |
//! |----------------|-----------------------------|-----------------------------|
//! | [`Reducer::Overwrite`]  | `LastValue`        | **No** — second write is [`AgentGraphError::InvalidUpdate`] |
//! | [`Reducer::Append`]     | `BinaryOperatorAggregate` (list concat) | Yes |
//! | [`Reducer::DeepMerge`]  | custom merge reducer | Yes |
//! | [`Reducer::AddMessages`]| `add_messages`    | Yes (ID-aware upsert + append) |
//!
//! [`StateSpec`] is the graph's state schema: channel name → reducer. It also
//! performs super-step write validation in [`StateSpec::apply_super_step`].

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{AgentGraphError, Result};

/// The shared graph state: a thin wrapper over a JSON object (`Map<String, Value>`).
///
/// This is the "untyped typed-dict" of the engine: nodes read the full state
/// snapshot and return partial updates keyed by channel name. Type safety for
/// concrete applications is layered on top via serde (de)serialization of
/// individual channel values.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct State {
    inner: Map<String, Value>,
}

impl State {
    /// An empty state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Wrap an existing JSON object.
    pub fn from_map(inner: Map<String, Value>) -> Self {
        Self { inner }
    }

    /// Build a state from any serializable value that is a JSON object.
    pub fn from_value(value: Value) -> Result<Self> {
        match value {
            Value::Object(inner) => Ok(Self { inner }),
            other => Err(AgentGraphError::InvalidUpdate(format!(
                "state must be a JSON object, got {other}"
            ))),
        }
    }

    /// Serialize the whole state back into a [`Value::Object`].
    pub fn to_value(&self) -> Value {
        Value::Object(self.inner.clone())
    }

    /// Consume the state, returning the underlying map.
    pub fn into_map(self) -> Map<String, Value> {
        self.inner
    }

    /// Borrow the underlying map.
    pub fn as_map(&self) -> &Map<String, Value> {
        &self.inner
    }

    /// Get a channel's current value.
    pub fn get(&self, channel: &str) -> Option<&Value> {
        self.inner.get(channel)
    }

    /// `true` if the channel exists in the state (regardless of value).
    pub fn contains(&self, channel: &str) -> bool {
        self.inner.contains_key(channel)
    }

    /// Directly set a channel's value, bypassing reducer semantics.
    ///
    /// Intended for engine internals (initial state seeding, checkpoint
    /// restore). Nodes should always go through reducers.
    pub fn insert(&mut self, channel: impl Into<String>, value: Value) {
        self.inner.insert(channel.into(), value);
    }

    /// Deserialize a channel into a concrete type.
    pub fn get_as<T: serde::de::DeserializeOwned>(&self, channel: &str) -> Result<Option<T>> {
        match self.inner.get(channel) {
            None => Ok(None),
            Some(v) => Ok(Some(serde_json::from_value(v.clone())?)),
        }
    }

    /// Number of channels currently present.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// `true` if no channels are present.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

impl From<Map<String, Value>> for State {
    fn from(inner: Map<String, Value>) -> Self {
        Self::from_map(inner)
    }
}

/// Per-channel merge semantics.
///
/// A reducer is conceptually a binary function
/// `reduce(current: Option<&Value>, update: Value) -> Value`, mirroring
/// LangGraph's `Annotated[T, reducer]` channel annotations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum Reducer {
    /// `LastValue` semantics: the update replaces the current value.
    ///
    /// **At most one write per super-step** — a second write to the same
    /// channel within one super-step fails with
    /// [`AgentGraphError::InvalidUpdate`]. This is the default for any
    /// channel and the classic production failure mode in parallel graphs;
    /// if you need fan-in, use [`Reducer::Append`], [`Reducer::DeepMerge`],
    /// or [`Reducer::AddMessages`].
    #[default]
    Overwrite,

    /// List-concat semantics: the current value is treated as an array
    /// (missing/non-array current values start as `[]`). If the update is an
    /// array it is extended onto the current array; otherwise the update is
    /// pushed as a single element.
    Append,

    /// Recursive object merge: two JSON objects are merged key-by-key
    /// (nested objects merge recursively); any non-object pair resolves to
    /// the update value. A missing current value resolves to the update.
    DeepMerge,

    /// LangGraph `add_messages` semantics over a message array.
    ///
    /// The current value is treated as an array of message objects. The
    /// update may be a single message object or an array of messages. Each
    /// incoming message is **upserted**: if it has an `"id"` field equal to
    /// an existing message's `"id"`, the existing message is replaced in
    /// place; otherwise the message is appended.
    AddMessages,
}

impl Reducer {
    /// Whether this channel accepts multiple writes within one super-step.
    ///
    /// Only `LastValue`-style channels ([`Reducer::Overwrite`]) are
    /// single-write; aggregating reducers exist precisely to support
    /// parallel fan-in.
    pub fn allows_multiple_writes(&self) -> bool {
        !matches!(self, Reducer::Overwrite)
    }

    /// Apply one update to a channel's current value.
    ///
    /// `current` is `None` when the channel has never been written.
    pub fn apply(&self, current: Option<&Value>, update: Value) -> Value {
        match self {
            Reducer::Overwrite => update,
            Reducer::Append => match current {
                Some(Value::Array(existing)) => {
                    let mut out = existing.clone();
                    match update {
                        Value::Array(items) => out.extend(items),
                        single => out.push(single),
                    }
                    Value::Array(out)
                }
                // No array yet: start a fresh one from the update.
                _ => match update {
                    Value::Array(items) => Value::Array(items),
                    single => Value::Array(vec![single]),
                },
            },
            Reducer::DeepMerge => match current {
                Some(cur) => deep_merge(cur, &update),
                None => update,
            },
            Reducer::AddMessages => add_messages(current, update),
        }
    }
}

/// Recursive JSON object merge. Non-object pairs resolve to `b`.
fn deep_merge(a: &Value, b: &Value) -> Value {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let mut merged = x.clone();
            for (k, v) in y {
                let next = match merged.get(k) {
                    Some(cur) => deep_merge(cur, v),
                    None => v.clone(),
                };
                merged.insert(k.clone(), next);
            }
            Value::Object(merged)
        }
        _ => b.clone(),
    }
}

/// `add_messages` semantics: ID-aware upsert + append over a message array.
fn add_messages(current: Option<&Value>, update: Value) -> Value {
    let mut messages: Vec<Value> = match current {
        Some(Value::Array(existing)) => existing.clone(),
        _ => Vec::new(),
    };
    let incoming: Vec<Value> = match update {
        Value::Array(items) => items,
        single => vec![single],
    };
    for msg in incoming {
        let msg_id = msg.get("id").and_then(Value::as_str).map(str::to_owned);
        let pos = msg_id.as_deref().and_then(|id| {
            messages
                .iter()
                .position(|m| m.get("id").and_then(Value::as_str) == Some(id))
        });
        match pos {
            Some(i) => messages[i] = msg,
            None => messages.push(msg),
        }
    }
    Value::Array(messages)
}

/// The graph's state schema: channel name → [`Reducer`].
///
/// The spec serves two roles:
///
/// 1. **Merge semantics**: at each super-step barrier, node updates are
///    merged into the shared state via the channel's reducer
///    ([`StateSpec::apply_super_step`]).
/// 2. **Write validation** (`LastValue` rule): within one super-step, a
///    single-write channel may receive at most one write across *all*
///    nodes that ran in parallel. A second write yields
///    [`AgentGraphError::InvalidUpdate`], mirroring LangGraph's
///    `InvalidUpdateError: Can receive only one value per step`.
///
/// Writes to channels **not declared** in the spec are also rejected with
/// [`AgentGraphError::InvalidUpdate`]; the spec is the complete schema.
#[derive(Debug, Clone, Default)]
pub struct StateSpec {
    channels: HashMap<String, Reducer>,
}

impl StateSpec {
    /// An empty spec (no channels declared).
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder-style: declare a channel with the given reducer.
    pub fn channel(mut self, name: impl Into<String>, reducer: Reducer) -> Self {
        self.channels.insert(name.into(), reducer);
        self
    }

    /// Mutable variant of [`StateSpec::channel`].
    pub fn add_channel(&mut self, name: impl Into<String>, reducer: Reducer) -> &mut Self {
        self.channels.insert(name.into(), reducer);
        self
    }

    /// The reducer for a channel, defaulting to [`Reducer::Overwrite`]
    /// (`LastValue` semantics).
    pub fn reducer_for(&self, channel: &str) -> Reducer {
        self.channels.get(channel).copied().unwrap_or_default()
    }

    /// All declared channel names.
    pub fn channel_names(&self) -> impl Iterator<Item = &str> {
        self.channels.keys().map(String::as_str)
    }

    /// `true` if the channel is declared in this spec.
    pub fn has_channel(&self, channel: &str) -> bool {
        self.channels.contains_key(channel)
    }

    /// Validate and merge **all writes of one super-step** into `state`.
    ///
    /// `writes` is the collection of `(node_name, updates)` pairs produced by
    /// the nodes that ran in this super-step. Each entry is one node's
    /// partial update map (`channel -> value`), as carried by
    /// [`crate::node::NodeOutput::updates`].
    ///
    /// Semantics, in order:
    ///
    /// 1. Every written channel must be declared in this spec
    ///    ([`AgentGraphError::InvalidUpdate`] otherwise).
    /// 2. A channel whose reducer does **not** allow multiple writes
    ///    (i.e. [`Reducer::Overwrite`]) may appear in at most one node's
    ///    updates per super-step; a second write is
    ///    [`AgentGraphError::InvalidUpdate`].
    /// 3. Surviving writes are merged in iteration order via the channel
    ///    reducer.
    ///
    /// On error the state is left unmodified for the offending channel and
    /// the caller (executor) should abort the super-step — LangGraph treats
    /// a super-step as transactional.
    pub fn apply_super_step<I, S>(&self, state: &mut State, writes: I) -> Result<()>
    where
        I: IntoIterator<Item = (S, HashMap<String, Value>)>,
        S: AsRef<str>,
    {
        // Collect first so validation borrows live in `collected` (owned),
        // avoiding borrow/move conflicts with the input iterator.
        let collected: Vec<(String, HashMap<String, Value>)> = writes
            .into_iter()
            .map(|(node, updates)| (node.as_ref().to_owned(), updates))
            .collect();

        // Pass 1: validation — collect write counts per channel.
        let mut write_counts: HashMap<&str, usize> = HashMap::new();
        let mut first_writer: HashMap<&str, &str> = HashMap::new();

        for (node, updates) in &collected {
            let mut seen_in_node: HashSet<&str> = HashSet::new();
            for channel in updates.keys() {
                if !self.has_channel(channel) {
                    return Err(AgentGraphError::InvalidUpdate(format!(
                        "node `{node}` wrote to undeclared channel `{channel}`; \
                         declare it in the StateSpec"
                    )));
                }
                // A HashMap can hold one value per key, so duplicate writes
                // within a single node are impossible; track defensively
                // anyway to keep the invariant explicit.
                if !seen_in_node.insert(channel.as_str()) {
                    return Err(AgentGraphError::InvalidUpdate(format!(
                        "node `{node}` wrote channel `{channel}` twice in one update"
                    )));
                }
                let count = write_counts.entry(channel.as_str()).or_insert(0);
                *count += 1;
                first_writer
                    .entry(channel.as_str())
                    .or_insert(node.as_str());
                if *count > 1 && !self.reducer_for(channel).allows_multiple_writes() {
                    return Err(AgentGraphError::InvalidUpdate(format!(
                        "channel `{channel}` can receive only one value per super-step \
                         (reducer: {:?}); already written by node `{}`, second write from \
                         node `{node}`. Use a multi-write reducer (Append/DeepMerge/\
                         AddMessages) to handle concurrent writes.",
                        self.reducer_for(channel),
                        first_writer[channel.as_str()],
                    )));
                }
            }
        }

        // Pass 2: merge via reducers.
        for (_node, updates) in collected {
            for (channel, update) in updates {
                let reducer = self.reducer_for(&channel);
                let merged = reducer.apply(state.get(&channel), update);
                state.insert(channel, merged);
            }
        }
        Ok(())
    }

    /// Convenience: merge a single node's updates (e.g. outside the parallel
    /// super-step path). Single-write validation trivially passes since only
    /// one writer is involved.
    pub fn apply_single(
        &self,
        state: &mut State,
        node: &str,
        updates: HashMap<String, Value>,
    ) -> Result<()> {
        self.apply_super_step(state, [(node, updates)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn updates(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn overwrite_replaces() {
        let mut state = State::new();
        let spec = StateSpec::new().channel("x", Reducer::Overwrite);
        spec.apply_single(&mut state, "n1", updates(&[("x", json!(1))]))
            .unwrap();
        spec.apply_single(&mut state, "n2", updates(&[("x", json!(2))]))
            .unwrap();
        assert_eq!(state.get("x"), Some(&json!(2)));
    }

    #[test]
    fn last_value_double_write_fails() {
        let mut state = State::new();
        let spec = StateSpec::new().channel("x", Reducer::Overwrite);
        let writes = vec![
            ("a".to_string(), updates(&[("x", json!(1))])),
            ("b".to_string(), updates(&[("x", json!(2))])),
        ];
        let err = spec.apply_super_step(&mut state, writes).unwrap_err();
        assert!(matches!(err, AgentGraphError::InvalidUpdate(_)));
    }

    #[test]
    fn append_allows_fan_in() {
        let mut state = State::new();
        let spec = StateSpec::new().channel("xs", Reducer::Append);
        let writes = vec![
            ("a".to_string(), updates(&[("xs", json!([1, 2]))])),
            ("b".to_string(), updates(&[("xs", json!(3))])),
        ];
        spec.apply_super_step(&mut state, writes).unwrap();
        assert_eq!(state.get("xs"), Some(&json!([1, 2, 3])));
    }

    #[test]
    fn deep_merge_is_recursive() {
        let mut state = State::from_value(json!({"cfg": {"a": 1, "nested": {"x": 1}}})).unwrap();
        let spec = StateSpec::new().channel("cfg", Reducer::DeepMerge);
        spec.apply_single(
            &mut state,
            "n",
            updates(&[("cfg", json!({"nested": {"y": 2}}))]),
        )
        .unwrap();
        assert_eq!(
            state.get("cfg"),
            Some(&json!({"a": 1, "nested": {"x": 1, "y": 2}}))
        );
    }

    #[test]
    fn add_messages_upserts_by_id() {
        let mut state = State::from_value(json!({
            "messages": [{"id": "m1", "content": "old"}, {"content": "plain"}]
        }))
        .unwrap();
        let spec = StateSpec::new().channel("messages", Reducer::AddMessages);
        spec.apply_single(
            &mut state,
            "n",
            updates(&[(
                "messages",
                json!([
                    {"id": "m1", "content": "new"},
                    {"content": "appended"}
                ]),
            )]),
        )
        .unwrap();
        assert_eq!(
            state.get("messages"),
            Some(&json!([
                {"id": "m1", "content": "new"},
                {"content": "plain"},
                {"content": "appended"}
            ]))
        );
    }

    #[test]
    fn undeclared_channel_rejected() {
        let mut state = State::new();
        let spec = StateSpec::new().channel("x", Reducer::Overwrite);
        let err = spec
            .apply_single(&mut state, "n", updates(&[("y", json!(1))]))
            .unwrap_err();
        assert!(matches!(err, AgentGraphError::InvalidUpdate(_)));
    }
}
