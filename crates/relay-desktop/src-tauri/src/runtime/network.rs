use iroh::EndpointAddr;
use relay_core::contracts::{DagSnapshot, RuntimeMode, WireRequest, WireResponse};
use relay_core::engine::RelayEngine;
use relay_core::error::{RelayError, RelayResult};
use relay_core::sandbox::WorktreeSandbox;
use relay_core::sync::{
    AutoReconnectConfig, AutoReconnectHandle, NetworkProfile, ReconnectedPeer, RelayNode,
    RelayNodeConfig, RelayNodeEvent, spawn_auto_reconnect,
};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time;

const RELAY_READY_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) struct Network {
    pub(super) mode: RuntimeMode,
    pub(super) local: Arc<RelayNode>,
    pub(super) loopback_host: Option<Arc<RelayNode>>,
    pub(super) remote: Option<EndpointAddr>,
    pub(super) last_pushed_commit: Option<String>,
    pub(super) auto_reconnect: Option<AutoReconnectHandle>,
    auto_reconnect_gen: u64,
}

pub(super) type NodeEvents = mpsc::UnboundedReceiver<RelayNodeEvent>;
pub(super) type ReconnectEvents = mpsc::UnboundedReceiver<ReconnectedPeer>;

impl Network {
    pub(super) async fn start(
        mode: RuntimeMode,
        data_dir: &Path,
        engine: &Arc<RelayEngine>,
        sandbox: Option<WorktreeSandbox>,
    ) -> RelayResult<(Self, Vec<NodeEvents>)> {
        if mode == RuntimeMode::LocalDualSandbox {
            let profile = NetworkProfile::LocalOnly;
            let (host, host_events) =
                start_node(data_dir, "dual-host", engine, sandbox, profile).await?;
            let (local, client_events) =
                match start_node(data_dir, "dual-client", engine, None, profile).await {
                    Ok(client) => client,
                    Err(error) => {
                        shutdown_node(&host).await;
                        return Err(error);
                    }
                };
            host.set_remote_peer(local.addr());
            let host_addr = host.addr();
            local.set_remote_peer(host_addr.clone());
            if let Err(error) = local.ping_peer(host_addr, "desktop-loopback").await {
                tokio::join!(shutdown_node(&host), shutdown_node(&local));
                return Err(error);
            }
            let network = Self {
                mode,
                local,
                loopback_host: Some(host),
                remote: None,
                last_pushed_commit: None,
                auto_reconnect: None,
                auto_reconnect_gen: 0,
            };
            network.sync_loopback_head(engine.repo_id()).await;
            return Ok((network, vec![host_events, client_events]));
        }

        let (store_dir, sandbox) = match mode {
            RuntimeMode::Host => ("host", sandbox),
            _ => ("client", None),
        };
        let standard = start_node(
            data_dir,
            store_dir,
            engine,
            sandbox.clone(),
            NetworkProfile::StandardN0,
        )
        .await;
        let (local, events) = match standard {
            Ok((node, events)) => {
                if time::timeout(RELAY_READY_TIMEOUT, node.endpoint().online())
                    .await
                    .is_err()
                {
                    tracing::debug!("Relay discovery timed out; keeping the endpoint available on LAN");
                }
                (node, events)
            }
            Err(error) => {
                tracing::debug!(%error, "Standard iroh profile unavailable");
                start_node(
                    data_dir,
                    store_dir,
                    engine,
                    sandbox,
                    NetworkProfile::LanOnly,
                )
                .await?
            }
        };
        if let Err(error) = local.preload_trusted_peer_addrs().await {
            tracing::warn!(%error, "Unable to preload trusted peer addresses");
        }
        Ok((
            Self {
                mode,
                local,
                loopback_host: None,
                remote: None,
                last_pushed_commit: None,
                auto_reconnect: None,
                auto_reconnect_gen: 0,
            },
            vec![events],
        ))
    }

    pub(super) fn start_auto_reconnect(
        &mut self,
        engine: &Arc<RelayEngine>,
    ) -> Option<(u64, ReconnectEvents)> {
        self.start_auto_reconnect_with_config(
            engine,
            AutoReconnectConfig {
                sender_device: self.local.id().to_string(),
                ..Default::default()
            },
        )
    }

    pub(super) fn start_auto_reconnect_with_config(
        &mut self,
        engine: &Arc<RelayEngine>,
        config: AutoReconnectConfig,
    ) -> Option<(u64, ReconnectEvents)> {
        self.cancel_auto_reconnect();
        if self.mode == RuntimeMode::LocalDualSandbox {
            return None;
        }
        let (handle, events) = spawn_auto_reconnect(self.local.clone(), engine.clone(), config);
        let generation = self.auto_reconnect_gen;
        self.auto_reconnect = Some(handle);
        Some((generation, events))
    }

    pub(super) fn cancel_auto_reconnect(&mut self) {
        self.auto_reconnect_gen = self.auto_reconnect_gen.wrapping_add(1);
        if let Some(handle) = self.auto_reconnect.take() {
            handle.cancel();
        }
    }

    pub(super) fn should_restart_auto_reconnect(&self) -> bool {
        self.mode != RuntimeMode::LocalDualSandbox
            && self.remote.is_none()
            && !self.local.peer_status().connected
    }

    pub(super) fn can_accept_reconnect(&self, node: &Arc<RelayNode>, generation: u64) -> bool {
        Arc::ptr_eq(&self.local, node)
            && self.auto_reconnect.is_some()
            && self.auto_reconnect_gen == generation
    }

    pub(super) async fn manual_ping_peer(
        &mut self,
        engine: &Arc<RelayEngine>,
        address: EndpointAddr,
        reconnect_config: Option<AutoReconnectConfig>,
    ) -> Result<(), (RelayError, Option<(u64, ReconnectEvents)>)> {
        self.cancel_auto_reconnect();
        let local_id = self.local.id().to_string();
        if let Err(error) = self.local.ping_peer(address, &local_id).await {
            if let Some(previous) = &self.remote {
                self.local.set_remote_peer(previous.clone());
            }
            let restarted = if self.should_restart_auto_reconnect() {
                match reconnect_config {
                    Some(config) => self.start_auto_reconnect_with_config(engine, config),
                    None => self.start_auto_reconnect(engine),
                }
            } else {
                None
            };
            return Err((error, restarted));
        }
        Ok(())
    }

    pub(super) fn executor_node(&self) -> &Arc<RelayNode> {
        self.loopback_host.as_ref().unwrap_or(&self.local)
    }

    pub(super) async fn sync_loopback_head(&self, repo_id: &str) {
        if let Some(host) = &self.loopback_host
            && let Err(error) = fetch_dag(&self.local, host.addr(), repo_id).await
        {
            tracing::warn!(%error, "Unable to read the loopback Host DAG");
        }
    }

    pub(super) async fn shutdown(mut self) {
        self.cancel_auto_reconnect();
        if let Some(host) = self.loopback_host {
            tokio::join!(shutdown_node(&self.local), shutdown_node(&host));
        } else {
            shutdown_node(&self.local).await;
        }
    }
}

async fn start_node(
    data_dir: &Path,
    data_name: &str,
    engine: &Arc<RelayEngine>,
    sandbox: Option<WorktreeSandbox>,
    profile: NetworkProfile,
) -> RelayResult<(Arc<RelayNode>, NodeEvents)> {
    let (sender, receiver) = mpsc::unbounded_channel();
    let config = RelayNodeConfig { profile, sandbox };
    let node =
        RelayNode::start_with_events(&data_dir.join(data_name), engine.clone(), config, sender)
            .await?;
    Ok((Arc::new(node), receiver))
}

async fn shutdown_node(node: &RelayNode) {
    if let Err(error) = node.shutdown_shared().await {
        tracing::warn!(node_id = %node.id(), %error, "Unable to shut down Relay node");
    }
}

pub(super) async fn fetch_dag(
    node: &RelayNode,
    address: EndpointAddr,
    repo_id: &str,
) -> RelayResult<DagSnapshot> {
    let request = WireRequest::FetchDagSnapshot {
        repo_id: repo_id.to_owned(),
    };
    match node.send_rpc(address, request).await? {
        WireResponse::DagSnapshot(snapshot) => Ok(snapshot),
        response => Err(rpc_error(response, "DAG snapshot")),
    }
}

pub(super) fn rpc_error(response: WireResponse, expected: &str) -> RelayError {
    let message = match response {
        WireResponse::Error { code, message } => format!("remote {code}: {message}"),
        _ => format!("expected {expected} reply"),
    };
    RelayError::Transport(message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use relay_core::contracts::{PeerRole, TrustedPeerRecord};

    fn fast_reconnect_config(sender_device: String) -> AutoReconnectConfig {
        AutoReconnectConfig {
            initial_backoff: Duration::from_millis(40),
            max_backoff: Duration::from_millis(100),
            attempt_timeout: Duration::from_millis(200),
            sender_device,
            max_attempts: None,
        }
    }

    async fn local_client_network(
        dir: &Path,
        name: &str,
    ) -> (Network, Arc<RelayEngine>, NodeEvents) {
        let engine = Arc::new(RelayEngine::in_memory("desktop-reconnect-test", None).await.unwrap());
        engine.set_runtime_mode(RuntimeMode::Client).await.unwrap();
        let (local, events) = start_node(dir, name, &engine, None, NetworkProfile::LocalOnly)
            .await
            .unwrap();
        let network = Network {
            mode: RuntimeMode::Client,
            local,
            loopback_host: None,
            remote: None,
            last_pushed_commit: None,
            auto_reconnect: None,
            auto_reconnect_gen: 0,
        };
        (network, engine, events)
    }

    #[tokio::test]
    async fn failed_manual_pair_restarts_auto_reconnect_without_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let (mut network, client_engine, _client_events) =
            local_client_network(dir.path(), "client").await;

        let now = Utc::now();
        let offline_id = iroh::SecretKey::generate().public();
        let offline_addr = EndpointAddr::new(offline_id)
            .with_ip_addr("127.0.0.1:1".parse().unwrap());
        client_engine
            .upsert_trusted_peer(&TrustedPeerRecord {
                node_id: offline_id.to_string(),
                label: "Offline Host".into(),
                role: PeerRole::Host,
                ticket_or_addrs: serde_json::to_string(&offline_addr).unwrap(),
                paired_at: now,
                last_seen_at: now,
                revoked_at: None,
            })
            .await
            .unwrap();

        let cfg = fast_reconnect_config(network.local.id().to_string());
        let (gen_1, mut rx_1) = network
            .start_auto_reconnect_with_config(&client_engine, cfg.clone())
            .expect("initial auto-reconnect started");
        assert!(network.can_accept_reconnect(&network.local, gen_1));

        // Failed manual pair against a mistyped/unreachable ticket cancels gen_1 and restarts gen_2.
        let bad_target = EndpointAddr::new(iroh::SecretKey::generate().public());
        let (_, restarted) = network
            .manual_ping_peer(&client_engine, bad_target, Some(cfg))
            .await
            .expect_err("manual ping to unreachable ticket must fail");
        let (gen_2, mut rx_2) = restarted.expect("auto-reconnect must restart when offline");

        assert_ne!(gen_1, gen_2);
        assert!(!network.can_accept_reconnect(&network.local, gen_1));
        assert!(network.can_accept_reconnect(&network.local, gen_2));
        assert!(network.auto_reconnect.is_some());
        // The cancelled gen_1 task must terminate cleanly so no duplicate tasks run concurrently.
        assert!(
            tokio::time::timeout(Duration::from_secs(2), rx_1.recv())
                .await
                .expect("old auto-reconnect channel closed")
                .is_none()
        );

        // Bringing a saved trusted host online allows the restarted auto-reconnect loop to connect.
        let host_engine =
            Arc::new(RelayEngine::in_memory("desktop-reconnect-test", None).await.unwrap());
        host_engine
            .upsert_trusted_peer(&TrustedPeerRecord {
                node_id: network.local.id().to_string(),
                label: "Laptop".into(),
                role: PeerRole::Client,
                ticket_or_addrs: serde_json::to_string(&network.local.addr()).unwrap(),
                paired_at: now,
                last_seen_at: now,
                revoked_at: None,
            })
            .await
            .unwrap();
        let (host, _host_events) =
            start_node(dir.path(), "host", &host_engine, None, NetworkProfile::LocalOnly)
                .await
                .unwrap();
        let later = Utc::now();
        client_engine
            .upsert_trusted_peer(&TrustedPeerRecord {
                node_id: host.id().to_string(),
                label: "Online Host".into(),
                role: PeerRole::Host,
                ticket_or_addrs: serde_json::to_string(&host.addr()).unwrap(),
                paired_at: later,
                last_seen_at: later,
                revoked_at: None,
            })
            .await
            .unwrap();

        let reconnected = tokio::time::timeout(Duration::from_secs(5), rx_2.recv())
            .await
            .expect("restarted auto-reconnect timed out")
            .expect("restarted auto-reconnect connected");
        assert_eq!(reconnected.record.node_id, host.id().to_string());
        assert!(network.local.peer_status().connected);

        network.shutdown().await;
        shutdown_node(&host).await;
    }

    #[tokio::test]
    async fn failed_manual_pair_does_not_restart_auto_reconnect_when_connected_or_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let (mut network, client_engine, _client_events) =
            local_client_network(dir.path(), "connected-client").await;
        let host_engine =
            Arc::new(RelayEngine::in_memory("desktop-reconnect-test", None).await.unwrap());
        let (host, _host_events) =
            start_node(dir.path(), "connected-host", &host_engine, None, NetworkProfile::LocalOnly)
                .await
                .unwrap();
        host.open_pairing_window(Duration::from_secs(30));

        // Establish an active manual connection first.
        let host_addr = host.addr();
        network
            .manual_ping_peer(&client_engine, host_addr.clone(), None)
            .await
            .expect("initial manual pair succeeded");
        network.remote = Some(host_addr);
        assert!(network.local.peer_status().connected);
        assert!(!network.should_restart_auto_reconnect());

        // A subsequent failed manual pair preserves the active peer and does not spawn auto-reconnect.
        let bad_target = EndpointAddr::new(iroh::SecretKey::generate().public());
        let cfg = fast_reconnect_config(network.local.id().to_string());
        let (_, restarted) = network
            .manual_ping_peer(&client_engine, bad_target, Some(cfg))
            .await
            .expect_err("manual ping to unreachable peer must fail");
        assert!(restarted.is_none());
        assert!(network.auto_reconnect.is_none());
        assert!(network.local.peer_status().connected);
        assert_eq!(
            network.local.peer_status().remote_node_id.as_deref(),
            Some(host.id().to_string().as_str())
        );

        // LocalDualSandbox mode is never eligible for startup auto-reconnect.
        let sandbox_engine =
            Arc::new(RelayEngine::in_memory("desktop-reconnect-test", None).await.unwrap());
        let (mut sandbox_net, _events) =
            Network::start(RuntimeMode::LocalDualSandbox, dir.path(), &sandbox_engine, None)
                .await
                .unwrap();
        assert!(!sandbox_net.should_restart_auto_reconnect());
        assert!(sandbox_net.start_auto_reconnect(&sandbox_engine).is_none());

        sandbox_net.shutdown().await;
        network.shutdown().await;
        shutdown_node(&host).await;
    }
}
