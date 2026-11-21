use super::{NodeState, RPC_TIMEOUT, TempBundle, transport_error};
use crate::contracts::{GitSyncBundleMeta, WireRequest, WireResponse};
use crate::error::{RelayError, RelayResult};
use crate::sandbox::WorktreeSandbox;
use chrono::Utc;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh_blobs::Hash;
use serde::{Serialize, de::DeserializeOwned};
use std::sync::Arc;
use tracing::debug;

const MAX_PAYLOAD: usize = 16 * 1024 * 1024;

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
                        _ => "request_failed",
                    };
                    error_response(code, err.to_string())
                }
            },
            Err(RelayError::Serde(err)) => error_response("invalid_request", err.to_string()),
            Err(err) => return Err(err),
        };
        let frame = match encode_frame(&response) {
            Ok(frame) => frame,
            Err(err) => encode_frame(&error_response("invalid_response", err.to_string()))?,
        };
        write_frame(&mut send, &frame).await
    }

    async fn handle(&self, conn: &Connection, request: WireRequest) -> RelayResult<WireResponse> {
        let id = conn.remote_id();
        let engine = &self.node.engine;
        self.node.peer().record_inbound(conn);
        match request {
            WireRequest::Ping { sender_device, .. } => {
                debug!(node_id = %id, %sender_device, "Received Relay ping");
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
                Ok(WireResponse::DagSnapshot(snapshot))
            }
            WireRequest::FetchDagSnapshot { repo_id } => {
                engine.check_repo(&repo_id)?;
                Ok(WireResponse::DagSnapshot(engine.dag_snapshot(true).await))
            }
            WireRequest::PushSyncBundle { meta, dag_delta } => {
                engine.check_repo(&meta.repo_id)?;
                engine.check_repo(&dag_delta.repo_id)?;
                if let Some(sandbox) = &self.sandbox && !meta.git_bundle_blob_hash.is_empty() {
                    self.import_bundle(conn, sandbox, &meta).await?;
                }
                engine.apply_dag_delta(dag_delta).await?;
                self.node.peer().record_commit(id, Some(&meta.base_commit_sha));
                Ok(WireResponse::Ack {
                    message: "sync bundle applied".into(),
                })
            }
            WireRequest::SubmitTask { spec } => {
                let (record, _) = engine.submit_task(spec).await?;
                Ok(WireResponse::TaskState(record))
            }
            WireRequest::ResolveApproval {
                task_id,
                patch_id,
                decision,
                ..
            } => {
                let record = engine
                    .resolve_approval(task_id, patch_id.as_deref(), decision)
                    .await?;
                Ok(WireResponse::TaskState(record))
            }
            WireRequest::FetchTaskState { task_id } => {
                let record = engine
                    .get_task(task_id)
                    .await?
                    .ok_or_else(|| RelayError::TaskNotFound(task_id.to_string()))?;
                Ok(WireResponse::TaskState(record))
            }
        }
    }

    async fn import_bundle(
        &self,
        conn: &Connection,
        sandbox: &WorktreeSandbox,
        meta: &GitSyncBundleMeta,
    ) -> RelayResult<()> {
        let hash = parse_hash(&meta.git_bundle_blob_hash)?;
        let addr = self.node.peer().addr(conn.remote_id());
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
