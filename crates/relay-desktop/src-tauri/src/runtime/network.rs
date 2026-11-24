use iroh::EndpointAddr;
use relay_core::contracts::{DagSnapshot, RuntimeMode, WireRequest, WireResponse};
use relay_core::engine::RelayEngine;
use relay_core::error::{RelayError, RelayResult};
use relay_core::sandbox::WorktreeSandbox;
use relay_core::sync::{NetworkProfile, RelayNode, RelayNodeConfig, RelayNodeEvent};
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
}

pub(super) type NodeEvents = mpsc::UnboundedReceiver<RelayNodeEvent>;

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
        Ok((
            Self {
                mode,
                local,
                loopback_host: None,
                remote: None,
                last_pushed_commit: None,
            },
            vec![events],
        ))
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

    pub(super) async fn shutdown(self) {
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
