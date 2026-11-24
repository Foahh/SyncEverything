mod peer;
mod rpc;

pub use rpc::RelayRpcHandler;

use crate::contracts::{
    ApprovalDecision, DagDelta, DagSnapshot, GitSyncBundleMeta, PeerStatus, RELAY_RPC_ALPN,
    TaskId, TaskRecord, TaskSpec, WireRequest, WireResponse,
};
use crate::engine::RelayEngine;
use crate::error::{RelayError, RelayResult};
use crate::sandbox::{WorktreeSandbox, compute_env_fingerprint};
use chrono::Utc;
use iroh::endpoint::{Connection, presets};
use iroh::protocol::Router;
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayMode};
use iroh_blobs::store::fs::FsStore;
use iroh_blobs::{BlobsProtocol, Hash};
use peer::PeerState;
use rpc::{encode_frame, read_frame, write_frame};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tracing::info;
use uuid::Uuid;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const RPC_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NetworkProfile {
    #[default]
    StandardN0,
    LocalOnly,
}

#[derive(Debug, Clone, Default)]
pub struct RelayNodeConfig {
    pub profile: NetworkProfile,
    pub sandbox: Option<WorktreeSandbox>,
}

/// A mutation committed by an incoming Relay RPC.
#[derive(Debug)]
pub enum RelayNodeEvent {
    TaskSubmitted(TaskId),
    ApprovalResolved(TaskId, Option<String>, ApprovalDecision),
    DagChanged,
    PeerPaired,
}

/// One iroh endpoint serving the Relay RPC and BLAKE3 blob protocols.
pub struct RelayNode {
    shared: Arc<NodeState>,
    router: Router,
}

#[derive(Debug)]
struct NodeState {
    endpoint: Endpoint,
    store: FsStore,
    engine: Arc<RelayEngine>,
    blob_dir: PathBuf,
    profile: NetworkProfile,
    peer: Mutex<PeerState>,
    events: Option<mpsc::UnboundedSender<RelayNodeEvent>>,
}

impl RelayNode {
    /// Start a node for the default repository with an in-memory engine.
    pub async fn start(blob_dir: &Path) -> RelayResult<Self> {
        let engine = Arc::new(RelayEngine::in_memory("default-repo", None).await?);
        Self::start_with_config(blob_dir, engine, RelayNodeConfig::default()).await
    }

    pub async fn start_with_engine(
        blob_dir: &Path,
        engine: Arc<RelayEngine>,
        sandbox: Option<WorktreeSandbox>,
        profile: NetworkProfile,
    ) -> RelayResult<Self> {
        Self::start_with_config(blob_dir, engine, RelayNodeConfig { profile, sandbox }).await
    }

    pub async fn start_with_config(
        blob_dir: &Path,
        engine: Arc<RelayEngine>,
        config: RelayNodeConfig,
    ) -> RelayResult<Self> {
        Self::start_inner(blob_dir, engine, config, None).await
    }

    /// Start an interactive node. Incoming mutations require a paired peer.
    pub async fn start_with_events(
        blob_dir: &Path,
        engine: Arc<RelayEngine>,
        config: RelayNodeConfig,
        events: mpsc::UnboundedSender<RelayNodeEvent>,
    ) -> RelayResult<Self> {
        Self::start_inner(blob_dir, engine, config, Some(events)).await
    }

    async fn start_inner(
        blob_dir: &Path,
        engine: Arc<RelayEngine>,
        config: RelayNodeConfig,
        events: Option<mpsc::UnboundedSender<RelayNodeEvent>>,
    ) -> RelayResult<Self> {
        let blob_dir = std::path::absolute(blob_dir)?;
        tokio::fs::create_dir_all(&blob_dir).await?;
        let endpoint = match config.profile {
            NetworkProfile::StandardN0 => Endpoint::bind(presets::N0).await,
            NetworkProfile::LocalOnly => {
                Endpoint::builder(presets::Minimal)
                    .clear_ip_transports()
                    .bind_addr((Ipv4Addr::LOCALHOST, 0))
                    .map_err(transport_error)?
                    .relay_mode(RelayMode::Disabled)
                    .bind()
                    .await
            }
        }
        .map_err(transport_error)?;
        let store = FsStore::load(&blob_dir).await.map_err(transport_error)?;
        let blobs = BlobsProtocol::new(&store, None);
        let shared = Arc::new(NodeState {
            endpoint: endpoint.clone(),
            store,
            engine,
            blob_dir,
            profile: config.profile,
            peer: Mutex::default(),
            events,
        });
        let router = Router::builder(endpoint)
            .accept(iroh_blobs::ALPN, blobs)
            .accept(RELAY_RPC_ALPN, RelayRpcHandler::new(shared.clone(), config.sandbox))
            .spawn();

        info!(node_id = %shared.endpoint.id(), profile = ?config.profile, "Relay node started");
        Ok(Self { shared, router })
    }

    pub fn id(&self) -> EndpointId {
        self.endpoint().id()
    }

    pub fn addr(&self) -> EndpointAddr {
        if self.shared.profile == NetworkProfile::LocalOnly {
            // An explicitly bound loopback socket is usable before address discovery runs.
            return self.endpoint().bound_sockets().into_iter().fold(
                EndpointAddr::new(self.id()),
                EndpointAddr::with_ip_addr,
            );
        }
        self.endpoint().addr()
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.shared.endpoint
    }

    pub fn store(&self) -> &FsStore {
        &self.shared.store
    }

    pub fn peer_status(&self) -> PeerStatus {
        self.shared.peer().status(self.id(), &self.shared.engine)
    }

    pub fn set_remote_peer(&self, addr: EndpointAddr) {
        let mut peer = self.shared.peer();
        peer.select(addr.id);
        peer.register(addr);
    }

    /// Allow the next incoming ping to establish a pairing during a user-opened window.
    pub fn open_pairing_window(&self, duration: Duration) {
        self.shared.peer().open_pairing_window(duration);
    }

    /// Supply a return route for the authenticated sender of an offline bundle announcement.
    pub fn register_peer_addr(&self, addr: EndpointAddr) {
        self.shared.peer().register(addr);
    }

    pub async fn shutdown(self) -> RelayResult<()> {
        self.shutdown_shared().await
    }

    pub async fn shutdown_shared(&self) -> RelayResult<()> {
        self.router.shutdown().await.map_err(transport_error)
    }

    pub async fn send_rpc(
        &self,
        peer_addr: EndpointAddr,
        request: WireRequest,
    ) -> RelayResult<WireResponse> {
        let encoded = encode_frame(&request)?;
        let remote_id = peer_addr.id;
        self.set_remote_peer(peer_addr);
        let (addr, cached) = {
            let peer = self.shared.peer();
            (peer.addr(remote_id), peer.outbound(remote_id))
        };
        let conn = match cached {
            Some(conn) => conn,
            None => self.shared.connect(addr, RELAY_RPC_ALPN).await?,
        };
        let started = Instant::now();
        let response = tokio::time::timeout(RPC_TIMEOUT, async {
            let (mut send, mut recv) = conn.open_bi().await.map_err(transport_error)?;
            write_frame(&mut send, &encoded).await?;
            read_frame(&mut recv).await
        })
        .await
        .unwrap_or_else(|_| Err(RelayError::Transport("Relay RPC timed out".into())));

        let mut peer = self.shared.peer();
        match response {
            Ok(response) => {
                let commit = match (&request, &response) {
                    (
                        WireRequest::SyncDagDelta { .. } | WireRequest::FetchDagSnapshot { .. },
                        WireResponse::DagSnapshot(snapshot),
                    ) => {
                        snapshot.head_commit_sha.as_deref()
                    }
                    (WireRequest::PushSyncBundle { meta, .. }, WireResponse::Ack { .. }) => {
                        Some(meta.base_commit_sha.as_str())
                    }
                    _ => None,
                };
                peer.record_outbound(conn, started.elapsed(), commit);
                Ok(response)
            }
            Err(err) => {
                peer.forget_outbound(&conn);
                Err(err)
            }
        }
    }

    /// Return the remote node's status from its Pong.
    pub async fn ping_peer(
        &self,
        peer_addr: EndpointAddr,
        sender_device: &str,
    ) -> RelayResult<PeerStatus> {
        let request = WireRequest::Ping {
            sender_device: sender_device.to_owned(),
            sent_at: Utc::now(),
        };
        match self.send_rpc(peer_addr, request).await? {
            WireResponse::Pong { status, .. } => Ok(status),
            response => Err(unexpected_response(response, "pong")),
        }
    }

    pub async fn sync_dag_delta(
        &self,
        peer_addr: EndpointAddr,
        delta: DagDelta,
    ) -> RelayResult<DagSnapshot> {
        let request = WireRequest::SyncDagDelta { delta };
        match self.send_rpc(peer_addr, request).await? {
            WireResponse::DagSnapshot(snapshot) => Ok(snapshot),
            response => Err(unexpected_response(response, "dag_snapshot")),
        }
    }

    /// Fetch and pin a blob by its BLAKE3 hash.
    pub async fn pull_blob_from(&self, peer_addr: EndpointAddr, hash: Hash) -> RelayResult<()> {
        self.shared.pull_blob_from(peer_addr, hash).await
    }

    pub(crate) async fn publish_evidence_log(&self, text: &str) -> RelayResult<()> {
        self.store().blobs().add_slice(text.as_bytes()).with_tag().await.map_err(transport_error)?;
        Ok(())
    }

    pub(crate) async fn fetch_evidence_log(&self, hash_text: &str) -> RelayResult<String> {
        let hash: Hash = hash_text.parse().map_err(transport_error)?;
        let addr = self.shared.peer().selected_addr().ok_or_else(|| {
            RelayError::Transport("no paired peer is available for the evidence log".into())
        })?;
        tokio::time::timeout(RPC_TIMEOUT, self.pull_blob_from(addr, hash))
            .await
            .map_err(|_| RelayError::Transport("evidence log transfer timed out".into()))??;
        let bytes = self.store().blobs().get_bytes(hash).await.map_err(transport_error)?;
        String::from_utf8(bytes.into()).map_err(transport_error)
    }

    /// Publish the Git bundle and optional working diff before announcing their hashes.
    pub async fn push_sync_bundle(
        &self,
        peer_addr: EndpointAddr,
        repo_id: &str,
        sandbox: &WorktreeSandbox,
        dag_delta: DagDelta,
    ) -> RelayResult<GitSyncBundleMeta> {
        self.shared.engine.check_repo(repo_id)?;
        self.shared.engine.check_repo(&dag_delta.repo_id)?;
        let bundle = TempBundle::new(&self.shared.blob_dir, "outgoing").await?;
        sandbox.create_git_bundle(&bundle.path).await?;
        let base_commit_sha = sandbox.git_bundle_head(&bundle.path).await?;
        let branch_name = sandbox.branch_name().await?;
        let working_diff = sandbox.create_working_diff_patch().await?;
        let env_fingerprint = compute_env_fingerprint(sandbox.repo_root())?;
        if sandbox.head_commit_sha().await? != base_commit_sha {
            return Err(RelayError::Git(
                "HEAD changed while preparing the sync bundle".into(),
            ));
        }

        let tag = self
            .store()
            .blobs()
            .add_path(&bundle.path)
            .with_tag()
            .await
            .map_err(transport_error)?;
        let working_diff_blob_hash = if let Some(diff) = working_diff {
            let tag = self
                .store()
                .blobs()
                .add_slice(diff.as_bytes())
                .with_tag()
                .await
                .map_err(transport_error)?;
            Some(tag.hash.to_string())
        } else {
            None
        };
        let meta = GitSyncBundleMeta {
            repo_id: repo_id.to_owned(),
            base_commit_sha,
            branch_name,
            git_bundle_blob_hash: tag.hash.to_string(),
            working_diff_blob_hash,
            env_fingerprint,
            created_at: Utc::now(),
        };
        let request = WireRequest::PushSyncBundle {
            meta: meta.clone(),
            dag_delta,
        };
        match self.send_rpc(peer_addr, request).await? {
            WireResponse::Ack { .. } => Ok(meta),
            response => Err(unexpected_response(response, "ack")),
        }
    }

    pub async fn submit_remote_task(
        &self,
        peer_addr: EndpointAddr,
        spec: TaskSpec,
    ) -> RelayResult<TaskRecord> {
        self.send_task_rpc(peer_addr, WireRequest::SubmitTask { spec })
            .await
    }

    pub async fn resolve_remote_approval(
        &self,
        peer_addr: EndpointAddr,
        task_id: TaskId,
        patch_id: Option<String>,
        decision: ApprovalDecision,
    ) -> RelayResult<TaskRecord> {
        let request = WireRequest::ResolveApproval {
            task_id,
            patch_id,
            decision,
            comment: None,
        };
        self.send_task_rpc(peer_addr, request).await
    }

    async fn send_task_rpc(
        &self,
        peer_addr: EndpointAddr,
        request: WireRequest,
    ) -> RelayResult<TaskRecord> {
        match self.send_rpc(peer_addr, request).await? {
            WireResponse::TaskState(record) => Ok(record),
            response => Err(unexpected_response(response, "task_state")),
        }
    }

    /// Deliver queued requests in order, stopping at the first failure.
    pub async fn flush_outbox(&self, peer_addr: EndpointAddr) -> RelayResult<u32> {
        let engine = &self.shared.engine;
        let _guard = engine.lock_outbox_flush().await;
        let mut delivered = 0;
        for (seq, _, payload) in engine.list_pending_outbox().await? {
            let request = serde_json::from_value(payload)?;
            let expected = reply_kind(&request);
            let response = self.send_rpc(peer_addr.clone(), request).await?;
            if response_kind(&response) != expected {
                return Err(unexpected_response(response, expected));
            }
            engine.mark_outbox_delivered(seq).await?;
            delivered += 1;
        }
        Ok(delivered)
    }
}

impl NodeState {
    fn peer(&self) -> MutexGuard<'_, PeerState> {
        self.peer.lock().expect("peer state lock poisoned")
    }

    async fn connect(&self, addr: EndpointAddr, alpn: &[u8]) -> RelayResult<Connection> {
        let addr = if self.profile == NetworkProfile::LocalOnly {
            let mut local = EndpointAddr::new(addr.id);
            for ip in addr.ip_addrs().filter(|ip| ip.ip().is_loopback()) {
                local = local.with_ip_addr(*ip);
            }
            if local.is_empty() {
                return Err(RelayError::Transport(format!(
                    "no registered loopback address for {}",
                    addr.id
                )));
            }
            local
        } else {
            addr
        };
        tokio::time::timeout(CONNECT_TIMEOUT, self.endpoint.connect(addr, alpn))
            .await
            .map_err(|_| RelayError::Transport("peer connection timed out".into()))?
            .map_err(transport_error)
    }

    async fn pull_blob_from(&self, addr: EndpointAddr, hash: Hash) -> RelayResult<()> {
        if !self.store.blobs().has(hash).await.map_err(transport_error)? {
            let conn = self.connect(addr, iroh_blobs::ALPN).await?;
            self.store
                .remote()
                .fetch(conn, hash)
                .await
                .map_err(transport_error)?;
            self.store.tags().create(hash).await.map_err(transport_error)?;
        }
        Ok(())
    }
}

struct TempBundle {
    path: PathBuf,
}

impl TempBundle {
    async fn new(blob_dir: &Path, subdir: &str) -> RelayResult<Self> {
        let dir = blob_dir.join(subdir);
        tokio::fs::create_dir_all(&dir).await?;
        Ok(Self {
            path: dir.join(format!("{}.bundle", Uuid::now_v7())),
        })
    }
}

impl Drop for TempBundle {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn reply_kind(request: &WireRequest) -> &'static str {
    match request {
        WireRequest::Ping { .. } => "pong",
        WireRequest::PushSyncBundle { .. } => "ack",
        WireRequest::SyncDagDelta { .. } | WireRequest::FetchDagSnapshot { .. } => "dag_snapshot",
        WireRequest::SubmitTask { .. }
        | WireRequest::ResolveApproval { .. }
        | WireRequest::FetchTaskState { .. } => "task_state",
    }
}

fn response_kind(response: &WireResponse) -> &'static str {
    match response {
        WireResponse::Pong { .. } => "pong",
        WireResponse::Ack { .. } => "ack",
        WireResponse::DagSnapshot(_) => "dag_snapshot",
        WireResponse::TaskState(_) => "task_state",
        WireResponse::Error { .. } => "error",
    }
}

fn unexpected_response(response: WireResponse, expected: &str) -> RelayError {
    match response {
        WireResponse::Error { code, message } => {
            RelayError::Transport(format!("remote {code}: {message}"))
        }
        _ => RelayError::Transport(format!(
            "expected {expected} reply, received {}",
            response_kind(&response)
        )),
    }
}

/// Parse either a raw `EndpointAddr` JSON ticket or a daemon JSON object containing `addr` or `pairing_ticket`.
pub fn parse_pairing_ticket(raw: &str) -> RelayResult<EndpointAddr> {
    let trimmed = raw.trim();
    if let Ok(addr) = serde_json::from_str::<EndpointAddr>(trimmed) {
        return Ok(addr);
    }
    let value: serde_json::Value = serde_json::from_str(trimmed)?;
    if let Some(addr_val) = value.get("addr") {
        return Ok(serde_json::from_value(addr_val.clone())?);
    }
    if let Some(ticket_val) = value.get("pairing_ticket") {
        if let Some(nested) = ticket_val.as_str() {
            return Ok(serde_json::from_str(nested)?);
        }
        return Ok(serde_json::from_value(ticket_val.clone())?);
    }
    Err(RelayError::Transport(
        "pairing ticket must be a valid iroh EndpointAddr JSON or daemon output containing 'addr'"
            .into(),
    ))
}

fn transport_error(error: impl std::fmt::Display) -> RelayError {
    RelayError::Transport(error.to_string())
}

#[cfg(test)]
mod tests;
