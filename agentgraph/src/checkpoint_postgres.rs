//! Postgres-backed checkpointer (crate feature `postgres`).
//!
//! Stores checkpoints in a single table, `agentgraph_checkpoints`:
//!
//! | column         | type          | notes                              |
//! |----------------|---------------|------------------------------------|
//! | `thread_id`    | `text`        | part of the primary key            |
//! | `checkpoint_id`| `text`        | part of the primary key            |
//! | `step`         | `bigint`      | super-step index                   |
//! | `state`        | `jsonb`       | full channel state                 |
//! | `next_nodes`   | `jsonb`       | JSON array of node names           |
//! | `created_at`   | `timestamptz` | wall-clock creation time (UTC)     |
//!
//! The primary key `(thread_id, checkpoint_id)` enforces the no-overwrite
//! contract of the [`Checkpointer`] trait at the database level: a duplicate
//! `put` fails with a unique-violation error mapped to
//! [`AgentGraphError::Checkpoint`].
//!
//! [`PostgresCheckpointer::connect`] runs an idempotent auto-migration
//! (`CREATE TABLE IF NOT EXISTS`) before returning, so a fresh database is
//! ready to use immediately.
//!
//! All `sqlx` errors are mapped to [`AgentGraphError::Checkpoint`]; all
//! statements use bound parameters (`$1`, `$2`, ...), never string
//! interpolation, so thread ids and checkpoint payloads cannot inject SQL.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

use crate::checkpoint::{Checkpoint, Checkpointer};
use crate::error::{AgentGraphError, Result};
use crate::state::State;

/// The table checkpoints are stored in.
pub const TABLE_NAME: &str = "agentgraph_checkpoints";

/// Idempotent schema migration run by [`PostgresCheckpointer::connect`].
const CREATE_TABLE_SQL: &str = "\
CREATE TABLE IF NOT EXISTS agentgraph_checkpoints (
    thread_id     text        NOT NULL,
    checkpoint_id text        NOT NULL,
    step          bigint      NOT NULL,
    state         jsonb       NOT NULL,
    next_nodes    jsonb       NOT NULL,
    created_at    timestamptz NOT NULL,
    PRIMARY KEY (thread_id, checkpoint_id)
)";

/// Insert one checkpoint. No `ON CONFLICT` clause: a duplicate
/// `(thread_id, checkpoint_id)` must surface as an error (SQLSTATE 23505),
/// preserving the trait's no-overwrite contract.
const INSERT_SQL: &str = "\
INSERT INTO agentgraph_checkpoints
    (thread_id, checkpoint_id, step, state, next_nodes, created_at)
VALUES ($1, $2, $3, $4, $5, $6)";

/// The most recent checkpoint for a thread. Recency is defined by super-step
/// (insertion order), with `created_at` as a deterministic tie-break.
const GET_LATEST_SQL: &str = "\
SELECT thread_id, checkpoint_id, step, state, next_nodes, created_at
FROM agentgraph_checkpoints
WHERE thread_id = $1
ORDER BY step DESC, created_at DESC
LIMIT 1";

/// All checkpoints for a thread, oldest first (time-travel listing).
const LIST_SQL: &str = "\
SELECT thread_id, checkpoint_id, step, state, next_nodes, created_at
FROM agentgraph_checkpoints
WHERE thread_id = $1
ORDER BY step ASC";

/// Transaction-scoped advisory lock key used by
/// [`PostgresCheckpointer::migrate`] to serialize concurrent migrations.
const MIGRATION_LOCK_KEY: i64 = 0x6167_7067_5f6d_6967; // "agpg_mig"

/// Map a `sqlx` error to [`AgentGraphError::Checkpoint`], giving duplicate
/// checkpoint ids (SQLSTATE 23505, unique_violation) a clearer message.
fn map_sqlx_error(op: &str, err: sqlx::Error) -> AgentGraphError {
    if let sqlx::Error::Database(db_err) = &err {
        if db_err.code().as_deref() == Some("23505") {
            return AgentGraphError::Checkpoint(format!(
                "{op}: duplicate checkpoint id (unique violation): {db_err}"
            ));
        }
    }
    AgentGraphError::Checkpoint(format!("{op}: {err}"))
}

/// One row of `agentgraph_checkpoints`, decoupled from the live database so
/// the serde mapping is unit-testable without a Postgres server.
#[derive(Debug, Clone, PartialEq)]
struct CheckpointRow {
    thread_id: String,
    checkpoint_id: String,
    step: i64,
    state: Value,
    next_nodes: Value,
    created_at: DateTime<Utc>,
}

impl CheckpointRow {
    /// Checkpoint -> row: `usize` step to `bigint`, state to `jsonb` object,
    /// `next_nodes` to a `jsonb` array.
    fn from_checkpoint(checkpoint: &Checkpoint) -> Result<Self> {
        let step = i64::try_from(checkpoint.step).map_err(|_| {
            AgentGraphError::Checkpoint(format!(
                "step {} does not fit into a Postgres bigint",
                checkpoint.step
            ))
        })?;
        Ok(Self {
            thread_id: checkpoint.thread_id.clone(),
            checkpoint_id: checkpoint.id.clone(),
            step,
            state: checkpoint.state.to_value(),
            next_nodes: serde_json::to_value(&checkpoint.next_nodes)?,
            created_at: checkpoint.created_at,
        })
    }

    /// Decode a query result row by column name, mapping decode failures to
    /// [`AgentGraphError::Checkpoint`].
    fn from_pg_row(row: &sqlx::postgres::PgRow) -> Result<Self> {
        let get = |name: &str| format!("failed to decode column `{name}`");
        Ok(Self {
            thread_id: row
                .try_get("thread_id")
                .map_err(|e| map_sqlx_error(&get("thread_id"), e))?,
            checkpoint_id: row
                .try_get("checkpoint_id")
                .map_err(|e| map_sqlx_error(&get("checkpoint_id"), e))?,
            step: row
                .try_get("step")
                .map_err(|e| map_sqlx_error(&get("step"), e))?,
            state: row
                .try_get("state")
                .map_err(|e| map_sqlx_error(&get("state"), e))?,
            next_nodes: row
                .try_get("next_nodes")
                .map_err(|e| map_sqlx_error(&get("next_nodes"), e))?,
            created_at: row
                .try_get("created_at")
                .map_err(|e| map_sqlx_error(&get("created_at"), e))?,
        })
    }

    /// Row -> checkpoint: `bigint` step back to `usize` (negative values are
    /// rejected), `jsonb` state back to [`State`] (must be a JSON object),
    /// `jsonb` array back to `Vec<String>`.
    fn into_checkpoint(self) -> Result<Checkpoint> {
        let step = usize::try_from(self.step).map_err(|_| {
            AgentGraphError::Checkpoint(format!(
                "stored step {} for checkpoint `{}` is negative; cannot map to usize",
                self.step, self.checkpoint_id
            ))
        })?;
        let state = State::from_value(self.state)?;
        let next_nodes: Vec<String> = serde_json::from_value(self.next_nodes)?;
        Ok(Checkpoint {
            id: self.checkpoint_id,
            thread_id: self.thread_id,
            step,
            state,
            next_nodes,
            created_at: self.created_at,
        })
    }
}

/// Postgres-backed [`Checkpointer`]: durable, multi-process safe, and suitable
/// for production deployments where several executor instances share one
/// database. Clone it freely — clones share the same connection pool.
#[derive(Debug, Clone)]
pub struct PostgresCheckpointer {
    pool: PgPool,
}

impl PostgresCheckpointer {
    /// Connect to Postgres at `url` (e.g.
    /// `postgres://user:pass@localhost/dbname`) and run the idempotent
    /// auto-migration (`CREATE TABLE IF NOT EXISTS agentgraph_checkpoints`)
    /// before returning. Connection and migration failures surface as
    /// [`AgentGraphError::Checkpoint`].
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(url)
            .await
            .map_err(|e| map_sqlx_error("connect", e))?;
        let checkpointer = Self { pool };
        checkpointer.migrate().await?;
        Ok(checkpointer)
    }

    /// Wrap an existing pool (no migration is run — call
    /// [`PostgresCheckpointer::migrate`] explicitly if needed).
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Run the idempotent schema migration. Safe to call repeatedly, and
    /// safe under concurrency: a transaction-scoped advisory lock serializes
    /// concurrent migrators, avoiding the `CREATE TABLE IF NOT EXISTS`
    /// check-then-create race (duplicate key on `pg_type_typname_nsp_index`).
    pub async fn migrate(&self) -> Result<()> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| map_sqlx_error("migrate", e))?;
        sqlx::query("SELECT pg_advisory_xact_lock($1)")
            .bind(MIGRATION_LOCK_KEY)
            .execute(&mut *tx)
            .await
            .map_err(|e| map_sqlx_error("migrate", e))?;
        sqlx::query(CREATE_TABLE_SQL)
            .execute(&mut *tx)
            .await
            .map_err(|e| map_sqlx_error("migrate", e))?;
        tx.commit()
            .await
            .map_err(|e| map_sqlx_error("migrate", e))?;
        Ok(())
    }

    /// The underlying connection pool.
    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

#[async_trait]
impl Checkpointer for PostgresCheckpointer {
    async fn put(&self, checkpoint: Checkpoint) -> Result<()> {
        let row = CheckpointRow::from_checkpoint(&checkpoint)?;
        sqlx::query(INSERT_SQL)
            .bind(&row.thread_id)
            .bind(&row.checkpoint_id)
            .bind(row.step)
            .bind(&row.state)
            .bind(&row.next_nodes)
            .bind(row.created_at)
            .execute(&self.pool)
            .await
            .map_err(|e| map_sqlx_error("put checkpoint", e))?;
        Ok(())
    }

    async fn get_latest(&self, thread_id: &str) -> Result<Option<Checkpoint>> {
        let row = sqlx::query(GET_LATEST_SQL)
            .bind(thread_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| map_sqlx_error("get latest checkpoint", e))?;
        row.map(|r| CheckpointRow::from_pg_row(&r).and_then(CheckpointRow::into_checkpoint))
            .transpose()
    }

    async fn list(&self, thread_id: &str) -> Result<Vec<Checkpoint>> {
        let rows = sqlx::query(LIST_SQL)
            .bind(thread_id)
            .fetch_all(&self.pool)
            .await
            .map_err(|e| map_sqlx_error("list checkpoints", e))?;
        rows.iter()
            .map(|r| CheckpointRow::from_pg_row(r).and_then(CheckpointRow::into_checkpoint))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_checkpoint() -> Checkpoint {
        let mut state = State::new();
        state.insert("answer", serde_json::json!(42));
        state.insert(
            "messages",
            serde_json::json!([{"role": "user", "content": "hi"}]),
        );
        Checkpoint::new("thread-1", 3, state, vec!["node_b".into(), "node_c".into()])
    }

    // ---- SQL generation (no database required) ----

    #[test]
    fn create_table_sql_matches_schema_contract() {
        assert!(CREATE_TABLE_SQL.starts_with("CREATE TABLE IF NOT EXISTS agentgraph_checkpoints"));
        for column in [
            "thread_id     text",
            "checkpoint_id text",
            "step          bigint",
            "state         jsonb",
            "next_nodes    jsonb",
            "created_at    timestamptz",
            "PRIMARY KEY (thread_id, checkpoint_id)",
        ] {
            assert!(
                CREATE_TABLE_SQL.contains(column),
                "migration SQL is missing `{column}`:\n{CREATE_TABLE_SQL}"
            );
        }
    }

    #[test]
    fn insert_sql_binds_all_columns_without_upsert() {
        assert!(INSERT_SQL.starts_with("INSERT INTO agentgraph_checkpoints"));
        assert!(INSERT_SQL.contains("VALUES ($1, $2, $3, $4, $5, $6)"));
        // The no-overwrite contract relies on the PK violation erroring out.
        assert!(!INSERT_SQL.to_uppercase().contains("ON CONFLICT"));
    }

    #[test]
    fn get_latest_sql_orders_by_step_desc_then_created_at_desc() {
        assert!(GET_LATEST_SQL.contains("WHERE thread_id = $1"));
        assert!(GET_LATEST_SQL.contains("ORDER BY step DESC, created_at DESC"));
        assert!(GET_LATEST_SQL.contains("LIMIT 1"));
    }

    #[test]
    fn list_sql_orders_by_step_asc() {
        assert!(LIST_SQL.contains("WHERE thread_id = $1"));
        assert!(LIST_SQL.contains("ORDER BY step ASC"));
        assert!(!LIST_SQL.contains("LIMIT"));
    }

    #[test]
    fn statements_use_bound_parameters_only() {
        for sql in [INSERT_SQL, GET_LATEST_SQL, LIST_SQL] {
            assert!(sql.contains("$1"), "statement must bind parameters: {sql}");
        }
    }

    // ---- Serde / row mapping (no database required) ----

    #[test]
    fn checkpoint_row_roundtrip_preserves_all_fields() {
        let checkpoint = sample_checkpoint();
        let row = CheckpointRow::from_checkpoint(&checkpoint).unwrap();

        assert_eq!(row.thread_id, checkpoint.thread_id);
        assert_eq!(row.checkpoint_id, checkpoint.id);
        assert_eq!(row.step, checkpoint.step as i64);
        assert!(row.state.is_object());
        assert!(row.next_nodes.is_array());

        let back = row.into_checkpoint().unwrap();
        assert_eq!(back.id, checkpoint.id);
        assert_eq!(back.thread_id, checkpoint.thread_id);
        assert_eq!(back.step, checkpoint.step);
        assert_eq!(back.state, checkpoint.state);
        assert_eq!(back.next_nodes, checkpoint.next_nodes);
        assert_eq!(back.created_at, checkpoint.created_at);
    }

    #[test]
    fn row_with_negative_step_is_rejected() {
        let mut row = CheckpointRow::from_checkpoint(&sample_checkpoint()).unwrap();
        row.step = -1;
        let err = row.into_checkpoint().unwrap_err();
        assert!(matches!(err, AgentGraphError::Checkpoint(_)));
    }

    #[test]
    fn row_with_non_object_state_is_rejected() {
        let mut row = CheckpointRow::from_checkpoint(&sample_checkpoint()).unwrap();
        row.state = serde_json::json!([1, 2, 3]);
        assert!(row.into_checkpoint().is_err());
    }

    #[test]
    fn row_with_non_array_next_nodes_is_rejected() {
        let mut row = CheckpointRow::from_checkpoint(&sample_checkpoint()).unwrap();
        row.next_nodes = serde_json::json!("not-an-array");
        assert!(row.into_checkpoint().is_err());
    }

    // ---- Live integration test (opt-in; requires a real Postgres) ----

    /// Full roundtrip against a live database. Not run by default:
    /// `cargo test --features postgres -- --ignored` with `DATABASE_URL` set.
    #[tokio::test]
    #[ignore = "requires a live Postgres; set DATABASE_URL to run"]
    async fn postgres_live_roundtrip() {
        let Ok(url) = std::env::var("DATABASE_URL") else {
            eprintln!("DATABASE_URL not set; skipping live Postgres test");
            return;
        };
        let store = PostgresCheckpointer::connect(&url).await.unwrap();

        // Isolate the test in its own thread id.
        let thread = format!("test-{}", uuid::Uuid::new_v4());
        assert!(store.get_latest(&thread).await.unwrap().is_none());
        assert!(store.list(&thread).await.unwrap().is_empty());

        let cp0 = sample_checkpoint_with(&thread, 0);
        let cp1 = sample_checkpoint_with(&thread, 1);
        store.put(cp0.clone()).await.unwrap();
        store.put(cp1.clone()).await.unwrap();

        // Latest = highest step.
        let latest = store.get_latest(&thread).await.unwrap().unwrap();
        assert_eq!(latest.id, cp1.id);

        // List = ascending steps.
        let all = store.list(&thread).await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].id, cp0.id);
        assert_eq!(all[1].id, cp1.id);
        assert_eq!(all[0].state, cp0.state);
        assert_eq!(all[1].next_nodes, cp1.next_nodes);

        // Duplicate (thread_id, checkpoint_id) is rejected.
        let err = store.put(cp1).await.unwrap_err();
        assert!(matches!(err, AgentGraphError::Checkpoint(_)));

        // Cleanup.
        sqlx::query("DELETE FROM agentgraph_checkpoints WHERE thread_id = $1")
            .bind(&thread)
            .execute(store.pool())
            .await
            .unwrap();
    }

    fn sample_checkpoint_with(thread: &str, step: usize) -> Checkpoint {
        let mut state = State::new();
        state.insert("answer", serde_json::json!(step));
        Checkpoint::new(thread, step, state, vec!["next".into()])
    }
}
