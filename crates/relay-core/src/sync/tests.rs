use super::*;
use crate::contracts::{
    EdgeKind, Freshness, GraphEdge, GraphNode, JsonValue, NodeKind, RuntimeMode, TaskState,
    TransportPath,
};
use std::collections::BTreeMap;
use tokio::process::Command;

const REPO: &str = "sync-fixture";
const COMMIT: &str = "0123456789012345678901234567890123456789";

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("relay-sync-{}", Uuid::now_v7()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn start_local(
    temp: &TempDir,
    name: &str,
    sandbox: Option<WorktreeSandbox>,
) -> (RelayNode, Arc<RelayEngine>) {
    let repo_root = sandbox.as_ref().map(|sandbox| sandbox.repo_root().to_owned());
    let engine = Arc::new(RelayEngine::in_memory(REPO, repo_root).await.unwrap());
    let node = RelayNode::start_with_engine(
        &temp.join(name),
        engine.clone(),
        sandbox,
        NetworkProfile::LocalOnly,
    )
    .await
    .unwrap();
    (node, engine)
}

fn dag_delta(commit: &str) -> DagDelta {
    let now = Utc::now();
    let node = |id: &str, payload| GraphNode {
        id: id.into(),
        label: id.into(),
        payload,
        freshness: Freshness::Fresh,
        device_origin: "client".into(),
        created_at: now,
        updated_at: now,
    };
    DagDelta {
        repo_id: REPO.into(),
        upsert_nodes: vec![
            node(
                "decision",
                NodeKind::Decision {
                    rationale: "Compare the baseline".into(),
                    constraints: vec!["Use the fixture dataset".into()],
                },
            ),
            node(
                "artifact",
                NodeKind::Artifact {
                    git_commit_sha: commit.into(),
                    env_fingerprint: "test-env".into(),
                    working_tree_dirty: false,
                    changed_files: vec!["pyproject.toml".into()],
                },
            ),
            node(
                "evidence",
                NodeKind::Evidence {
                    task_id: Uuid::now_v7().to_string(),
                    preset: "smoke".into(),
                    metrics_json: JsonValue::Number(0.875),
                    log_blob_hash: "test-log".into(),
                },
            ),
        ],
        upsert_edges: vec![
            GraphEdge {
                id: "decision-artifact".into(),
                from: "decision".into(),
                to: "artifact".into(),
                kind: EdgeKind::Informs,
            },
            GraphEdge {
                id: "artifact-evidence".into(),
                from: "artifact".into(),
                to: "evidence".into(),
                kind: EdgeKind::ProducedFrom,
            },
        ],
    }
}

fn assert_delta(snapshot: &DagSnapshot, delta: &DagDelta, commit: &str) {
    assert_eq!(snapshot.repo_id, delta.repo_id);
    assert_eq!(snapshot.head_commit_sha.as_deref(), Some(commit));
    assert_eq!(snapshot.stale_count, 0);
    let mut nodes = delta.upsert_nodes.clone();
    let mut edges = delta.upsert_edges.clone();
    nodes.sort_by(|left, right| left.id.cmp(&right.id));
    edges.sort_by(|left, right| left.id.cmp(&right.id));
    assert_eq!(snapshot.nodes, nodes);
    assert_eq!(snapshot.edges, edges);
}

#[tokio::test]
async fn dual_node_ping_and_dag_delta_sync_over_iroh() {
    let temp = TempDir::new();
    let (host, host_engine) = start_local(&temp, "host-blobs", None).await;
    let (client, client_engine) = start_local(&temp, "client-blobs", None).await;
    client_engine.set_runtime_mode(RuntimeMode::Client).await.unwrap();
    assert_eq!(client.peer_status().mode, RuntimeMode::Client);
    assert_eq!(client.peer_status().transport_path, TransportPath::Offline);

    let pong = client.ping_peer(host.addr(), "client").await.unwrap();
    assert_eq!(pong.mode, RuntimeMode::Host);
    assert_eq!(pong.local_node_id, host.id().to_string());
    assert_eq!(pong.remote_node_id, Some(client.id().to_string()));
    assert_eq!(pong.transport_path, TransportPath::Loopback);
    assert!(pong.connected);

    let status = client.peer_status();
    assert!(status.connected);
    assert_eq!(status.remote_node_id, Some(host.id().to_string()));
    assert_eq!(status.transport_path, TransportPath::Loopback);
    assert!(status.rtt_ms.is_some());

    let delta = dag_delta(COMMIT);
    let snapshot = client.sync_dag_delta(host.addr(), delta.clone()).await.unwrap();
    assert_delta(&snapshot, &delta, COMMIT);
    assert_delta(&host_engine.dag_snapshot(true).await.unwrap(), &delta, COMMIT);
    assert_eq!(client.peer_status().last_synced_commit.as_deref(), Some(COMMIT));

    let result = client
        .send_rpc(host.addr(), WireRequest::FetchDagSnapshot { repo_id: REPO.into() })
        .await
        .unwrap();
    let WireResponse::DagSnapshot(snapshot) = result else {
        panic!("expected DAG snapshot, received {result:?}");
    };
    assert_delta(&snapshot, &delta, COMMIT);

    host.shutdown().await.unwrap();
    client.shutdown().await.unwrap();
}

async fn git_fixture(repo: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .kill_on_drop(true)
        .output()
        .await
        .unwrap();
    assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim_end().to_owned()
}

#[tokio::test]
async fn mirrored_evidence_reads_paired_log_then_uses_offline_cache() {
    use crate::contracts::ReadEvidenceLogArgs;
    use crate::mcp::RelayMcpServer;

    let temp = TempDir::new();
    let (host, _) = start_local(&temp, "log-host", None).await;
    let (client, engine) = start_local(&temp, "log-client", None).await;
    let client = Arc::new(client);
    client.set_remote_peer(host.addr());
    let log = "first line\nlast line\n";
    host.publish_evidence_log(log).await.unwrap();
    let mut delta = dag_delta(COMMIT);
    let evidence = delta.upsert_nodes.iter_mut().find(|node| node.id == "evidence").unwrap();
    let NodeKind::Evidence { log_blob_hash, .. } = &mut evidence.payload else { unreachable!() };
    *log_blob_hash = blake3::hash(log.as_bytes()).to_hex().to_string();
    engine.apply_dag_delta(delta.clone()).await.unwrap();
    let server = RelayMcpServer::new(engine.clone(), None, Some(client.clone()));
    let args = ReadEvidenceLogArgs { node_id: "evidence".into(), max_lines: Some(1) };
    assert_eq!(server.handle_read_evidence_log(args.clone()).await.unwrap().log_tail, "last line");
    host.shutdown().await.unwrap();
    assert_eq!(server.handle_read_evidence_log(args.clone()).await.unwrap().log_tail, "last line");
    let evidence = delta.upsert_nodes.iter_mut().find(|node| node.id == "evidence").unwrap();
    evidence.freshness = Freshness::Stale {
        caused_by: "artifact".into(), reason: "new commit".into(), marked_at: Utc::now(),
    };
    engine.apply_dag_delta(delta).await.unwrap();
    assert!(matches!(
        server.handle_read_evidence_log(args).await,
        Err(RelayError::StaleEvidenceBlocked { .. })
    ));
    client.shutdown_shared().await.unwrap();
}

async fn init_repo(repo: &Path) {
    std::fs::create_dir(repo).unwrap();
    git_fixture(repo, &["init", "--initial-branch=main"]).await;
    git_fixture(repo, &["config", "core.autocrlf", "false"]).await;
}

#[tokio::test]
async fn dual_node_git_bundle_transfer_and_task_submission_over_iroh_blobs() {
    let temp = TempDir::new();
    let source = temp.join("client repo");
    let target = temp.join("host repo");
    init_repo(&source).await;
    init_repo(&target).await;
    let pyproject = "[project]\nname = \"relay-fixture\"\nversion = \"0.1.0\"\n";
    std::fs::write(source.join("pyproject.toml"), pyproject).unwrap();
    std::fs::write(
        source.join("relay.toml"),
        r#"[project]
repo_id = "sync-fixture"
name = "Relay sync fixture"
[presets.smoke]
description = "Fixture smoke check"
program = "python"
args = ["--version"]
"#,
    )
    .unwrap();
    git_fixture(&source, &["add", "--all"]).await;
    let tree = git_fixture(&source, &["write-tree"]).await;
    let commit = git_fixture(
        &source,
        &[
            "-c", "user.name=Relay Test", "-c", "user.email=relay-test@example.invalid",
            "-c", "commit.gpgsign=false", "commit-tree", &tree, "-m", "Initial sync fixture",
        ],
    )
    .await;
    git_fixture(&source, &["update-ref", "HEAD", &commit]).await;
    std::fs::write(
        source.join("pyproject.toml"),
        format!("{pyproject}description = \"Local changes\"\n"),
    )
    .unwrap();

    let sender = WorktreeSandbox::new(&source, temp.join("source-worktrees"));
    let receiver = WorktreeSandbox::new(&target, temp.join("target-worktrees"));
    let (host, host_engine) = start_local(&temp, "host-blobs", Some(receiver.clone())).await;
    let (client, _) = start_local(&temp, "client-blobs", None).await;
    let delta = dag_delta(&commit);
    let meta = client
        .push_sync_bundle(host.addr(), REPO, &sender, delta.clone())
        .await
        .unwrap();
    assert_eq!(meta.base_commit_sha, commit);
    assert_eq!(meta.branch_name, "main");
    let reply = host
        .send_rpc(client.addr(), WireRequest::PushSyncBundle {
            meta: meta.clone(),
            dag_delta: delta.clone(),
        })
        .await
        .unwrap();
    assert!(matches!(reply, WireResponse::Error { .. }));
    assert!(client.shared.engine.dag_snapshot(true).await.unwrap().nodes.is_empty());
    let hash: Hash = meta.git_bundle_blob_hash.parse().unwrap();
    let blob = host.store().blobs().get_bytes(hash).await.unwrap();
    assert_eq!(hash.as_bytes(), blake3::hash(&blob).as_bytes());
    assert_eq!(blob, client.store().blobs().get_bytes(hash).await.unwrap());
    let diff_hash: Hash = meta.working_diff_blob_hash.unwrap().parse().unwrap();
    let patch = host.store().blobs().get_bytes(diff_hash).await.unwrap();
    assert!(String::from_utf8_lossy(&patch).contains("+description = \"Local changes\""));

    let incoming_ref = format!("refs/relay/incoming/sync-{}", &commit[..12]);
    assert_eq!(git_fixture(&target, &["rev-parse", &incoming_ref]).await, commit);
    assert_eq!(
        receiver.changed_files_in_commit(&commit).await.unwrap(),
        vec!["pyproject.toml", "relay.toml"]
    );
    assert_delta(&host_engine.dag_snapshot(true).await.unwrap(), &delta, &commit);
    assert_eq!(host.peer_status().last_synced_commit.as_deref(), Some(commit.as_str()));

    let task_id = Uuid::now_v7();
    let spec = TaskSpec {
        task_id,
        repo_id: REPO.into(),
        base_commit_sha: commit,
        command_preset: "smoke".into(),
        params: BTreeMap::new(),
        require_approval: true,
        requested_by_device: "client".into(),
        user_note: None,
    };
    let queued = client.submit_remote_task(host.addr(), spec.clone()).await.unwrap();
    assert_eq!(queued.spec, spec);
    assert_eq!(queued.state, TaskState::Queued);
    assert_eq!(host_engine.get_task(task_id).await.unwrap(), Some(queued.clone()));
    assert_eq!(client.submit_remote_task(host.addr(), spec).await.unwrap(), queued);
    assert_eq!(host_engine.list_tasks().await.unwrap().len(), 1);

    let result = client
        .send_rpc(host.addr(), WireRequest::FetchTaskState { task_id })
        .await
        .unwrap();
    assert!(matches!(result, WireResponse::TaskState(record) if record == queued));
    let missing = client
        .send_rpc(host.addr(), WireRequest::FetchTaskState { task_id: Uuid::now_v7() })
        .await
        .unwrap();
    assert!(matches!(missing, WireResponse::Error { code, .. } if code == "not_found"));
    let rejected = client
        .resolve_remote_approval(host.addr(), task_id, None, ApprovalDecision::Rejected)
        .await
        .unwrap();
    assert_eq!(rejected.state, TaskState::Rejected);
    assert_eq!(host_engine.get_task(task_id).await.unwrap(), Some(rejected));

    client.shutdown().await.unwrap();
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn offline_outbox_flushes_when_peer_comes_online() {
    let temp = TempDir::new();
    let (client, client_engine) = start_local(&temp, "client-blobs", None).await;
    let delta = dag_delta(COMMIT);
    let request = WireRequest::SyncDagDelta { delta: delta.clone() };
    client_engine
        .enqueue_outbox(&serde_json::to_value(request).unwrap())
        .await
        .unwrap();
    assert_eq!(client.peer_status().pending_outbox_count, 1);

    let (host, host_engine) = start_local(&temp, "host-blobs", None).await;
    let offline = EndpointAddr::new(host.id());
    assert!(client.flush_outbox(offline).await.is_err());
    assert_eq!(client.peer_status().pending_outbox_count, 1);
    assert!(!client.peer_status().connected);

    assert_eq!(client.flush_outbox(host.addr()).await.unwrap(), 1);
    assert_eq!(client.peer_status().pending_outbox_count, 0);
    assert_eq!(client_engine.pending_outbox_count().await.unwrap(), 0);
    assert_eq!(client.flush_outbox(host.addr()).await.unwrap(), 0);
    assert_delta(&host_engine.dag_snapshot(true).await.unwrap(), &delta, COMMIT);

    client.shutdown().await.unwrap();
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn outbox_replays_tasks_only_to_their_selected_workstation() {
    let temp = TempDir::new();
    let (client, client_engine) = start_local(&temp, "scoped-client", None).await;
    let (first, first_engine) = start_local(&temp, "first-host", None).await;
    let (second, second_engine) = start_local(&temp, "second-host", None).await;
    let queue = |id| TaskSpec {
        task_id: id,
        repo_id: REPO.into(),
        base_commit_sha: COMMIT.into(),
        command_preset: "smoke".into(),
        params: BTreeMap::new(),
        require_approval: true,
        requested_by_device: client.id().to_string(),
        user_note: None,
    };
    let first_task = Uuid::now_v7();
    let second_task = Uuid::now_v7();
    client_engine.queue_peer_task(queue(first_task), Some(first.id().to_string())).await.unwrap();
    client_engine.queue_peer_task(queue(second_task), Some(second.id().to_string())).await.unwrap();

    assert_eq!(client.flush_outbox(second.addr()).await.unwrap(), 1);
    assert!(second_engine.get_task(first_task).await.unwrap().is_none());
    assert!(second_engine.get_task(second_task).await.unwrap().is_some());
    assert_eq!(client.peer_status().pending_outbox_count, 0);
    assert_eq!(
        client_engine
            .pending_outbox_count_for_peer(Some(&second.id().to_string()))
            .await
            .unwrap(),
        0
    );
    assert_eq!(client_engine.pending_outbox_count().await.unwrap(), 1);

    assert_eq!(client.flush_outbox(first.addr()).await.unwrap(), 1);
    assert!(first_engine.get_task(first_task).await.unwrap().is_some());
    assert!(first_engine.get_task(second_task).await.unwrap().is_none());
    assert_eq!(client.peer_status().pending_outbox_count, 0);
    assert_eq!(client_engine.pending_outbox_count().await.unwrap(), 0);

    client.shutdown().await.unwrap();
    first.shutdown().await.unwrap();
    second.shutdown().await.unwrap();
}

#[tokio::test]
async fn interactive_host_accepts_one_ticket_and_emits_each_rpc_mutation_once() {
    let temp = TempDir::new();
    let engine = Arc::new(RelayEngine::in_memory(REPO, None).await.unwrap());
    let (events, mut changes) = mpsc::unbounded_channel();
    let host = RelayNode::start_with_events(
        &temp.join("interactive-host"),
        engine,
        RelayNodeConfig { profile: NetworkProfile::LocalOnly, sandbox: None },
        events,
    )
    .await
    .unwrap();
    let (client, _) = start_local(&temp, "pairing-client", None).await;
    let spec = TaskSpec {
        task_id: Uuid::now_v7(),
        repo_id: REPO.into(),
        base_commit_sha: COMMIT.into(),
        command_preset: "smoke".into(),
        params: BTreeMap::new(),
        require_approval: true,
        requested_by_device: "client".into(),
        user_note: None,
    };
    assert!(client.submit_remote_task(host.addr(), spec.clone()).await.is_err());
    assert!(changes.try_recv().is_err());

    host.open_pairing_window(Duration::from_secs(30));
    let ticket = serde_json::to_string(&host.addr()).unwrap();
    let address = serde_json::from_str::<EndpointAddr>(&ticket).unwrap();
    client.ping_peer(address.clone(), "client").await.unwrap();
    assert!(matches!(changes.try_recv().unwrap(), RelayNodeEvent::PeerPaired));

    for _ in 0..2 {
        client.submit_remote_task(address.clone(), spec.clone()).await.unwrap();
    }
    assert!(matches!(
        changes.try_recv().unwrap(),
        RelayNodeEvent::TaskSubmitted(id) if id == spec.task_id
    ));

    for _ in 0..2 {
        let task = client
            .resolve_remote_approval(address.clone(), spec.task_id, None, ApprovalDecision::Rejected)
            .await
            .unwrap();
        assert_eq!(task.state, TaskState::Rejected);
    }
    assert!(matches!(
        changes.try_recv().unwrap(),
        RelayNodeEvent::ApprovalResolved(id, None, ApprovalDecision::Rejected) if id == spec.task_id
    ));
    assert!(changes.try_recv().is_err());

    host.shutdown().await.unwrap();
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn parse_pairing_ticket_accepts_raw_and_daemon_formats() {
    let temp = TempDir::new();
    let (node, _) = start_local(&temp, "ticket-node", None).await;
    let expected = node.addr();

    let raw = serde_json::to_string(&expected).unwrap();
    assert_eq!(parse_pairing_ticket(&raw).unwrap(), expected);

    let daemon_wrapped = serde_json::json!({
        "node_id": node.id().to_string(),
        "addr": expected,
    })
    .to_string();
    assert_eq!(parse_pairing_ticket(&daemon_wrapped).unwrap(), expected);

    let nested_ticket = serde_json::json!({
        "pairing_ticket": raw,
    })
    .to_string();
    assert_eq!(parse_pairing_ticket(&nested_ticket).unwrap(), expected);

    node.shutdown().await.unwrap();
}

#[tokio::test]
async fn lan_only_nodes_advertise_direct_ips_without_relay_and_sync() {
    let temp = TempDir::new();
    let host_engine = Arc::new(RelayEngine::in_memory(REPO, None).await.unwrap());
    let client_engine = Arc::new(RelayEngine::in_memory(REPO, None).await.unwrap());
    client_engine.set_runtime_mode(RuntimeMode::Client).await.unwrap();

    let host = RelayNode::start_with_engine(
        &temp.join("host-lan"),
        host_engine.clone(),
        None,
        NetworkProfile::LanOnly,
    )
    .await
    .unwrap();
    let client = RelayNode::start_with_engine(
        &temp.join("client-lan"),
        client_engine,
        None,
        NetworkProfile::LanOnly,
    )
    .await
    .unwrap();

    let host_addr = host.addr();
    let client_addr = client.addr();
    assert!(host_addr.relay_urls().next().is_none());
    assert!(client_addr.relay_urls().next().is_none());
    assert!(host_addr.ip_addrs().next().is_some());
    assert!(client_addr.ip_addrs().next().is_some());

    let pong = client.ping_peer(host_addr.clone(), "lan-client").await.unwrap();
    assert!(pong.connected);
    assert!(matches!(
        pong.transport_path,
        TransportPath::DirectLan | TransportPath::Loopback
    ));

    let delta = dag_delta(COMMIT);
    let snapshot = client.sync_dag_delta(host_addr, delta.clone()).await.unwrap();
    assert_delta(&snapshot, &delta, COMMIT);
    assert_delta(&host_engine.dag_snapshot(true).await.unwrap(), &delta, COMMIT);

    host.shutdown().await.unwrap();
    client.shutdown().await.unwrap();
}

#[test]
fn lan_ticket_keeps_dialable_ipv4_and_ipv6_only() {
    let id = iroh::SecretKey::generate().public();
    let valid = [
        "192.168.1.4:7608".parse().unwrap(),
        "[fd00::4]:7608".parse().unwrap(),
    ];
    let mut ticket = EndpointAddr::new(id)
        .with_relay_url("https://relay.example.invalid".parse().unwrap());
    for ip in valid {
        ticket = ticket.with_ip_addr(ip);
    }
    for raw in [
        "0.0.0.0:7608",
        "[::]:7608",
        "192.168.1.4:0",
        "224.0.0.1:7608",
        "[ff02::1]:7608",
        "255.255.255.255:7608",
        "[::ffff:255.255.255.255]:7608",
    ] {
        ticket = ticket.with_ip_addr(raw.parse().unwrap());
    }
    let direct = direct_lan_addr(&ticket);
    assert_eq!(direct.id, id);
    assert!(direct.relay_urls().next().is_none());
    assert_eq!(direct.ip_addrs().count(), valid.len());
    assert!(valid.iter().all(|ip| direct.ip_addrs().any(|addr| addr == ip)));
}

#[tokio::test]
async fn wire_cancel_task_cancels_queued_and_running_tasks_over_local_only_peer_rpc() {
    let temp = TempDir::new();
    let (host, host_engine) = start_local(&temp, "cancel-host", None).await;
    let (client, client_engine) = start_local(&temp, "cancel-client", None).await;
    client_engine
        .set_runtime_mode(RuntimeMode::Client)
        .await
        .unwrap();

    let queued_id = Uuid::now_v7();
    let submitted = client
        .submit_remote_task(
            host.addr(),
            TaskSpec {
                task_id: queued_id,
                repo_id: REPO.into(),
                base_commit_sha: COMMIT.into(),
                command_preset: "smoke".into(),
                params: BTreeMap::new(),
                require_approval: false,
                requested_by_device: "client".into(),
                user_note: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(submitted.state, TaskState::Queued);

    // 1) Cancel queued task over LocalOnly QUIC RPC.
    let cancelled = client
        .cancel_remote_task(host.addr(), queued_id)
        .await
        .unwrap();
    assert_eq!(cancelled.state, TaskState::Cancelled);
    assert_eq!(
        host_engine.get_task(queued_id).await.unwrap().unwrap().state,
        TaskState::Cancelled
    );

    // 2) Replay returns the same cancellation, while an unknown task is rejected.
    let repeated = client
        .cancel_remote_task(host.addr(), queued_id)
        .await
        .unwrap();
    assert_eq!(repeated, cancelled);
    let missing = client
        .send_rpc(
            host.addr(),
            WireRequest::CancelTask {
                task_id: Uuid::now_v7(),
            },
        )
        .await
        .unwrap();
    assert!(matches!(
        missing,
        WireResponse::Error { ref code, .. } if code == "not_found"
    ));

    // 3) Cancelling an actively executing task signals its cancellation token on Host.
    let running_id = Uuid::now_v7();
    client
        .submit_remote_task(
            host.addr(),
            TaskSpec {
                task_id: running_id,
                repo_id: REPO.into(),
                base_commit_sha: COMMIT.into(),
                command_preset: "smoke".into(),
                params: BTreeMap::new(),
                require_approval: false,
                requested_by_device: "client".into(),
                user_note: None,
            },
        )
        .await
        .unwrap();
    host_engine
        .transition_task(running_id, TaskState::Executing, None, None)
        .await
        .unwrap();
    let guard = host_engine
        .register_task_cancellation(running_id)
        .await
        .unwrap();
    assert!(!guard.token().is_cancelled());

    let remote_cancelled = client
        .cancel_remote_task(host.addr(), running_id)
        .await
        .unwrap();
    assert_eq!(remote_cancelled.state, TaskState::Cancelled);
    assert!(guard.token().is_cancelled());
    drop(guard);
    assert_eq!(host_engine.active_cancellation_count(), 0);

    host.shutdown().await.unwrap();
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn host_revocation_gate_blocks_all_rpcs_and_blobs_from_revoked_peer() {
    let temp = TempDir::new();
    let host_engine = Arc::new(RelayEngine::in_memory(REPO, None).await.unwrap());
    let (events, _changes) = mpsc::unbounded_channel();
    let host = RelayNode::start_with_events(
        &temp.join("revoke-host"),
        host_engine.clone(),
        RelayNodeConfig {
            profile: NetworkProfile::LocalOnly,
            sandbox: None,
        },
        events,
    )
    .await
    .unwrap();
    let (client, client_engine) = start_local(&temp, "revoke-client", None).await;
    client_engine
        .set_runtime_mode(RuntimeMode::Client)
        .await
        .unwrap();

    // 1. Pair client with host and confirm both sides persist trusted_peers
    host.open_pairing_window(Duration::from_secs(30));
    let pong = client.ping_peer(host.addr(), "test-laptop").await.unwrap();
    assert!(pong.connected);

    let host_peers = host_engine.list_trusted_peers().await.unwrap();
    assert_eq!(host_peers.len(), 1);
    assert_eq!(host_peers[0].node_id, client.id().to_string());
    assert_eq!(host_peers[0].label, "test-laptop");
    assert_eq!(host_peers[0].role, PeerRole::Client);
    assert!(!host_peers[0].is_revoked());

    let client_peers = client_engine.list_trusted_peers().await.unwrap();
    assert_eq!(client_peers.len(), 1);
    assert_eq!(client_peers[0].node_id, host.id().to_string());
    assert_eq!(client_peers[0].role, PeerRole::Host);

    // 2. Verify RPCs succeed while trusted
    let spec_before = TaskSpec {
        task_id: Uuid::now_v7(),
        repo_id: REPO.into(),
        base_commit_sha: COMMIT.into(),
        command_preset: "smoke".into(),
        params: BTreeMap::new(),
        require_approval: true,
        requested_by_device: "test-laptop".into(),
        user_note: None,
    };
    client
        .submit_remote_task(host.addr(), spec_before)
        .await
        .unwrap();
    assert_eq!(host_engine.list_tasks().await.unwrap().len(), 1);

    let log_text = "verified log content\n";
    host.publish_evidence_log(log_text).await.unwrap();
    let log_hash: Hash = blake3::hash(log_text.as_bytes()).to_hex().to_string().parse().unwrap();
    let blob_connection = client.endpoint().connect(host.addr(), iroh_blobs::ALPN).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let accepted = host.shared.active_connections.lock().unwrap().len();
            if accepted >= 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    }).await.expect("both peer connections were accepted");

    // 3. Revoke client on the host and disconnect live QUIC handles
    let revoked = host_engine
        .revoke_trusted_peer(&client.id().to_string())
        .await
        .unwrap();
    assert!(revoked.is_revoked());
    host.disconnect_peer(client.id());
    tokio::time::timeout(Duration::from_secs(3), blob_connection.closed())
        .await.expect("active blob connection was closed");

    // Even if a new pairing window is opened, revocation gate MUST still block
    host.open_pairing_window(Duration::from_secs(30));

    // 4. Every WireRequest from the revoked peer must be immediately rejected with PEER_REVOKED
    let ping_reply = client
        .send_rpc(
            host.addr(),
            WireRequest::Ping {
                sender_device: "test-laptop".into(),
                sent_at: Utc::now(),
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(&ping_reply, WireResponse::Error { code, .. } if code == "PEER_REVOKED"),
        "expected PEER_REVOKED for Ping, got {ping_reply:?}"
    );
    assert!(!client.peer_status().connected);

    let dag_reply = client
        .send_rpc(
            host.addr(),
            WireRequest::FetchDagSnapshot {
                repo_id: REPO.into(),
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(&dag_reply, WireResponse::Error { code, .. } if code == "PEER_REVOKED"),
        "expected PEER_REVOKED for FetchDagSnapshot, got {dag_reply:?}"
    );

    let spec_after = TaskSpec {
        task_id: Uuid::now_v7(),
        repo_id: REPO.into(),
        base_commit_sha: COMMIT.into(),
        command_preset: "smoke".into(),
        params: BTreeMap::new(),
        require_approval: false,
        requested_by_device: "test-laptop".into(),
        user_note: Some("should be blocked".into()),
    };
    let task_reply = client
        .send_rpc(host.addr(), WireRequest::SubmitTask { spec: spec_after })
        .await
        .unwrap();
    assert!(
        matches!(&task_reply, WireResponse::Error { code, .. } if code == "PEER_REVOKED"),
        "expected PEER_REVOKED for SubmitTask, got {task_reply:?}"
    );
    // Confirm SubmitTask was never executed
    assert_eq!(host_engine.list_tasks().await.unwrap().len(), 1);

    // Confirm blob transfer is also blocked for revoked peers
    assert!(client.pull_blob_from(host.addr(), log_hash).await.is_err());

    host.shutdown().await.unwrap();
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn restarted_interactive_host_accepts_persisted_trusted_client_without_pairing_window() {
    let temp = TempDir::new();
    let host_db = temp.join("host.db");
    let client_db = temp.join("client.db");
    let host_blobs = temp.join("host-blobs");
    let client_blobs = temp.join("client-blobs");

    let (host_id, client_id) = {
        let host_engine = Arc::new(RelayEngine::open(REPO, &host_db, None).await.unwrap());
        let client_engine = Arc::new(RelayEngine::open(REPO, &client_db, None).await.unwrap());
        client_engine
            .set_runtime_mode(RuntimeMode::Client)
            .await
            .unwrap();
        let (events, _rx) = mpsc::unbounded_channel();
        let host = RelayNode::start_with_events(
            &host_blobs,
            host_engine,
            RelayNodeConfig {
                profile: NetworkProfile::LocalOnly,
                sandbox: None,
            },
            events,
        )
        .await
        .unwrap();
        let client = RelayNode::start_with_engine(
            &client_blobs,
            client_engine,
            None,
            NetworkProfile::LocalOnly,
        )
        .await
        .unwrap();

        host.open_pairing_window(Duration::from_secs(30));
        client.ping_peer(host.addr(), "laptop-1").await.unwrap();
        let ids = (host.id(), client.id());
        host.shutdown().await.unwrap();
        client.shutdown().await.unwrap();
        ids
    };

    // Restart both nodes from persisted state; do NOT open a pairing window
    let host_engine = Arc::new(RelayEngine::open(REPO, &host_db, None).await.unwrap());
    let client_engine = Arc::new(RelayEngine::open(REPO, &client_db, None).await.unwrap());
    let (events, mut changes) = mpsc::unbounded_channel();
    let host = RelayNode::start_with_events(
        &host_blobs,
        host_engine,
        RelayNodeConfig {
            profile: NetworkProfile::LocalOnly,
            sandbox: None,
        },
        events,
    )
    .await
    .unwrap();
    let client = RelayNode::start_with_engine(
        &client_blobs,
        client_engine,
        None,
        NetworkProfile::LocalOnly,
    )
    .await
    .unwrap();

    assert_eq!(host.id(), host_id);
    assert_eq!(client.id(), client_id);

    let pong = client.ping_peer(host.addr(), "laptop-1").await.unwrap();
    assert!(pong.connected);
    assert!(matches!(changes.try_recv().unwrap(), RelayNodeEvent::PeerPaired));

    host.shutdown().await.unwrap();
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn startup_auto_reconnect_leaves_replay_to_the_runtime() {
    let temp = TempDir::new();
    let host_db = temp.join("auto-host.db");
    let client_db = temp.join("auto-client.db");
    let host_blobs = temp.join("auto-host-blobs");
    let client_blobs = temp.join("auto-client-blobs");

    let client_engine = Arc::new(RelayEngine::open(REPO, &client_db, None).await.unwrap());
    client_engine
        .set_runtime_mode(RuntimeMode::Client)
        .await
        .unwrap();
    let client = Arc::new(
        RelayNode::start_with_engine(
            &client_blobs,
            client_engine.clone(),
            None,
            NetworkProfile::LocalOnly,
        )
        .await
        .unwrap(),
    );

    // Seed an unreachable peer address + pending outbox item on the client
    let unreachable_id = iroh::SecretKey::generate().public();
    let unreachable_addr = EndpointAddr::new(unreachable_id)
        .with_ip_addr("127.0.0.1:1".parse().unwrap());
    let now = Utc::now();
    client_engine
        .upsert_trusted_peer(&TrustedPeerRecord {
            node_id: unreachable_id.to_string(),
            label: "Offline Host".into(),
            role: PeerRole::Host,
            ticket_or_addrs: serde_json::to_string(&unreachable_addr).unwrap(),
            paired_at: now,
            last_seen_at: now,
            revoked_at: None,
        })
        .await
        .unwrap();

    let delta = dag_delta(COMMIT);
    client_engine
        .enqueue_outbox(&serde_json::to_value(WireRequest::SyncDagDelta { delta: delta.clone() }).unwrap())
        .await
        .unwrap();
    assert_eq!(client.peer_status().pending_outbox_count, 1);

    // Start background auto-reconnect: must NOT block startup and must degrade gracefully to Offline
    let (handle, mut reconnected_rx) = spawn_auto_reconnect(
        client.clone(),
        client_engine.clone(),
        AutoReconnectConfig {
            initial_backoff: Duration::from_millis(50),
            max_backoff: Duration::from_millis(150),
            attempt_timeout: Duration::from_millis(200),
            sender_device: "auto-laptop".into(),
            max_attempts: None,
        },
    );
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(!client.peer_status().connected);
    assert_eq!(client.peer_status().transport_path, TransportPath::Offline);
    assert_eq!(client.peer_status().pending_outbox_count, 1);

    // Bring a trusted host online and record its live address in client SQLite
    let host_engine = Arc::new(RelayEngine::open(REPO, &host_db, None).await.unwrap());
    host_engine
        .upsert_trusted_peer(&TrustedPeerRecord {
            node_id: client.id().to_string(),
            label: "auto-laptop".into(),
            role: PeerRole::Client,
            ticket_or_addrs: serde_json::to_string(&client.addr()).unwrap(),
            paired_at: now,
            last_seen_at: now,
            revoked_at: None,
        })
        .await
        .unwrap();
    let (host_events, _host_rx) = mpsc::unbounded_channel();
    let host = RelayNode::start_with_events(
        &host_blobs,
        host_engine.clone(),
        RelayNodeConfig {
            profile: NetworkProfile::LocalOnly,
            sandbox: None,
        },
        host_events,
    )
    .await
    .unwrap();
    let later = Utc::now();
    client_engine
        .upsert_trusted_peer(&TrustedPeerRecord {
            node_id: host.id().to_string(),
            label: "Online Workstation".into(),
            role: PeerRole::Host,
            ticket_or_addrs: serde_json::to_string(&host.addr()).unwrap(),
            paired_at: later,
            last_seen_at: later,
            revoked_at: None,
        })
        .await
        .unwrap();

    let reconnected = tokio::time::timeout(Duration::from_secs(5), reconnected_rx.recv())
        .await
        .expect("auto-reconnect timed out")
        .expect("auto-reconnect channel closed");
    assert_eq!(reconnected.record.node_id, host.id().to_string());
    assert!(client.peer_status().connected);
    assert_eq!(client.peer_status().pending_outbox_count, 1);
    assert!(host_engine.dag_snapshot(true).await.unwrap().nodes.is_empty());
    assert_eq!(client.flush_outbox(reconnected.addr).await.unwrap(), 1);
    assert_eq!(client.peer_status().pending_outbox_count, 0);
    assert_delta(&host_engine.dag_snapshot(true).await.unwrap(), &delta, COMMIT);

    handle.cancel();
    client.shutdown_shared().await.unwrap();
    host.shutdown().await.unwrap();
}

#[tokio::test]
async fn startup_auto_reconnect_skips_revoked_hosts_and_other_clients() {
    let temp = TempDir::new();
    let (host, _) = start_local(&temp, "skip-revoked-host", None).await;
    let (other_client, other_engine) = start_local(&temp, "other-client", None).await;
    other_engine.set_runtime_mode(RuntimeMode::Client).await.unwrap();
    let (client, client_engine) = start_local(&temp, "skip-revoked-client", None).await;
    let client = Arc::new(client);
    let now = Utc::now();
    client_engine
        .set_runtime_mode(RuntimeMode::Client)
        .await
        .unwrap();
    client_engine.upsert_trusted_peer(&TrustedPeerRecord {
        node_id: other_client.id().to_string(),
        label: "Another Laptop".into(),
        role: PeerRole::Client,
        ticket_or_addrs: serde_json::to_string(&other_client.addr()).unwrap(),
        paired_at: now,
        last_seen_at: now,
        revoked_at: None,
    }).await.unwrap();

    client_engine
        .upsert_trusted_peer(&TrustedPeerRecord {
            node_id: host.id().to_string(),
            label: "Revoked Host".into(),
            role: PeerRole::Host,
            ticket_or_addrs: serde_json::to_string(&host.addr()).unwrap(),
            paired_at: now,
            last_seen_at: now,
            revoked_at: Some(now),
        })
        .await
        .unwrap();

    let (handle, mut reconnected_rx) = spawn_auto_reconnect(
        client.clone(),
        client_engine.clone(),
        AutoReconnectConfig {
            initial_backoff: Duration::from_millis(25),
            max_backoff: Duration::from_millis(100),
            attempt_timeout: Duration::from_millis(200),
            sender_device: "client".into(),
            max_attempts: Some(3),
        },
    );
    // Neither saved device is an active host candidate.
    let recv = tokio::time::timeout(Duration::from_secs(2), reconnected_rx.recv())
        .await
        .unwrap();
    assert!(recv.is_none());
    assert!(!client.peer_status().connected);
    handle.cancel();

    client.shutdown_shared().await.unwrap();
    host.shutdown().await.unwrap();
    other_client.shutdown().await.unwrap();
}

#[tokio::test]
async fn startup_auto_reconnect_allows_peer_role_while_blocking_same_role() {
    let temp = TempDir::new();
    let now = Utc::now();

    // 1. Client mode: skips another Client, reconnects to a PeerRole::Peer device.
    let (other_client, other_client_engine) = start_local(&temp, "peer-other-client", None).await;
    other_client_engine
        .set_runtime_mode(RuntimeMode::Client)
        .await
        .unwrap();
    let (peer_device, peer_device_engine) = start_local(&temp, "peer-role-target", None).await;
    let (client, client_engine) = start_local(&temp, "peer-role-client", None).await;
    let client = Arc::new(client);
    client_engine
        .set_runtime_mode(RuntimeMode::Client)
        .await
        .unwrap();

    peer_device_engine
        .upsert_trusted_peer(&TrustedPeerRecord {
            node_id: client.id().to_string(),
            label: "Laptop Client".into(),
            role: PeerRole::Client,
            ticket_or_addrs: serde_json::to_string(&client.addr()).unwrap(),
            paired_at: now,
            last_seen_at: now,
            revoked_at: None,
        })
        .await
        .unwrap();
    client_engine
        .upsert_trusted_peer(&TrustedPeerRecord {
            node_id: other_client.id().to_string(),
            label: "Other Client".into(),
            role: PeerRole::Client,
            ticket_or_addrs: serde_json::to_string(&other_client.addr()).unwrap(),
            paired_at: now,
            last_seen_at: now,
            revoked_at: None,
        })
        .await
        .unwrap();
    client_engine
        .upsert_trusted_peer(&TrustedPeerRecord {
            node_id: peer_device.id().to_string(),
            label: "Symmetric Peer".into(),
            role: PeerRole::Peer,
            ticket_or_addrs: serde_json::to_string(&peer_device.addr()).unwrap(),
            paired_at: now,
            last_seen_at: now,
            revoked_at: None,
        })
        .await
        .unwrap();

    let (handle, mut reconnected_rx) = spawn_auto_reconnect(
        client.clone(),
        client_engine.clone(),
        AutoReconnectConfig {
            initial_backoff: Duration::from_millis(25),
            max_backoff: Duration::from_millis(100),
            attempt_timeout: Duration::from_millis(400),
            sender_device: "laptop".into(),
            max_attempts: Some(3),
        },
    );
    let reconnected = tokio::time::timeout(Duration::from_secs(5), reconnected_rx.recv())
        .await
        .expect("client auto-reconnect to PeerRole::Peer timed out")
        .expect("client auto-reconnect channel closed");
    assert_eq!(reconnected.record.node_id, peer_device.id().to_string());
    assert_eq!(reconnected.record.role, PeerRole::Peer);
    handle.cancel();

    // 2. Host mode: skips another Host, reconnects to a PeerRole::Peer device.
    let (other_host, other_host_engine) = start_local(&temp, "peer-other-host", None).await;
    other_host_engine
        .set_runtime_mode(RuntimeMode::Host)
        .await
        .unwrap();
    let (host, host_engine) = start_local(&temp, "peer-role-host", None).await;
    let host = Arc::new(host);
    host_engine
        .set_runtime_mode(RuntimeMode::Host)
        .await
        .unwrap();
    peer_device_engine
        .upsert_trusted_peer(&TrustedPeerRecord {
            node_id: host.id().to_string(),
            label: "Workstation Host".into(),
            role: PeerRole::Host,
            ticket_or_addrs: serde_json::to_string(&host.addr()).unwrap(),
            paired_at: now,
            last_seen_at: now,
            revoked_at: None,
        })
        .await
        .unwrap();
    host_engine
        .upsert_trusted_peer(&TrustedPeerRecord {
            node_id: other_host.id().to_string(),
            label: "Other Host".into(),
            role: PeerRole::Host,
            ticket_or_addrs: serde_json::to_string(&other_host.addr()).unwrap(),
            paired_at: now,
            last_seen_at: now,
            revoked_at: None,
        })
        .await
        .unwrap();
    host_engine
        .upsert_trusted_peer(&TrustedPeerRecord {
            node_id: peer_device.id().to_string(),
            label: "Symmetric Peer".into(),
            role: PeerRole::Peer,
            ticket_or_addrs: serde_json::to_string(&peer_device.addr()).unwrap(),
            paired_at: now,
            last_seen_at: now,
            revoked_at: None,
        })
        .await
        .unwrap();

    let (host_handle, mut host_rx) = spawn_auto_reconnect(
        host.clone(),
        host_engine.clone(),
        AutoReconnectConfig {
            initial_backoff: Duration::from_millis(25),
            max_backoff: Duration::from_millis(100),
            attempt_timeout: Duration::from_millis(400),
            sender_device: "workstation".into(),
            max_attempts: Some(3),
        },
    );
    let host_reconnected = tokio::time::timeout(Duration::from_secs(5), host_rx.recv())
        .await
        .expect("host auto-reconnect to PeerRole::Peer timed out")
        .expect("host auto-reconnect channel closed");
    assert_eq!(host_reconnected.record.node_id, peer_device.id().to_string());
    assert_eq!(host_reconnected.record.role, PeerRole::Peer);
    host_handle.cancel();

    client.shutdown_shared().await.unwrap();
    host.shutdown_shared().await.unwrap();
    peer_device.shutdown().await.unwrap();
    other_client.shutdown().await.unwrap();
    other_host.shutdown().await.unwrap();
}

#[tokio::test]
async fn simultaneous_identity_initialization_uses_one_persisted_key() {
    let temp = TempDir::new();
    let (first, second) = tokio::join!(
        load_or_create_secret_key(&temp.0),
        load_or_create_secret_key(&temp.0)
    );
    assert_eq!(first.unwrap().public(), second.unwrap().public());
}

#[tokio::test]
async fn failed_flush_to_peer_a_does_not_strand_unassigned_outbox_for_peer_b() {
    let temp = TempDir::new();
    let (client, client_engine) = start_local(&temp, "unstranded-client", None).await;
    client_engine
        .set_runtime_mode(RuntimeMode::Client)
        .await
        .unwrap();

    // Interactive Peer A has no open pairing window so it rejects unpaired RPCs.
    let peer_a_engine = Arc::new(RelayEngine::in_memory(REPO, None).await.unwrap());
    let (events_a, _rx_a) = mpsc::unbounded_channel();
    let peer_a = RelayNode::start_with_events(
        &temp.join("unstranded-peer-a"),
        peer_a_engine.clone(),
        RelayNodeConfig {
            profile: NetworkProfile::LocalOnly,
            sandbox: None,
        },
        events_a,
    )
    .await
    .unwrap();
    let (peer_b, peer_b_engine) = start_local(&temp, "unstranded-peer-b", None).await;

    let delta = dag_delta(COMMIT);
    client_engine
        .enqueue_outbox(
            &serde_json::to_value(WireRequest::SyncDagDelta {
                delta: delta.clone(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
    let task_id = Uuid::now_v7();
    client_engine
        .queue_peer_task(
            TaskSpec {
                task_id,
                repo_id: REPO.into(),
                base_commit_sha: COMMIT.into(),
                command_preset: "smoke".into(),
                params: BTreeMap::new(),
                require_approval: true,
                requested_by_device: client.id().to_string(),
                user_note: None,
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(client.peer_status().pending_outbox_count, 2);

    // 1. Failing flush against an unreachable peer must not pin unassigned entries.
    let unreachable_peer = EndpointAddr::new(iroh::SecretKey::generate().public());
    assert!(client.flush_outbox(unreachable_peer).await.is_err());
    assert_eq!(client.peer_status().pending_outbox_count, 2);
    assert!(!client.peer_status().connected);

    // 2. Failing flush against Peer A (rejected as unpaired) must not pin unassigned entries either.
    assert!(client.flush_outbox(peer_a.addr()).await.is_err());
    assert_eq!(client.peer_status().pending_outbox_count, 2);
    assert!(!client.peer_status().connected);
    assert!(peer_a_engine.dag_snapshot(true).await.unwrap().nodes.is_empty());
    assert!(peer_a_engine.get_task(task_id).await.unwrap().is_none());

    // 3. Peer B later connects and receives both unassigned entries in order.
    assert_eq!(client.flush_outbox(peer_b.addr()).await.unwrap(), 2);
    assert_eq!(client.peer_status().pending_outbox_count, 0);
    assert_eq!(client_engine.pending_outbox_count().await.unwrap(), 0);
    assert_eq!(client.flush_outbox(peer_b.addr()).await.unwrap(), 0);
    assert_delta(&peer_b_engine.dag_snapshot(true).await.unwrap(), &delta, COMMIT);
    assert!(peer_b_engine.get_task(task_id).await.unwrap().is_some());

    client.shutdown().await.unwrap();
    peer_a.shutdown().await.unwrap();
    peer_b.shutdown().await.unwrap();
}

#[tokio::test]
async fn pending_outbox_count_scopes_to_active_peer_and_excludes_other_or_revoked_peers() {
    let temp = TempDir::new();
    let (client, client_engine) = start_local(&temp, "count-client", None).await;
    client_engine
        .set_runtime_mode(RuntimeMode::Client)
        .await
        .unwrap();
    let (peer_a, _peer_a_engine) = start_local(&temp, "count-peer-a", None).await;
    let (peer_b, peer_b_engine) = start_local(&temp, "count-peer-b", None).await;
    let revoked_id = iroh::SecretKey::generate().public().to_string();

    let now = Utc::now();
    client_engine
        .upsert_trusted_peer(&TrustedPeerRecord {
            node_id: revoked_id.clone(),
            label: "Revoked Workstation".into(),
            role: PeerRole::Host,
            ticket_or_addrs: String::new(),
            paired_at: now,
            last_seen_at: now,
            revoked_at: Some(now),
        })
        .await
        .unwrap();

    let make_spec = |id| TaskSpec {
        task_id: id,
        repo_id: REPO.into(),
        base_commit_sha: COMMIT.into(),
        command_preset: "smoke".into(),
        params: BTreeMap::new(),
        require_approval: true,
        requested_by_device: client.id().to_string(),
        user_note: None,
    };
    let task_a = Uuid::now_v7();
    let task_revoked = Uuid::now_v7();
    let task_b = Uuid::now_v7();
    client_engine
        .queue_peer_task(make_spec(task_a), Some(peer_a.id().to_string()))
        .await
        .unwrap();
    client_engine
        .queue_peer_task(make_spec(task_revoked), Some(revoked_id.clone()))
        .await
        .unwrap();
    client_engine
        .queue_peer_task(make_spec(task_b), Some(peer_b.id().to_string()))
        .await
        .unwrap();
    let delta = dag_delta(COMMIT);
    client_engine
        .enqueue_outbox(
            &serde_json::to_value(WireRequest::SyncDagDelta {
                delta: delta.clone(),
            })
            .unwrap(),
        )
        .await
        .unwrap();

    // Offline with no selected host: counts Peer A + Peer B + unassigned (3), excludes revoked peer.
    assert_eq!(client.peer_status().remote_node_id, None);
    assert_eq!(client.peer_status().pending_outbox_count, 3);
    assert_eq!(client_engine.pending_outbox_count().await.unwrap(), 3);
    assert_eq!(
        client_engine
            .pending_outbox_count_for_peer(Some(&revoked_id))
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        client_engine
            .pending_outbox_count_for_peer(Some(&peer_b.id().to_string()))
            .await
            .unwrap(),
        2
    );

    // Connected to Peer B: reports Peer B + unassigned (2), ignoring Peer A and revoked peer.
    client.ping_peer(peer_b.addr(), "client").await.unwrap();
    let status = client.peer_status();
    assert!(status.connected);
    assert_eq!(status.remote_node_id.as_deref(), Some(peer_b.id().to_string().as_str()));
    assert_eq!(status.pending_outbox_count, 2);

    // After flushing to Peer B: connected status reports 0 even though Peer A & revoked peer still have queued rows.
    assert_eq!(client.flush_outbox(peer_b.addr()).await.unwrap(), 2);
    assert_eq!(client.peer_status().pending_outbox_count, 0);
    assert_eq!(
        client_engine
            .pending_outbox_count_for_peer(Some(&peer_b.id().to_string()))
            .await
            .unwrap(),
        0
    );
    assert_eq!(client_engine.pending_outbox_count().await.unwrap(), 1);
    assert!(peer_b_engine.get_task(task_b).await.unwrap().is_some());
    assert!(peer_b_engine.get_task(task_a).await.unwrap().is_none());
    assert!(peer_b_engine.get_task(task_revoked).await.unwrap().is_none());
    assert_delta(&peer_b_engine.dag_snapshot(true).await.unwrap(), &delta, COMMIT);

    // Disconnecting returns to offline-no-host view (1 remaining for Peer A; revoked peer still excluded).
    client.disconnect_peer(peer_b.id());
    assert_eq!(client.peer_status().pending_outbox_count, 1);

    // Revoking Peer A immediately removes Peer A's queued row from pending counts.
    client_engine
        .upsert_trusted_peer(&TrustedPeerRecord {
            node_id: peer_a.id().to_string(),
            label: "Peer A".into(),
            role: PeerRole::Host,
            ticket_or_addrs: String::new(),
            paired_at: now,
            last_seen_at: now,
            revoked_at: None,
        })
        .await
        .unwrap();
    client_engine
        .revoke_trusted_peer(&peer_a.id().to_string())
        .await
        .unwrap();
    assert_eq!(client.peer_status().pending_outbox_count, 0);
    assert_eq!(client_engine.pending_outbox_count().await.unwrap(), 0);

    client.shutdown().await.unwrap();
    peer_a.shutdown().await.unwrap();
    peer_b.shutdown().await.unwrap();
}

#[tokio::test]
async fn ping_revoked_during_handshake_disconnects_and_closes_host_connections() {
    let temp = TempDir::new();
    let host_db = temp.join("race-host.db");
    let host_engine = Arc::new(RelayEngine::open(REPO, &host_db, None).await.unwrap());
    let (events, mut changes) = mpsc::unbounded_channel();
    let host = RelayNode::start_with_events(
        &temp.join("race-host-blobs"),
        host_engine,
        RelayNodeConfig {
            profile: NetworkProfile::LocalOnly,
            sandbox: None,
        },
        events,
    )
    .await
    .unwrap();
    let (client, client_engine) = start_local(&temp, "race-client-blobs", None).await;
    client_engine
        .set_runtime_mode(RuntimeMode::Client)
        .await
        .unwrap();

    host.open_pairing_window(Duration::from_secs(30));

    // Open a concurrent blob connection so we verify ALL host handles for the peer are closed.
    let blob_conn = client
        .endpoint()
        .connect(host.addr(), iroh_blobs::ALPN)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if !host.shared.active_connections.lock().unwrap().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("blob connection accepted on host");

    // Hold a SQLite write transaction so Ping passes the pre-dispatch read gate and record_inbound,
    // then blocks right at post-handshake upsert_trusted_peer (WAL readers do not block on writers).
    let lock_store = crate::storage::RelayStore::connect(&host_db).await.unwrap();
    let mut tx = lock_store.begin_write().await.unwrap();

    let client_id = client.id();
    let host_addr = host.addr();
    let client_endpoint = client.endpoint().clone();
    let ping_task = tokio::spawn(async move {
        let rpc_conn = client_endpoint
            .connect(host_addr, RELAY_RPC_ALPN)
            .await
            .unwrap();
        let (mut send, mut recv) = rpc_conn.open_bi().await.unwrap();
        let payload = encode_frame(&WireRequest::Ping {
            sender_device: "race-laptop".into(),
            sent_at: Utc::now(),
        })
        .unwrap();
        write_frame(&mut send, &payload).await.unwrap();
        let response: WireResponse = read_frame(&mut recv).await.unwrap();
        (rpc_conn, response)
    });

    // Wait until host has passed the initial gate + record_inbound and is blocked on upsert_trusted_peer.
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if host.peer_status().remote_node_id.as_deref() == Some(client_id.to_string().as_str())
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("host recorded inbound Ping peer before upsert");

    // Revoke the client inside the held write tx and commit so post-handshake upsert sees revoked_at.
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO trusted_peers (node_id, label, role, ticket_or_addrs, paired_at, last_seen_at, revoked_at)
         VALUES (?1, 'Revoked Race Peer', 'client', '', ?2, ?2, ?2)",
    )
    .bind(client_id.to_string())
    .bind(&now)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let (rpc_conn, response) = tokio::time::timeout(Duration::from_secs(3), ping_task)
        .await
        .expect("ping task completed")
        .unwrap();
    assert!(
        matches!(&response, WireResponse::Error { code, .. } if code == "PEER_REVOKED"),
        "expected PEER_REVOKED from post-handshake race branch, got {response:?}"
    );

    // Even though the client keeps rpc_conn and blob_conn open, the host must close both handles.
    tokio::time::timeout(Duration::from_secs(3), rpc_conn.closed())
        .await
        .expect("host closed Ping RPC connection");
    tokio::time::timeout(Duration::from_secs(3), blob_conn.closed())
        .await
        .expect("host closed active blob connection");

    assert!(!host.peer_status().connected);
    assert_eq!(host.peer_status().remote_node_id, None);
    assert!(changes.try_recv().is_err());
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if host.shared.active_connections.lock().unwrap().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("host cleared all active connections for revoked peer");

    drop(lock_store);
    host.shutdown().await.unwrap();
    client.shutdown().await.unwrap();
}
