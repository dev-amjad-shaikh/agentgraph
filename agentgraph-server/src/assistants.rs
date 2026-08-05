//! Assistants: named aliases for a registered graph plus default config.
//!
//! An assistant binds a human-readable name and a free-form `config` /
//! `metadata` blob to a registered graph, so clients can create runs by
//! `assistant_id` instead of repeating a graph name and config on every
//! call. Records live in memory and are persisted as one JSON file per
//! assistant under `{store_path}/assistants/{assistant_id}.json`; they are
//! reloaded when the router is built.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One assistant: a named alias for a graph with default config metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AssistantRecord {
    pub assistant_id: String,
    pub name: String,
    /// Registered graph this assistant runs.
    pub graph: String,
    /// Free-form config metadata; `config.recursion_limit` is honored as a
    /// run default, everything else is stored verbatim.
    #[serde(default)]
    pub config: Value,
    #[serde(default)]
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
}

/// The on-disk directory holding one JSON file per assistant.
pub(crate) fn dir(store_root: &Path) -> PathBuf {
    store_root.join("assistants")
}

/// Load all persisted assistants, skipping (with a warning) any file that
/// fails to parse.
pub(crate) fn load(store_root: &Path) -> HashMap<String, AssistantRecord> {
    let mut out = HashMap::new();
    let Ok(entries) = std::fs::read_dir(dir(store_root)) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let parsed = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<AssistantRecord>(&raw).ok());
        match parsed {
            Some(record) => {
                out.insert(record.assistant_id.clone(), record);
            }
            None => {
                tracing::warn!(path = %path.display(), "skipping unreadable assistant file")
            }
        }
    }
    out
}

/// Persist one assistant record (create or overwrite).
pub(crate) async fn persist(store_root: &Path, record: &AssistantRecord) -> std::io::Result<()> {
    let dir = dir(store_root);
    tokio::fs::create_dir_all(&dir).await?;
    let raw = serde_json::to_vec_pretty(record).expect("assistant serialization is infallible");
    tokio::fs::write(dir.join(format!("{}.json", record.assistant_id)), raw).await
}
