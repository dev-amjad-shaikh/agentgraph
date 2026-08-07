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

use crate::error::{Result, RustyError};
use crate::record::{CheckpointHeader, JournalRef};
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

    /// Flight Recorder provenance (R0.5): checkpoint format version, graph
    /// version/content hash, active policy version, and the run's logical
    /// clock value at creation. See [`CheckpointHeader`] for the semantics.
    ///
    /// `#[serde(default)]` keeps checkpoints written before R0.5 (which have
    /// no header field) deserializable: they load with
    /// [`CheckpointHeader::default`] — current format version, unversioned
    /// graph, static policy.
    #[serde(default)]
    pub header: CheckpointHeader,

    /// The journal head at this boundary (`None` pre-R0.5), binding this
    /// state snapshot to the run evidence that produced it.
    #[serde(default)]
    pub journal_ref: Option<JournalRef>,
}

impl Checkpoint {
    /// A new checkpoint with a fresh UUID v4 id and current timestamp.
    ///
    /// Convenience constructor used by tests and pre-R0.5 call paths: the
    /// header falls back to [`CheckpointHeader::default`] and no journal
    /// reference is stamped. The executor mints checkpoints field-by-field
    /// instead, sourcing id/timestamp from the run's determinism seams and
    /// stamping the real header and journal head.
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
            header: CheckpointHeader::default(),
            journal_ref: None,
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
    /// never been checkpointed. Recency is defined by **insertion (put)
    /// order — the last successfully stored checkpoint wins** — not by
    /// super-step number: replay on the same thread legitimately appends
    /// checkpoints whose `step` is at or below the existing head, and a
    /// later resume must continue that newest timeline.
    ///
    /// Backends without an explicit insertion sequence use `created_at` as
    /// the insertion proxy. That is exact as long as checkpoints are minted
    /// fresh ([`Checkpoint::new`]) when stored and forked histories are
    /// copied oldest-first, which [`Checkpointer::fork_thread`] does.
    async fn get_latest(&self, thread_id: &str) -> Result<Option<Checkpoint>>;

    /// All checkpoints for a thread, oldest first (time-travel listing).
    ///
    /// The order is total and identical across backends — ascending
    /// `(step, created_at, id)` — so that [`Checkpointer::fork_thread`]'s
    /// truncation-by-position is deterministic even when replay has appended
    /// several checkpoints sharing the same `step`.
    async fn list(&self, thread_id: &str) -> Result<Vec<Checkpoint>>;

    /// Fetch one specific checkpoint of a thread by id (time-travel handle).
    ///
    /// The default implementation lists the thread and finds the id, which is
    /// correct (if not maximally efficient) for every reasonable backend.
    /// Returns `None` when the thread has no checkpoint with that id.
    async fn get_by_id(&self, thread_id: &str, checkpoint_id: &str) -> Result<Option<Checkpoint>> {
        let all = self.list(thread_id).await?;
        Ok(all.into_iter().find(|c| c.id == checkpoint_id))
    }

    /// Fork a thread's history into a new thread (time travel).
    ///
    /// Copies the source thread's checkpoints — oldest first — into
    /// `dst_thread`, preserving each checkpoint's `id`, `step`, `state`,
    /// `next_nodes`, and `created_at`; only the `thread_id` changes. When
    /// `at_checkpoint_id` is given, only checkpoints up to and including that
    /// checkpoint are copied (fork from a mid-history point); when `None`,
    /// the full history is copied. Returns the number of checkpoints copied.
    ///
    /// The default implementation re-`put`s each selected checkpoint with the
    /// destination thread id. This is correct for every implementation whose
    /// `put` uniqueness scope is per-thread — including both built-in impls
    /// ([`InMemoryCheckpointer`] keys its map by thread, and
    /// [`JsonFileCheckpointer`] stores under `{dir}/{thread_id}/`), so reused
    /// ids cannot collide across threads. An implementation whose `put`
    /// enforces globally unique ids, or whose storage path ignores
    /// `checkpoint.thread_id`, **must override this method** (e.g. a SQL
    /// backend with a global primary key would mint fresh ids or insert with
    /// an explicit thread column).
    ///
    /// Errors when the source thread has no checkpoints, when
    /// `at_checkpoint_id` does not exist on the source thread, or when
    /// `src_thread == dst_thread` (ids would collide within one thread).
    async fn fork_thread(
        &self,
        src_thread: &str,
        dst_thread: &str,
        at_checkpoint_id: Option<&str>,
    ) -> Result<usize> {
        if src_thread == dst_thread {
            return Err(RustyError::Checkpoint(format!(
                "cannot fork thread `{src_thread}` onto itself: checkpoint ids would collide"
            )));
        }
        let all = self.list(src_thread).await?;
        if all.is_empty() {
            return Err(RustyError::Checkpoint(format!(
                "cannot fork thread `{src_thread}`: no checkpoints found"
            )));
        }
        let selected: Vec<Checkpoint> = match at_checkpoint_id {
            None => all,
            Some(id) => {
                let pos = all.iter().position(|c| c.id == id).ok_or_else(|| {
                    RustyError::Checkpoint(format!(
                        "cannot fork thread `{src_thread}`: unknown checkpoint id `{id}`"
                    ))
                })?;
                all[..=pos].to_vec()
            }
        };
        let copied = selected.len();
        for mut checkpoint in selected {
            checkpoint.thread_id = dst_thread.to_string();
            self.put(checkpoint).await?;
        }
        tracing::info!(
            src_thread = %src_thread,
            dst_thread = %dst_thread,
            copied = copied,
            "thread history forked"
        );
        Ok(copied)
    }
}

/// In-memory checkpointer: thread-safe (all operations take a single mutex
/// over the store), lost on restart. Suitable for development, tests, and
/// ephemeral runs.
#[derive(Debug, Default, Clone)]
pub struct InMemoryCheckpointer {
    // thread_id -> checkpoints in insertion (super-step) order.
    inner: Arc<Mutex<HashMap<String, Vec<Checkpoint>>>>,
}

impl InMemoryCheckpointer {
    /// An empty store. Clones of the returned checkpointer share the same
    /// underlying map.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, Vec<Checkpoint>>>> {
        self.inner
            .lock()
            .map_err(|_| RustyError::Checkpoint("in-memory checkpointer lock poisoned".into()))
    }
}

#[async_trait]
impl Checkpointer for InMemoryCheckpointer {
    async fn put(&self, checkpoint: Checkpoint) -> Result<()> {
        let mut guard = self.lock()?;
        let entry = guard.entry(checkpoint.thread_id.clone()).or_default();
        if entry.iter().any(|c| c.id == checkpoint.id) {
            return Err(RustyError::Checkpoint(format!(
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
        let mut all = guard.get(thread_id).cloned().unwrap_or_default();
        // Same total order as every other backend (ascending
        // `(step, created_at, id)`), not raw insertion order: replay can
        // append out-of-step-order checkpoints, and fork truncation must be
        // deterministic across backends.
        all.sort_by(|a, b| {
            a.step
                .cmp(&b.step)
                .then(a.created_at.cmp(&b.created_at))
                .then(a.id.cmp(&b.id))
        });
        Ok(all)
    }
}

/// File-backed checkpointer: one pretty-printed JSON file per checkpoint
/// (`{dir}/{thread_id}/{checkpoint_id}.json`) plus a `latest` pointer file
/// (`{dir}/{thread_id}/latest` holding the most recent checkpoint id), using
/// only `serde_json` and `tokio::fs` — no database dependencies.
///
/// Writes are atomic: payload is written to a uniquely named temp file in the
/// same directory and then renamed over the target path, so a crash mid-write
/// can never leave a truncated checkpoint file behind. Puts are serialized
/// per thread (an in-process lock per `thread_id`), so concurrent same-thread
/// puts cannot interleave the checkpoint file and pointer writes and leave
/// `latest` pointing at the older checkpoint. The lock is per-process:
/// multiple writer PROCESSES over the same directory are not serialized —
/// treat one writer process per thread directory as a precondition.
///
/// Read paths are forgiving: a missing thread directory yields `None` / an
/// empty list, a missing or corrupt `latest` pointer falls back to scanning
/// the checkpoint files, and individual corrupt checkpoint files are skipped
/// during scans. Genuine IO failures surface as
/// [`RustyError::Checkpoint`].
#[derive(Debug, Clone)]
pub struct JsonFileCheckpointer {
    dir: PathBuf,
    // Per-thread put locks behind a map mutex: the map is locked only long
    // enough to clone the per-thread `Arc`, never across the put itself.
    put_locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
}

impl JsonFileCheckpointer {
    /// A checkpointer rooted at `dir` (created lazily on first `put`).
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            put_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The root directory checkpoints are stored under.
    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    /// The lock serializing `put` for one thread. `Clone`s of this
    /// checkpointer share the same lock map, so clones still serialize
    /// against each other.
    fn put_lock(&self, thread_id: &str) -> Result<Arc<tokio::sync::Mutex<()>>> {
        let mut map = self
            .put_locks
            .lock()
            .map_err(|_| RustyError::Checkpoint("put-lock map poisoned".into()))?;
        Ok(map
            .entry(thread_id.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone())
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
            return Err(RustyError::Checkpoint(format!(
                "failed to write temp file `{}`: {e}",
                tmp.display()
            )));
        }
        if let Err(e) = tokio::fs::rename(&tmp, path).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err(RustyError::Checkpoint(format!(
                "failed to rename `{}` -> `{}`: {e}",
                tmp.display(),
                path.display()
            )));
        }
        Ok(())
    }

    /// Read and deserialize one checkpoint file, mapping IO and JSON errors
    /// to [`RustyError::Checkpoint`].
    async fn read_checkpoint(path: &PathBuf) -> Result<Checkpoint> {
        let bytes = tokio::fs::read(path).await.map_err(|e| {
            RustyError::Checkpoint(format!(
                "failed to read checkpoint file `{}`: {e}",
                path.display()
            ))
        })?;
        serde_json::from_slice(&bytes).map_err(|e| {
            RustyError::Checkpoint(format!("corrupt checkpoint file `{}`: {e}", path.display()))
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
                return Err(RustyError::Checkpoint(format!(
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
                    return Err(RustyError::Checkpoint(format!(
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
        checkpoints.sort_by(|a, b| {
            a.step
                .cmp(&b.step)
                .then(a.created_at.cmp(&b.created_at))
                .then(a.id.cmp(&b.id))
        });
        Ok(checkpoints)
    }
}

#[async_trait]
impl Checkpointer for JsonFileCheckpointer {
    async fn put(&self, checkpoint: Checkpoint) -> Result<()> {
        // Serialize the whole put (checkpoint file THEN pointer file) per
        // thread: without this, two concurrent same-thread puts can
        // interleave (file A, file B, pointer B, pointer A) and leave
        // `latest` pointing at the older checkpoint. Held across `.await`s,
        // hence a tokio mutex — a std guard would make the future `!Send`.
        let lock = self.put_lock(&checkpoint.thread_id)?;
        let _put_guard = lock.lock().await;

        let thread_dir = self.thread_dir(&checkpoint.thread_id);
        tokio::fs::create_dir_all(&thread_dir).await.map_err(|e| {
            RustyError::Checkpoint(format!(
                "failed to create thread directory `{}`: {e}",
                thread_dir.display()
            ))
        })?;

        let path = self.checkpoint_path(&checkpoint.thread_id, &checkpoint.id);
        // Preserve the no-overwrite contract: checkpoint ids are unique by
        // construction, so an existing file means a duplicate `put`.
        if tokio::fs::try_exists(&path).await.map_err(|e| {
            RustyError::Checkpoint(format!(
                "failed to stat checkpoint file `{}`: {e}",
                path.display()
            ))
        })? {
            return Err(RustyError::Checkpoint(format!(
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
        // Fallback: the checkpoint with the greatest `created_at` — the
        // insertion-order proxy shared with the other backends (see the
        // trait's `get_latest` contract), not the highest step, so a replay
        // that appended lower-step checkpoints still resumes the newest
        // timeline.
        Ok(self
            .scan_thread(thread_id)
            .await?
            .into_iter()
            .max_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id))))
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
        assert!(matches!(err, RustyError::Checkpoint(_)));
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
            Self(
                std::env::temp_dir()
                    .join(format!("rusty-checkpoint-test-{}", uuid::Uuid::new_v4())),
            )
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

        // Recency is insertion order, not highest step: the pointer tracks
        // the most recent put (step 1), and the scan fallback agrees because
        // the freshest `created_at` is also the last put's.
        let latest = store.get_latest("t1").await.unwrap().unwrap();
        assert_eq!(latest.step, 1);
    }

    /// The `get_latest`/`list` contract is backend-independent: recency =
    /// insertion order, listing = ascending `(step, created_at, id)`. Every
    /// `Checkpointer` impl must agree with this test.
    #[tokio::test]
    async fn recency_and_list_order_agree_across_backends() {
        let tmp = TestDir::new();
        let memory = InMemoryCheckpointer::new();
        let json_file = JsonFileCheckpointer::new(tmp.0.clone());

        // Out-of-step-order puts (as replay-on-same-thread produces): each
        // checkpoint is minted fresh, so `created_at` increases per put.
        let steps = [2usize, 0, 1];
        for step in steps {
            memory.put(cp("t1", step)).await.unwrap();
            json_file.put(cp("t1", step)).await.unwrap();
        }

        let stores: [&dyn Checkpointer; 2] = [&memory, &json_file];
        for store in stores {
            // Latest = last put (step 1), not highest step (step 2).
            let latest = store.get_latest("t1").await.unwrap().unwrap();
            assert_eq!(latest.step, 1, "backend disagrees on recency");
            // List = ascending step order regardless of put order.
            let listed: Vec<usize> = store
                .list("t1")
                .await
                .unwrap()
                .iter()
                .map(|c| c.step)
                .collect();
            assert_eq!(listed, [0, 1, 2], "backend disagrees on list order");
        }
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
        assert!(matches!(err, RustyError::Checkpoint(_)));
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

    #[tokio::test]
    async fn get_by_id_hit_and_miss() {
        let store = InMemoryCheckpointer::new();
        let cp0 = cp("t1", 0);
        let id0 = cp0.id.clone();
        store.put(cp0).await.unwrap();
        store.put(cp("t1", 1)).await.unwrap();
        store.put(cp("t2", 0)).await.unwrap();

        let hit = store.get_by_id("t1", &id0).await.unwrap().unwrap();
        assert_eq!(hit.id, id0);
        assert_eq!(hit.step, 0);
        assert_eq!(hit.thread_id, "t1");

        // Unknown id on an existing thread, and any id on an unknown thread.
        assert!(store.get_by_id("t1", "no-such-id").await.unwrap().is_none());
        assert!(store.get_by_id("t2", &id0).await.unwrap().is_none());
        assert!(store.get_by_id("never-seen", &id0).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn fork_full_history_copies_all_checkpoints() {
        let store = InMemoryCheckpointer::new();
        let mut src = Vec::new();
        for step in 0..3 {
            let mut state = State::new();
            state.insert("n", serde_json::json!(step));
            let checkpoint = Checkpoint::new("src", step, state, vec!["next".into()]);
            src.push(checkpoint.clone());
            store.put(checkpoint).await.unwrap();
        }

        let copied = store.fork_thread("src", "dst", None).await.unwrap();
        assert_eq!(copied, 3);

        let dst = store.list("dst").await.unwrap();
        assert_eq!(dst.len(), 3);
        for (forked, original) in dst.iter().zip(src.iter()) {
            // Everything is preserved except the thread id (ids may be reused
            // across threads; uniqueness is per-thread).
            assert_eq!(forked.id, original.id);
            assert_eq!(forked.step, original.step);
            assert_eq!(forked.state, original.state);
            assert_eq!(forked.next_nodes, original.next_nodes);
            assert_eq!(forked.created_at, original.created_at);
            assert_eq!(forked.thread_id, "dst");
        }
        assert_eq!(store.get_latest("dst").await.unwrap().unwrap().step, 2);

        // The source thread is untouched.
        assert_eq!(store.list("src").await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn fork_at_mid_checkpoint_truncates_history() {
        let store = InMemoryCheckpointer::new();
        let mut ids = Vec::new();
        for step in 0..4 {
            let checkpoint = cp("src", step);
            ids.push(checkpoint.id.clone());
            store.put(checkpoint).await.unwrap();
        }

        // Fork at the step-1 checkpoint: only steps 0 and 1 are copied.
        let copied = store
            .fork_thread("src", "dst", Some(&ids[1]))
            .await
            .unwrap();
        assert_eq!(copied, 2);

        let dst = store.list("dst").await.unwrap();
        assert_eq!(dst.len(), 2);
        assert_eq!(dst[0].id, ids[0]);
        assert_eq!(dst[1].id, ids[1]);
        assert_eq!(dst[0].step, 0);
        assert_eq!(dst[1].step, 1);
        // Latest of the fork is the cut point, not the source's head.
        assert_eq!(store.get_latest("dst").await.unwrap().unwrap().id, ids[1]);
    }

    #[tokio::test]
    async fn fork_errors_on_empty_src_unknown_id_and_self_fork() {
        let store = InMemoryCheckpointer::new();
        let checkpoint = cp("src", 0);
        let id0 = checkpoint.id.clone();
        store.put(checkpoint).await.unwrap();

        let err = store.fork_thread("empty", "dst", None).await.unwrap_err();
        assert!(matches!(err, RustyError::Checkpoint(_)));

        let err = store
            .fork_thread("src", "dst", Some("no-such-id"))
            .await
            .unwrap_err();
        assert!(matches!(err, RustyError::Checkpoint(_)));

        let err = store
            .fork_thread("src", "src", Some(&id0))
            .await
            .unwrap_err();
        assert!(matches!(err, RustyError::Checkpoint(_)));

        // Failed forks leave no partial state behind on the destination.
        assert!(store.list("dst").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn json_file_fork_across_threads_persists_correctly() {
        let tmp = TestDir::new();
        let store = JsonFileCheckpointer::new(tmp.0.clone());

        let mut ids = Vec::new();
        for step in 0..3 {
            let mut state = State::new();
            state.insert("n", serde_json::json!(step));
            let checkpoint = Checkpoint::new("src", step, state, vec!["next".into()]);
            ids.push(checkpoint.id.clone());
            store.put(checkpoint).await.unwrap();
        }

        let copied = store
            .fork_thread("src", "dst", Some(&ids[1]))
            .await
            .unwrap();
        assert_eq!(copied, 2);

        // Files land under the destination thread's own directory (reused
        // ids live in a different path, so no collision).
        assert!(tmp.0.join("dst").join(format!("{}.json", ids[0])).exists());
        assert!(tmp.0.join("dst").join(format!("{}.json", ids[1])).exists());
        assert!(!tmp.0.join("dst").join(format!("{}.json", ids[2])).exists());

        // The forked files carry the destination thread id in their payload.
        let latest = store.get_latest("dst").await.unwrap().unwrap();
        assert_eq!(latest.id, ids[1]);
        assert_eq!(latest.thread_id, "dst");
        assert_eq!(latest.step, 1);

        // Durable across instances (process restart): the fork survives.
        let reopened = JsonFileCheckpointer::new(tmp.0.clone());
        let dst = reopened.list("dst").await.unwrap();
        assert_eq!(dst.len(), 2);
        assert_eq!(dst[0].id, ids[0]);
        assert_eq!(dst[1].id, ids[1]);
        assert!(dst.iter().all(|c| c.thread_id == "dst"));

        // The source thread is untouched.
        assert_eq!(reopened.list("src").await.unwrap().len(), 3);
        assert_eq!(
            reopened.get_latest("src").await.unwrap().unwrap().id,
            ids[2]
        );
    }
}
