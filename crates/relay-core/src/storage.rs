use crate::contracts::{
    EdgeKind, Freshness, GraphEdge, GraphNode, HandoffSummary, NodeKind, PatchProposal, RuntimeMode,
    TaskId, TaskRecord, TaskSpec, TaskState,
};
use crate::error::{RelayError, RelayResult};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::types::Json;
use sqlx::{Sqlite, SqliteConnection, SqlitePool, Transaction};
use std::path::Path;

/// SQLite schema for the DAG, task records, handoffs, logs, and offline sync.
pub const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS dag_nodes (
    id TEXT PRIMARY KEY NOT NULL,
    repo_id TEXT NOT NULL,
    label TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    freshness_json TEXT NOT NULL,
    device_origin TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS dag_edges (
    id TEXT PRIMARY KEY NOT NULL,
    repo_id TEXT NOT NULL,
    from_node TEXT NOT NULL,
    to_node TEXT NOT NULL,
    kind TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS tasks (
    task_id TEXT PRIMARY KEY NOT NULL,
    repo_id TEXT NOT NULL,
    spec_json TEXT NOT NULL,
    state TEXT NOT NULL,
    active_proposal_json TEXT,
    preflight_summary TEXT,
    evidence_node_id TEXT,
    error_message TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS handoffs (
    handoff_id TEXT PRIMARY KEY NOT NULL,
    repo_id TEXT NOT NULL,
    source_device TEXT NOT NULL,
    base_commit_sha TEXT NOT NULL,
    summary TEXT NOT NULL,
    active_task_count INTEGER NOT NULL,
    stale_evidence_count INTEGER NOT NULL,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS log_blobs (
    log_blob_hash TEXT PRIMARY KEY NOT NULL,
    content TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS runtime_modes (
    repo_id TEXT PRIMARY KEY NOT NULL,
    mode TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sync_outbox (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    repo_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    delivered INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS dag_nodes_repo ON dag_nodes (repo_id);
CREATE INDEX IF NOT EXISTS dag_edges_repo ON dag_edges (repo_id);
CREATE INDEX IF NOT EXISTS tasks_repo_created ON tasks (repo_id, created_at DESC);
CREATE INDEX IF NOT EXISTS handoffs_repo_created ON handoffs (repo_id, created_at DESC);
CREATE INDEX IF NOT EXISTS sync_outbox_pending ON sync_outbox (repo_id, seq) WHERE delivered = 0;
"#;

#[derive(Clone, Debug)]
pub struct RelayStore {
    pool: SqlitePool,
}

impl RelayStore {
    pub async fn connect(db_path: &Path) -> RelayResult<Self> {
        if let Some(parent) = db_path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            tokio::fs::create_dir_all(parent).await?;
        }
        let opts = SqliteConnectOptions::new()
            .filename(db_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await?;
        Self::init_schema(pool).await
    }

    pub async fn connect_in_memory() -> RelayResult<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?;
        Self::init_schema(pool).await
    }

    async fn init_schema(pool: SqlitePool) -> RelayResult<Self> {
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        for stmt in SCHEMA_SQL
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            sqlx::query(stmt).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub(crate) async fn begin_write(&self) -> RelayResult<Transaction<'static, Sqlite>> {
        Ok(self.pool.begin_with("BEGIN IMMEDIATE").await?)
    }

    pub async fn upsert_dag_node(&self, repo_id: &str, node: &GraphNode) -> RelayResult<()> {
        write_node(&mut *self.pool.acquire().await?, repo_id, node).await
    }

    pub async fn upsert_dag_edge(&self, repo_id: &str, edge: &GraphEdge) -> RelayResult<()> {
        write_edge(&mut *self.pool.acquire().await?, repo_id, edge).await
    }

    pub async fn load_dag(&self, repo_id: &str) -> RelayResult<(Vec<GraphNode>, Vec<GraphEdge>)> {
        type NodeRow = (
            String,
            String,
            Json<NodeKind>,
            Json<Freshness>,
            String,
            DateTime<Utc>,
            DateTime<Utc>,
        );
        type EdgeRow = (String, String, String, Json<EdgeKind>);

        let mut tx = self.pool.begin().await?;
        let nodes = sqlx::query_as::<_, NodeRow>(
            "SELECT id, label, payload_json, freshness_json, device_origin, created_at, updated_at
             FROM dag_nodes WHERE repo_id = ? ORDER BY id",
        )
        .bind(repo_id)
        .fetch_all(&mut *tx)
        .await?;
        let edges = sqlx::query_as::<_, EdgeRow>(
            "SELECT id, from_node, to_node, kind FROM dag_edges WHERE repo_id = ? ORDER BY id",
        )
        .bind(repo_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;

        let nodes = nodes
            .into_iter()
            .map(
                |(id, label, Json(payload), Json(freshness), device_origin, created_at, updated_at)| {
                    GraphNode {
                        id,
                        label,
                        payload,
                        freshness,
                        device_origin,
                        created_at,
                        updated_at,
                    }
                },
            )
            .collect();
        let edges = edges
            .into_iter()
            .map(|(id, from, to, Json(kind))| GraphEdge {
                id,
                from,
                to,
                kind,
            })
            .collect();
        Ok((nodes, edges))
    }

    pub async fn update_nodes_freshness(&self, nodes: &[GraphNode]) -> RelayResult<()> {
        let mut tx = self.begin_write().await?;
        write_freshness(&mut tx, nodes.iter()).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn insert_task_idempotent(
        &self,
        record: &TaskRecord,
    ) -> RelayResult<(TaskRecord, bool)> {
        let mut tx = self.begin_write().await?;
        let result = insert_task(&mut tx, record).await?;
        tx.commit().await?;
        Ok(result)
    }

    pub async fn update_task(&self, record: &TaskRecord) -> RelayResult<()> {
        write_task(&mut *self.pool.acquire().await?, record).await
    }

    pub async fn get_task(&self, task_id: TaskId) -> RelayResult<Option<TaskRecord>> {
        fetch_task(&mut *self.pool.acquire().await?, task_id).await
    }

    pub async fn list_tasks(&self, repo_id: Option<&str>) -> RelayResult<Vec<TaskRecord>> {
        fetch_tasks(&mut *self.pool.acquire().await?, repo_id).await
    }

    pub async fn insert_handoff(&self, handoff: &HandoffSummary) -> RelayResult<()> {
        write_handoff(&mut *self.pool.acquire().await?, handoff).await
    }

    pub async fn list_handoffs(&self, limit: u32) -> RelayResult<Vec<HandoffSummary>> {
        self.list_repo_handoffs(None, limit).await
    }

    pub(crate) async fn list_repo_handoffs(
        &self,
        repo_id: Option<&str>,
        limit: u32,
    ) -> RelayResult<Vec<HandoffSummary>> {
        type HandoffRow = (String, String, String, String, String, u32, u32, DateTime<Utc>);
        let rows = sqlx::query_as::<_, HandoffRow>(
            "SELECT handoff_id, repo_id, source_device, base_commit_sha, summary,
                    active_task_count, stale_evidence_count, created_at
             FROM handoffs WHERE (?1 IS NULL OR repo_id = ?1)
             ORDER BY created_at DESC, rowid DESC LIMIT ?2",
        )
        .bind(repo_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(
                    handoff_id,
                    repo_id,
                    source_device,
                    base_commit_sha,
                    summary,
                    active_task_count,
                    stale_evidence_count,
                    created_at,
                )| HandoffSummary {
                    handoff_id,
                    repo_id,
                    source_device,
                    base_commit_sha,
                    summary,
                    active_task_count,
                    stale_evidence_count,
                    created_at,
                },
            )
            .collect())
    }

    pub async fn put_log_blob(&self, blob_hash: &str, content: &str) -> RelayResult<()> {
        write_log_blob(&mut *self.pool.acquire().await?, blob_hash, content).await
    }

    pub async fn get_log_blob(&self, blob_hash: &str) -> RelayResult<Option<String>> {
        Ok(sqlx::query_scalar("SELECT content FROM log_blobs WHERE log_blob_hash = ?")
            .bind(blob_hash)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub(crate) async fn get_runtime_mode(&self, repo_id: &str) -> RelayResult<Option<RuntimeMode>> {
        let row = sqlx::query_scalar::<_, Json<RuntimeMode>>(
            "SELECT mode FROM runtime_modes WHERE repo_id = ?",
        )
        .bind(repo_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|Json(mode)| mode))
    }

    pub(crate) async fn put_runtime_mode(&self, repo_id: &str, mode: RuntimeMode) -> RelayResult<()> {
        sqlx::query(
            "INSERT INTO runtime_modes (repo_id, mode) VALUES (?, ?)
             ON CONFLICT (repo_id) DO UPDATE SET mode = excluded.mode",
        )
        .bind(repo_id)
        .bind(Json(mode))
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn enqueue_outbox(&self, repo_id: &str, payload: &Value) -> RelayResult<i64> {
        let mut conn = self.pool.acquire().await?;
        write_outbox(&mut conn, repo_id, payload).await
    }

    pub async fn list_pending_outbox(
        &self,
        repo_id: Option<&str>,
    ) -> RelayResult<Vec<(i64, String, Value)>> {
        let rows = sqlx::query_as::<_, (i64, String, Json<Value>)>(
            "SELECT seq, repo_id, payload_json FROM sync_outbox
             WHERE delivered = 0 AND (?1 IS NULL OR repo_id = ?1) ORDER BY seq",
        )
        .bind(repo_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(seq, repo, Json(payload))| (seq, repo, payload))
            .collect())
    }

    pub async fn mark_outbox_delivered(&self, seq: i64) -> RelayResult<()> {
        self.mark_repo_outbox_delivered(None, seq).await?;
        Ok(())
    }

    pub(crate) async fn mark_repo_outbox_delivered(
        &self,
        repo_id: Option<&str>,
        seq: i64,
    ) -> RelayResult<bool> {
        let result = sqlx::query(
            "UPDATE sync_outbox SET delivered = 1
             WHERE seq = ?1 AND delivered = 0 AND (?2 IS NULL OR repo_id = ?2)",
        )
        .bind(seq)
        .bind(repo_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() != 0)
    }

    pub async fn count_pending_outbox(&self) -> RelayResult<u32> {
        self.count_repo_pending_outbox(None).await
    }

    pub(crate) async fn count_repo_pending_outbox(&self, repo_id: Option<&str>) -> RelayResult<u32> {
        Ok(sqlx::query_scalar(
            "SELECT COUNT(*) FROM sync_outbox WHERE delivered = 0 AND (?1 IS NULL OR repo_id = ?1)",
        )
        .bind(repo_id)
        .fetch_one(&self.pool)
        .await?)
    }
}

pub(crate) async fn write_node(
    conn: &mut SqliteConnection,
    repo_id: &str,
    node: &GraphNode,
) -> RelayResult<()> {
    let result = sqlx::query(
        "INSERT INTO dag_nodes (id, repo_id, label, payload_json, freshness_json, device_origin,
                                created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET label = excluded.label, payload_json = excluded.payload_json,
             freshness_json = excluded.freshness_json, device_origin = excluded.device_origin,
             created_at = excluded.created_at, updated_at = excluded.updated_at
         WHERE dag_nodes.repo_id = excluded.repo_id",
    )
    .bind(&node.id)
    .bind(repo_id)
    .bind(&node.label)
    .bind(Json(&node.payload))
    .bind(Json(&node.freshness))
    .bind(&node.device_origin)
    .bind(timestamp(node.created_at))
    .bind(timestamp(node.updated_at))
    .execute(conn)
    .await?;
    if result.rows_affected() == 0 {
        return Err(RelayError::Internal(format!(
            "node {} belongs to another repository",
            node.id,
        )));
    }
    Ok(())
}

pub(crate) async fn write_edge(
    conn: &mut SqliteConnection,
    repo_id: &str,
    edge: &GraphEdge,
) -> RelayResult<()> {
    let result = sqlx::query(
        "INSERT INTO dag_edges (id, repo_id, from_node, to_node, kind) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT (id) DO UPDATE SET from_node = excluded.from_node, to_node = excluded.to_node,
             kind = excluded.kind
         WHERE dag_edges.repo_id = excluded.repo_id",
    )
    .bind(&edge.id)
    .bind(repo_id)
    .bind(&edge.from)
    .bind(&edge.to)
    .bind(Json(edge.kind))
    .execute(conn)
    .await?;
    if result.rows_affected() == 0 {
        return Err(RelayError::Internal(format!(
            "edge {} belongs to another repository",
            edge.id,
        )));
    }
    Ok(())
}

pub(crate) async fn write_freshness<'a>(
    conn: &mut SqliteConnection,
    nodes: impl IntoIterator<Item = &'a GraphNode>,
) -> RelayResult<()> {
    for node in nodes {
        let result = sqlx::query(
            "UPDATE dag_nodes SET freshness_json = ?, updated_at = ? WHERE id = ?",
        )
        .bind(Json(&node.freshness))
        .bind(timestamp(node.updated_at))
        .bind(&node.id)
        .execute(&mut *conn)
        .await?;
        if result.rows_affected() == 0 {
            return Err(RelayError::NodeNotFound(node.id.clone()));
        }
    }
    Ok(())
}

pub(crate) async fn write_outbox(
    conn: &mut SqliteConnection,
    repo_id: &str,
    payload: &Value,
) -> RelayResult<i64> {
    let result = sqlx::query(
        "INSERT INTO sync_outbox (repo_id, payload_json, created_at) VALUES (?, ?, ?)",
    )
    .bind(repo_id)
    .bind(Json(payload))
    .bind(timestamp(Utc::now()))
    .execute(conn)
    .await?;
    Ok(result.last_insert_rowid())
}

pub(crate) async fn insert_task(
    conn: &mut SqliteConnection,
    record: &TaskRecord,
) -> RelayResult<(TaskRecord, bool)> {
    let result = sqlx::query(
        "INSERT OR IGNORE INTO tasks (task_id, repo_id, spec_json, state, active_proposal_json,
                                      preflight_summary, evidence_node_id, error_message,
                                      created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(record.spec.task_id.to_string())
    .bind(&record.spec.repo_id)
    .bind(Json(&record.spec))
    .bind(Json(record.state))
    .bind(record.active_proposal.as_ref().map(Json))
    .bind(&record.preflight_summary)
    .bind(&record.evidence_node_id)
    .bind(&record.error_message)
    .bind(timestamp(record.created_at))
    .bind(timestamp(record.updated_at))
    .execute(&mut *conn)
    .await?;
    if result.rows_affected() != 0 {
        return Ok((record.clone(), true));
    }
    let existing = fetch_task(conn, record.spec.task_id)
        .await?
        .ok_or_else(|| RelayError::TaskNotFound(record.spec.task_id.to_string()))?;
    Ok((existing, false))
}

pub(crate) async fn write_task(conn: &mut SqliteConnection, record: &TaskRecord) -> RelayResult<()> {
    let result = sqlx::query(
        "UPDATE tasks SET spec_json = ?, state = ?, active_proposal_json = ?, preflight_summary = ?,
                          evidence_node_id = ?, error_message = ?, created_at = ?, updated_at = ?
         WHERE task_id = ? AND repo_id = ?",
    )
    .bind(Json(&record.spec))
    .bind(Json(record.state))
    .bind(record.active_proposal.as_ref().map(Json))
    .bind(&record.preflight_summary)
    .bind(&record.evidence_node_id)
    .bind(&record.error_message)
    .bind(timestamp(record.created_at))
    .bind(timestamp(record.updated_at))
    .bind(record.spec.task_id.to_string())
    .bind(&record.spec.repo_id)
    .execute(conn)
    .await?;
    if result.rows_affected() == 0 {
        return Err(RelayError::TaskNotFound(record.spec.task_id.to_string()));
    }
    Ok(())
}

type TaskRow = (
    Json<TaskSpec>,
    Json<TaskState>,
    Option<Json<PatchProposal>>,
    Option<String>,
    Option<String>,
    Option<String>,
    DateTime<Utc>,
    DateTime<Utc>,
);

const TASK_SELECT_BY_ID: &str =
    "SELECT spec_json, state, active_proposal_json, preflight_summary, evidence_node_id,
            error_message, created_at, updated_at FROM tasks WHERE task_id = ?";

const TASK_SELECT_BY_REPO: &str =
    "SELECT spec_json, state, active_proposal_json, preflight_summary, evidence_node_id,
            error_message, created_at, updated_at FROM tasks
     WHERE (?1 IS NULL OR repo_id = ?1)
     ORDER BY created_at DESC, rowid DESC";

fn task_record(
    (
        Json(spec),
        Json(state),
        proposal,
        preflight_summary,
        evidence_node_id,
        error_message,
        created_at,
        updated_at,
    ): TaskRow,
) -> TaskRecord {
    TaskRecord {
        spec,
        state,
        active_proposal: proposal.map(|Json(proposal)| proposal),
        preflight_summary,
        evidence_node_id,
        error_message,
        created_at,
        updated_at,
    }
}

pub(crate) async fn fetch_task(
    conn: &mut SqliteConnection,
    task_id: TaskId,
) -> RelayResult<Option<TaskRecord>> {
    let row = sqlx::query_as::<_, TaskRow>(TASK_SELECT_BY_ID)
        .bind(task_id.to_string())
        .fetch_optional(conn)
        .await?;
    Ok(row.map(task_record))
}

pub(crate) async fn fetch_tasks(
    conn: &mut SqliteConnection,
    repo_id: Option<&str>,
) -> RelayResult<Vec<TaskRecord>> {
    let rows = sqlx::query_as::<_, TaskRow>(TASK_SELECT_BY_REPO)
        .bind(repo_id)
        .fetch_all(conn)
        .await?;
    Ok(rows.into_iter().map(task_record).collect())
}

pub(crate) async fn write_handoff(
    conn: &mut SqliteConnection,
    handoff: &HandoffSummary,
) -> RelayResult<()> {
    sqlx::query(
        "INSERT INTO handoffs (handoff_id, repo_id, source_device, base_commit_sha, summary,
                               active_task_count, stale_evidence_count, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&handoff.handoff_id)
    .bind(&handoff.repo_id)
    .bind(&handoff.source_device)
    .bind(&handoff.base_commit_sha)
    .bind(&handoff.summary)
    .bind(handoff.active_task_count)
    .bind(handoff.stale_evidence_count)
    .bind(timestamp(handoff.created_at))
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn write_log_blob(
    conn: &mut SqliteConnection,
    blob_hash: &str,
    content: &str,
) -> RelayResult<()> {
    sqlx::query("INSERT INTO log_blobs (log_blob_hash, content) VALUES (?, ?) ON CONFLICT DO NOTHING")
        .bind(blob_hash)
        .bind(content)
        .execute(conn)
        .await?;
    Ok(())
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn initializes_sqlite_schema_in_memory() {
        let store = RelayStore::connect_in_memory().await.unwrap();
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM tasks")
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert_eq!(row.0, 0);
    }
}
