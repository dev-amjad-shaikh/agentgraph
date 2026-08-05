//! Checkpointing: thread-scoped, versioned state snapshots.
//!
//! Checkpoints happen **at super-step boundaries, never mid-node** — so on
//! resume the affected node re-runs from its start (node logic must be
//! idempotent). One primitive, four use cases: durable execution,
//! human-in-the-loop (interrupt → serialize → approve → resume), time travel
//! (load any historical checkpoint, fork alternate paths), and
//! partial-failure recovery.
//!
//! - [`InMemoryCheckpointer`] — RAM-only, lost on restart (dev/test).
//! - [`JsonFileCheckpointer`] — one JSON file per checkpoint under a
//!   directory, pure `serde_json` + `tokio::fs` (durable across restarts).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{AgentGraphError, Result};
use crate::state::State;

/// A versioned snapshot of one thread's state at a super-step boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Unique checkpoint id (UUID v4; also serves as a fork handle for
    /// time travel).
    pub id: String,

    /// The thread (session) this checkpoint belongs to. Threads cannot see
    /// each other's state.
    pub thread_id: String,

    /// Zero-based super-step index at whose boundary this snapshot was taken.
    pub step: usize,

    /// The full channel state at the boundary.
    pub state: State,

    /// The node set scheduled to run in the *next* super-step. Restored on
    /// resume so execution continues exactly where it suspended.
    pub next_nodes: Vec<String>,

    /// Wall-clock creation time (UTC).
    pub created_at: DateTime<Utc>,
}

impl Checkpoint {
    /// A new checkpoint with a fresh UUID v4 id and current timestamp.
    pub fn new(
        thread_id: impl Into<String>,
        step: usize,
        state: State,
        next_nodes: Vec<String>,
    ) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            thread_id: thread_id.into(),
            step,
            state,
            next_nodes,
            created_at: Utc::now(),
        }
    }
}

/// The checkpointer interface (the LangGraph `BaseCheckpointSaver` analog).
///
/// Implementations must be safe to share across tasks (`Send + Sync`) and
/// are typically held as `Arc<dyn Checkpointer>` by the executor.
#[async_trait]
pub trait Checkpointer: Send + Sync {
    /// Persist a checkpoint. Implementations must not overwrite an existing
    /// checkpoint with the same id (ids are unique by construction).
    async fn put(&self, checkpoint: Checkpoint) -> Result<()>;

    /// The most recent checkpoint for a thread, or `None` if the thread has
    /// never been checkpointed. Recency is defined by insertion order
    /// (monotonic super-steps), not wall-clock time.
    async fn get_latest(&self, thread_id: &str) -> Result<Option<Checkpoint>>;

    /// All checkpoints for a thread, oldest first (time-travel listing).
    async fn list(&self, thread_id: &str) -> Result<Vec<Checkpoint>>;
}

/// In-memory checkpointer: fast, thread-safe, lost on restart. Suitable for
/// development, tests, and ephemeral runs.
#[derive(Debug, Default, Clone)]
pub struct InMemoryCheckpointer {
    // thread_id -> checkpoints in insertion (super-step) order.
    inner: Arc<Mutex<HashMap<String, Vec<Checkpoint>>>>,
}

impl InMemoryCheckpointer {
    /// An empty in-memory store.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, Vec<Checkpoint>>>> {
        self.inner
            .lock()
            .map_err(|_| AgentGraphError::Checkpoint("in-memory checkpointer lock poisoned".into()))
    }
}

#[async_trait]
impl Checkpointer for InMemoryCheckpointer {
    async fn put(&self, checkpoint: Checkpoint) -> Result<()> {
        let mut guard = self.lock()?;
        let entry = guard.entry(checkpoint.thread_id.clone()).or_default();
        if entry.iter().any(|c| c.id == checkpoint.id) {
            return Err(AgentGraphError::Checkpoint(format!(
                "checkpoint id `{}` already exists for thread `{}`",
                checkpoint.id, checkpoint.thread_id
            )));
        }
        tracing::debug!(
            thread_id = %checkpoint.thread_id,
            checkpoint_id = %checkpoint.id,
            step = checkpoint.step,
            "checkpoint stored (in-memory)"
        );
        entry.push(checkpoint);
        Ok(())
    }

    async fn get_latest(&self, thread_id: &str) -> Result<Option<Checkpoint>> {
        let guard = self.lock()?;
        Ok(guard.get(thread_id).and_then(|v| v.last()).cloned())
    }

    async fn list(&self, thread_id: &str) -> Result<Vec<Checkpoint>> {
        let guard = self.lock()?;
        Ok(guard.get(thread_id).cloned().unwrap_or_default())
    }
}

/// File-backed checkpointer: one pretty-printed JSON file per checkpoint
/// (`{dir}/{thread_id}/{checkpoint_id}.json`) plus a `latest` pointer file
/// (`{dir}/{thread_id}/latest` holding the most recent checkpoint id), using
/// only `serde_json` and `tokio::fs` — no database dependencies.
///
/// Writes are atomic: payload is written to a uniquely named temp file in the
/// same directory and then renamed over the target path, so a crash mid-write
/// can never leave a truncated checkpoint file behind.
///
/// Read paths are forgiving: a missing thread directory yields `None` / an
/// empty list, a missing or corrupt `latest` pointer falls back to scanning
/// the checkpoint files, and individual corrupt checkpoint files are skipped
/// during scans. Genuine IO failures surface as
/// [`AgentGraphError::Checkpoint`].
#[derive(Debug, Clone)]
pub struct JsonFileCheckpointer {
    dir: PathBuf,
}

impl JsonFileCheckpointer {
    /// A checkpointer rooted at `dir` (created lazily on first `put`).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The root directory checkpoints are stored under.
    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    /// `{dir}/{thread_id}/` — per-thread subdirectory.
    fn thread_dir(&self, thread_id: &str) -> PathBuf {
        self.dir.join(thread_id)
    }

    /// `{dir}/{thread_id}/{checkpoint_id}.json`.
    fn checkpoint_path(&self, thread_id: &str, checkpoint_id: &str) -> PathBuf {
        self.thread_dir(thread_id)
            .join(format!("{checkpoint_id}.json"))
    }

    /// `{dir}/{thread_id}/latest` — pointer file holding the most recent
    /// checkpoint id (plain text).
    fn latest_path(&self, thread_id: &str) -> PathBuf {
        self.thread_dir(thread_id).join("latest")
    }

    /// Atomically write `bytes` to `path` via a unique temp file + rename.
    /// The temp file lives in the same directory so the rename stays on one
    /// filesystem. Best-effort temp cleanup on failure.
    async fn atomic_write(path: &PathBuf, bytes: &[u8]) -> Result<()> {
        let tmp = path.with_file_name(format!(
            ".{}.tmp-{}",
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "checkpoint".into()),
            uuid::Uuid::new_v4()
        ));
        if let Err(e) = tokio::fs::write(&tmp, bytes).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(AgentGraphError::Checkpoint(format!(
                "failed to write temp file `{}`: {e}",
                tmp.display()
            )));
        }
        if let Err(e) = tokio::fs::rename(&tmp, path).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(AgentGraphError::Checkpoint(format!(
                "failed to rename `{}` -> `{}`: {e}",
                tmp.display(),
                path.display()
            )));
        }
        Ok(())
    }

    /// Read and deserialize one checkpoint file, mapping IO and JSON errors
    /// to [`AgentGraphError::Checkpoint`].
    async fn read_checkpoint(path: &PathBuf) -> Result<Checkpoint> {
        let bytes = tokio::fs::read(path).await.map_err(|e| {
            AgentGraphError::Checkpoint(format!(
                "failed to read checkpoint file `{}`: {e}",
                path.display()
            ))
        })?;
        serde_json::from_slice(&bytes).map_err(|e| {
            AgentGraphError::Checkpoint(format!(
                "corrupt checkpoint file `{}`: {e}",
                path.display()
            ))
        })
    }

    /// Load every parseable `*.json` checkpoint in a thread directory,
    /// sorted by `(step, created_at)` ascending (oldest first). A missing
    /// directory yields an empty vec; corrupt files are skipped.
    async fn scan_thread(&self, thread_id: &str) -> Result<Vec<Checkpoint>> {
        let dir = self.thread_dir(thread_id);
        let mut entries = match tokio::fs::read_dir(&dir).await {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => {
                return Err(AgentGraphError::Checkpoint(format!(
                    "failed to read thread directory `{}`: {e}",
                    dir.display()
                )))
            }
        };

        let mut checkpoints = Vec::new();
        loop {
            let entry = match entries.next_entry().await {
                Ok(Some(entry)) => entry,
                Ok(None) => break,
                Err(e) => {
                    return Err(AgentGraphError::Checkpoint(format!(
                        "failed to iterate thread directory `{}`: {e}",
                        dir.display()
                    )))
                }
            };
            let path = entry.path();
            let is_json = path.extension().is_some_and(|ext| ext == "json");
            if !is_json {
                continue;
            }
            match Self::read_checkpoint(&path).await {
                Ok(cp) => checkpoints.push(cp),
                // Graceful degradation: one corrupt file must not poison the
                // whole thread's history.
                Err(e) => {
                    tracing::warn!(
                        thread_id = %thread_id,
                        path = %path.display(),
                        error = %e,
                        "skipping corrupt checkpoint file during scan"
                    );
                    continue;
                }
            }
        }
        checkpoints.sort_by(|a, b| a.step.cmp(&b.step).then(a.created_at.cmp(&b.created_at)));
        Ok(checkpoints)
    }
}

#[async_trait]
impl Checkpointer for JsonFileCheckpointer {
    async fn put(&self, checkpoint: Checkpoint) -> Result<()> {
        let thread_dir = self.thread_dir(&checkpoint.thread_id);
        tokio::fs::create_dir_all(&thread_dir).await.map_err(|e| {
            AgentGraphError::Checkpoint(format!(
                "failed to create thread directory `{}`: {e}",
                thread_dir.display()
            ))
        })?;

        let path = self.checkpoint_path(&checkpoint.thread_id, &checkpoint.id);
        // Preserve the no-overwrite contract: checkpoint ids are unique by
        // construction, so an existing file means a duplicate `put`.
        if tokio::fs::try_exists(&path).await.map_err(|e| {
            AgentGraphError::Checkpoint(format!(
                "failed to stat checkpoint file `{}`: {e}",
                path.display()
            ))
        })? {
            return Err(AgentGraphError::Checkpoint(format!(
                "checkpoint id `{}` already exists for thread `{}`",
                checkpoint.id, checkpoint.thread_id
            )));
        }

        let bytes = serde_json::to_vec_pretty(&checkpoint)?;
        Self::atomic_write(&path, &bytes).await?;

        // Update the latest pointer (also atomically). Written after the
        // checkpoint file itself so a crash never leaves a dangling pointer.
        Self::atomic_write(
            &self.latest_path(&checkpoint.thread_id),
            checkpoint.id.as_bytes(),
        )
        .await?;
        tracing::debug!(
            thread_id = %checkpoint.thread_id,
            checkpoint_id = %checkpoint.id,
            step = checkpoint.step,
            path = %path.display(),
            "checkpoint persisted (json file)"
        );
        Ok(())
    }

    async fn get_latest(&self, thread_id: &str) -> Result<Option<Checkpoint>> {
        let latest_path = self.latest_path(thread_id);
        // Fast path: follow the latest pointer. A missing/corrupt pointer or
        // a dangling target falls back to a full scan rather than failing.
        if let Ok(id_bytes) = tokio::fs::read(&latest_path).await {
            if let Ok(id) = std::str::from_utf8(&id_bytes) {
                let id = id.trim();
                if !id.is_empty() {
                    let path = self.checkpoint_path(thread_id, id);
                    match Self::read_checkpoint(&path).await {
                        Ok(cp) => return Ok(Some(cp)),
                        Err(e) => tracing::warn!(
                            thread_id = %thread_id,
                            path = %path.display(),
                            error = %e,
                            "latest pointer target unreadable; falling back to directory scan"
                        ),
                    }
                }
            }
        }
        // Fallback: highest-step checkpoint from a directory scan.
        Ok(self.scan_thread(thread_id).await?.into_iter().last())
    }

    async fn list(&self, thread_id: &str) -> Result<Vec<Checkpoint>> {
        self.scan_thread(thread_id).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cp(thread: &str, step: usize) -> Checkpoint {
        Checkpoint::new(thread, step, State::new(), vec!["next".into()])
    }

    #[tokio::test]
    async fn in_memory_roundtrip() {
        let store = InMemoryCheckpointer::new();
        assert!(store.get_latest("t1").await.unwrap().is_none());
        assert!(store.list("t1").await.unwrap().is_empty());

        store.put(cp("t1", 0)).await.unwrap();
        store.put(cp("t1", 1)).await.unwrap();
        store.put(cp("t2", 0)).await.unwrap();

        let latest = store.get_latest("t1").await.unwrap().unwrap();
        assert_eq!(latest.step, 1);
        assert_eq!(latest.next_nodes, vec!["next".to_string()]);

        let all = store.list("t1").await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].step, 0);
        assert_eq!(all[1].step, 1);

        // Threads are isolated.
        assert_eq!(store.list("t2").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn in_memory_rejects_duplicate_id() {
        let store = InMemoryCheckpointer::new();
        let checkpoint = cp("t1", 0);
        store.put(checkpoint.clone()).await.unwrap();
        let err = store.put(checkpoint).await.unwrap_err();
        assert!(matches!(err, AgentGraphError::Checkpoint(_)));
    }

    #[tokio::test]
    async fn checkpoint_serializes() {
        let checkpoint = cp("t1", 2);
        let json = serde_json::to_string(&checkpoint).unwrap();
        let back: Checkpoint = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, checkpoint.id);
        assert_eq!(back.step, 2);
    }

    /// Unique temp root under the OS temp dir, removed on drop.
    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!(
                "agentgraph-checkpoint-test-{}",
                uuid::Uuid::new_v4()
            )))
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn json_file_roundtrip() {
        let tmp = TestDir::new();
        let store = JsonFileCheckpointer::new(tmp.0.clone());

        assert!(store.get_latest("t1").await.unwrap().is_none());
        assert!(store.list("t1").await.unwrap().is_empty());

        let mut state = State::new();
        state.insert("answer", serde_json::json!(42));
        let cp0 = Checkpoint::new("t1", 0, state.clone(), vec!["node_b".into()]);
        let id0 = cp0.id.clone();
        store.put(cp0).await.unwrap();

        // File layout: <root>/<thread_id>/<checkpoint_id>.json
        assert!(tmp.0.join("t1").join(format!("{id0}.json")).exists());

        let back = store.get_latest("t1").await.unwrap().unwrap();
        assert_eq!(back.id, id0);
        assert_eq!(back.thread_id, "t1");
        assert_eq!(back.step, 0);
        assert_eq!(back.state, state);
        assert_eq!(back.next_nodes, vec!["node_b".to_string()]);

        // Threads are isolated.
        assert!(store.get_latest("t2").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn json_file_latest_pointer_tracks_most_recent_put() {
        let tmp = TestDir::new();
        let store = JsonFileCheckpointer::new(tmp.0.clone());

        store.put(cp("t1", 0)).await.unwrap();
        let cp1 = cp("t1", 1);
        let id1 = cp1.id.clone();
        store.put(cp1).await.unwrap();

        // The pointer file holds the most recent checkpoint id as text.
        let pointer = std::fs::read_to_string(tmp.0.join("t1").join("latest")).unwrap();
        assert_eq!(pointer, id1);

        let latest = store.get_latest("t1").await.unwrap().unwrap();
        assert_eq!(latest.id, id1);
        assert_eq!(latest.step, 1);
    }

    #[tokio::test]
    async fn json_file_list_sorted_by_step_regardless_of_put_order() {
        let tmp = TestDir::new();
        let store = JsonFileCheckpointer::new(tmp.0.clone());

        store.put(cp("t1", 2)).await.unwrap();
        store.put(cp("t1", 0)).await.unwrap();
        store.put(cp("t1", 1)).await.unwrap();

        let all = store.list("t1").await.unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].step, 0);
        assert_eq!(all[1].step, 1);
        assert_eq!(all[2].step, 2);

        // Latest falls back to the highest step via the pointer's target,
        // which is the most recent put (step 1) — insertion-order recency.
        // The pointer contract is "most recent put"; the scan fallback
        // returns highest step. Both are valid; here the pointer wins.
        let latest = store.get_latest("t1").await.unwrap().unwrap();
        assert_eq!(latest.step, 1);
    }

    #[tokio::test]
    async fn json_file_missing_thread_returns_none_and_empty() {
        let tmp = TestDir::new();
        let store = JsonFileCheckpointer::new(tmp.0.clone());

        assert!(store.get_latest("never-seen").await.unwrap().is_none());
        assert!(store.list("never-seen").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn json_file_rejects_duplicate_id() {
        let tmp = TestDir::new();
        let store = JsonFileCheckpointer::new(tmp.0.clone());

        let checkpoint = cp("t1", 0);
        store.put(checkpoint.clone()).await.unwrap();
        let err = store.put(checkpoint).await.unwrap_err();
        assert!(matches!(err, AgentGraphError::Checkpoint(_)));
    }

    #[tokio::test]
    async fn json_file_durable_across_instances() {
        let tmp = TestDir::new();
        let cp0 = cp("t1", 0);
        let id0 = cp0.id.clone();

        JsonFileCheckpointer::new(tmp.0.clone())
            .put(cp0)
            .await
            .unwrap();

        // A fresh instance over the same root sees the checkpoint
        // (simulates process restart).
        let reopened = JsonFileCheckpointer::new(tmp.0.clone());
        let latest = reopened.get_latest("t1").await.unwrap().unwrap();
        assert_eq!(latest.id, id0);
        assert_eq!(reopened.list("t1").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn json_file_corrupt_files_are_handled_gracefully() {
        let tmp = TestDir::new();
        let store = JsonFileCheckpointer::new(tmp.0.clone());

        let good = cp("t1", 0);
        let good_id = good.id.clone();
        store.put(good).await.unwrap();

        // A corrupt checkpoint file next to a valid one must not break
        // list/get_latest.
        std::fs::write(tmp.0.join("t1").join("garbage.json"), b"{not json!!").unwrap();
        let all = store.list("t1").await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, good_id);

        // A corrupt latest pointer falls back to scanning.
        std::fs::write(tmp.0.join("t1").join("latest"), b"no-such-checkpoint-id").unwrap();
        let latest = store.get_latest("t1").await.unwrap().unwrap();
        assert_eq!(latest.id, good_id);

        // Garbage bytes in the pointer also fall back to scanning.
        std::fs::write(tmp.0.join("t1").join("latest"), [0xff, 0xfe, 0x00]).unwrap();
        let latest = store.get_latest("t1").await.unwrap().unwrap();
        assert_eq!(latest.id, good_id);
    }
}
