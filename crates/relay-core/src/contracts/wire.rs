use crate::contracts::{
    ApprovalDecision, DagSnapshot, GraphEdge, GraphNode, PeerStatus, TaskId, TaskRecord, TaskSpec,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use specta::Type;

/// Custom ALPN registered alongside `iroh_blobs::ALPN` on a single `iroh::Endpoint`.
pub const RELAY_RPC_ALPN: &[u8] = b"relay/rpc/1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct GitSyncBundleMeta {
    pub repo_id: String,
    pub base_commit_sha: String,
    pub branch_name: String,
    pub git_bundle_blob_hash: String,
    pub working_diff_blob_hash: Option<String>,
    pub env_fingerprint: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Type)]
pub struct DagDelta {
    pub repo_id: String,
    pub upsert_nodes: Vec<GraphNode>,
    pub upsert_edges: Vec<GraphEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireRequest {
    Ping {
        sender_device: String,
        sent_at: DateTime<Utc>,
    },
    PushSyncBundle {
        meta: GitSyncBundleMeta,
        dag_delta: DagDelta,
    },
    SyncDagDelta {
        delta: DagDelta,
    },
    FetchDagSnapshot {
        repo_id: String,
    },
    SubmitTask {
        spec: TaskSpec,
    },
    ResolveApproval {
        task_id: TaskId,
        patch_id: Option<String>,
        decision: ApprovalDecision,
        comment: Option<String>,
    },
    FetchTaskState {
        task_id: TaskId,
    },
    CancelTask {
        task_id: TaskId,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WireResponse {
    Pong {
        status: PeerStatus,
        responded_at: DateTime<Utc>,
    },
    Ack {
        message: String,
    },
    DagSnapshot(DagSnapshot),
    TaskState(TaskRecord),
    Error {
        code: String,
        message: String,
    },
}
