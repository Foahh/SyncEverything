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

    let offline = EndpointAddr::new(iroh::SecretKey::generate().public());
    assert!(client.flush_outbox(offline).await.is_err());
    assert_eq!(client.peer_status().pending_outbox_count, 1);
    assert!(!client.peer_status().connected);

    let (host, host_engine) = start_local(&temp, "host-blobs", None).await;
    assert_eq!(client.flush_outbox(host.addr()).await.unwrap(), 1);
    assert_eq!(client.peer_status().pending_outbox_count, 0);
    assert_eq!(client_engine.pending_outbox_count().await.unwrap(), 0);
    assert_eq!(client.flush_outbox(host.addr()).await.unwrap(), 0);
    assert_delta(&host_engine.dag_snapshot(true).await.unwrap(), &delta, COMMIT);

    client.shutdown().await.unwrap();
    host.shutdown().await.unwrap();
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
