use super::{RelayNode, parse_pairing_ticket};
use crate::contracts::{PeerRole, PeerStatus, RuntimeMode, TrustedPeerRecord};
use crate::engine::RelayEngine;
use crate::error::RelayError;
use iroh::EndpointAddr;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// Configuration for non-blocking background startup auto-reconnect.
#[derive(Debug, Clone)]
pub struct AutoReconnectConfig {
    pub initial_backoff: Duration,
    pub max_backoff: Duration,
    pub attempt_timeout: Duration,
    pub sender_device: String,
    /// Optional cap on retry rounds (`None` retries until connected, cancelled, or no candidates remain).
    pub max_attempts: Option<u32>,
}

impl Default for AutoReconnectConfig {
    fn default() -> Self {
        Self {
            initial_backoff: Duration::from_secs(2),
            max_backoff: Duration::from_secs(60),
            attempt_timeout: Duration::from_secs(5),
            sender_device: "relay-peer".into(),
            max_attempts: None,
        }
    }
}

/// Emitted when startup auto-reconnect establishes a connection to a trusted peer.
#[derive(Debug, Clone)]
pub struct ReconnectedPeer {
    pub record: TrustedPeerRecord,
    pub addr: EndpointAddr,
    pub status: PeerStatus,
}

/// Owns the background auto-reconnect task and cancels it cleanly on `cancel()` or drop.
#[derive(Debug)]
pub struct AutoReconnectHandle {
    cancel_tx: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl AutoReconnectHandle {
    pub fn cancel(&self) {
        let _ = self.cancel_tx.send(true);
        self.task.abort();
    }

    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

impl Drop for AutoReconnectHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// Spawn a non-blocking background loop that reads non-revoked trusted peers from SQLite
/// and retries connecting with exponential backoff until one connects or the handle is cancelled.
pub fn spawn_auto_reconnect(
    node: Arc<RelayNode>,
    engine: Arc<RelayEngine>,
    config: AutoReconnectConfig,
) -> (AutoReconnectHandle, mpsc::UnboundedReceiver<ReconnectedPeer>) {
    let (cancel_tx, mut cancel_rx) = watch::channel(false);
    let (reconnected_tx, reconnected_rx) = mpsc::unbounded_channel();

    let task = tokio::spawn(async move {
        let local_id = node.id().to_string();
        let expected_role = match engine.observed_runtime_mode() {
            RuntimeMode::Client => PeerRole::Host,
            RuntimeMode::Host => PeerRole::Client,
            RuntimeMode::LocalDualSandbox => return,
        };
        let initial = config.initial_backoff.max(Duration::from_millis(10));
        let max_backoff = config.max_backoff.max(initial);
        let mut backoff = initial;
        let mut attempts: u32 = 0;
        let mut remotely_revoked: BTreeSet<String> = BTreeSet::new();

        loop {
            if *cancel_rx.borrow() {
                break;
            }
            if node.peer_status().connected {
                debug!("Auto-reconnect stopping because node is already connected");
                break;
            }

            let peers = match engine.list_trusted_peers().await {
                Ok(peers) => peers,
                Err(error) => {
                    warn!(%error, "Unable to read trusted peers for auto-reconnect");
                    break;
                }
            };

            let candidates: Vec<(TrustedPeerRecord, EndpointAddr)> = peers
                .into_iter()
                .filter(|peer| {
                    !peer.is_revoked()
                        && peer.node_id != local_id
                        && peer.role == expected_role
                        && !remotely_revoked.contains(&peer.node_id)
                })
                .filter_map(|peer| {
                    parse_pairing_ticket(&peer.ticket_or_addrs)
                        .ok()
                        .filter(|addr| addr.id.to_string() == peer.node_id)
                        .map(|addr| (peer, addr))
                })
                .collect();

            if candidates.is_empty() {
                debug!("No eligible non-revoked trusted peers available for auto-reconnect");
                break;
            }

            let mut connected = false;
            for (record, addr) in candidates {
                if *cancel_rx.borrow() || node.peer_status().connected {
                    connected = true;
                    break;
                }
                if engine.is_peer_revoked(&record.node_id).await.unwrap_or(true) {
                    continue;
                }

                let ping_attempt = tokio::select! {
                    _ = cancel_rx.changed() => {
                        if *cancel_rx.borrow() {
                            connected = true;
                            break;
                        }
                        continue;
                    }
                    result = tokio::time::timeout(
                        config.attempt_timeout.max(Duration::from_millis(50)),
                        node.ping_peer(addr.clone(), &config.sender_device),
                    ) => result,
                };

                match ping_attempt {
                    Ok(Ok(remote_status)) => {
                        info!(
                            node_id = %record.node_id,
                            label = %record.label,
                            "Auto-reconnected to trusted peer"
                        );
                        // The runtime must synchronize repository inputs before replaying queued work.
                        let _ = reconnected_tx.send(ReconnectedPeer {
                            record,
                            addr,
                            status: remote_status,
                        });
                        connected = true;
                        break;
                    }
                    Ok(Err(error)) => {
                        if matches!(error, RelayError::PeerRevoked(_)) {
                            warn!(
                                node_id = %record.node_id,
                                "Trusted peer rejected auto-reconnect with PEER_REVOKED; skipping further retries"
                            );
                            remotely_revoked.insert(record.node_id.clone());
                        } else {
                            debug!(
                                node_id = %record.node_id,
                                %error,
                                "Trusted peer unreachable during startup auto-reconnect; staying offline"
                            );
                        }
                    }
                    Err(_) => {
                        debug!(
                            node_id = %record.node_id,
                            "Trusted peer connection attempt timed out during startup auto-reconnect; staying offline"
                        );
                    }
                }
            }

            if connected {
                break;
            }

            attempts = attempts.saturating_add(1);
            if config.max_attempts.is_some_and(|max| attempts >= max) {
                debug!(attempts, "Startup auto-reconnect reached max attempts");
                break;
            }

            tokio::select! {
                _ = cancel_rx.changed() => {
                    if *cancel_rx.borrow() {
                        break;
                    }
                }
                _ = tokio::time::sleep(backoff) => {}
            }
            backoff = backoff.saturating_mul(2).min(max_backoff);
        }
    });

    (AutoReconnectHandle { cancel_tx, task }, reconnected_rx)
}
