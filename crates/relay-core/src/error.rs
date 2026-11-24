use thiserror::Error;

#[derive(Debug, Error)]
pub enum RelayError {
    #[error("node not found: {0}")]
    NodeNotFound(String),

    #[error("task not found: {0}")]
    TaskNotFound(String),

    #[error("proposal not found: {0}")]
    ProposalNotFound(String),

    #[error("log blob {hash} for evidence {node_id} is missing")]
    MissingEvidenceLog { node_id: String, hash: String },

    #[error("invalid command preset '{0}' (not declared in relay.toml)")]
    UnknownPreset(String),

    #[error("invalid state transition for task {task_id}: {from} -> {to}")]
    InvalidStateTransition {
        task_id: String,
        from: String,
        to: String,
    },

    #[error("stale evidence blocked: node {node_id} was invalidated by {caused_by} ({reason})")]
    StaleEvidenceBlocked {
        node_id: String,
        caused_by: String,
        reason: String,
    },

    #[error("git command failed: {0}")]
    Git(String),

    #[error("uv command failed: {0}")]
    Uv(String),

    #[error("transport error: {0}")]
    Transport(String),

    #[error("storage error: {0}")]
    Storage(#[from] sqlx::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("toml parse error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("llm client error: {0}")]
    Llm(String),

    #[error("internal error: {0}")]
    Internal(String),
}

pub type RelayResult<T> = Result<T, RelayError>;
