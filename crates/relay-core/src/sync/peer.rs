use crate::contracts::{PeerStatus, TransportPath};
use crate::engine::RelayEngine;
use iroh::endpoint::{Connection, WeakConnectionHandle};
use iroh::{EndpointAddr, EndpointId, TransportAddr};
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Debug, Default)]
pub(super) struct PeerState {
    addrs: BTreeMap<EndpointId, EndpointAddr>,
    selected: Option<EndpointId>,
    outbound: Option<Connection>,
    inbound: Option<WeakConnectionHandle>,
    rtt: Option<Duration>,
    last_synced_commit: Option<String>,
}

impl PeerState {
    pub(super) fn register(&mut self, addr: EndpointAddr) {
        if !addr.is_empty() {
            self.addrs.insert(addr.id, addr);
        }
    }

    pub(super) fn addr(&self, peer: EndpointId) -> EndpointAddr {
        self.addrs
            .get(&peer)
            .cloned()
            .unwrap_or_else(|| EndpointAddr::new(peer))
    }

    pub(super) fn select(&mut self, peer: EndpointId) {
        if self.selected != Some(peer) {
            self.selected = Some(peer);
            self.outbound = None;
            self.inbound = None;
            self.rtt = None;
            self.last_synced_commit = None;
        }
    }

    pub(super) fn outbound(&self, peer: EndpointId) -> Option<Connection> {
        self.outbound
            .as_ref()
            .filter(|conn| conn.remote_id() == peer && conn.close_reason().is_none())
            .cloned()
    }

    pub(super) fn forget_outbound(&mut self, conn: &Connection) {
        if self
            .outbound
            .as_ref()
            .is_some_and(|cached| cached.stable_id() == conn.stable_id())
        {
            self.outbound = None;
            self.rtt = None;
        }
    }

    pub(super) fn record_outbound(
        &mut self,
        conn: Connection,
        rtt: Duration,
        commit: Option<&str>,
    ) {
        if self.selected == Some(conn.remote_id()) {
            self.record_commit(conn.remote_id(), commit);
            self.outbound = Some(conn);
            self.rtt = Some(rtt);
        }
    }

    pub(super) fn record_inbound(&mut self, conn: &Connection) {
        let peer = conn.remote_id();
        if self.selected.is_none() {
            self.select(peer);
        }
        if self.selected == Some(peer) {
            self.inbound = Some(conn.weak_handle());
        }
    }

    pub(super) fn record_commit(&mut self, peer: EndpointId, commit: Option<&str>) {
        if self.selected == Some(peer) && let Some(commit) = commit {
            self.last_synced_commit = Some(commit.to_owned());
        }
    }

    pub(super) fn status(&self, local: EndpointId, engine: &RelayEngine) -> PeerStatus {
        let mut status = self.base_status(local, self.selected, engine);
        if let Some(conn) = self.selected.and_then(|peer| self.outbound(peer)) {
            apply_connection(&mut status, &conn, self.rtt);
        } else if let Some(conn) = self.inbound.as_ref().and_then(WeakConnectionHandle::upgrade) {
            apply_connection(&mut status, &conn, None);
        }
        status
    }

    pub(super) fn status_for_connection(
        &self,
        local: EndpointId,
        engine: &RelayEngine,
        conn: &Connection,
    ) -> PeerStatus {
        let mut status = self.base_status(local, Some(conn.remote_id()), engine);
        apply_connection(&mut status, conn, None);
        status
    }

    fn base_status(
        &self,
        local: EndpointId,
        remote: Option<EndpointId>,
        engine: &RelayEngine,
    ) -> PeerStatus {
        PeerStatus {
            mode: engine.observed_runtime_mode(),
            local_node_id: local.to_string(),
            remote_node_id: remote.map(|id| id.to_string()),
            transport_path: TransportPath::Offline,
            connected: false,
            rtt_ms: None,
            last_synced_commit: if self.selected == remote {
                self.last_synced_commit.clone()
            } else {
                None
            },
            pending_outbox_count: engine.observed_outbox_count(),
        }
    }
}

fn apply_connection(status: &mut PeerStatus, conn: &Connection, rpc_rtt: Option<Duration>) {
    if conn.close_reason().is_some() {
        return;
    }
    status.connected = true;
    let paths = conn.paths();
    if let Some(path) = paths
        .iter()
        .find(|path| path.is_selected())
        .or_else(|| paths.iter().next())
    {
        status.transport_path = match path.remote_addr() {
            TransportAddr::Relay(_) => TransportPath::Relay,
            TransportAddr::Ip(addr) if addr.ip().is_loopback() => TransportPath::Loopback,
            TransportAddr::Ip(_) => TransportPath::DirectLan,
            _ => TransportPath::Offline,
        };
        status.rtt_ms = Some(rtt_ms(rpc_rtt.unwrap_or_else(|| path.rtt())));
    } else {
        status.rtt_ms = rpc_rtt.map(rtt_ms);
    }
}

fn rtt_ms(duration: Duration) -> u32 {
    u32::try_from(duration.as_millis()).unwrap_or(u32::MAX)
}
