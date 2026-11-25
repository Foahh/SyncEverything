use super::*;
use crate::contracts::{JsonValue, PatchKind};
use chrono::{DateTime, Duration};
use serde_json::json;

const REPO: &str = "forecast";

struct TempDb(PathBuf);

impl TempDb {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("relay-sprint1-{}", Uuid::now_v7()));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> PathBuf {
        self.0.join("relay.db")
    }

    async fn open(&self) -> RelayEngine {
        RelayEngine::open(REPO, self.path(), Some(self.0.clone())).await.unwrap()
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        for name in ["relay.db", "relay.db-wal", "relay.db-shm"] {
            let _ = std::fs::remove_file(self.0.join(name));
        }
        let _ = std::fs::remove_dir(&self.0);
    }
}

fn node(id: &str, payload: NodeKind) -> GraphNode {
    let now = Utc::now();
    GraphNode {
        id: id.into(),
        label: id.into(),
        payload,
        freshness: Freshness::Fresh,
        device_origin: "client".into(),
        created_at: now,
        updated_at: now,
    }
}

fn artifact(id: &str, sha: &str) -> GraphNode {
    node(id, NodeKind::Artifact {
        git_commit_sha: sha.into(),
        env_fingerprint: "uv-lock".into(),
        working_tree_dirty: false,
        changed_files: vec!["src/train.py".into()],
    })
}

fn decision(id: &str) -> GraphNode {
    node(id, NodeKind::Decision {
        rationale: "Compare model errors".into(),
        constraints: vec!["Use the smoke preset".into()],
    })
}

fn evidence(id: &str, task_id: TaskId) -> GraphNode {
    node(id, NodeKind::Evidence {
        task_id: task_id.to_string(),
        preset: "smoke".into(),
        metrics_json: JsonValue::from(json!({ "mae": 0.142 })),
        log_blob_hash: String::new(),
    })
}

fn edge(id: &str, from: &str, to: &str, kind: EdgeKind) -> GraphEdge {
    GraphEdge { id: id.into(), from: from.into(), to: to.into(), kind }
}

fn delta(nodes: Vec<GraphNode>, edges: Vec<GraphEdge>) -> DagDelta {
    DagDelta { repo_id: REPO.into(), upsert_nodes: nodes, upsert_edges: edges }
}

fn spec(task_id: TaskId, sha: &str) -> TaskSpec {
    TaskSpec {
        task_id,
        repo_id: REPO.into(),
        base_commit_sha: sha.into(),
        command_preset: "smoke".into(),
        params: BTreeMap::from([("window".into(), "14".into())]),
        require_approval: true,
        requested_by_device: "client".into(),
        user_note: Some("Compare forecast error".into()),
    }
}

async fn close(engine: RelayEngine) {
    engine.store.pool().close().await;
}

#[tokio::test]
async fn reloads_full_dag_and_latest_artifact_in_wal_mode() {
    let db = TempDb::new();
    let engine = db.open().await;
    assert_eq!(engine.repo_root(), Some(db.0.as_path()));
    let journal: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(engine.store.pool())
        .await
        .unwrap();
    assert_eq!(journal, "wal");

    let mut old = artifact("a-old", "old");
    old.updated_at -= Duration::seconds(1);
    let new = artifact("a-new", "new");
    engine.upsert_node_and_edges(old.clone(), vec![]).await.unwrap();
    let link = edge("informs", "decision", "a-new", EdgeKind::Informs);
    let before = engine.apply_dag_delta(delta(
        vec![new.clone(), decision("decision")],
        vec![edge("informs", "decision", "a-old", EdgeKind::Informs)],
    )).await.unwrap();
    assert_eq!(before.head_commit_sha.as_deref(), Some("new"));
    let before = engine.apply_dag_delta(delta(vec![], vec![link.clone()])).await.unwrap();
    assert_eq!(before.edges, vec![link]);

    engine.set_runtime_mode(RuntimeMode::Client).await.unwrap();
    close(engine).await;
    let reopened = db.open().await;
    let after = reopened.dag_snapshot(true).await.unwrap();
    assert_eq!(after.repo_id, REPO);
    assert_eq!(after.head_commit_sha, before.head_commit_sha);
    assert_eq!(after.nodes, before.nodes);
    assert_eq!(after.edges, before.edges);
    assert_eq!(after.stale_count, 0);
    assert_eq!(reopened.runtime_mode().await, RuntimeMode::Client);
    close(reopened).await;
}

#[tokio::test]
async fn supersession_persists_transitive_staleness_and_blocks_logs_after_reload() {
    let db = TempDb::new();
    let engine = db.open().await;
    let old = artifact("a-old", "old");
    engine.upsert_node_and_edges(old.clone(), vec![]).await.unwrap();

    let finished = Uuid::now_v7();
    engine.submit_task(spec(finished, "old")).await.unwrap();
    engine.transition_task(finished, TaskState::Executing, None, None).await.unwrap();
    let first = evidence("ev-z", finished);
    engine.record_evidence_result(finished, "a-old", first, "genuine old log\n")
        .await.unwrap();

    let queued = Uuid::now_v7();
    let unrelated = Uuid::now_v7();
    engine.submit_task(spec(queued, "old")).await.unwrap();
    engine.submit_task(spec(unrelated, "new")).await.unwrap();

    let downstream = evidence("ev-a", finished);
    engine.apply_dag_delta(delta(
        vec![
            artifact("a-later", "later"),
            decision("decision"),
            downstream.clone(),
            artifact("a-new", "new"),
            evidence("ev-fresh", unrelated),
        ],
        vec![
            edge("derived", "ev-z", "decision", EdgeKind::DerivedFrom),
            edge("informs", "decision", "a-later", EdgeKind::Informs),
            edge("produced-a", "a-later", "ev-a", EdgeKind::ProducedFrom),
            edge("produced-fresh", "a-new", "ev-fresh", EdgeKind::ProducedFrom),
        ],
    )).await.unwrap();
    assert_eq!(engine.read_evidence_log("ev-z", 10).await.unwrap().1, "genuine old log");

    let new = engine.dag_snapshot(true).await.unwrap().nodes.into_iter()
        .find(|node| node.id == "a-new").unwrap();
    let report = engine.supersede_artifact("a-old", new.clone(), "upstream edit").await.unwrap();
    assert_eq!(report.trigger_node_id, "a-new");
    assert_eq!(report.invalidated_evidence_ids, ["ev-a", "ev-z"]);
    assert!(engine.supersede_artifact("a-old", new, "replay").await.unwrap()
        .invalidated_evidence_ids.is_empty());
    engine.apply_dag_delta(delta(vec![downstream], vec![])).await.unwrap();

    let before = engine.dag_snapshot(true).await.unwrap();
    for id in ["ev-a", "ev-z"] {
        let evidence = before.nodes.iter().find(|node| node.id == id).unwrap();
        assert!(matches!(
            &evidence.freshness,
            Freshness::Stale { caused_by, reason, .. }
                if caused_by == "a-new" && reason == "upstream edit",
        ));
        assert!(matches!(
            engine.read_evidence_log(id, 10).await,
            Err(RelayError::StaleEvidenceBlocked { .. }),
        ));
    }
    assert_eq!(before.stale_count, 2);
    assert!(before.edges.contains(&supersedes_edge("a-old", "a-new")));
    let fresh = engine.dag_snapshot(false).await.unwrap();
    assert!(fresh.nodes.iter().any(|node| node.id == "ev-fresh"));
    assert!(!fresh.edges.iter().any(|edge| [edge.from.as_str(), edge.to.as_str()]
        .iter().any(|id| ["ev-a", "ev-z"].contains(id))));

    close(engine).await;
    let reopened = db.open().await;
    let after = reopened.dag_snapshot(true).await.unwrap();
    assert_eq!(after.nodes, before.nodes);
    assert_eq!(after.edges, before.edges);
    assert_eq!(after.head_commit_sha, before.head_commit_sha);
    assert_eq!(after.stale_count, before.stale_count);
    assert!(matches!(
        reopened.read_evidence_log("ev-z", 0).await,
        Err(RelayError::StaleEvidenceBlocked { node_id, caused_by, reason })
            if node_id == "ev-z" && caused_by == "a-new" && reason == "upstream edit",
    ));
    let stale = reopened.get_task(queued).await.unwrap().unwrap();
    assert_eq!(stale.state, TaskState::Stale);
    assert_eq!(stale.error_message.as_deref(), Some("upstream edit"));
    assert_eq!(reopened.get_task(finished).await.unwrap().unwrap().state, TaskState::Completed);
    assert_eq!(reopened.get_task(unrelated).await.unwrap().unwrap().state, TaskState::Queued);
    close(reopened).await;
}

#[tokio::test]
async fn edge_replacements_validate_the_finished_graph_atomically() {
    let db = TempDb::new();
    let engine = db.open().await;
    engine.apply_dag_delta(delta(
        vec![artifact("a", "one"), artifact("b", "two"), artifact("c", "three")],
        vec![edge("first", "a", "b", EdgeKind::Supersedes), edge("second", "b", "c", EdgeKind::Supersedes)],
    )).await.unwrap();
    let reversed = vec![edge("first", "c", "b", EdgeKind::Supersedes), edge("second", "b", "a", EdgeKind::Supersedes)];
    let before = engine.apply_dag_delta(delta(vec![], reversed.clone())).await.unwrap();
    assert_eq!(engine.apply_dag_delta(delta(vec![], reversed)).await.unwrap().edges, before.edges);
    assert!(engine.apply_dag_delta(delta(
        vec![decision("uncommitted")], vec![edge("cycle", "a", "c", EdgeKind::Supersedes)],
    )).await.is_err());
    close(engine).await;
    let reopened = db.open().await;
    let after = reopened.dag_snapshot(true).await.unwrap();
    assert_eq!(after.edges, before.edges);
    assert_eq!(after.nodes, before.nodes);
    close(reopened).await;
}

#[tokio::test]
async fn independent_engines_validate_reads_and_writes_against_the_same_database() {
    let db = TempDb::new();
    let writer = db.open().await;
    let reader = db.open().await;
    writer.upsert_node_and_edges(artifact("old", "a"), vec![]).await.unwrap();
    let id = Uuid::now_v7();
    reader.submit_task(spec(id, "a")).await.unwrap();
    reader.transition_task(id, TaskState::Executing, None, None).await.unwrap();
    reader.record_evidence_result(id, "old", evidence("metrics", id), "old metrics").await.unwrap();
    let (mut cached_metrics, _) = reader.read_evidence_log("metrics", 1).await.unwrap();

    writer.supersede_artifact("old", artifact("head", "b"), "new revision").await.unwrap();
    assert!(matches!(reader.read_evidence_log("metrics", 1).await, Err(RelayError::StaleEvidenceBlocked { .. })));
    cached_metrics.updated_at += Duration::days(1);
    reader.apply_dag_delta(delta(vec![cached_metrics], vec![])).await.unwrap();
    assert_eq!(reader.dag_snapshot(false).await.unwrap().stale_count, 1);
    assert_eq!(writer.dag_snapshot(false).await.unwrap().stale_count, 1);
    assert!(matches!(writer.read_evidence_log("metrics", 1).await, Err(RelayError::StaleEvidenceBlocked { .. })));

    reader.store.pool().close().await;
    assert!(reader.dag_snapshot(true).await.is_err());
    close(writer).await;
}

#[tokio::test]
async fn late_results_for_a_superseded_artifact_stay_quarantined() {
    let db = TempDb::new();
    let engine = db.open().await;
    engine.upsert_node_and_edges(artifact("old", "a"), vec![]).await.unwrap();
    engine.supersede_artifact("old", artifact("head", "b"), "new commit").await.unwrap();

    let id = Uuid::now_v7();
    let peer_result = evidence("from-peer", id);
    engine.apply_dag_delta(delta(
        vec![peer_result.clone(), evidence("current", id)],
        vec![
            edge("old-result", "old", "from-peer", EdgeKind::ProducedFrom),
            edge("head-result", "head", "current", EdgeKind::ProducedFrom),
        ],
    )).await.unwrap();
    engine.submit_task(spec(id, "a")).await.unwrap();
    engine.transition_task(id, TaskState::Executing, None, None).await.unwrap();
    engine.record_evidence_result(id, "old", evidence("local", id), "old metrics").await.unwrap();
    let historical_id = Uuid::now_v7();
    engine.submit_task(spec(historical_id, "unseen")).await.unwrap();
    engine.transition_task(historical_id, TaskState::Executing, None, None).await.unwrap();
    engine.record_worktree_evidence(
        historical_id, &artifact("historical", "unseen"), evidence("archived", historical_id), "older metrics",
    ).await.unwrap();
    assert_eq!(engine.head_commit_sha().await.unwrap().as_deref(), Some("b"));
    for result in ["from-peer", "local", "archived"] {
        assert!(matches!(engine.read_evidence_log(result, 10).await,
            Err(RelayError::StaleEvidenceBlocked { caused_by, .. }) if caused_by == "head"));
    }
    // An older installation could have persisted a fresh result after supersession.
    engine.store.update_nodes_freshness(&[peer_result]).await.unwrap();
    close(engine).await;
    let reopened = db.open().await;
    let snapshot = reopened.dag_snapshot(false).await.unwrap();
    assert_eq!(snapshot.stale_count, 3);
    assert!(snapshot.nodes.iter().any(|node| node.id == "current"));
    let (stored, _) = reopened.store.load_dag(REPO).await.unwrap();
    assert!(stored.iter().any(|node| node.id == "from-peer" && matches!(node.freshness, Freshness::Stale { .. })));
    close(reopened).await;
}

#[tokio::test]
async fn duplicate_uuidv7_replays_return_the_current_record_without_another_handoff() {
    let engine = RelayEngine::in_memory(REPO, None).await.unwrap();
    let id = Uuid::now_v7();
    let original = spec(id, "base");
    let (first, second) = tokio::join!(
        engine.submit_task(original.clone()),
        engine.submit_task(original.clone()),
    );
    let (first, second) = (first.unwrap(), second.unwrap());
    assert_ne!(first.1, second.1);
    assert_eq!(first.0, second.0);

    let current = engine.transition_task(id, TaskState::PreflightRunning, None, None).await.unwrap();
    let mut replay = original;
    replay.user_note = Some("A different payload".into());
    assert_eq!(engine.submit_task(replay).await.unwrap(), (current.clone(), false));
    assert_eq!(engine.list_tasks().await.unwrap(), vec![current]);

    let handoffs = engine.list_handoffs(20).await.unwrap();
    assert_eq!(handoffs.len(), 1);
    assert_eq!(handoffs[0].handoff_id, id.to_string());
    assert_eq!(handoffs[0].source_device, "client");
    assert_eq!(handoffs[0].active_task_count, 1);
    assert_eq!(handoffs[0].summary, "Compare forecast error");
    assert!(engine.list_handoffs(0).await.unwrap().is_empty());

    let (generated, inserted) = engine.submit_task(spec(Uuid::nil(), "base")).await.unwrap();
    assert!(inserted);
    assert_eq!(generated.spec.task_id.get_version_num(), 7);
    assert_eq!(engine.list_tasks().await.unwrap()[0], generated);
}

#[tokio::test]
async fn lifecycle_persists_patch_approval_evidence_and_utf8_log() {
    let db = TempDb::new();
    let engine = db.open().await;
    let id = Uuid::now_v7();
    let (queued, inserted) = engine.submit_task(spec(id, "base")).await.unwrap();
    assert!(inserted);
    assert_eq!(queued.state, TaskState::Queued);
    let log = "first\r\nsecond\r\nlast: \u{6d4b}\u{8bd5}\r\n";
    let hash = blake3::hash(log.as_bytes()).to_hex().to_string();
    engine.upsert_node_and_edges(artifact("artifact", "base"), vec![]).await.unwrap();
    assert!(matches!(
        engine.record_evidence_result(id, "artifact", evidence("ev", id), log).await,
        Err(RelayError::InvalidStateTransition { .. }),
    ));
    assert_eq!(engine.store.get_log_blob(&hash).await.unwrap(), None);
    assert_eq!(engine.dag_snapshot(true).await.unwrap().nodes.len(), 1);

    let preflight = engine.transition_task(id, TaskState::PreflightRunning, Some("missing scipy".into()), None)
        .await.unwrap();
    assert_eq!(preflight.state, TaskState::PreflightRunning);
    let patch = PatchProposal {
        patch_id: "patch-1".into(),
        task_id: id,
        kind: PatchKind::DependencySpec,
        summary: "Add scipy".into(),
        diff_preview: "+ scipy".into(),
        worktree_branch: "relay/task-1".into(),
        smoke_verified: true,
        smoke_stdout_tail: "ok".into(),
        created_at: Utc::now(),
    };
    let proposed = engine.attach_patch_proposal(patch.clone()).await.unwrap();
    assert_eq!(proposed.state, TaskState::SelfHealingProposed);
    assert_eq!(proposed.active_proposal.as_ref(), Some(&patch));
    assert!(matches!(
        engine.resolve_approval(id, Some("wrong"), ApprovalDecision::Approved).await,
        Err(RelayError::ProposalNotFound(patch_id)) if patch_id == "wrong",
    ));
    assert_eq!(engine.get_task(id).await.unwrap(), Some(proposed.clone()));

    let approved = engine.resolve_approval(id, Some("patch-1"), ApprovalDecision::Approved)
        .await.unwrap();
    assert_eq!(approved.state, TaskState::Executing);
    let completed = engine.record_evidence_result(id, "artifact", evidence("ev", id), log)
        .await.unwrap();
    assert_eq!(completed.state, TaskState::Completed);
    assert_eq!(completed.evidence_node_id.as_deref(), Some("ev"));
    assert_eq!(completed.preflight_summary.as_deref(), Some("missing scipy"));
    assert_eq!(completed.active_proposal, Some(patch));
    assert!(matches!(
        engine.transition_task(id, TaskState::Executing, None, None).await,
        Err(RelayError::InvalidStateTransition { task_id, from, to })
            if task_id == id.to_string() && from == "Completed" && to == "Executing",
    ));
    assert_eq!(engine.dag_snapshot(true).await.unwrap().edges, [
        edge("produced_from:artifact->ev", "artifact", "ev", EdgeKind::ProducedFrom),
    ]);

    close(engine).await;
    let reopened = db.open().await;
    assert_eq!(reopened.get_task(id).await.unwrap(), Some(completed));
    let (node, tail) = reopened.read_evidence_log("ev", 2).await.unwrap();
    assert_eq!(tail, "second\nlast: \u{6d4b}\u{8bd5}");
    assert!(matches!(node.payload, NodeKind::Evidence { log_blob_hash, .. } if log_blob_hash == hash));
    assert_eq!(reopened.read_evidence_log("ev", 0).await.unwrap().1, "");
    assert_eq!(reopened.store.get_log_blob(&hash).await.unwrap().as_deref(), Some(log));
    close(reopened).await;
}

#[tokio::test]
async fn outbox_survives_reopen_and_delivers_only_in_its_repository() {
    let db = TempDb::new();
    let engine = db.open().await;
    let other = RelayEngine::open("other", db.path(), None).await.unwrap();
    let payload = json!({"type": "dag_delta", "nodes": ["a"]});
    let first = engine.enqueue_outbox(&payload).await.unwrap();
    let second = engine.enqueue_outbox(&json!({"type": "task"})).await.unwrap();
    let foreign = other.enqueue_outbox(&json!({"type": "bundle"})).await.unwrap();
    assert!(first < second && second < foreign);
    assert_eq!(engine.pending_outbox_count().await.unwrap(), 2);
    assert_eq!(engine.store.count_pending_outbox().await.unwrap(), 3);

    engine.mark_outbox_delivered(foreign).await.unwrap();
    engine.mark_outbox_delivered(first).await.unwrap();
    engine.mark_outbox_delivered(first).await.unwrap();
    assert_eq!(other.pending_outbox_count().await.unwrap(), 1);
    close(other).await;
    close(engine).await;

    let reopened = db.open().await;
    assert_eq!(reopened.pending_outbox_count().await.unwrap(), 1);
    assert_eq!(reopened.store.list_pending_outbox(Some(REPO)).await.unwrap(), [
        (second, REPO.into(), json!({"type": "task"})),
    ]);
    reopened.mark_outbox_delivered(second).await.unwrap();
    assert_eq!(reopened.pending_outbox_count().await.unwrap(), 0);
    assert_eq!(reopened.store.count_pending_outbox().await.unwrap(), 1);
    close(reopened).await;
}

#[tokio::test]
async fn failed_database_write_keeps_dag_and_task_unchanged() {
    let engine = RelayEngine::in_memory(REPO, None).await.unwrap();
    engine.upsert_node_and_edges(artifact("artifact", "base"), vec![]).await.unwrap();
    let id = Uuid::now_v7();
    engine.submit_task(spec(id, "base")).await.unwrap();
    engine.transition_task(id, TaskState::Executing, None, None).await.unwrap();

    engine.store.upsert_dag_edge(
        "other",
        &edge("produced_from:artifact->ev", "other-a", "other-e", EdgeKind::ProducedFrom),
    ).await.unwrap();
    let result = engine.record_evidence_result(id, "artifact", evidence("ev", id), "log").await;
    assert!(result.is_err());
    let (nodes, edges) = engine.store.load_dag(REPO).await.unwrap();
    assert_eq!(nodes.len(), 1);
    assert!(edges.is_empty());
    assert_eq!(engine.dag_snapshot(true).await.unwrap().nodes, nodes);
    assert_eq!(engine.get_task(id).await.unwrap().unwrap().state, TaskState::Executing);
    assert!(engine.store.get_log_blob(&blake3::hash(b"log").to_hex().to_string())
        .await.unwrap().is_none());
}

#[tokio::test]
async fn delta_updates_invalidate_evidence_and_reject_missing_edge_endpoints() {
    let engine = RelayEngine::in_memory(REPO, None).await.unwrap();
    let source = artifact("artifact", "old");
    engine.upsert_node_and_edges(source.clone(), vec![]).await.unwrap();
    let mut updated = artifact("artifact", "new");
    updated.updated_at = source.updated_at + Duration::seconds(1);
    let link = edge("produced", "artifact", "ev", EdgeKind::ProducedFrom);
    let invalid = engine.apply_dag_delta(delta(vec![updated.clone()], vec![link.clone()])).await;
    assert!(matches!(invalid, Err(RelayError::NodeNotFound(id)) if id == "ev"));
    assert_eq!(engine.store.load_dag(REPO).await.unwrap().0, [source]);

    let ev = evidence("ev", Uuid::now_v7());
    let updated = engine.apply_dag_delta(delta(vec![updated, ev.clone()], vec![link])).await.unwrap();
    assert_eq!(updated.head_commit_sha.as_deref(), Some("new"));
    assert_eq!(updated.stale_count, 1);
    let mut replay = ev;
    replay.updated_at = DateTime::from_timestamp(4_000_000_000, 0).unwrap();
    let replayed = engine.apply_dag_delta(delta(vec![replay], vec![])).await.unwrap();
    assert_eq!(replayed.stale_count, 1);
    assert!(matches!(
        engine.read_evidence_log("ev", 100).await,
        Err(RelayError::StaleEvidenceBlocked { .. }),
    ));
}

#[tokio::test]
async fn desktop_mode_and_queued_peer_decisions_survive_restart() {
    let db = TempDb::new();
    let engine = RelayEngine::open_with_default_mode(
        REPO,
        db.path(),
        None,
        RuntimeMode::LocalDualSandbox,
    )
    .await
    .unwrap();
    assert_eq!(engine.runtime_mode().await, RuntimeMode::LocalDualSandbox);
    engine.set_runtime_mode(RuntimeMode::Client).await.unwrap();

    let id = Uuid::now_v7();
    let spec = spec(id, "base");
    let queued = engine.queue_peer_task(spec.clone(), None).await.unwrap();
    assert_eq!(engine.queue_peer_task(spec, None).await.unwrap(), queued);
    let decision = engine
        .queue_peer_approval(id, None, ApprovalDecision::Rejected, None)
        .await
        .unwrap();
    assert_eq!(decision.state, TaskState::Rejected);
    assert_eq!(engine.observed_outbox_count(), 2);
    close(engine).await;

    let engine = db.open().await;
    assert_eq!(engine.runtime_mode().await, RuntimeMode::Client);
    assert_eq!(engine.get_task(id).await.unwrap(), Some(decision));
    assert_eq!(engine.list_handoffs(10).await.unwrap().len(), 1);
    let outbox = engine.list_pending_outbox().await.unwrap();
    assert!(matches!(
        serde_json::from_value::<WireRequest>(outbox[0].2.clone()).unwrap(),
        WireRequest::SubmitTask { spec } if spec.task_id == id
    ));
    assert!(matches!(
        serde_json::from_value::<WireRequest>(outbox[1].2.clone()).unwrap(),
        WireRequest::ResolveApproval { task_id, decision: ApprovalDecision::Rejected, .. }
            if task_id == id
    ));
    close(engine).await;
}

#[tokio::test]
async fn cancellation_handles_queued_active_raii_cleanup_and_persisted_partial_logs() {
    let db = TempDb::new();
    let engine = db.open().await;

    // 1) Queued task cancels immediately.
    let queued_id = Uuid::now_v7();
    engine.submit_task(spec(queued_id, "base")).await.unwrap();
    let cancelled_queued = engine.cancel_task(queued_id).await.unwrap();
    assert_eq!(cancelled_queued.state, TaskState::Cancelled);

    // 2) A cancellation replay acknowledges the original result without another outbox entry.
    assert_eq!(engine.cancel_task(queued_id).await.unwrap(), cancelled_queued);
    let offline_id = Uuid::now_v7();
    engine.queue_peer_task(spec(offline_id, "base"), None).await.unwrap();
    let offline = engine.queue_peer_cancel(offline_id, None).await.unwrap();
    assert_eq!(engine.queue_peer_cancel(offline_id, None).await.unwrap(), offline);
    assert_eq!(engine.observed_outbox_count(), 2);
    assert!(matches!(
        engine.cancel_task(Uuid::now_v7()).await,
        Err(RelayError::TaskNotFound(_))
    ));

    // 3) Registering a cancellation guard on an already-cancelled task fails immediately
    //    and leaves no leaked registry handle.
    assert!(matches!(
        engine.register_task_cancellation(queued_id).await,
        Err(RelayError::TaskCancelled(_))
    ));
    assert_eq!(engine.active_cancellation_count(), 0);

    // 4) Active executing task receives signal, cleans up handle on drop, and persists partial log.
    let running_id = Uuid::now_v7();
    engine.submit_task(spec(running_id, "base")).await.unwrap();
    engine
        .transition_task(running_id, TaskState::Executing, None, None)
        .await
        .unwrap();
    let guard = engine.register_task_cancellation(running_id).await.unwrap();
    assert!(engine.has_active_cancellation(running_id));
    assert_eq!(engine.active_cancellation_count(), 1);
    assert!(!guard.token().is_cancelled());

    let cancelled_running = engine.cancel_task(running_id).await.unwrap();
    assert_eq!(cancelled_running.state, TaskState::Cancelled);
    assert!(guard.token().is_cancelled());
    drop(guard);
    assert!(!engine.has_active_cancellation(running_id));
    assert_eq!(engine.active_cancellation_count(), 0);

    let partial = "epoch 1 loss=0.42\nwarning: shutting down\n";
    let (updated, hash) = engine
        .record_cancelled_task_log(running_id, partial)
        .await
        .unwrap();
    assert_eq!(updated.state, TaskState::Cancelled);
    assert_eq!(hash, blake3::hash(partial.as_bytes()).to_hex().to_string());
    assert_eq!(
        updated.preflight_summary.as_deref(),
        Some("Cancelled by user")
    );
    close(engine).await;

    let reopened = db.open().await;
    let stored = reopened.read_task_log(running_id).await.unwrap().unwrap();
    assert_eq!(stored, (hash, partial.to_owned()));
    close(reopened).await;
}

#[tokio::test]
async fn cancellation_written_by_another_engine_stops_the_running_operation() {
    let db = TempDb::new();
    let worker = db.open().await;
    let controller = db.open().await;
    let task_id = Uuid::now_v7();
    worker.submit_task(spec(task_id, "base")).await.unwrap();
    let guard = worker.register_task_cancellation(task_id).await.unwrap();
    let mut token = guard.token();
    assert!(!token.is_cancelled());

    controller.cancel_task(task_id).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), token.cancelled())
        .await
        .expect("the worker did not observe the persisted cancellation");
    assert!(token.is_cancelled());
    drop(guard);
    close(worker).await;
    close(controller).await;
}
