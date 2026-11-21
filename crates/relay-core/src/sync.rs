use crate::contracts::{RELAY_RPC_ALPN, WireRequest, WireResponse};
use crate::error::{RelayError, RelayResult};
use iroh::endpoint::{Connection, presets};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointId};
use iroh_blobs::BlobsProtocol;
use iroh_blobs::store::fs::FsStore;
use std::path::Path;
use tracing::info;

/// Custom QUIC RPC handler mounted on `RELAY_RPC_ALPN` alongside `iroh_blobs::ALPN`.
#[derive(Debug, Clone, Default)]
pub struct RelayRpcHandler;

impl ProtocolHandler for RelayRpcHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        while let Ok((mut send, mut recv)) = connection.accept_bi().await {
            let raw = recv
                .read_to_end(16 * 1024 * 1024)
                .await
                .map_err(AcceptError::from_err)?;
            let req: WireRequest =
                serde_json::from_slice(&raw).map_err(AcceptError::from_err)?;
            let resp = match req {
                WireRequest::Ping { sender_device, .. } => WireResponse::Ack {
                    message: format!("pong:{sender_device}"),
                },
                _ => WireResponse::Error {
                    code: "not_implemented".into(),
                    message: "RPC handler skeleton — wire up in Track 1 sprint".into(),
                },
            };
            let encoded = serde_json::to_vec(&resp).map_err(AcceptError::from_err)?;
            send.write_all(&encoded)
                .await
                .map_err(AcceptError::from_err)?;
            send.finish().map_err(AcceptError::from_err)?;
        }
        Ok(())
    }
}

/// Dual-ALPN `iroh` 1.5 transport node serving both BLAKE3 blobs and Relay control RPCs.
pub struct RelayNode {
    endpoint: Endpoint,
    store: FsStore,
    router: Router,
}

impl RelayNode {
    pub async fn start(blob_dir: &Path) -> RelayResult<Self> {
        tokio::fs::create_dir_all(blob_dir).await?;
        let endpoint = Endpoint::bind(presets::N0)
            .await
            .map_err(|e| RelayError::Transport(e.to_string()))?;
        let store = FsStore::load(blob_dir)
            .await
            .map_err(|e| RelayError::Transport(e.to_string()))?;
        let blobs = BlobsProtocol::new(&store, None);

        let router = Router::builder(endpoint.clone())
            .accept(iroh_blobs::ALPN, blobs)
            .accept(RELAY_RPC_ALPN, RelayRpcHandler)
            .spawn();

        info!(node_id = %endpoint.id(), "Relay iroh 1.5 node started");
        Ok(Self {
            endpoint,
            store,
            router,
        })
    }

    pub fn id(&self) -> EndpointId {
        self.endpoint.id()
    }

    pub fn store(&self) -> &FsStore {
        &self.store
    }

    pub async fn shutdown(self) -> RelayResult<()> {
        self.router
            .shutdown()
            .await
            .map_err(|e| RelayError::Transport(e.to_string()))?;
        Ok(())
    }
}
