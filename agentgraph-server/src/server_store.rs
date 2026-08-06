//! Server-side persistence for the platform surface: assistants, crons,
//! threads, and the cross-thread KV store.
//!
//! [`ServerStore`] is the async CRUD trait the routes program against. Two
//! implementations ship:
//!
//! - [`JsonFileStore`] — the default. Existing v0.2 behavior, extracted:
//!   assistants, crons, and threads live in an in-memory index persisted as
//!   one JSON file per record under `{store_path}/{assistants,crons,threads}/`;
//!   KV items are pure file-backed reads/writes under `{store_path}/store/`.
//! - [`PostgresStore`] (feature `postgres`) — tables `server_assistants`,
//!   `server_crons`, `server_threads`, and `server_kv` with JSONB payloads,
//!   auto-migrated on (lazy) connect. Selected via
//!   `ServerConfig::with_postgres(url)`.
//!
//! All trait errors are plain `String`s; routes map them to 500s — no store
//! error is ever a client error (validation happens before the store call).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use tokio::sync::Mutex;

use crate::assistants::{self, AssistantRecord};
use crate::crons::{self, CronRecord};
use crate::store::{self, StoreItem};
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
}

// --------------------------------------------------------------------- //
// PostgresStore — feature `postgres`
// --------------------------------------------------------------------- //

#[cfg(feature = "postgres")]
mod postgres {
    use chrono::{DateTime, Utc};
    use serde_json::Value;
    use sqlx::{PgPool, Row};
    use tokio::sync::OnceCell;

    use super::{ServerStore, StoreResult};
    use crate::assistants::AssistantRecord;
    use crate::crons::CronRecord;
    use crate::store::StoreItem;
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

    /// All idempotent migration statements, executed in order on connect.
    pub(crate) const MIGRATION_SQL: &[&str] = &[
        CREATE_ASSISTANTS_SQL,
        CREATE_CRONS_SQL,
        CREATE_THREADS_SQL,
        CREATE_KV_SQL,
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
    }

    /// Lazily-connecting [`Checkpointer`] facade over core's
    /// [`PostgresCheckpointer`]: connects (and auto-migrates
    /// `agentgraph_checkpoints`) on first checkpoint operation, keeping
    /// [`crate::router`] synchronous.
    pub(crate) struct LazyPostgresCheckpointer {
        url: String,
        inner: OnceCell<agentgraph::checkpoint_postgres::PostgresCheckpointer>,
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
        ) -> agentgraph::error::Result<&agentgraph::checkpoint_postgres::PostgresCheckpointer>
        {
            self.inner
                .get_or_try_init(|| {
                    agentgraph::checkpoint_postgres::PostgresCheckpointer::connect(&self.url)
                })
                .await
        }
    }

    #[async_trait::async_trait]
    impl agentgraph::checkpoint::Checkpointer for LazyPostgresCheckpointer {
        async fn put(
            &self,
            checkpoint: agentgraph::checkpoint::Checkpoint,
        ) -> agentgraph::error::Result<()> {
            self.cp().await?.put(checkpoint).await
        }

        async fn get_latest(
            &self,
            thread_id: &str,
        ) -> agentgraph::error::Result<Option<agentgraph::checkpoint::Checkpoint>> {
            self.cp().await?.get_latest(thread_id).await
        }

        async fn list(
            &self,
            thread_id: &str,
        ) -> agentgraph::error::Result<Vec<agentgraph::checkpoint::Checkpoint>> {
            self.cp().await?.list(thread_id).await
        }

        async fn get_by_id(
            &self,
            thread_id: &str,
            checkpoint_id: &str,
        ) -> agentgraph::error::Result<Option<agentgraph::checkpoint::Checkpoint>> {
            self.cp().await?.get_by_id(thread_id, checkpoint_id).await
        }

        async fn fork_thread(
            &self,
            src_thread: &str,
            dst_thread: &str,
            at_checkpoint_id: Option<&str>,
        ) -> agentgraph::error::Result<usize> {
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
            assert_eq!(MIGRATION_SQL.len(), 4);
            for stmt in MIGRATION_SQL {
                assert!(
                    stmt.contains("CREATE TABLE IF NOT EXISTS"),
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
