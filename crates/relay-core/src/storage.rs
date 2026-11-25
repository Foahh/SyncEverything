use crate::agent::{AgentApprovalCard, AgentChatMessage, AgentChatRole, AgentToolTrace};
use crate::contracts::{
    EdgeKind, Freshness, GraphEdge, GraphNode, HandoffSummary, NodeKind, PatchProposal, PeerRole,
    RuntimeMode, TaskId, TaskRecord, TaskSpec, TaskState, TrustedPeerRecord,
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

CREATE TABLE IF NOT EXISTS task_log_refs (
    task_id TEXT PRIMARY KEY NOT NULL,
    repo_id TEXT NOT NULL,
    log_blob_hash TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS runtime_modes (
    repo_id TEXT PRIMARY KEY NOT NULL,
    mode TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS observed_git_heads (
    repo_id TEXT NOT NULL,
    checkout BLOB NOT NULL,
    commit_sha TEXT NOT NULL,
    PRIMARY KEY (repo_id, checkout)
);

CREATE TABLE IF NOT EXISTS sync_outbox (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    repo_id TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    delivered INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS sync_outbox_recipients (
    seq INTEGER PRIMARY KEY REFERENCES sync_outbox (seq) ON DELETE CASCADE,
    node_id TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS chat_messages (
    id TEXT PRIMARY KEY NOT NULL,
    repo_id TEXT NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    traces_json TEXT NOT NULL,
    approval_json TEXT,
    error_message TEXT,
    created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS trusted_peers (
    node_id TEXT PRIMARY KEY NOT NULL,
    label TEXT NOT NULL,
    role TEXT NOT NULL,
    ticket_or_addrs TEXT NOT NULL,
    paired_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    revoked_at TEXT
);

CREATE INDEX IF NOT EXISTS dag_nodes_repo ON dag_nodes (repo_id);
CREATE INDEX IF NOT EXISTS dag_edges_repo ON dag_edges (repo_id);
CREATE INDEX IF NOT EXISTS tasks_repo_created ON tasks (repo_id, created_at DESC);
CREATE INDEX IF NOT EXISTS handoffs_repo_created ON handoffs (repo_id, created_at DESC);
CREATE INDEX IF NOT EXISTS sync_outbox_pending ON sync_outbox (repo_id, seq) WHERE delivered = 0;
CREATE INDEX IF NOT EXISTS chat_messages_repo_created ON chat_messages (repo_id, created_at ASC);
CREATE INDEX IF NOT EXISTS trusted_peers_active_seen ON trusted_peers (revoked_at, last_seen_at DESC);

-- A persisted counter works across pooled connections, processes, and direct SQLite imports.
CREATE TABLE IF NOT EXISTS dag_revision (id INTEGER PRIMARY KEY CHECK (id = 1), value INTEGER NOT NULL);
INSERT OR IGNORE INTO dag_revision (id, value) VALUES (1, 0);
CREATE TRIGGER IF NOT EXISTS dag_node_insert AFTER INSERT ON dag_nodes
BEGIN UPDATE dag_revision SET value = value + 1; END;
CREATE TRIGGER IF NOT EXISTS dag_node_update AFTER UPDATE ON dag_nodes
BEGIN UPDATE dag_revision SET value = value + 1; END;
CREATE TRIGGER IF NOT EXISTS dag_node_delete AFTER DELETE ON dag_nodes
BEGIN UPDATE dag_revision SET value = value + 1; END;
CREATE TRIGGER IF NOT EXISTS dag_edge_insert AFTER INSERT ON dag_edges
BEGIN UPDATE dag_revision SET value = value + 1; END;
CREATE TRIGGER IF NOT EXISTS dag_edge_update AFTER UPDATE ON dag_edges
BEGIN UPDATE dag_revision SET value = value + 1; END;
CREATE TRIGGER IF NOT EXISTS dag_edge_delete AFTER DELETE ON dag_edges
BEGIN UPDATE dag_revision SET value = value + 1; END;
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
            // The sole connection owns this database. An interrupted acquire must
            // never discard it while awaiting a health check.
            .test_before_acquire(false)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect("sqlite::memory:")
            .await?;
        Self::init_schema(pool).await
    }

    async fn init_schema(pool: SqlitePool) -> RelayResult<Self> {
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::raw_sql(SCHEMA_SQL).execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(Self { pool })
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub(crate) async fn begin_write(&self) -> RelayResult<Transaction<'static, Sqlite>> {
        Ok(self.pool.begin_with("BEGIN IMMEDIATE").await?)
    }

    pub(crate) async fn observed_git_head(&self, repo_id: &str, checkout: &Path) -> RelayResult<Option<String>> {
        Self::fetch_observed_head(&mut *self.pool.acquire().await?, repo_id, checkout).await
    }

    pub(crate) async fn fetch_observed_head(
        conn: &mut SqliteConnection,
        repo_id: &str,
        checkout: &Path,
    ) -> RelayResult<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT commit_sha FROM observed_git_heads WHERE repo_id = ? AND checkout = ?",
        )
            .bind(repo_id)
            .bind(checkout.as_os_str().as_encoded_bytes())
            .fetch_optional(conn)
            .await?)
    }

    pub(crate) async fn write_observed_head(
        conn: &mut SqliteConnection,
        repo_id: &str,
        checkout: &Path,
        sha: &str,
    ) -> RelayResult<()> {
        sqlx::query("INSERT INTO observed_git_heads (repo_id, checkout, commit_sha) VALUES (?, ?, ?)
                     ON CONFLICT (repo_id, checkout) DO UPDATE SET commit_sha = excluded.commit_sha")
            .bind(repo_id)
            .bind(checkout.as_os_str().as_encoded_bytes())
            .bind(sha)
            .execute(conn)
            .await?;
        Ok(())
    }

    pub async fn upsert_dag_node(&self, repo_id: &str, node: &GraphNode) -> RelayResult<()> {
        write_node(&mut *self.pool.acquire().await?, repo_id, node).await
    }

    pub async fn upsert_dag_edge(&self, repo_id: &str, edge: &GraphEdge) -> RelayResult<()> {
        write_edge(&mut *self.pool.acquire().await?, repo_id, edge).await
    }

    pub async fn load_dag(&self, repo_id: &str) -> RelayResult<(Vec<GraphNode>, Vec<GraphEdge>)> {
        let mut tx = self.pool.begin().await?;
        let rows = Self::fetch_dag(&mut tx, repo_id).await?;
        tx.commit().await?;
        Ok(rows)
    }

    pub(crate) async fn fetch_dag(
        conn: &mut SqliteConnection,
        repo_id: &str,
    ) -> RelayResult<(Vec<GraphNode>, Vec<GraphEdge>)> {
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

        let nodes = sqlx::query_as::<_, NodeRow>(
            "SELECT id, label, payload_json, freshness_json, device_origin, created_at, updated_at
             FROM dag_nodes WHERE repo_id = ? ORDER BY id",
        )
        .bind(repo_id)
        .fetch_all(&mut *conn)
        .await?;
        let edges = sqlx::query_as::<_, EdgeRow>(
            "SELECT id, from_node, to_node, kind FROM dag_edges WHERE repo_id = ? ORDER BY id",
        )
        .bind(repo_id)
        .fetch_all(conn)
        .await?;

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
        Self::fetch_log_blob(&mut *self.pool.acquire().await?, blob_hash).await
    }

    pub(crate) async fn fetch_log_blob(conn: &mut SqliteConnection, blob_hash: &str) -> RelayResult<Option<String>> {
        Ok(sqlx::query_scalar("SELECT content FROM log_blobs WHERE log_blob_hash = ?")
            .bind(blob_hash)
            .fetch_optional(conn)
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
        write_outbox(&mut conn, repo_id, payload, None).await
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

    pub(crate) async fn claim_peer_outbox(
        &self,
        repo_id: &str,
        node_id: &str,
    ) -> RelayResult<Vec<(i64, Value)>> {
        let mut tx = self.begin_write().await?;
        // Unassigned work is attached to the first host before any request can be sent.
        sqlx::query(
            "INSERT OR IGNORE INTO sync_outbox_recipients (seq, node_id)
             SELECT seq, ? FROM sync_outbox WHERE repo_id = ? AND delivered = 0",
        )
        .bind(node_id)
        .bind(repo_id)
        .execute(&mut *tx)
        .await?;
        let rows = sqlx::query_as::<_, (i64, Json<Value>)>(
            "SELECT o.seq, o.payload_json FROM sync_outbox o
             JOIN sync_outbox_recipients r ON r.seq = o.seq
             WHERE o.repo_id = ? AND o.delivered = 0 AND r.node_id = ? ORDER BY o.seq",
        )
        .bind(repo_id)
        .bind(node_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows.into_iter().map(|(seq, Json(payload))| (seq, payload)).collect())
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

    pub async fn list_chat_messages(
        &self,
        repo_id: &str,
        limit: u32,
    ) -> RelayResult<Vec<AgentChatMessage>> {
        type ChatRow = (
            String,
            String,
            Json<AgentChatRole>,
            String,
            Json<Vec<AgentToolTrace>>,
            Option<Json<AgentApprovalCard>>,
            Option<String>,
            DateTime<Utc>,
        );
        let rows = sqlx::query_as::<_, ChatRow>(
            "SELECT id, repo_id, role, content, traces_json, approval_json, error_message, created_at
             FROM (
                 SELECT id, repo_id, role, content, traces_json, approval_json, error_message, created_at, rowid
                 FROM chat_messages
                 WHERE repo_id = ?1
                 ORDER BY created_at DESC, rowid DESC
                 LIMIT ?2
             )
             ORDER BY created_at ASC, rowid ASC",
        )
        .bind(repo_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(id, repo_id, Json(role), content, Json(traces), approval, error, created_at)| {
                    AgentChatMessage {
                        id,
                        repo_id,
                        role,
                        content,
                        traces,
                        approval: approval.map(|Json(card)| card),
                        error,
                        created_at,
                    }
                },
            )
            .collect())
    }

    pub async fn upsert_chat_message(&self, message: &AgentChatMessage) -> RelayResult<()> {
        let result = sqlx::query(
            "INSERT INTO chat_messages (id, repo_id, role, content, traces_json, approval_json, error_message, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (id) DO UPDATE SET
                 content = excluded.content,
                 traces_json = excluded.traces_json,
                 approval_json = CASE
                     WHEN json_extract(chat_messages.approval_json, '$.resolved') IS NOT NULL
                       AND json_extract(chat_messages.approval_json, '$.task_id') = json_extract(excluded.approval_json, '$.task_id')
                       AND json_extract(chat_messages.approval_json, '$.patch_id') IS json_extract(excluded.approval_json, '$.patch_id')
                     THEN chat_messages.approval_json
                     ELSE excluded.approval_json END,
                 error_message = excluded.error_message
             WHERE chat_messages.repo_id = excluded.repo_id",
        )
        .bind(&message.id)
        .bind(&message.repo_id)
        .bind(Json(message.role))
        .bind(&message.content)
        .bind(Json(&message.traces))
        .bind(message.approval.as_ref().map(Json))
        .bind(&message.error)
        .bind(timestamp(message.created_at))
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(RelayError::Internal(format!(
                "chat message {} belongs to another repository",
                message.id,
            )));
        }
        Ok(())
    }

    pub async fn clear_chat_messages(&self, repo_id: &str) -> RelayResult<()> {
        sqlx::query("DELETE FROM chat_messages WHERE repo_id = ?")
            .bind(repo_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn resolve_chat_approvals(
        &self,
        repo_id: &str,
        task_id: &str,
        patch_id: Option<&str>,
        decision: &str,
    ) -> RelayResult<u32> {
        let mut tx = self.begin_write().await?;
        let rows = sqlx::query_as::<_, (String, Json<AgentApprovalCard>)>(
            "SELECT id, approval_json FROM chat_messages
             WHERE repo_id = ? AND approval_json IS NOT NULL",
        )
        .bind(repo_id)
        .fetch_all(&mut *tx)
        .await?;

        let mut updated = 0u32;
        for (id, Json(mut card)) in rows {
            if card.task_id == task_id && card.patch_id.as_deref() == patch_id && card.resolved.is_none() {
                card.resolved = Some(decision.to_owned());
                sqlx::query(
                    "UPDATE chat_messages SET approval_json = ? WHERE id = ? AND repo_id = ?",
                )
                .bind(Json(&card))
                .bind(&id)
                .bind(repo_id)
                .execute(&mut *tx)
                .await?;
                updated += 1;
            }
        }
        tx.commit().await?;
        Ok(updated)
    }

    /// Insert or update a trusted peer record.
    ///
    /// Preserves `paired_at`, existing non-empty custom labels, and existing revocation state
    /// (never silently unrevokes a revoked peer).
    pub async fn upsert_trusted_peer(
        &self,
        peer: &TrustedPeerRecord,
    ) -> RelayResult<TrustedPeerRecord> {
        let node_id = peer.node_id.trim();
        if node_id.is_empty() {
            return Err(RelayError::Internal("peer node_id cannot be empty".into()));
        }
        let label = peer.label.trim();
        let effective_label = if label.is_empty() { node_id } else { label };
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO trusted_peers (node_id, label, role, ticket_or_addrs, paired_at, last_seen_at, revoked_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (node_id) DO UPDATE SET
                 label = CASE
                     WHEN trusted_peers.revoked_at IS NOT NULL THEN trusted_peers.label
                     WHEN trusted_peers.label != '' THEN trusted_peers.label
                     ELSE excluded.label
                 END,
                 role = CASE
                     WHEN trusted_peers.revoked_at IS NOT NULL THEN trusted_peers.role
                     WHEN excluded.role != 'peer' THEN excluded.role
                     ELSE trusted_peers.role
                 END,
                 ticket_or_addrs = CASE
                     WHEN trusted_peers.revoked_at IS NOT NULL THEN trusted_peers.ticket_or_addrs
                     WHEN excluded.ticket_or_addrs != '' THEN excluded.ticket_or_addrs
                     ELSE trusted_peers.ticket_or_addrs
                 END,
                 last_seen_at = CASE
                     WHEN trusted_peers.revoked_at IS NOT NULL THEN trusted_peers.last_seen_at
                     WHEN excluded.last_seen_at > trusted_peers.last_seen_at THEN excluded.last_seen_at
                     ELSE trusted_peers.last_seen_at
                 END,
                 revoked_at = COALESCE(trusted_peers.revoked_at, excluded.revoked_at)",
        )
        .bind(node_id)
        .bind(effective_label)
        .bind(peer.role.as_str())
        .bind(&peer.ticket_or_addrs)
        .bind(timestamp(peer.paired_at))
        .bind(timestamp(peer.last_seen_at))
        .bind(peer.revoked_at.map(timestamp))
        .execute(&mut *tx)
        .await?;
        let stored = fetch_trusted_peer(&mut tx, node_id)
            .await?
            .ok_or_else(|| RelayError::PeerNotFound(node_id.to_owned()))?;
        tx.commit().await?;
        Ok(stored)
    }

    pub async fn list_trusted_peers(&self) -> RelayResult<Vec<TrustedPeerRecord>> {
        let rows = sqlx::query_as::<_, TrustedPeerRow>(TRUSTED_PEER_SELECT_ALL)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(trusted_peer_record).collect())
    }

    pub async fn get_trusted_peer(&self, node_id: &str) -> RelayResult<Option<TrustedPeerRecord>> {
        fetch_trusted_peer(&mut *self.pool.acquire().await?, node_id).await
    }

    pub async fn is_peer_revoked(&self, node_id: &str) -> RelayResult<bool> {
        let revoked: i64 = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM trusted_peers WHERE node_id = ? AND revoked_at IS NOT NULL)",
        )
        .bind(node_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(revoked != 0)
    }

    pub async fn touch_trusted_peer_seen(&self, node_id: &str) -> RelayResult<bool> {
        let result = sqlx::query(
            "UPDATE trusted_peers SET last_seen_at = ? WHERE node_id = ? AND revoked_at IS NULL",
        )
        .bind(timestamp(Utc::now()))
        .bind(node_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() != 0)
    }

    pub async fn rename_trusted_peer(
        &self,
        node_id: &str,
        label: &str,
    ) -> RelayResult<TrustedPeerRecord> {
        let trimmed = label.trim();
        if trimmed.is_empty() {
            return Err(RelayError::Internal("device label cannot be empty".into()));
        }
        let mut tx = self.begin_write().await?;
        let result = sqlx::query("UPDATE trusted_peers SET label = ? WHERE node_id = ?")
            .bind(trimmed)
            .bind(node_id)
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() == 0 {
            return Err(RelayError::PeerNotFound(node_id.to_owned()));
        }
        let updated = fetch_trusted_peer(&mut tx, node_id)
            .await?
            .ok_or_else(|| RelayError::PeerNotFound(node_id.to_owned()))?;
        tx.commit().await?;
        Ok(updated)
    }

    pub async fn revoke_trusted_peer(&self, node_id: &str) -> RelayResult<TrustedPeerRecord> {
        let mut tx = self.begin_write().await?;
        let result = sqlx::query(
            "UPDATE trusted_peers SET revoked_at = COALESCE(revoked_at, ?) WHERE node_id = ?",
        )
        .bind(timestamp(Utc::now()))
        .bind(node_id)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            return Err(RelayError::PeerNotFound(node_id.to_owned()));
        }
        let updated = fetch_trusted_peer(&mut tx, node_id)
            .await?
            .ok_or_else(|| RelayError::PeerNotFound(node_id.to_owned()))?;
        tx.commit().await?;
        Ok(updated)
    }

    pub async fn remove_trusted_peer(&self, node_id: &str) -> RelayResult<bool> {
        let result = sqlx::query("DELETE FROM trusted_peers WHERE node_id = ?")
            .bind(node_id)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() != 0)
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
    node_id: Option<&str>,
) -> RelayResult<i64> {
    let result = sqlx::query(
        "INSERT INTO sync_outbox (repo_id, payload_json, created_at) VALUES (?, ?, ?)",
    )
    .bind(repo_id)
    .bind(Json(payload))
    .bind(timestamp(Utc::now()))
    .execute(&mut *conn)
    .await?;
    let seq = result.last_insert_rowid();
    if let Some(node_id) = node_id {
        sqlx::query("INSERT INTO sync_outbox_recipients (seq, node_id) VALUES (?, ?)")
            .bind(seq)
            .bind(node_id)
            .execute(conn)
            .await?;
    }
    Ok(seq)
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

pub(crate) async fn write_task_log_ref(
    conn: &mut SqliteConnection,
    repo_id: &str,
    task_id: TaskId,
    blob_hash: &str,
) -> RelayResult<()> {
    sqlx::query(
        "INSERT INTO task_log_refs (task_id, repo_id, log_blob_hash, updated_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT (task_id) DO UPDATE SET
             log_blob_hash = excluded.log_blob_hash,
             updated_at = excluded.updated_at
         WHERE task_log_refs.repo_id = excluded.repo_id",
    )
    .bind(task_id.to_string())
    .bind(repo_id)
    .bind(blob_hash)
    .bind(timestamp(Utc::now()))
    .execute(conn)
    .await?;
    Ok(())
}

pub(crate) async fn fetch_task_log_ref(
    conn: &mut SqliteConnection,
    repo_id: &str,
    task_id: TaskId,
) -> RelayResult<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT log_blob_hash FROM task_log_refs WHERE task_id = ? AND repo_id = ?",
    )
    .bind(task_id.to_string())
    .bind(repo_id)
    .fetch_optional(conn)
    .await?)
}

type TrustedPeerRow = (
    String,
    String,
    String,
    String,
    DateTime<Utc>,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
);

const TRUSTED_PEER_SELECT_BY_ID: &str =
    "SELECT node_id, label, role, ticket_or_addrs, paired_at, last_seen_at, revoked_at
     FROM trusted_peers WHERE node_id = ?";

const TRUSTED_PEER_SELECT_ALL: &str =
    "SELECT node_id, label, role, ticket_or_addrs, paired_at, last_seen_at, revoked_at
     FROM trusted_peers
     ORDER BY (revoked_at IS NOT NULL) ASC, last_seen_at DESC, node_id ASC";

fn trusted_peer_record(
    (node_id, label, role, ticket_or_addrs, paired_at, last_seen_at, revoked_at): TrustedPeerRow,
) -> TrustedPeerRecord {
    TrustedPeerRecord {
        node_id,
        label,
        role: PeerRole::from_db_str(&role),
        ticket_or_addrs,
        paired_at,
        last_seen_at,
        revoked_at,
    }
}

async fn fetch_trusted_peer(
    conn: &mut SqliteConnection,
    node_id: &str,
) -> RelayResult<Option<TrustedPeerRecord>> {
    let row = sqlx::query_as::<_, TrustedPeerRow>(TRUSTED_PEER_SELECT_BY_ID)
        .bind(node_id)
        .fetch_optional(conn)
        .await?;
    Ok(row.map(trusted_peer_record))
}

fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Nanos, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;

    fn sample_peer(node_id: &str, label: &str, role: PeerRole, seen_offset_secs: i64) -> TrustedPeerRecord {
        let base = Utc::now() + ChronoDuration::seconds(seen_offset_secs);
        TrustedPeerRecord {
            node_id: node_id.into(),
            label: label.into(),
            role,
            ticket_or_addrs: format!(r#"{{"id":"{node_id}","addrs":["127.0.0.1:7608"]}}"#),
            paired_at: base - ChronoDuration::minutes(10),
            last_seen_at: base,
            revoked_at: None,
        }
    }

    #[tokio::test]
    async fn initializes_sqlite_schema_in_memory() {
        let store = RelayStore::connect_in_memory().await.unwrap();
        let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM tasks")
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert_eq!(row.0, 0);
        let peers = store.list_trusted_peers().await.unwrap();
        assert!(peers.is_empty());
    }

    #[tokio::test]
    async fn trusted_peers_crud_rename_revoke_and_remove() {
        let store = RelayStore::connect_in_memory().await.unwrap();
        let peer_a = sample_peer("node-alpha", "Lab Workstation", PeerRole::Host, 0);
        let peer_b = sample_peer("node-beta", "Travel Laptop", PeerRole::Client, 30);

        let inserted_a = store.upsert_trusted_peer(&peer_a).await.unwrap();
        assert_eq!(inserted_a.node_id, "node-alpha");
        assert_eq!(inserted_a.label, "Lab Workstation");
        assert_eq!(inserted_a.role, PeerRole::Host);
        assert!(!inserted_a.is_revoked());
        assert!(!store.is_peer_revoked("node-alpha").await.unwrap());
        assert!(!store.is_peer_revoked("unknown-node").await.unwrap());

        store.upsert_trusted_peer(&peer_b).await.unwrap();

        // Active peers ordered by last_seen_at DESC (peer_b is newer than peer_a)
        let listed = store.list_trusted_peers().await.unwrap();
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0].node_id, "node-beta");
        assert_eq!(listed[1].node_id, "node-alpha");

        // Rename updates label and rejects empty labels
        assert!(store.rename_trusted_peer("node-alpha", "   ").await.is_err());
        assert!(matches!(
            store.rename_trusted_peer("missing", "Name").await,
            Err(RelayError::PeerNotFound(_))
        ));
        let renamed = store
            .rename_trusted_peer("node-alpha", "  GPU Tower 4090  ")
            .await
            .unwrap();
        assert_eq!(renamed.label, "GPU Tower 4090");

        // Subsequent upsert preserves custom label and original paired_at, updates ticket and last_seen_at
        let mut refreshed_a = peer_a.clone();
        refreshed_a.label = "Generic Fallback".into();
        refreshed_a.ticket_or_addrs = r#"{"id":"node-alpha","addrs":["10.0.0.8:9999"]}"#.into();
        refreshed_a.paired_at = Utc::now() + ChronoDuration::hours(1);
        refreshed_a.last_seen_at = Utc::now() + ChronoDuration::seconds(90);
        let stored_a = store.upsert_trusted_peer(&refreshed_a).await.unwrap();
        assert_eq!(stored_a.label, "GPU Tower 4090");
        assert_eq!(stored_a.paired_at, peer_a.paired_at);
        assert_eq!(stored_a.ticket_or_addrs, refreshed_a.ticket_or_addrs);
        assert_eq!(stored_a.last_seen_at, refreshed_a.last_seen_at);

        // Touch updates last_seen_at for active peer
        assert!(store.touch_trusted_peer_seen("node-beta").await.unwrap());

        // Revoke marks revoked_at, moves peer after active peers, and is idempotent
        let revoked_b = store.revoke_trusted_peer("node-beta").await.unwrap();
        assert!(revoked_b.is_revoked());
        assert!(store.is_peer_revoked("node-beta").await.unwrap());
        let revoked_b_again = store.revoke_trusted_peer("node-beta").await.unwrap();
        assert_eq!(revoked_b_again.revoked_at, revoked_b.revoked_at);

        // Upsert and touch MUST NOT silently unrevoke or mutate a revoked peer
        let mut attempt_unrevoke = peer_b.clone();
        attempt_unrevoke.revoked_at = None;
        attempt_unrevoke.label = "Sneaky Label".into();
        let still_revoked = store.upsert_trusted_peer(&attempt_unrevoke).await.unwrap();
        assert_eq!(still_revoked.revoked_at, revoked_b.revoked_at);
        assert_eq!(still_revoked.label, "Travel Laptop");
        assert!(store.is_peer_revoked("node-beta").await.unwrap());
        assert!(!store.touch_trusted_peer_seen("node-beta").await.unwrap());

        // Ordering puts active peers first, revoked peers after
        let listed = store.list_trusted_peers().await.unwrap();
        assert_eq!(listed[0].node_id, "node-alpha");
        assert_eq!(listed[1].node_id, "node-beta");

        // Remove hard deletes the record
        assert!(store.remove_trusted_peer("node-beta").await.unwrap());
        assert!(!store.remove_trusted_peer("node-beta").await.unwrap());
        assert!(store.get_trusted_peer("node-beta").await.unwrap().is_none());
        assert!(!store.is_peer_revoked("node-beta").await.unwrap());
    }

    #[tokio::test]
    async fn trusted_peers_persist_across_sqlite_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("relay.db");

        let active = sample_peer("node-host-1", "Office Workstation", PeerRole::Host, 10);
        let revoked = sample_peer("node-client-2", "Old Laptop", PeerRole::Client, 5);

        {
            let store = RelayStore::connect(&db_path).await.unwrap();
            store.upsert_trusted_peer(&active).await.unwrap();
            store.upsert_trusted_peer(&revoked).await.unwrap();
            store.revoke_trusted_peer(&revoked.node_id).await.unwrap();
        }

        let reopened = RelayStore::connect(&db_path).await.unwrap();
        let peers = reopened.list_trusted_peers().await.unwrap();
        assert_eq!(peers.len(), 2);
        assert_eq!(peers[0].node_id, active.node_id);
        assert_eq!(peers[0].label, "Office Workstation");
        assert_eq!(peers[0].role, PeerRole::Host);
        assert!(!peers[0].is_revoked());

        assert_eq!(peers[1].node_id, revoked.node_id);
        assert_eq!(peers[1].label, "Old Laptop");
        assert_eq!(peers[1].role, PeerRole::Client);
        assert!(peers[1].is_revoked());
        assert!(reopened.is_peer_revoked(&revoked.node_id).await.unwrap());
    }
}
