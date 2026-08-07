//! Cross-thread key-value store, JSON-file-backed under the store root.
//!
//! Items are namespaced: `PUT /store/{namespace}/{key}` writes an arbitrary
//! JSON value, persisted as `{store_path}/store/{namespace}/{key}.json`
//! with `{namespace, key, value, created_at, updated_at}` inside. There is
//! no in-memory index — reads, lists, and deletes go straight to the file
//! system, so the store survives restarts by construction. Namespace and
//! key segments are restricted to `[A-Za-z0-9._-]` (1–128 chars) to keep
//! the mapping to paths unambiguous.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ApiError;

/// One stored item as persisted on disk and returned over the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct StoreItem {
    pub namespace: String,
    pub key: String,
    pub value: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Reject segments that could escape the store root or collide on disk:
/// anything outside `[A-Za-z0-9._-]` (1–128 chars), or all-dots segments
/// (`.`, `..`, `…`) that would resolve as parent-directory components.
pub(crate) fn validate_segment(kind: &str, segment: &str) -> Result<(), ApiError> {
    let ok = !segment.is_empty()
        && segment.len() <= 128
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && !segment.chars().all(|c| c == '.');
    if ok {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!(
            "invalid {kind} `{segment}` (allowed: [A-Za-z0-9._-], 1..=128 chars)"
        )))
    }
}

fn namespace_dir(store_root: &Path, namespace: &str) -> PathBuf {
    store_root.join("store").join(namespace)
}

fn item_path(store_root: &Path, namespace: &str, key: &str) -> PathBuf {
    namespace_dir(store_root, namespace).join(format!("{key}.json"))
}

/// Read one item (`None` when absent). A corrupt item file reads as absent
/// — but loudly, matching `list`'s warn-and-skip behavior, since silent
/// corruption would make a later `put` answer `201` and reset `created_at`.
pub(crate) async fn get(
    store_root: &Path,
    namespace: &str,
    key: &str,
) -> std::io::Result<Option<StoreItem>> {
    let path = item_path(store_root, namespace, key);
    let raw = match tokio::fs::read(&path).await {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    match serde_json::from_slice(&raw) {
        Ok(item) => Ok(Some(item)),
        Err(error) => {
            tracing::warn!(path = %path.display(), %error, "skipping corrupt store item");
            Ok(None)
        }
    }
}

/// Insert or replace one item. Returns the record plus `true` when the key
/// was newly created (creation time is preserved on overwrite).
pub(crate) async fn put(
    store_root: &Path,
    namespace: &str,
    key: &str,
    value: Value,
) -> std::io::Result<(StoreItem, bool)> {
    let existing = get(store_root, namespace, key).await?;
    let now = Utc::now();
    let item = StoreItem {
        namespace: namespace.to_string(),
        key: key.to_string(),
        value,
        created_at: existing.as_ref().map(|i| i.created_at).unwrap_or(now),
        updated_at: now,
    };
    let created = existing.is_none();
    let dir = namespace_dir(store_root, namespace);
    tokio::fs::create_dir_all(&dir).await?;
    let raw = serde_json::to_vec_pretty(&item).expect("store item serialization is infallible");
    tokio::fs::write(item_path(store_root, namespace, key), raw).await?;
    Ok((item, created))
}

/// Delete one item. Returns `true` when it existed.
pub(crate) async fn delete(store_root: &Path, namespace: &str, key: &str) -> std::io::Result<bool> {
    match tokio::fs::remove_file(item_path(store_root, namespace, key)).await {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// List all items in one namespace, sorted by key. A missing namespace
/// lists as empty; unreadable entries are skipped with a warning.
pub(crate) async fn list(store_root: &Path, namespace: &str) -> std::io::Result<Vec<StoreItem>> {
    let dir = namespace_dir(store_root, namespace);
    let mut entries = match tokio::fs::read_dir(&dir).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut items = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match tokio::fs::read(&path)
            .await
            .ok()
            .and_then(|raw| serde_json::from_slice::<StoreItem>(&raw).ok())
        {
            Some(item) => items.push(item),
            None => tracing::warn!(path = %path.display(), "skipping unreadable store item"),
        }
    }
    items.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(items)
}
