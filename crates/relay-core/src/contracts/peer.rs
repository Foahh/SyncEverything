use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::fmt;

/// Role of a persisted trusted peer device.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Type)]
#[serde(rename_all = "snake_case")]
pub enum PeerRole {
    Host,
    Client,
    Peer,
}

impl PeerRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Client => "client",
            Self::Peer => "peer",
        }
    }

    pub fn from_db_str(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "host" => Self::Host,
            "client" => Self::Client,
            _ => Self::Peer,
        }
    }
}

impl fmt::Display for PeerRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Persistent trusted peer entry stored in SQLite and exported to desktop UI via Specta.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct TrustedPeerRecord {
    pub node_id: String,
    pub label: String,
    pub role: PeerRole,
    pub ticket_or_addrs: String,
    pub paired_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl TrustedPeerRecord {
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }
}
