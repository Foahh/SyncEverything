use super::{
    NodeState, RPC_TIMEOUT, RelayNodeEvent, TempBundle, default_peer_label, transport_error,
};
use crate::contracts::{
    GitSyncBundleMeta, PeerRole, RuntimeMode, TrustedPeerRecord, WireRequest, WireResponse,
};
use crate::error::{RelayError, RelayResult};
use crate::sandbox::WorktreeSandbox;
use chrono::Utc;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh_blobs::{BlobsProtocol, Hash};
use serde::{Serialize, de::DeserializeOwned};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};

const MAX_PAYLOAD: usize = 16 * 1024 * 1024;
const REVOKED_FLUSH_TIMEOUT: Duration = Duration::from_millis(250);

/// Wrapper around `BlobsProtocol` that enforces the host revocation gate before accepting blob transfers.
#[derive(Debug, Clone)]
pub(super) struct RevocationGuardedBlobs {
    blobs: BlobsProtocol,
    node: Arc<NodeState>,
}

impl RevocationGuardedBlobs {
    pub(super) fn new(blobs: BlobsProtocol, node: Arc<NodeState>) -> Self {
        Self { blobs, node }
    }
}

impl ProtocolHandler for RevocationGuardedBlobs {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let _active = self.node.track_connection(&conn);
        let remote_node_id = conn.remote_id().to_string();
        match self.node.engine.is_peer_revoked(&remote_node_id).await {
            Ok(true) => {
                warn!(node_id = %remote_node_id, "Rejected blob connection from revoked peer");
                conn.close(1u32.into(), b"PEER_REVOKED");
                return Ok(());
            }
            Err(err) => {
                warn!(
                    node_id = %remote_node_id,
                    %err,
                    "Unable to verify peer revocation status for blob transfer; rejecting"
                );
                conn.close(1u32.into(), b"PEER_REVOKED");
                return Ok(());
            }
            Ok(false) => {}
        }
        self.blobs.accept(conn).await
    }

    async fn shutdown(&self) {
        self.blobs.shutdown().await;
    }
}

/// QUIC RPC handler mounted alongside the blob protocol.
#[derive(Debug, Clone)]
pub struct RelayRpcHandler {
    node: Arc<NodeState>,
    sandbox: Option<WorktreeSandbox>,
}

impl RelayRpcHandler {
    pub(super) fn new(node: Arc<NodeState>, sandbox: Option<WorktreeSandbox>) -> Self {
        Self { node, sandbox }
    }

    async fn serve_stream(
        &self,
        conn: &Connection,
        mut send: SendStream,
        mut recv: RecvStream,
    ) -> RelayResult<()> {
        let response = match read_frame(&mut recv).await {
            Ok(request) => match self.handle(conn, request).await {
                Ok(response) => response,
                Err(err) => {
                    let code = match &err {
                        RelayError::TaskNotFound(_) | RelayError::ProposalNotFound(_) => {
                            "not_found"
                        }
                        RelayError::InvalidStateTransition { .. } => "invalid_state",
                        _ => "request_failed",
                    };
                    error_response(code, err.to_string())
                }
            },
            Err(RelayError::Serde(err)) => error_response("invalid_request", err.to_string()),
            Err(err) => return Err(err),
        };
        let is_revoked = matches!(
            &response,
            WireResponse::Error { code, .. } if code == "PEER_REVOKED"
        );
        let frame = match encode_frame(&response) {
            Ok(frame) => frame,
            Err(err) => encode_frame(&error_response("invalid_response", err.to_string()))?,
        };
        let write_result = write_frame(&mut send, &frame).await;
        if is_revoked {
            if write_result.is_ok() {
                let _ = tokio::time::timeout(REVOKED_FLUSH_TIMEOUT, send.stopped()).await;
            }
            self.node.disconnect_peer(conn.remote_id());
            conn.close(1u32.into(), b"PEER_REVOKED");
        }
        write_result
    }

    async fn handle(&self, conn: &Connection, request: WireRequest) -> RelayResult<WireResponse> {
        let id = conn.remote_id();
        let remote_node_id = id.to_string();
        let engine = &self.node.engine;

        // Mandatory revocation gate: check remote authenticated NodeId BEFORE dispatching any RPC.
        let known_peer = engine.get_trusted_peer(&remote_node_id).await?;
        if known_peer.as_ref().is_some_and(TrustedPeerRecord::is_revoked) {
            warn!(node_id = %remote_node_id, "Rejected RPC from revoked peer");
            return Ok(error_response(
                "PEER_REVOKED",
                format!("peer {remote_node_id} has been revoked by this host"),
            ));
        }

        let is_trusted_in_db = known_peer.is_some();
        let mut newly_paired = false;
        {
            let mut peer = self.node.peer();
            if self.node.events.is_some() && !peer.is_selected(id) {
                if is_trusted_in_db {
                    newly_paired = peer.selected_addr().is_none();
                } else if matches!(&request, WireRequest::Ping { .. }) && peer.accept_pairing(id) {
                    newly_paired = true;
                } else {
                    return Ok(error_response("unpaired_peer", "peer is not paired".into()));
                }
            }
            peer.record_inbound(conn);
        }

        if !matches!(&request, WireRequest::Ping { .. }) {
            if engine.observed_runtime_mode() != RuntimeMode::LocalDualSandbox && is_trusted_in_db {
                let _ = engine.touch_trusted_peer_seen(&remote_node_id).await;
            }
            if newly_paired {
                self.notify(RelayNodeEvent::PeerPaired);
            }
        }

        match request {
            WireRequest::Ping { sender_device, .. } => {
                debug!(node_id = %id, %sender_device, "Received Relay ping");
                if engine.observed_runtime_mode() != RuntimeMode::LocalDualSandbox {
                    let role = match engine.observed_runtime_mode() {
                        RuntimeMode::Host => PeerRole::Client,
                        RuntimeMode::Client => PeerRole::Host,
                        RuntimeMode::LocalDualSandbox => PeerRole::Peer,
                    };
                    let trimmed_sender = sender_device.trim();
                    let label = if !trimmed_sender.is_empty() && trimmed_sender != remote_node_id {
                        trimmed_sender.to_owned()
                    } else {
                        default_peer_label(&remote_node_id, role)
                    };
                    let addr = self.node.peer().addr(id).with_addrs(
                        conn.paths().iter().map(|path| path.remote_addr().to_owned()),
                    );
                    let now = Utc::now();
                    let record = TrustedPeerRecord {
                        node_id: remote_node_id.clone(),
                        label,
                        role,
                        ticket_or_addrs: serde_json::to_string(&addr)?,
                        paired_at: now,
                        last_seen_at: now,
                        revoked_at: None,
                    };
                    if engine.upsert_trusted_peer(&record).await?.is_revoked() {
                        warn!(node_id = %remote_node_id, "Rejected Ping from revoked peer after handshake");
                        self.node.disconnect_peer_except(id, conn.stable_id());
                        return Ok(error_response("PEER_REVOKED", format!("peer {remote_node_id} has been revoked by this host")));
                    }
                }
                if newly_paired {
                    self.notify(RelayNodeEvent::PeerPaired);
                }
                let status =
                    self.node
                        .peer()
                        .status_for_connection(self.node.endpoint.id(), engine, conn);
                Ok(WireResponse::Pong {
                    status,
                    responded_at: Utc::now(),
                })
            }
            WireRequest::SyncDagDelta { delta } => {
                let snapshot = engine.apply_dag_delta(delta).await?;
                self.node
                    .peer()
                    .record_commit(id, snapshot.head_commit_sha.as_deref());
                self.notify(RelayNodeEvent::DagChanged);
                Ok(WireResponse::DagSnapshot(snapshot))
            }
            WireRequest::FetchDagSnapshot { repo_id } => {
                engine.check_repo(&repo_id)?;
                let snapshot = engine.dag_snapshot(true).await?;
                self.node
                    .peer()
                    .record_commit(id, snapshot.head_commit_sha.as_deref());
                Ok(WireResponse::DagSnapshot(snapshot))
            }
            WireRequest::PushSyncBundle { meta, dag_delta } => {
                engine.check_repo(&meta.repo_id)?;
                engine.check_repo(&dag_delta.repo_id)?;
                let sandbox = self.sandbox.as_ref().ok_or_else(|| {
                    RelayError::Git("no repository is configured to import the sync bundle".into())
                })?;
                self.import_bundle(conn, sandbox, &meta).await?;
                engine.apply_dag_delta(dag_delta).await?;
                self.node.peer().record_commit(id, Some(&meta.base_commit_sha));
                self.notify(RelayNodeEvent::DagChanged);
                Ok(WireResponse::Ack {
                    message: "sync bundle applied".into(),
                })
            }
            WireRequest::SubmitTask { spec } => {
                let (record, inserted) = engine.submit_task(spec).await?;
                if inserted {
                    self.notify(RelayNodeEvent::TaskSubmitted(record.spec.task_id));
                }
                Ok(WireResponse::TaskState(record))
            }
            WireRequest::ResolveApproval {
                task_id,
                patch_id,
                decision,
                ..
            } => {
                let record = if self.node.events.is_some() {
                    let (record, changed) = engine
                        .resolve_peer_approval(task_id, patch_id.as_deref(), decision)
                        .await?;
                    if changed {
                        self.notify(RelayNodeEvent::ApprovalResolved(task_id, patch_id, decision));
                    }
                    record
                } else {
                    engine.resolve_approval(task_id, patch_id.as_deref(), decision).await?
                };
                Ok(WireResponse::TaskState(record))
            }
            WireRequest::FetchTaskState { task_id } => {
                let record = engine
                    .get_task(task_id)
                    .await?
                    .ok_or_else(|| RelayError::TaskNotFound(task_id.to_string()))?;
                Ok(WireResponse::TaskState(record))
            }
            WireRequest::CancelTask { task_id } => {
                let record = engine.cancel_task(task_id).await?;
                self.notify(RelayNodeEvent::TaskCancelled(task_id));
                Ok(WireResponse::TaskState(record))
            }
        }
    }

    fn notify(&self, event: RelayNodeEvent) {
        if let Some(events) = &self.node.events {
            let _ = events.send(event);
        }
    }

    async fn import_bundle(
        &self,
        conn: &Connection,
        sandbox: &WorktreeSandbox,
        meta: &GitSyncBundleMeta,
    ) -> RelayResult<()> {
        let hash = parse_hash(&meta.git_bundle_blob_hash)?;
        // Only the initiating peer needs the pairing ticket. Use its live RPC paths
        // for the separate blob connection, including when discovery is disabled.
        let addr = self.node.peer().addr(conn.remote_id()).with_addrs(
            conn.paths().iter().map(|path| path.remote_addr().to_owned()),
        );
        self.node.pull_blob_from(addr.clone(), hash).await?;
        if let Some(diff_hash) = &meta.working_diff_blob_hash {
            self.node.pull_blob_from(addr, parse_hash(diff_hash)?).await?;
        }

        let bundle = TempBundle::new(&self.node.blob_dir, "incoming").await?;
        self.node
            .store
            .blobs()
            .export(hash, &bundle.path)
            .await
            .map_err(transport_error)?;
        let commit = sandbox.git_bundle_head(&bundle.path).await?;
        if !commit.eq_ignore_ascii_case(&meta.base_commit_sha) {
            return Err(RelayError::Git(format!(
                "bundle HEAD {commit} does not match announced commit {}",
                meta.base_commit_sha
            )));
        }
        let target = format!("sync-{}", &commit[..12]);
        let imported = sandbox.import_git_bundle(&bundle.path, &target).await?;
        if imported != commit {
            return Err(RelayError::Git(format!("imported {imported} instead of {commit}")));
        }
        Ok(())
    }
}

impl ProtocolHandler for RelayRpcHandler {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let _active = self.node.track_connection(&conn);
        while let Ok((send, recv)) = conn.accept_bi().await {
            match tokio::time::timeout(RPC_TIMEOUT, self.serve_stream(&conn, send, recv)).await {
                Ok(Ok(())) => {}
                Ok(Err(err)) => {
                    debug!(node_id = %conn.remote_id(), %err, "Relay RPC stream failed");
                }
                Err(_) => debug!(node_id = %conn.remote_id(), "Relay RPC stream timed out"),
            }
        }
        Ok(())
    }
}

fn error_response(code: &str, message: String) -> WireResponse {
    WireResponse::Error {
        code: code.to_owned(),
        message,
    }
}

fn parse_hash(hash: &str) -> RelayResult<Hash> {
    hash.strip_prefix("iroh:")
        .unwrap_or(hash)
        .parse()
        .map_err(transport_error)
}

pub(super) fn encode_frame(message: &impl Serialize) -> RelayResult<Vec<u8>> {
    let payload = serde_json::to_vec(message)?;
    check_size(payload.len())?;
    Ok(payload)
}

pub(super) async fn write_frame(send: &mut SendStream, payload: &[u8]) -> RelayResult<()> {
    send.write_all(&(payload.len() as u32).to_be_bytes())
        .await
        .map_err(transport_error)?;
    send.write_all(payload).await.map_err(transport_error)?;
    send.finish().map_err(transport_error)
}

pub(super) async fn read_frame<T: DeserializeOwned>(recv: &mut RecvStream) -> RelayResult<T> {
    let mut prefix = [0; 4];
    recv.read_exact(&mut prefix).await.map_err(transport_error)?;
    let size = u32::from_be_bytes(prefix) as usize;
    check_size(size)?;
    let mut payload = vec![0; size];
    recv.read_exact(&mut payload).await.map_err(transport_error)?;
    Ok(serde_json::from_slice(&payload)?)
}

fn check_size(size: usize) -> RelayResult<()> {
    if size > MAX_PAYLOAD {
        return Err(RelayError::Transport("Relay RPC payload exceeds 16 MiB".into()));
    }
    Ok(())
}
