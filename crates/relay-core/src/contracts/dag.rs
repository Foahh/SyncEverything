use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::BTreeMap;

pub type NodeId = String;

/// Specta-safe JSON value using `f64` numbers (avoids Specta BigInt export error on `serde_json::Value`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Type)]
#[serde(untagged)]
pub enum JsonValue {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl From<serde_json::Value> for JsonValue {
    fn from(value: serde_json::Value) -> Self {
        match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(b) => Self::Bool(b),
            serde_json::Value::Number(n) => Self::Number(n.as_f64().unwrap_or(0.0)),
            serde_json::Value::String(s) => Self::String(s),
            serde_json::Value::Array(arr) => Self::Array(arr.into_iter().map(Into::into).collect()),
            serde_json::Value::Object(map) => {
                Self::Object(map.into_iter().map(|(k, v)| (k, v.into())).collect())
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NodeKind {
    Decision {
        rationale: String,
        constraints: Vec<String>,
    },
    Artifact {
        git_commit_sha: String,
        env_fingerprint: String,
        working_tree_dirty: bool,
        changed_files: Vec<String>,
    },
    Evidence {
        task_id: String,
        preset: String,
        metrics_json: JsonValue,
        log_blob_hash: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Type)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Informs,
    ProducedFrom,
    Supersedes,
    DerivedFrom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Freshness {
    Fresh,
    Stale {
        caused_by: NodeId,
        reason: String,
        marked_at: DateTime<Utc>,
    },
    PendingVerify,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Type)]
pub struct GraphNode {
    pub id: NodeId,
    pub label: String,
    pub payload: NodeKind,
    pub freshness: Freshness,
    pub device_origin: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct GraphEdge {
    pub id: String,
    pub from: NodeId,
    pub to: NodeId,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Type)]
pub struct DagSnapshot {
    pub repo_id: String,
    pub head_commit_sha: Option<String>,
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub stale_count: u32,
    pub fresh_evidence_count: u32,
    pub stale_evidence_count: u32,
    pub metric_authority_note: Option<String>,
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct StalePropagationReport {
    pub trigger_node_id: NodeId,
    pub invalidated_evidence_ids: Vec<NodeId>,
    pub reason: String,
}
