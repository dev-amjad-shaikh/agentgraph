//! Server-side persistence for the platform surface: assistants, crons,
//! threads, and the cross-thread KV store.
//!
//! [`ServerStore`] is the async CRUD trait the routes program against. Two
//! implementations ship:
//!
//! - [`JsonFileStore`] — the default. Existing v0.2 behavior, extracted:
//!   assistants, crons, and threads live in an in-memory index persisted as
//!   one JSON file per record under `{store_path}/{assistants,crons,threads}/`;
//!   KV items are pure file-backed reads/writes under `{store_path}/store/`;
//!   Flight Recorder journals are one file per run under
//!   `{store_path}/journals/`; durable tasks are an in-memory index persisted
//!   as one file per task under `{store_path}/tasks/` (R0.6).
//! - [`PostgresStore`] (feature `postgres`) — tables `server_assistants`,
//!   `server_crons`, `server_threads`, `server_kv`, `server_journals`, and
//!   `server_tasks` (the R0.6 durable task queue, column-mapped for
//!   `FOR UPDATE SKIP LOCKED` claiming), auto-migrated on (lazy) connect.
//!   Selected via `ServerConfig::with_postgres(url)`.
//!
//! All trait errors are plain `String`s; routes map them to 500s — no store
//! error is ever a client error (validation happens before the store call).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rusty_agent_runtime::journal::JournalSnapshot;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::assistants::{self, AssistantRecord};
use crate::crons::{self, CronRecord};
use crate::journals;
use crate::store::{self, StoreItem};
use crate::tasks::{self, MutationOutcome, TaskRecord, TaskStatus};
use crate::threads::{self, ThreadRecord};

/// Store operation result. The `String` payload is a 500-class internal
/// failure (IO error, DB error, serialization bug).
pub(crate) type StoreResult<T> = Result<T, String>;

/// Async CRUD backing the assistants / crons / KV routes.
///
/// `create_*` methods are check-and-insert: they return `false` when the id
/// already exists **without writing**, so routes can answer 409. `upsert_*`
/// unconditionally overwrites (used by the cron scheduler's bookkeeping).
/// The KV `kv_put` returns the item plus a `created` flag, preserving
/// `created_at` across overwrites.
#[async_trait::async_trait]
pub(crate) trait ServerStore: Send + Sync {
    /// Insert a new assistant; `false` (no write) when the id exists.
    async fn create_assistant(&self, record: &AssistantRecord) -> StoreResult<bool>;
    /// Fetch one assistant by id.
    async fn get_assistant(&self, assistant_id: &str) -> StoreResult<Option<AssistantRecord>>;
    /// All assistants (order unspecified; routes sort).
    async fn list_assistants(&self) -> StoreResult<Vec<AssistantRecord>>;

    /// Insert a new cron; `false` (no write) when the id exists.
    async fn create_cron(&self, record: &CronRecord) -> StoreResult<bool>;
    /// Overwrite a cron (scheduler bookkeeping: `last_run_at`, `runs_fired`).
    async fn upsert_cron(&self, record: &CronRecord) -> StoreResult<()>;
    /// Fetch one cron by id.
    async fn get_cron(&self, cron_id: &str) -> StoreResult<Option<CronRecord>>;
    /// All crons (order unspecified; routes sort).
    async fn list_crons(&self) -> StoreResult<Vec<CronRecord>>;
    /// Delete a cron; `true` when it existed.
    async fn delete_cron(&self, cron_id: &str) -> StoreResult<bool>;

    /// Insert a new thread under its internal (tenant-scoped) id; `false`
    /// (no write) when the id exists. Thread records are durable so
    /// pre-restart checkpoints stay reachable through the API.
    async fn create_thread(&self, internal_id: &str, record: &ThreadRecord) -> StoreResult<bool>;
    /// Fetch one thread by internal (tenant-scoped) id.
    async fn get_thread(&self, internal_id: &str) -> StoreResult<Option<ThreadRecord>>;

    /// Insert or replace a KV item. Returns the stored item plus `true`
    /// when the key was newly created (`created_at` preserved on replace).
    async fn kv_put(
        &self,
        namespace: &str,
        key: &str,
        value: Value,
    ) -> StoreResult<(StoreItem, bool)>;
    /// Fetch one KV item (`None` when absent).
    async fn kv_get(&self, namespace: &str, key: &str) -> StoreResult<Option<StoreItem>>;
    /// Delete one KV item; `true` when it existed.
    async fn kv_delete(&self, namespace: &str, key: &str) -> StoreResult<bool>;
    /// All items in one namespace, sorted by key (empty for unknown
    /// namespaces).
    async fn kv_list(&self, namespace: &str) -> StoreResult<Vec<StoreItem>>;

    /// Persist a run's Flight Recorder journal snapshot, replacing any
    /// earlier snapshot of the same run (the journal grows at every
    /// checkpoint boundary; the final write lands at run completion).
    async fn put_journal(&self, snapshot: &JournalSnapshot) -> StoreResult<()>;
    /// Fetch the journal snapshot stored for `run_id` (`None` when none was
    /// persisted — e.g. a queued run, or one that failed before its first
    /// checkpoint boundary).
    async fn get_journal(&self, run_id: &str) -> StoreResult<Option<JournalSnapshot>>;

    // -- Durable task queue (R0.6) -------------------------------------- //

    /// Enqueue a task. With an idempotency key, a live task already carrying
    /// that key (same tenant) is returned unchanged with `deduplicated:
    /// true` — enqueue is safe to retry. Without a key the insert always
    /// creates (`false`).
    async fn enqueue_task(&self, record: &TaskRecord) -> StoreResult<(TaskRecord, bool)>;
    /// Atomically claim the oldest claimable task in `pools` for `worker_id`
    /// (tenant-scoped): queued tasks, backoff-elapsed failed tasks, and
    /// leased tasks past their visibility timeout. `None` when nothing is
    /// claimable (route answers 204).
    async fn claim_task(
        &self,
        tenant: &str,
        worker_id: &str,
        pools: &[String],
        lease_ms: u64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> StoreResult<Option<TaskRecord>>;
    /// Extend the lease held by `worker_id` (heartbeat).
    async fn heartbeat_task(
        &self,
        tenant: &str,
        task_id: &str,
        worker_id: &str,
        lease_ms: u64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> StoreResult<MutationOutcome>;
    /// Settle the task held by `worker_id` as completed, storing `result`.
    async fn complete_task(
        &self,
        tenant: &str,
        task_id: &str,
        worker_id: &str,
        result: Value,
        now: chrono::DateTime<chrono::Utc>,
    ) -> StoreResult<MutationOutcome>;
    /// Record a failed attempt on the task held by `worker_id`: requeue with
    /// backoff, dead-letter, or fail outright — decided by core's shared
    /// [`classify_retry`](rusty_agent_runtime::durable::classify_retry)
    /// policy inside [`crate::tasks::TaskRecord::fail`].
    async fn fail_task(
        &self,
        tenant: &str,
        task_id: &str,
        worker_id: &str,
        report: tasks::FailureReport,
        now: chrono::DateTime<chrono::Utc>,
    ) -> StoreResult<MutationOutcome>;
    /// Fetch one task, tenant-scoped (`None` for unknown or cross-tenant
    /// ids — the two are indistinguishable by design).
    async fn get_task(&self, tenant: &str, task_id: &str) -> StoreResult<Option<TaskRecord>>;
    /// List a tenant's tasks, optionally filtered to one status (the DLQ
    /// listing is `status == dead`), oldest first.
    async fn list_tasks(
        &self,
        tenant: &str,
        status: Option<TaskStatus>,
    ) -> StoreResult<Vec<TaskRecord>>;
}

// --------------------------------------------------------------------- //
// JsonFileStore — default, extracted v0.2 behavior
// --------------------------------------------------------------------- //

/// JSON-file-backed store rooted at `ServerConfig::store_path`.
///
/// Assistants, crons, and threads are served from an in-memory index
/// (loaded from disk at construction) with one file per record written
/// through on every mutation — exactly the v0.2 route behavior. KV items go
/// straight to the file system, serialized by `kv_lock` so `created_at`
/// preservation cannot race.
pub(crate) struct JsonFileStore {
    root: PathBuf,
    assistants: Mutex<HashMap<String, AssistantRecord>>,
    crons: Mutex<HashMap<String, CronRecord>>,
    threads: Mutex<HashMap<String, ThreadRecord>>,
    kv_lock: Mutex<()>,
    tasks: Mutex<HashMap<String, TaskRecord>>,
}

impl JsonFileStore {
    /// Load the persisted assistants/crons/threads under `root` into memory.
    pub(crate) fn load(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            assistants: Mutex::new(assistants::load(root)),
            crons: Mutex::new(crons::load(root)),
            threads: Mutex::new(threads::load(root)),
            kv_lock: Mutex::new(()),
            tasks: Mutex::new(tasks::load(root)),
        }
    }
}

fn io_err(context: &str) -> impl Fn(std::io::Error) -> String + '_ {
    move |e| format!("{context}: {e}")
}

#[async_trait::async_trait]
impl ServerStore for JsonFileStore {
    async fn create_assistant(&self, record: &AssistantRecord) -> StoreResult<bool> {
        let mut map = self.assistants.lock().await;
        if map.contains_key(&record.assistant_id) {
            return Ok(false);
        }
        // Hold the lock across the file write so a concurrent create of the
        // same id can't interleave.
        assistants::persist(&self.root, record)
            .await
            .map_err(io_err("persist assistant"))?;
        map.insert(record.assistant_id.clone(), record.clone());
        Ok(true)
    }

    async fn get_assistant(&self, assistant_id: &str) -> StoreResult<Option<AssistantRecord>> {
        Ok(self.assistants.lock().await.get(assistant_id).cloned())
    }

    async fn list_assistants(&self) -> StoreResult<Vec<AssistantRecord>> {
        Ok(self.assistants.lock().await.values().cloned().collect())
    }

    async fn create_cron(&self, record: &CronRecord) -> StoreResult<bool> {
        let mut map = self.crons.lock().await;
        if map.contains_key(&record.cron_id) {
            return Ok(false);
        }
        crons::persist(&self.root, record)
            .await
            .map_err(io_err("persist cron"))?;
        map.insert(record.cron_id.clone(), record.clone());
        Ok(true)
    }

    async fn upsert_cron(&self, record: &CronRecord) -> StoreResult<()> {
        let mut map = self.crons.lock().await;
        crons::persist(&self.root, record)
            .await
            .map_err(io_err("persist cron"))?;
        map.insert(record.cron_id.clone(), record.clone());
        Ok(())
    }

    async fn get_cron(&self, cron_id: &str) -> StoreResult<Option<CronRecord>> {
        Ok(self.crons.lock().await.get(cron_id).cloned())
    }

    async fn list_crons(&self) -> StoreResult<Vec<CronRecord>> {
        Ok(self.crons.lock().await.values().cloned().collect())
    }

    async fn delete_cron(&self, cron_id: &str) -> StoreResult<bool> {
        let mut map = self.crons.lock().await;
        let Some(record) = map.remove(cron_id) else {
            return Ok(false);
        };
        let path = crons::dir(&self.root).join(format!("{cron_id}.json"));
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(true),
            // The file is already gone; the in-memory index was authoritative.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
            // On removal failure the record must stay in memory: dropping it
            // here would let the orphaned file resurrect the cron on the
            // next restart while the API already answered `deleted: true`.
            Err(e) => {
                map.insert(cron_id.to_string(), record);
                Err(format!("remove cron file: {e}"))
            }
        }
    }

    async fn create_thread(&self, internal_id: &str, record: &ThreadRecord) -> StoreResult<bool> {
        let mut map = self.threads.lock().await;
        if map.contains_key(internal_id) {
            return Ok(false);
        }
        // Hold the lock across the file write so a concurrent create of the
        // same id can't interleave.
        threads::persist(&self.root, internal_id, record)
            .await
            .map_err(io_err("persist thread"))?;
        map.insert(internal_id.to_string(), record.clone());
        Ok(true)
    }

    async fn get_thread(&self, internal_id: &str) -> StoreResult<Option<ThreadRecord>> {
        Ok(self.threads.lock().await.get(internal_id).cloned())
    }

    async fn kv_put(
        &self,
        namespace: &str,
        key: &str,
        value: Value,
    ) -> StoreResult<(StoreItem, bool)> {
        let _guard = self.kv_lock.lock().await;
        store::put(&self.root, namespace, key, value)
            .await
            .map_err(io_err("put store item"))
    }

    async fn kv_get(&self, namespace: &str, key: &str) -> StoreResult<Option<StoreItem>> {
        store::get(&self.root, namespace, key)
            .await
            .map_err(io_err("get store item"))
    }

    async fn kv_delete(&self, namespace: &str, key: &str) -> StoreResult<bool> {
        let _guard = self.kv_lock.lock().await;
        store::delete(&self.root, namespace, key)
            .await
            .map_err(io_err("delete store item"))
    }

    async fn kv_list(&self, namespace: &str) -> StoreResult<Vec<StoreItem>> {
        store::list(&self.root, namespace)
            .await
            .map_err(io_err("list store namespace"))
    }

    async fn put_journal(&self, snapshot: &JournalSnapshot) -> StoreResult<()> {
        journals::persist(&self.root, snapshot)
            .await
            .map_err(io_err("persist journal"))
    }

    async fn get_journal(&self, run_id: &str) -> StoreResult<Option<JournalSnapshot>> {
        journals::get(&self.root, run_id)
            .await
            .map_err(io_err("get journal"))
    }

    async fn enqueue_task(&self, record: &TaskRecord) -> StoreResult<(TaskRecord, bool)> {
        let mut map = self.tasks.lock().await;
        if let Some(key) = &record.idempotency_key {
            // Linear dedup scan under the index lock: correct at the file
            // backend's scale (it backs single-binary deployments); the
            // Postgres backend enforces this with a unique index instead.
            if let Some(existing) = map
                .values()
                .find(|t| t.tenant == record.tenant && t.idempotency_key.as_deref() == Some(key))
            {
                return Ok((existing.clone(), true));
            }
        }
        tasks::persist(&self.root, record)
            .await
            .map_err(io_err("persist task"))?;
        map.insert(record.task_id.clone(), record.clone());
        Ok((record.clone(), false))
    }

    async fn claim_task(
        &self,
        tenant: &str,
        worker_id: &str,
        pools: &[String],
        lease_ms: u64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> StoreResult<Option<TaskRecord>> {
        let mut map = self.tasks.lock().await;
        // The whole claim (pick + mutate + persist) runs under the one
        // index lock, so two concurrent claims can never take the same
        // task — the file backend's SKIP LOCKED equivalent.
        let candidate = map
            .values()
            .filter(|t| {
                t.tenant == tenant && pools.iter().any(|p| p == &t.pool) && t.claimable_at(now)
            })
            .min_by(|a, b| {
                a.created_at
                    .cmp(&b.created_at)
                    .then_with(|| a.task_id.cmp(&b.task_id))
            })
            .map(|t| t.task_id.clone());
        let Some(task_id) = candidate else {
            return Ok(None);
        };
        let mut task = map
            .get(&task_id)
            .cloned()
            .expect("claim candidate came from the task index");
        task.claim(worker_id, lease_ms, now);
        tasks::persist(&self.root, &task)
            .await
            .map_err(io_err("persist task"))?;
        map.insert(task_id, task.clone());
        Ok(Some(task))
    }

    async fn heartbeat_task(
        &self,
        tenant: &str,
        task_id: &str,
        worker_id: &str,
        lease_ms: u64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> StoreResult<MutationOutcome> {
        self.mutate_task(tenant, task_id, worker_id, |task| {
            task.renew_lease(lease_ms, now);
        })
        .await
    }

    async fn complete_task(
        &self,
        tenant: &str,
        task_id: &str,
        worker_id: &str,
        result: Value,
        now: chrono::DateTime<chrono::Utc>,
    ) -> StoreResult<MutationOutcome> {
        self.mutate_task(tenant, task_id, worker_id, |task| {
            task.complete(result, now);
        })
        .await
    }

    async fn fail_task(
        &self,
        tenant: &str,
        task_id: &str,
        worker_id: &str,
        report: tasks::FailureReport,
        now: chrono::DateTime<chrono::Utc>,
    ) -> StoreResult<MutationOutcome> {
        self.mutate_task(tenant, task_id, worker_id, |task| {
            task.fail(report.error_class, &report.message, report.retryable, now);
        })
        .await
    }

    async fn get_task(&self, tenant: &str, task_id: &str) -> StoreResult<Option<TaskRecord>> {
        Ok(self
            .tasks
            .lock()
            .await
            .get(task_id)
            .filter(|t| t.tenant == tenant)
            .cloned())
    }

    async fn list_tasks(
        &self,
        tenant: &str,
        status: Option<TaskStatus>,
    ) -> StoreResult<Vec<TaskRecord>> {
        let mut tasks: Vec<TaskRecord> = self
            .tasks
            .lock()
            .await
            .values()
            .filter(|t| t.tenant == tenant && status.is_none_or(|s| t.status == s))
            .cloned()
            .collect();
        tasks.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.task_id.cmp(&b.task_id))
        });
        Ok(tasks)
    }
}

impl JsonFileStore {
    /// Shared skeleton of the lease-guarded task mutations (heartbeat /
    /// complete / fail): resolve tenant-scoped, check the lease, mutate a
    /// copy, persist, then swap the index. Persisting before the swap keeps
    /// a failed write from leaving state a restart would silently rewind.
    async fn mutate_task(
        &self,
        tenant: &str,
        task_id: &str,
        worker_id: &str,
        mutate: impl FnOnce(&mut TaskRecord),
    ) -> StoreResult<MutationOutcome> {
        let mut map = self.tasks.lock().await;
        let Some(current) = map.get(task_id) else {
            return Ok(MutationOutcome::Unknown);
        };
        // Cross-tenant ids are indistinguishable from unknown ones (404).
        if current.tenant != tenant {
            return Ok(MutationOutcome::Unknown);
        }
        if !current.leased_to(worker_id) {
            return Ok(MutationOutcome::LeaseLost);
        }
        let mut task = current.clone();
        mutate(&mut task);
        tasks::persist(&self.root, &task)
            .await
            .map_err(io_err("persist task"))?;
        map.insert(task_id.to_string(), task.clone());
        Ok(MutationOutcome::Applied(Box::new(task)))
    }
}

// --------------------------------------------------------------------- //
// PostgresStore — feature `postgres`
// --------------------------------------------------------------------- //

#[cfg(feature = "postgres")]
mod postgres {
    use chrono::{DateTime, Utc};
    use rusty_agent_runtime::journal::JournalSnapshot;
    use serde_json::Value;
    use sqlx::{PgPool, Row};
    use tokio::sync::OnceCell;

    use super::{ServerStore, StoreResult};
    use crate::assistants::AssistantRecord;
    use crate::crons::CronRecord;
    use crate::store::StoreItem;
    use crate::tasks::{self, MutationOutcome, TaskLease, TaskRecord, TaskStatus};
    use crate::threads::ThreadRecord;

    // -- Schema (auto-migrated on connect) ------------------------------ //

    /// `server_assistants`: one row per assistant, whole record as JSONB.
    pub(crate) const CREATE_ASSISTANTS_SQL: &str = "
        CREATE TABLE IF NOT EXISTS server_assistants (
            assistant_id TEXT PRIMARY KEY,
            payload      JSONB NOT NULL,
            created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
        )";

    /// `server_crons`: one row per cron, whole record as JSONB.
    pub(crate) const CREATE_CRONS_SQL: &str = "
        CREATE TABLE IF NOT EXISTS server_crons (
            cron_id    TEXT PRIMARY KEY,
            payload    JSONB NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT now()
        )";

    /// `server_threads`: one row per thread, whole record as JSONB.
    pub(crate) const CREATE_THREADS_SQL: &str = "
        CREATE TABLE IF NOT EXISTS server_threads (
            thread_id  TEXT PRIMARY KEY,
            payload    JSONB NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT now()
        )";

    /// `server_kv`: one row per (namespace, key), JSONB value plus explicit
    /// created/updated timestamps (`created_at` preserved across replaces).
    pub(crate) const CREATE_KV_SQL: &str = r#"
        CREATE TABLE IF NOT EXISTS server_kv (
            namespace  TEXT NOT NULL,
            "key"      TEXT NOT NULL,
            value      JSONB NOT NULL,
            created_at TIMESTAMPTZ NOT NULL,
            updated_at TIMESTAMPTZ NOT NULL,
            PRIMARY KEY (namespace, "key")
        )"#;

    /// `server_journals`: one row per run, the Flight Recorder journal
    /// snapshot as JSONB (`updated_at` tracks the journal's growth across
    /// checkpoint boundaries).
    pub(crate) const CREATE_JOURNALS_SQL: &str = "
        CREATE TABLE IF NOT EXISTS server_journals (
            run_id     TEXT PRIMARY KEY,
            payload    JSONB NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
        )";

    /// `server_tasks`: the durable task queue (R0.6). Unlike the record
    /// tables above this one is column-mapped, not JSONB-payloaded: claiming
    /// filters and locks on `status` / `pool` / lease columns, so they must
    /// be real columns. `status` spells [`crate::tasks::TaskStatus::as_str`].
    pub(crate) const CREATE_TASKS_SQL: &str = "
        CREATE TABLE IF NOT EXISTS server_tasks (
            task_id          TEXT PRIMARY KEY,
            tenant           TEXT NOT NULL,
            kind             TEXT NOT NULL,
            payload          JSONB NOT NULL,
            pool             TEXT NOT NULL,
            status           TEXT NOT NULL,
            lease_owner      TEXT,
            lease_expires_at TIMESTAMPTZ,
            attempt          INTEGER NOT NULL,
            max_attempts     INTEGER NOT NULL,
            error_class      TEXT,
            effect           TEXT,
            last_error       TEXT,
            idempotency_key  TEXT,
            result           JSONB,
            run_id           TEXT,
            thread_id        TEXT,
            next_attempt_at  TIMESTAMPTZ,
            created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
            updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
        )";

    /// Enqueue dedup: at most one live task per (tenant, idempotency_key).
    /// Partial — keyless tasks (NULLs) never conflict.
    pub(crate) const CREATE_TASKS_IDEMPOTENCY_INDEX_SQL: &str = "
        CREATE UNIQUE INDEX IF NOT EXISTS server_tasks_idempotency_unique
            ON server_tasks (tenant, idempotency_key)
            WHERE idempotency_key IS NOT NULL";

    /// Claim scans filter on exactly these three columns.
    pub(crate) const CREATE_TASKS_CLAIMABLE_INDEX_SQL: &str = "
        CREATE INDEX IF NOT EXISTS server_tasks_claimable
            ON server_tasks (tenant, pool, status)";

    /// All idempotent migration statements, executed in order on connect.
    pub(crate) const MIGRATION_SQL: &[&str] = &[
        CREATE_ASSISTANTS_SQL,
        CREATE_CRONS_SQL,
        CREATE_THREADS_SQL,
        CREATE_KV_SQL,
        CREATE_JOURNALS_SQL,
        CREATE_TASKS_SQL,
        CREATE_TASKS_IDEMPOTENCY_INDEX_SQL,
        CREATE_TASKS_CLAIMABLE_INDEX_SQL,
    ];

    /// Transaction-scoped advisory lock key serializing concurrent
    /// first-use migrations of the server tables.
    const MIGRATION_LOCK_KEY: i64 = 0x6167_7376_5f6d_6967; // "agsv_mig"

    // -- CRUD statements ------------------------------------------------ //

    /// Insert-only assistant create; returns no row on conflict → 409.
    pub(crate) const INSERT_ASSISTANT_SQL: &str = "
        INSERT INTO server_assistants (assistant_id, payload)
        VALUES ($1, $2)
        ON CONFLICT (assistant_id) DO NOTHING
        RETURNING assistant_id";

    pub(crate) const SELECT_ASSISTANT_SQL: &str =
        "SELECT payload FROM server_assistants WHERE assistant_id = $1";

    pub(crate) const LIST_ASSISTANTS_SQL: &str = "SELECT payload FROM server_assistants";

    /// Insert-only cron create; returns no row on conflict → 409.
    pub(crate) const INSERT_CRON_SQL: &str = "
        INSERT INTO server_crons (cron_id, payload)
        VALUES ($1, $2)
        ON CONFLICT (cron_id) DO NOTHING
        RETURNING cron_id";

    /// Full upsert for scheduler bookkeeping.
    pub(crate) const UPSERT_CRON_SQL: &str = "
        INSERT INTO server_crons (cron_id, payload)
        VALUES ($1, $2)
        ON CONFLICT (cron_id) DO UPDATE SET payload = EXCLUDED.payload";

    pub(crate) const SELECT_CRON_SQL: &str = "SELECT payload FROM server_crons WHERE cron_id = $1";

    pub(crate) const LIST_CRONS_SQL: &str = "SELECT payload FROM server_crons";

    pub(crate) const DELETE_CRON_SQL: &str = "DELETE FROM server_crons WHERE cron_id = $1";

    /// Insert-only thread create; returns no row on conflict → 409.
    pub(crate) const INSERT_THREAD_SQL: &str = "
        INSERT INTO server_threads (thread_id, payload)
        VALUES ($1, $2)
        ON CONFLICT (thread_id) DO NOTHING
        RETURNING thread_id";

    pub(crate) const SELECT_THREAD_SQL: &str =
        "SELECT payload FROM server_threads WHERE thread_id = $1";

    /// KV upsert that preserves `created_at` on replace and reports whether
    /// the row pre-existed (the `created` flag drives 201 vs 200).
    pub(crate) const UPSERT_KV_SQL: &str = r#"
        WITH existing AS (
            SELECT created_at FROM server_kv WHERE namespace = $1 AND "key" = $2
        ), upserted AS (
            INSERT INTO server_kv (namespace, "key", value, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5)
            ON CONFLICT (namespace, "key") DO UPDATE
                SET value = EXCLUDED.value, updated_at = EXCLUDED.updated_at
            RETURNING created_at, updated_at
        )
        SELECT u.created_at, u.updated_at, (e.created_at IS NULL) AS created
        FROM upserted u LEFT JOIN existing e ON TRUE"#;

    pub(crate) const SELECT_KV_SQL: &str = r#"
        SELECT value, created_at, updated_at
        FROM server_kv WHERE namespace = $1 AND "key" = $2"#;

    pub(crate) const DELETE_KV_SQL: &str =
        r#"DELETE FROM server_kv WHERE namespace = $1 AND "key" = $2"#;

    pub(crate) const LIST_KV_SQL: &str = r#"
        SELECT "key", value, created_at, updated_at
        FROM server_kv WHERE namespace = $1 ORDER BY "key""#;

    /// Journal upsert: the snapshot is rewritten at every checkpoint
    /// boundary, so `updated_at` moves while `created_at` is preserved.
    pub(crate) const UPSERT_JOURNAL_SQL: &str = "
        INSERT INTO server_journals (run_id, payload)
        VALUES ($1, $2)
        ON CONFLICT (run_id) DO UPDATE
            SET payload = EXCLUDED.payload, updated_at = now()";

    pub(crate) const SELECT_JOURNAL_SQL: &str =
        "SELECT payload FROM server_journals WHERE run_id = $1";

    // -- Task queue statements (R0.6) ------------------------------------ //

    /// Insert-only enqueue; `ON CONFLICT DO NOTHING` absorbs both the
    /// (effectively impossible) task-id collision and the idempotency-key
    /// dedup — a no-row result with a key set means *deduplicated*.
    pub(crate) const INSERT_TASK_SQL: &str = "
        INSERT INTO server_tasks (
            task_id, tenant, kind, payload, pool, status,
            attempt, max_attempts, error_class, effect, idempotency_key,
            run_id, thread_id, next_attempt_at, created_at, updated_at
        ) VALUES (
            $1, $2, $3, $4, $5, 'queued', 0, $6, NULL, $7, $8, $9, $10, NULL, $11, $11
        )
        ON CONFLICT DO NOTHING
        RETURNING task_id";

    /// The dedup read-back after an absorbed idempotency conflict.
    pub(crate) const SELECT_TASK_BY_IDEMPOTENCY_SQL: &str = "
        SELECT task_id, tenant, kind, payload, pool, status, lease_owner, lease_expires_at, \
            attempt, max_attempts, error_class, effect, last_error, idempotency_key, result, \
            run_id, thread_id, next_attempt_at, created_at, updated_at
        FROM server_tasks
        WHERE tenant = $1 AND idempotency_key = $2";

    /// Claim candidate selection, run inside a transaction: `FOR UPDATE
    /// SKIP LOCKED` makes concurrent workers take distinct tasks without
    /// blocking each other. Claimable = queued, backoff-elapsed failed, or
    /// leased past its visibility timeout.
    pub(crate) const CLAIM_SELECT_SQL: &str = "
        SELECT task_id, attempt FROM server_tasks
        WHERE tenant = $1
          AND pool = ANY($2)
          AND (
              (status IN ('queued', 'failed')
                  AND (next_attempt_at IS NULL OR next_attempt_at <= $3))
              OR (status = 'leased' AND lease_expires_at <= $3)
          )
        ORDER BY created_at, task_id
        LIMIT 1
        FOR UPDATE SKIP LOCKED";

    /// The claim itself, applied to the row locked by [`CLAIM_SELECT_SQL`]
    /// in the same transaction.
    pub(crate) const CLAIM_UPDATE_SQL: &str = "
        UPDATE server_tasks
        SET lease_owner = $2, lease_expires_at = $3, attempt = $4,
            status = 'leased', next_attempt_at = NULL, updated_at = $5
        WHERE task_id = $1
        RETURNING task_id, tenant, kind, payload, pool, status, lease_owner, lease_expires_at, \
            attempt, max_attempts, error_class, effect, last_error, idempotency_key, result, \
            run_id, thread_id, next_attempt_at, created_at, updated_at";

    /// Heartbeat: extends the lease only while the caller holds it. No row
    /// means unknown/cross-tenant (404) or lease lost (409), distinguished
    /// by [`TASK_EXISTS_SQL`].
    pub(crate) const HEARTBEAT_TASK_SQL: &str = "
        UPDATE server_tasks
        SET lease_expires_at = $4, updated_at = $5
        WHERE task_id = $1 AND tenant = $2 AND lease_owner = $3 AND status = 'leased'
        RETURNING task_id, tenant, kind, payload, pool, status, lease_owner, lease_expires_at, \
            attempt, max_attempts, error_class, effect, last_error, idempotency_key, result, \
            run_id, thread_id, next_attempt_at, created_at, updated_at";

    /// Complete: settle only the caller's own lease.
    pub(crate) const COMPLETE_TASK_SQL: &str = "
        UPDATE server_tasks
        SET status = 'completed', result = $4, lease_owner = NULL,
            lease_expires_at = NULL, next_attempt_at = NULL, updated_at = $5
        WHERE task_id = $1 AND tenant = $2 AND lease_owner = $3 AND status = 'leased'
        RETURNING task_id, tenant, kind, payload, pool, status, lease_owner, lease_expires_at, \
            attempt, max_attempts, error_class, effect, last_error, idempotency_key, result, \
            run_id, thread_id, next_attempt_at, created_at, updated_at";

    /// Fail, step 1: lock the row (the requeue-vs-dead decision needs the
    /// current attempt count, and concurrent settlement must serialize).
    pub(crate) const FAIL_SELECT_SQL: &str = "
        SELECT task_id, tenant, kind, payload, pool, status, lease_owner, lease_expires_at, \
            attempt, max_attempts, error_class, effect, last_error, idempotency_key, result, \
            run_id, thread_id, next_attempt_at, created_at, updated_at
        FROM server_tasks
        WHERE task_id = $1 AND tenant = $2
        FOR UPDATE";

    /// Fail, step 2: apply the decision computed in Rust
    /// ([`crate::tasks::TaskRecord::fail`]) to the locked row.
    pub(crate) const FAIL_UPDATE_SQL: &str = "
        UPDATE server_tasks
        SET status = $2, error_class = $3, last_error = $4,
            lease_owner = NULL, lease_expires_at = NULL,
            next_attempt_at = $5, updated_at = $6
        WHERE task_id = $1
        RETURNING task_id, tenant, kind, payload, pool, status, lease_owner, lease_expires_at, \
            attempt, max_attempts, error_class, effect, last_error, idempotency_key, result, \
            run_id, thread_id, next_attempt_at, created_at, updated_at";

    /// Tenant-scoped existence probe distinguishing 404 from 409 after a
    /// lease-guarded update matched no row.
    pub(crate) const TASK_EXISTS_SQL: &str =
        "SELECT task_id FROM server_tasks WHERE task_id = $1 AND tenant = $2";

    pub(crate) const SELECT_TASK_SQL: &str =
        "SELECT task_id, tenant, kind, payload, pool, status, lease_owner, lease_expires_at, \
            attempt, max_attempts, error_class, effect, last_error, idempotency_key, result, \
            run_id, thread_id, next_attempt_at, created_at, updated_at
        FROM server_tasks WHERE task_id = $1 AND tenant = $2";

    pub(crate) const LIST_TASKS_SQL: &str = "
        SELECT task_id, tenant, kind, payload, pool, status, lease_owner, lease_expires_at, \
            attempt, max_attempts, error_class, effect, last_error, idempotency_key, result, \
            run_id, thread_id, next_attempt_at, created_at, updated_at
        FROM server_tasks
        WHERE tenant = $1 ORDER BY created_at, task_id";

    pub(crate) const LIST_TASKS_BY_STATUS_SQL: &str = "
        SELECT task_id, tenant, kind, payload, pool, status, lease_owner, lease_expires_at, \
            attempt, max_attempts, error_class, effect, last_error, idempotency_key, result, \
            run_id, thread_id, next_attempt_at, created_at, updated_at
        FROM server_tasks
        WHERE tenant = $1 AND status = $2 ORDER BY created_at, task_id";

    // -- Row <-> record mapping (unit-tested without a database) -------- //

    /// Serialize a record for the JSONB `payload` column.
    pub(crate) fn record_to_payload<T: serde::Serialize>(record: &T) -> StoreResult<Value> {
        serde_json::to_value(record).map_err(|e| format!("serialize record: {e}"))
    }

    /// Deserialize a JSONB `payload` column back into a record.
    pub(crate) fn record_from_payload<T: serde::de::DeserializeOwned>(
        what: &str,
        payload: Value,
    ) -> StoreResult<T> {
        serde_json::from_value(payload).map_err(|e| format!("corrupt {what} payload: {e}"))
    }

    /// Assemble a wire-facing [`StoreItem`] from one `server_kv` row.
    pub(crate) fn kv_row_to_item(
        namespace: &str,
        key: &str,
        value: Value,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    ) -> StoreItem {
        StoreItem {
            namespace: namespace.to_string(),
            key: key.to_string(),
            value,
            created_at,
            updated_at,
        }
    }

    /// Assemble a [`TaskRecord`] from one `server_tasks` row (name-based, so
    /// additive columns never break the mapping). A corrupt `status` or a
    /// negative attempt count is a store error, not a panic — the same
    /// discipline as `record_from_payload`.
    pub(crate) fn task_from_row(row: &sqlx::postgres::PgRow) -> StoreResult<TaskRecord> {
        let status_raw: String = row.get("status");
        let status = TaskStatus::parse(&status_raw)
            .ok_or_else(|| format!("corrupt task status `{status_raw}`"))?;
        let attempt = u32::try_from(row.get::<i32, _>("attempt"))
            .map_err(|_| "corrupt task attempt (negative)".to_string())?;
        let max_attempts = u32::try_from(row.get::<i32, _>("max_attempts"))
            .map_err(|_| "corrupt task max_attempts (negative)".to_string())?;
        let lease = match (
            row.get::<Option<String>, _>("lease_owner"),
            row.get::<Option<DateTime<Utc>>, _>("lease_expires_at"),
        ) {
            (Some(owner), Some(expires_at)) => Some(TaskLease { owner, expires_at }),
            _ => None,
        };
        let error_class = row
            .get::<Option<String>, _>("error_class")
            .map(|raw| {
                tasks::parse_error_class(&raw)
                    .map_err(|_| format!("corrupt task error_class `{raw}`"))
            })
            .transpose()?;
        let effect = row
            .get::<Option<String>, _>("effect")
            .map(|raw| {
                tasks::parse_effect(&raw).map_err(|_| format!("corrupt task effect `{raw}`"))
            })
            .transpose()?;
        Ok(TaskRecord {
            task_id: row.get("task_id"),
            tenant: row.get("tenant"),
            kind: row.get("kind"),
            payload: row.get("payload"),
            pool: row.get("pool"),
            status,
            attempt,
            max_attempts,
            lease,
            error_class,
            effect,
            last_error: row.get("last_error"),
            idempotency_key: row.get("idempotency_key"),
            result: row.get("result"),
            run_id: row.get("run_id"),
            thread_id: row.get("thread_id"),
            next_attempt_at: row.get("next_attempt_at"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        })
    }

    fn db_err(context: &str) -> impl Fn(sqlx::Error) -> String + '_ {
        move |e| format!("{context}: {e}")
    }

    /// Postgres-backed store: assistants / crons / KV in `server_*` tables.
    ///
    /// The connection (and idempotent auto-migration) is established lazily
    /// on first use, so [`crate::router`] can stay synchronous.
    pub(crate) struct PostgresStore {
        url: String,
        pool: OnceCell<PgPool>,
    }

    impl PostgresStore {
        /// A store that will connect to `url` on first use.
        pub(crate) fn new(url: String) -> Self {
            Self {
                url,
                pool: OnceCell::new(),
            }
        }

        /// The connection pool, connecting + migrating on first call.
        ///
        /// The migration runs inside a transaction holding a
        /// transaction-scoped advisory lock, so concurrent first-use
        /// migrations (e.g. several tests or server instances booting against
        /// one fresh database) serialize instead of tripping the
        /// `CREATE TABLE IF NOT EXISTS` check-then-create race (duplicate key
        /// on `pg_type_typname_nsp_index`).
        async fn pool(&self) -> StoreResult<&PgPool> {
            self.pool
                .get_or_try_init(|| async {
                    let pool = PgPool::connect(&self.url)
                        .await
                        .map_err(db_err("connect postgres"))?;
                    let mut tx = pool
                        .begin()
                        .await
                        .map_err(db_err("migrate server tables"))?;
                    sqlx::query("SELECT pg_advisory_xact_lock($1)")
                        .bind(MIGRATION_LOCK_KEY)
                        .execute(&mut *tx)
                        .await
                        .map_err(db_err("migrate server tables"))?;
                    for stmt in MIGRATION_SQL {
                        sqlx::query(stmt)
                            .execute(&mut *tx)
                            .await
                            .map_err(db_err("migrate server tables"))?;
                    }
                    tx.commit().await.map_err(db_err("migrate server tables"))?;
                    Ok(pool)
                })
                .await
        }
    }

    #[async_trait::async_trait]
    impl ServerStore for PostgresStore {
        async fn create_assistant(&self, record: &AssistantRecord) -> StoreResult<bool> {
            let payload = record_to_payload(record)?;
            let row = sqlx::query(INSERT_ASSISTANT_SQL)
                .bind(&record.assistant_id)
                .bind(payload)
                .fetch_optional(self.pool().await?)
                .await
                .map_err(db_err("insert assistant"))?;
            Ok(row.is_some())
        }

        async fn get_assistant(&self, assistant_id: &str) -> StoreResult<Option<AssistantRecord>> {
            let row = sqlx::query(SELECT_ASSISTANT_SQL)
                .bind(assistant_id)
                .fetch_optional(self.pool().await?)
                .await
                .map_err(db_err("select assistant"))?;
            row.map(|r| record_from_payload("assistant", r.get::<Value, _>("payload")))
                .transpose()
        }

        async fn list_assistants(&self) -> StoreResult<Vec<AssistantRecord>> {
            let rows = sqlx::query(LIST_ASSISTANTS_SQL)
                .fetch_all(self.pool().await?)
                .await
                .map_err(db_err("list assistants"))?;
            rows.into_iter()
                .map(|r| record_from_payload("assistant", r.get::<Value, _>("payload")))
                .collect()
        }

        async fn create_cron(&self, record: &CronRecord) -> StoreResult<bool> {
            let payload = record_to_payload(record)?;
            let row = sqlx::query(INSERT_CRON_SQL)
                .bind(&record.cron_id)
                .bind(payload)
                .fetch_optional(self.pool().await?)
                .await
                .map_err(db_err("insert cron"))?;
            Ok(row.is_some())
        }

        async fn upsert_cron(&self, record: &CronRecord) -> StoreResult<()> {
            let payload = record_to_payload(record)?;
            sqlx::query(UPSERT_CRON_SQL)
                .bind(&record.cron_id)
                .bind(payload)
                .execute(self.pool().await?)
                .await
                .map_err(db_err("upsert cron"))?;
            Ok(())
        }

        async fn get_cron(&self, cron_id: &str) -> StoreResult<Option<CronRecord>> {
            let row = sqlx::query(SELECT_CRON_SQL)
                .bind(cron_id)
                .fetch_optional(self.pool().await?)
                .await
                .map_err(db_err("select cron"))?;
            row.map(|r| record_from_payload("cron", r.get::<Value, _>("payload")))
                .transpose()
        }

        async fn list_crons(&self) -> StoreResult<Vec<CronRecord>> {
            let rows = sqlx::query(LIST_CRONS_SQL)
                .fetch_all(self.pool().await?)
                .await
                .map_err(db_err("list crons"))?;
            rows.into_iter()
                .map(|r| record_from_payload("cron", r.get::<Value, _>("payload")))
                .collect()
        }

        async fn delete_cron(&self, cron_id: &str) -> StoreResult<bool> {
            let result = sqlx::query(DELETE_CRON_SQL)
                .bind(cron_id)
                .execute(self.pool().await?)
                .await
                .map_err(db_err("delete cron"))?;
            Ok(result.rows_affected() > 0)
        }

        async fn create_thread(
            &self,
            internal_id: &str,
            record: &ThreadRecord,
        ) -> StoreResult<bool> {
            let payload = record_to_payload(record)?;
            let row = sqlx::query(INSERT_THREAD_SQL)
                .bind(internal_id)
                .bind(payload)
                .fetch_optional(self.pool().await?)
                .await
                .map_err(db_err("insert thread"))?;
            Ok(row.is_some())
        }

        async fn get_thread(&self, internal_id: &str) -> StoreResult<Option<ThreadRecord>> {
            let row = sqlx::query(SELECT_THREAD_SQL)
                .bind(internal_id)
                .fetch_optional(self.pool().await?)
                .await
                .map_err(db_err("select thread"))?;
            row.map(|r| record_from_payload("thread", r.get::<Value, _>("payload")))
                .transpose()
        }

        async fn kv_put(
            &self,
            namespace: &str,
            key: &str,
            value: Value,
        ) -> StoreResult<(StoreItem, bool)> {
            let now = Utc::now();
            let row = sqlx::query(UPSERT_KV_SQL)
                .bind(namespace)
                .bind(key)
                .bind(&value)
                .bind(now) // created_at (ignored on conflict)
                .bind(now) // updated_at
                .fetch_one(self.pool().await?)
                .await
                .map_err(db_err("upsert store item"))?;
            let created_at: DateTime<Utc> = row.get("created_at");
            let updated_at: DateTime<Utc> = row.get("updated_at");
            let created: bool = row.get("created");
            Ok((
                kv_row_to_item(namespace, key, value, created_at, updated_at),
                created,
            ))
        }

        async fn kv_get(&self, namespace: &str, key: &str) -> StoreResult<Option<StoreItem>> {
            let row = sqlx::query(SELECT_KV_SQL)
                .bind(namespace)
                .bind(key)
                .fetch_optional(self.pool().await?)
                .await
                .map_err(db_err("get store item"))?;
            Ok(row.map(|r| {
                kv_row_to_item(
                    namespace,
                    key,
                    r.get::<Value, _>("value"),
                    r.get::<DateTime<Utc>, _>("created_at"),
                    r.get::<DateTime<Utc>, _>("updated_at"),
                )
            }))
        }

        async fn kv_delete(&self, namespace: &str, key: &str) -> StoreResult<bool> {
            let result = sqlx::query(DELETE_KV_SQL)
                .bind(namespace)
                .bind(key)
                .execute(self.pool().await?)
                .await
                .map_err(db_err("delete store item"))?;
            Ok(result.rows_affected() > 0)
        }

        async fn kv_list(&self, namespace: &str) -> StoreResult<Vec<StoreItem>> {
            let rows = sqlx::query(LIST_KV_SQL)
                .bind(namespace)
                .fetch_all(self.pool().await?)
                .await
                .map_err(db_err("list store namespace"))?;
            Ok(rows
                .into_iter()
                .map(|r| {
                    let key: String = r.get("key");
                    kv_row_to_item(
                        namespace,
                        &key,
                        r.get::<Value, _>("value"),
                        r.get::<DateTime<Utc>, _>("created_at"),
                        r.get::<DateTime<Utc>, _>("updated_at"),
                    )
                })
                .collect())
        }

        async fn put_journal(&self, snapshot: &JournalSnapshot) -> StoreResult<()> {
            let payload = record_to_payload(snapshot)?;
            sqlx::query(UPSERT_JOURNAL_SQL)
                .bind(&snapshot.run_id)
                .bind(payload)
                .execute(self.pool().await?)
                .await
                .map_err(db_err("upsert journal"))?;
            Ok(())
        }

        async fn get_journal(&self, run_id: &str) -> StoreResult<Option<JournalSnapshot>> {
            let row = sqlx::query(SELECT_JOURNAL_SQL)
                .bind(run_id)
                .fetch_optional(self.pool().await?)
                .await
                .map_err(db_err("select journal"))?;
            row.map(|r| record_from_payload("journal", r.get::<Value, _>("payload")))
                .transpose()
        }

        async fn enqueue_task(&self, record: &TaskRecord) -> StoreResult<(TaskRecord, bool)> {
            let row = sqlx::query(INSERT_TASK_SQL)
                .bind(&record.task_id)
                .bind(&record.tenant)
                .bind(&record.kind)
                .bind(&record.payload)
                .bind(&record.pool)
                .bind(record.max_attempts as i32)
                .bind(record.effect.map(tasks::effect_name))
                .bind(&record.idempotency_key)
                .bind(&record.run_id)
                .bind(&record.thread_id)
                .bind(record.created_at)
                .fetch_optional(self.pool().await?)
                .await
                .map_err(db_err("enqueue task"))?;
            if row.is_some() {
                return Ok((record.clone(), false));
            }
            // The insert was absorbed by a conflict. With an idempotency key
            // that is the dedup path: the live task carrying the key wins.
            let Some(key) = &record.idempotency_key else {
                return Err(format!(
                    "task id `{}` collided with an existing task",
                    record.task_id
                ));
            };
            let existing = sqlx::query(SELECT_TASK_BY_IDEMPOTENCY_SQL)
                .bind(&record.tenant)
                .bind(key)
                .fetch_optional(self.pool().await?)
                .await
                .map_err(db_err("enqueue task dedup lookup"))?;
            match existing {
                Some(row) => Ok((task_from_row(&row)?, true)),
                None => Err(format!(
                    "task insert for idempotency key `{key}` conflicted but no live task carries it"
                )),
            }
        }

        async fn claim_task(
            &self,
            tenant: &str,
            worker_id: &str,
            pools: &[String],
            lease_ms: u64,
            now: DateTime<Utc>,
        ) -> StoreResult<Option<TaskRecord>> {
            let pool = self.pool().await?;
            // Lock-and-update in one transaction: SKIP LOCKED lets
            // concurrent workers claim distinct tasks; the row lock holds
            // until the claim commits, so no two workers ever take one task.
            let mut tx = pool.begin().await.map_err(db_err("claim task"))?;
            let candidate = sqlx::query(CLAIM_SELECT_SQL)
                .bind(tenant)
                .bind(pools.to_vec())
                .bind(now)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_err("claim task"))?;
            let Some(candidate) = candidate else {
                tx.rollback().await.map_err(db_err("claim task"))?;
                return Ok(None);
            };
            let expires_at =
                now + chrono::Duration::milliseconds(lease_ms.min(i64::MAX as u64) as i64);
            let updated = sqlx::query(CLAIM_UPDATE_SQL)
                .bind(candidate.get::<String, _>("task_id"))
                .bind(worker_id)
                .bind(expires_at)
                .bind(candidate.get::<i32, _>("attempt") + 1)
                .bind(now)
                .fetch_one(&mut *tx)
                .await
                .map_err(db_err("claim task"))?;
            tx.commit().await.map_err(db_err("claim task"))?;
            Ok(Some(task_from_row(&updated)?))
        }

        async fn heartbeat_task(
            &self,
            tenant: &str,
            task_id: &str,
            worker_id: &str,
            lease_ms: u64,
            now: DateTime<Utc>,
        ) -> StoreResult<MutationOutcome> {
            let expires_at =
                now + chrono::Duration::milliseconds(lease_ms.min(i64::MAX as u64) as i64);
            let updated = sqlx::query(HEARTBEAT_TASK_SQL)
                .bind(task_id)
                .bind(tenant)
                .bind(worker_id)
                .bind(expires_at)
                .bind(now)
                .fetch_optional(self.pool().await?)
                .await
                .map_err(db_err("heartbeat task"))?;
            self.lease_outcome(tenant, task_id, updated).await
        }

        async fn complete_task(
            &self,
            tenant: &str,
            task_id: &str,
            worker_id: &str,
            result: Value,
            now: DateTime<Utc>,
        ) -> StoreResult<MutationOutcome> {
            let updated = sqlx::query(COMPLETE_TASK_SQL)
                .bind(task_id)
                .bind(tenant)
                .bind(worker_id)
                .bind(result)
                .bind(now)
                .fetch_optional(self.pool().await?)
                .await
                .map_err(db_err("complete task"))?;
            self.lease_outcome(tenant, task_id, updated).await
        }

        async fn fail_task(
            &self,
            tenant: &str,
            task_id: &str,
            worker_id: &str,
            report: tasks::FailureReport,
            now: DateTime<Utc>,
        ) -> StoreResult<MutationOutcome> {
            let pool = self.pool().await?;
            let mut tx = pool.begin().await.map_err(db_err("fail task"))?;
            let locked = sqlx::query(FAIL_SELECT_SQL)
                .bind(task_id)
                .bind(tenant)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_err("fail task"))?;
            let Some(locked) = locked else {
                tx.rollback().await.map_err(db_err("fail task"))?;
                return Ok(MutationOutcome::Unknown);
            };
            let mut task = task_from_row(&locked)?;
            if !task.leased_to(worker_id) {
                tx.rollback().await.map_err(db_err("fail task"))?;
                return Ok(MutationOutcome::LeaseLost);
            }
            // Retry / dead-letter / fail-outright, computed by the same
            // record logic the file backend runs — core's shared
            // `classify_retry` (one decision, one test surface).
            task.fail(report.error_class, &report.message, report.retryable, now);
            sqlx::query(FAIL_UPDATE_SQL)
                .bind(&task.task_id)
                .bind(task.status.as_str())
                .bind(task.error_class.map(tasks::error_class_name))
                .bind(&task.last_error)
                .bind(task.next_attempt_at)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(db_err("fail task"))?;
            tx.commit().await.map_err(db_err("fail task"))?;
            Ok(MutationOutcome::Applied(task))
        }

        async fn get_task(&self, tenant: &str, task_id: &str) -> StoreResult<Option<TaskRecord>> {
            let row = sqlx::query(SELECT_TASK_SQL)
                .bind(task_id)
                .bind(tenant)
                .fetch_optional(self.pool().await?)
                .await
                .map_err(db_err("get task"))?;
            row.as_ref().map(task_from_row).transpose()
        }

        async fn list_tasks(
            &self,
            tenant: &str,
            status: Option<TaskStatus>,
        ) -> StoreResult<Vec<TaskRecord>> {
            let rows = match status {
                Some(status) => sqlx::query(LIST_TASKS_BY_STATUS_SQL)
                    .bind(tenant)
                    .bind(status.as_str())
                    .fetch_all(self.pool().await?)
                    .await
                    .map_err(db_err("list tasks"))?,
                None => sqlx::query(LIST_TASKS_SQL)
                    .bind(tenant)
                    .fetch_all(self.pool().await?)
                    .await
                    .map_err(db_err("list tasks"))?,
            };
            rows.iter().map(task_from_row).collect()
        }
    }

    impl PostgresStore {
        /// Map a lease-guarded update's outcome: the updated row means
        /// applied; no row means either the task is unknown to this tenant
        /// (404) or the lease check failed (409) — the existence probe
        /// decides.
        async fn lease_outcome(
            &self,
            tenant: &str,
            task_id: &str,
            updated: Option<sqlx::postgres::PgRow>,
        ) -> StoreResult<MutationOutcome> {
            if let Some(row) = updated {
                return Ok(MutationOutcome::Applied(task_from_row(&row)?));
            }
            let exists = sqlx::query(TASK_EXISTS_SQL)
                .bind(task_id)
                .bind(tenant)
                .fetch_optional(self.pool().await?)
                .await
                .map_err(db_err("task existence probe"))?;
            Ok(if exists.is_some() {
                MutationOutcome::LeaseLost
            } else {
                MutationOutcome::Unknown
            })
        }
    }

    /// Lazily-connecting [`Checkpointer`] facade over core's
    /// [`PostgresCheckpointer`]: connects (and auto-migrates
    /// `rusty_checkpoints`) on first checkpoint operation, keeping
    /// [`crate::router`] synchronous.
    pub(crate) struct LazyPostgresCheckpointer {
        url: String,
        inner: OnceCell<rusty_agent_runtime::checkpoint_postgres::PostgresCheckpointer>,
    }

    impl LazyPostgresCheckpointer {
        /// A checkpointer that will connect to `url` on first use.
        pub(crate) fn new(url: String) -> Self {
            Self {
                url,
                inner: OnceCell::new(),
            }
        }

        async fn cp(
            &self,
        ) -> rusty_agent_runtime::error::Result<
            &rusty_agent_runtime::checkpoint_postgres::PostgresCheckpointer,
        > {
            self.inner
                .get_or_try_init(|| {
                    rusty_agent_runtime::checkpoint_postgres::PostgresCheckpointer::connect(
                        &self.url,
                    )
                })
                .await
        }
    }

    #[async_trait::async_trait]
    impl rusty_agent_runtime::checkpoint::Checkpointer for LazyPostgresCheckpointer {
        async fn put(
            &self,
            checkpoint: rusty_agent_runtime::checkpoint::Checkpoint,
        ) -> rusty_agent_runtime::error::Result<()> {
            self.cp().await?.put(checkpoint).await
        }

        async fn get_latest(
            &self,
            thread_id: &str,
        ) -> rusty_agent_runtime::error::Result<Option<rusty_agent_runtime::checkpoint::Checkpoint>>
        {
            self.cp().await?.get_latest(thread_id).await
        }

        async fn list(
            &self,
            thread_id: &str,
        ) -> rusty_agent_runtime::error::Result<Vec<rusty_agent_runtime::checkpoint::Checkpoint>>
        {
            self.cp().await?.list(thread_id).await
        }

        async fn get_by_id(
            &self,
            thread_id: &str,
            checkpoint_id: &str,
        ) -> rusty_agent_runtime::error::Result<Option<rusty_agent_runtime::checkpoint::Checkpoint>>
        {
            self.cp().await?.get_by_id(thread_id, checkpoint_id).await
        }

        async fn fork_thread(
            &self,
            src_thread: &str,
            dst_thread: &str,
            at_checkpoint_id: Option<&str>,
        ) -> rusty_agent_runtime::error::Result<usize> {
            self.cp()
                .await?
                .fork_thread(src_thread, dst_thread, at_checkpoint_id)
                .await
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use serde_json::json;

        #[test]
        fn migration_sql_creates_all_tables_idempotently() {
            assert_eq!(MIGRATION_SQL.len(), 8);
            for stmt in MIGRATION_SQL {
                assert!(
                    stmt.contains("IF NOT EXISTS"),
                    "migration must be idempotent: {stmt}"
                );
            }
            assert!(CREATE_ASSISTANTS_SQL.contains("server_assistants"));
            assert!(CREATE_ASSISTANTS_SQL.contains("JSONB"));
            assert!(CREATE_CRONS_SQL.contains("server_crons"));
            assert!(CREATE_CRONS_SQL.contains("JSONB"));
            assert!(CREATE_THREADS_SQL.contains("server_threads"));
            assert!(CREATE_THREADS_SQL.contains("JSONB"));
            assert!(CREATE_KV_SQL.contains("server_kv"));
            assert!(CREATE_KV_SQL.contains("JSONB"));
            assert!(CREATE_KV_SQL.contains("PRIMARY KEY (namespace"));
            assert!(CREATE_JOURNALS_SQL.contains("server_journals"));
            assert!(CREATE_JOURNALS_SQL.contains("JSONB"));
            assert!(CREATE_JOURNALS_SQL.contains("TEXT PRIMARY KEY"));
            assert!(CREATE_TASKS_SQL.contains("server_tasks"));
            assert!(CREATE_TASKS_SQL.contains("TEXT PRIMARY KEY"));
            assert!(CREATE_TASKS_IDEMPOTENCY_INDEX_SQL.contains("CREATE UNIQUE INDEX"));
            assert!(CREATE_TASKS_CLAIMABLE_INDEX_SQL.contains("CREATE INDEX"));
        }

        #[test]
        fn tasks_schema_has_claim_columns_and_scoped_idempotency() {
            // Claiming filters and locks on real columns (not JSONB).
            for col in [
                "tenant",
                "pool",
                "status",
                "lease_owner",
                "lease_expires_at",
                "next_attempt_at",
                "attempt",
                "max_attempts",
                "effect",
            ] {
                assert!(CREATE_TASKS_SQL.contains(col), "missing column {col}");
            }
            // Dedup is per tenant and partial: keyless tasks never conflict.
            assert!(CREATE_TASKS_IDEMPOTENCY_INDEX_SQL.contains("(tenant, idempotency_key)"));
            assert!(
                CREATE_TASKS_IDEMPOTENCY_INDEX_SQL.contains("WHERE idempotency_key IS NOT NULL")
            );
        }

        #[test]
        fn claim_sql_locks_and_skips_locked_rows() {
            assert!(CLAIM_SELECT_SQL.contains("FOR UPDATE SKIP LOCKED"));
            assert!(CLAIM_SELECT_SQL.contains("pool = ANY($2)"));
            assert!(CLAIM_SELECT_SQL.contains("status IN ('queued', 'failed')"));
            assert!(CLAIM_SELECT_SQL.contains("status = 'leased'"));
            assert!(CLAIM_SELECT_SQL.contains("lease_expires_at <= $3"));
            assert!(CLAIM_UPDATE_SQL.contains("status = 'leased'"));
            assert!(CLAIM_UPDATE_SQL.contains("next_attempt_at = NULL"));
        }

        #[test]
        fn lease_guarded_updates_check_owner_tenant_and_leased_status() {
            for sql in [HEARTBEAT_TASK_SQL, COMPLETE_TASK_SQL] {
                assert!(sql.contains("task_id = $1 AND tenant = $2 AND lease_owner = $3"));
                assert!(sql.contains("status = 'leased'"));
            }
            // Fail locks the row first: the attempt count read and the
            // requeue/dead write must serialize against other settlers.
            assert!(FAIL_SELECT_SQL.contains("FOR UPDATE"));
            assert!(FAIL_UPDATE_SQL.contains("lease_owner = NULL"));
        }

        #[test]
        fn journal_upsert_sql_overwrites_payload_and_bumps_updated_at() {
            assert!(UPSERT_JOURNAL_SQL.contains("ON CONFLICT (run_id) DO UPDATE"));
            assert!(UPSERT_JOURNAL_SQL.contains("payload = EXCLUDED.payload"));
            assert!(UPSERT_JOURNAL_SQL.contains("updated_at = now()"));
        }

        #[test]
        fn journal_payload_round_trip() {
            use rusty_agent_runtime::journal::{Clock, EventDraft, Journal};
            use rusty_agent_runtime::record::{Effect, RunEventKind};

            let journal = Journal::new("run-1", "thread-1", Clock::System);
            journal.record(EventDraft::new(RunEventKind::SuperStepStart, Effect::Pure));
            let snapshot = journal.snapshot();
            let payload = record_to_payload(&snapshot).unwrap();
            let back: JournalSnapshot = record_from_payload("journal", payload).unwrap();
            assert_eq!(back.run_id, snapshot.run_id);
            assert_eq!(back.thread_id, snapshot.thread_id);
            assert_eq!(back.events, snapshot.events);
            assert_eq!(back.head_hash, snapshot.head_hash);
        }

        #[test]
        fn kv_upsert_sql_preserves_created_at_and_reports_created_flag() {
            assert!(UPSERT_KV_SQL.contains("ON CONFLICT (namespace, \"key\") DO UPDATE"));
            assert!(UPSERT_KV_SQL.contains("updated_at = EXCLUDED.updated_at"));
            // The existing-row probe feeds the 201-vs-200 `created` flag.
            assert!(UPSERT_KV_SQL.contains("e.created_at IS NULL"));
        }

        #[test]
        fn cron_upsert_sql_overwrites_payload() {
            assert!(UPSERT_CRON_SQL.contains("ON CONFLICT (cron_id) DO UPDATE"));
            assert!(UPSERT_CRON_SQL.contains("payload = EXCLUDED.payload"));
        }

        #[test]
        fn assistant_payload_round_trip() {
            let record = AssistantRecord {
                assistant_id: "a-1".to_string(),
                name: "support-bot".to_string(),
                graph: "pipeline".to_string(),
                config: json!({"recursion_limit": 10}),
                metadata: json!({"team": "qa"}),
                created_at: Utc::now(),
            };
            let payload = record_to_payload(&record).unwrap();
            let back: AssistantRecord = record_from_payload("assistant", payload).unwrap();
            assert_eq!(back.assistant_id, record.assistant_id);
            assert_eq!(back.name, record.name);
            assert_eq!(back.graph, record.graph);
            assert_eq!(back.config, record.config);
            assert_eq!(back.metadata, record.metadata);
            assert_eq!(back.created_at, record.created_at);
        }

        #[test]
        fn cron_payload_round_trip() {
            let record = CronRecord {
                cron_id: "c-1".to_string(),
                graph: "pipeline".to_string(),
                interval_secs: Some(60),
                cron_expr: None,
                input: Some(json!({"seed": 1})),
                metadata: json!(null),
                on_run_completed: Default::default(),
                created_at: Utc::now(),
                last_run_at: Some(Utc::now()),
                runs_fired: 3,
            };
            let payload = record_to_payload(&record).unwrap();
            let back: CronRecord = record_from_payload("cron", payload).unwrap();
            assert_eq!(back.cron_id, record.cron_id);
            assert_eq!(back.interval_secs, record.interval_secs);
            assert_eq!(back.cron_expr, record.cron_expr);
            assert_eq!(back.input, record.input);
            assert_eq!(back.runs_fired, record.runs_fired);
            assert_eq!(back.last_run_at, record.last_run_at);
        }

        #[test]
        fn thread_payload_round_trip() {
            let record = ThreadRecord {
                thread_id: "t-1".to_string(),
                tenant: "acme".to_string(),
                graph: "pipeline".to_string(),
                metadata: json!({"origin": "cron"}),
                created_at: Utc::now(),
            };
            let payload = record_to_payload(&record).unwrap();
            let back: ThreadRecord = record_from_payload("thread", payload).unwrap();
            assert_eq!(back.thread_id, record.thread_id);
            assert_eq!(back.tenant, record.tenant);
            assert_eq!(back.graph, record.graph);
            assert_eq!(back.metadata, record.metadata);
            assert_eq!(back.created_at, record.created_at);
        }

        #[test]
        fn corrupt_payload_is_an_error_not_a_panic() {
            let result = record_from_payload::<AssistantRecord>("assistant", json!({"nope": 1}));
            assert!(result.is_err());
            assert!(result.unwrap_err().contains("corrupt assistant payload"));
        }

        #[test]
        fn kv_row_to_item_maps_all_columns() {
            let created = Utc::now();
            let updated = created + chrono::Duration::seconds(5);
            let item = kv_row_to_item("ns", "k", json!({"v": 1}), created, updated);
            assert_eq!(item.namespace, "ns");
            assert_eq!(item.key, "k");
            assert_eq!(item.value, json!({"v": 1}));
            assert_eq!(item.created_at, created);
            assert_eq!(item.updated_at, updated);
        }
    }
}

#[cfg(feature = "postgres")]
pub(crate) use postgres::{LazyPostgresCheckpointer, PostgresStore};
