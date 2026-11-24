use super::*;
use crate::contracts::{EdgeKind, FileEdit, RuntimeMode, TaskSpec};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tokio::process::Command;

const REPO_ID: &str = "test-forecast";
const MANIFEST: &str = r#"
[project]
repo_id = "test-forecast"
name = "MCP experiment fixture"

[presets.smoke]
description = "Run smoke"
program = "uv"
args = ["run", "--offline", "python", "src/app.py", "--smoke"]
timeout_secs = 30

[presets.train_grid]
description = "Emit grid metrics"
program = "uv"
args = ["run", "--offline", "python", "src/app.py", "--alpha", "0.5"]
allowed_params = ["alpha"]
timeout_secs = 30
"#;

const PYPROJECT: &str = r#"
[project]
name = "relay-mcp-fixture"
version = "0.1.0"
requires-python = ">=3.9"
dependencies = []

[tool.uv]
package = false
"#;

const PYTHON: &str = r#"import argparse
import json

parser = argparse.ArgumentParser()
parser.add_argument("--smoke", action="store_true")
parser.add_argument("--alpha", type=float)
args = parser.parse_args()
if args.smoke:
    print(SMOKE_MESSAGE)
else:
    print(json.dumps({"alpha": args.alpha, "smoke": SMOKE_MESSAGE}))
    print("training done")
"#;

fn fix_edits() -> Vec<FileEdit> {
    vec![FileEdit {
        file_path: "src/app.py".into(),
        old_text: Some("import argparse\nimport json\n".into()),
        new_text: "import argparse\nimport json\n\nSMOKE_MESSAGE = \"smoke passed\"\n".into(),
    }]
}

struct Fixture {
    temp: PathBuf,
    repo: PathBuf,
    sandbox: WorktreeSandbox,
    engine: Arc<RelayEngine>,
    sha: String,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_script(PYTHON).await
    }

    async fn with_script(script: &str) -> Self {
        let temp = std::env::temp_dir().join(format!("relay-mcp-test-{}", Uuid::now_v7()));
        let repo = temp.join("main");
        fs::create_dir_all(repo.join("src")).unwrap();
        git(&repo, &["init", "--initial-branch=main"]).await;
        git(&repo, &["config", "core.autocrlf", "false"]).await;
        fs::write(repo.join("src/app.py"), script).unwrap();
        fs::write(repo.join("pyproject.toml"), PYPROJECT).unwrap();
        fs::write(repo.join("relay.toml"), MANIFEST).unwrap();
        fs::write(repo.join(".gitignore"), ".venv/\n__pycache__/\n").unwrap();
        git(&repo, &["add", "--all"]).await;
        let tree = git(&repo, &["write-tree"]).await;
        let sha = git(
            &repo,
            &[
                "-c",
                "user.name=Relay Test",
                "-c",
                "user.email=relay-test@example.invalid",
                "commit-tree",
                &tree,
                "-m",
                "MCP test fixture",
            ],
        )
        .await;
        git(&repo, &["update-ref", "HEAD", &sha]).await;

        let sandbox = WorktreeSandbox::new(&repo, temp.join("worktrees"));
        let engine = Arc::new(RelayEngine::in_memory(REPO_ID, Some(repo.clone())).await.unwrap());
        Self { temp, repo, sandbox, engine, sha }
    }

    fn server(&self) -> RelayMcpServer {
        RelayMcpServer::new(self.engine.clone(), Some(self.sandbox.clone()), None)
    }

    async fn submit(&self, alpha: &str, require_approval: bool) -> TaskId {
        let id = Uuid::now_v7();
        let (_, inserted) = self
            .engine
            .submit_task(TaskSpec {
                task_id: id,
                repo_id: REPO_ID.into(),
                base_commit_sha: self.sha.clone(),
                command_preset: "train_grid".into(),
                params: BTreeMap::from([("alpha".into(), alpha.into())]),
                require_approval,
                requested_by_device: "fixture-client".into(),
                user_note: Some("Run the fixture grid".into()),
            })
            .await
            .unwrap();
        assert!(inserted);
        id
    }

    async fn task(&self, id: TaskId) -> TaskRecord {
        self.engine.get_task(id).await.unwrap().unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.temp);
    }
}

async fn git(repo: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo).args(args);
    for key in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR"] {
        command.env_remove(key);
    }
    let output = command.output().await.unwrap();
    assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

fn approval(id: TaskId, patch_id: Option<String>, approved_via_ui: bool) -> ApproveAndExecuteArgs {
    ApproveAndExecuteArgs {
        task_id: id.to_string(),
        patch_id,
        approved_via_ui: Some(approved_via_ui),
    }
}

#[tokio::test]
async fn host_clean_preflight_obeys_task_approval() {
    use crate::agent::{HostTaskOutcome, LlmEndpointConfig, run_host_task_pipeline};
    let fixture = Fixture::with_script(&format!("SMOKE_MESSAGE = 'ok'\n{PYTHON}")).await;
    let server = fixture.server();
    for require_approval in [true, false] {
        let id = fixture.submit("0.25", require_approval).await;
        let outcome = run_host_task_pipeline(&fixture.engine, &server, LlmEndpointConfig::default(), id).await.unwrap();
        if require_approval {
            assert!(matches!(outcome, HostTaskOutcome::AwaitingApproval));
            let task = fixture.task(id).await;
            assert_eq!(task.state, TaskState::PreflightRunning);
            assert!(task.evidence_node_id.is_none());
            server.handle_approve_and_execute(approval(id, None, true), None).await.unwrap();
        } else {
            assert!(matches!(outcome, HostTaskOutcome::Completed));
        }
        assert_eq!(fixture.task(id).await.state, TaskState::Completed);
    }
}

#[tokio::test]
async fn mcp_end_to_end_preflight_patch_approval_and_stale_block() {
    let fixture = Fixture::new().await;
    let task_id = fixture.submit("0.25", true).await;
    let server = fixture.server();

    fixture.engine.enqueue_outbox(&serde_json::json!({"type": "ping"})).await.unwrap();
    let status = server.handle_inspect_sync_status().await.unwrap();
    assert_eq!(status.mode, RuntimeMode::Host);
    assert_eq!(status.transport_path, TransportPath::Offline);
    assert_eq!(status.pending_outbox_count, 1);

    let handoffs = server.handle_list_handoffs(ListHandoffsArgs::default()).await.unwrap();
    assert_eq!(handoffs[0].handoff_id, task_id.to_string());
    let diff = server
        .handle_check_env_diff(CheckEnvDiffArgs { commit_sha: fixture.sha.clone() })
        .await
        .unwrap();
    assert!(diff.in_sync);
    assert_eq!(diff.ecosystem, "python");
    assert_eq!(diff.package_manager, "uv");

    let report = server
        .handle_run_preflight(RunPreflightArgs { task_id: task_id.to_string() })
        .await
        .unwrap();
    assert!(!report.smoke_passed);
    assert!(report.stderr_tail.contains("NameError"), "{}", report.raw_log);
    assert!(!report.error_snippets.is_empty());
    let task = fixture.task(task_id).await;
    assert_eq!(task.state, TaskState::PreflightRunning);
    assert!(task.preflight_summary.unwrap().contains("failed"));

    let worktree_file = server
        .handle_read_worktree_file(ReadWorktreeFileArgs {
            task_id: task_id.to_string(),
            file_path: "src/app.py".into(),
            start_line: Some(1),
            end_line: Some(15),
        })
        .await
        .unwrap();
    assert_eq!(worktree_file.file_path, "src/app.py");
    assert!(worktree_file.content.contains("print(SMOKE_MESSAGE)"));

    // Failed smoke verification rolls back worktree cleanly and allows an immediate retry.
    let failed_patch = server
        .handle_propose_patch(ProposeWorktreePatchArgs {
            task_id: task_id.to_string(),
            patch_kind: "code_diff".into(),
            summary: "Incomplete edit that still fails smoke".into(),
            dependency_spec: None,
            edits: Some(vec![FileEdit {
                file_path: "src/app.py".into(),
                old_text: Some("import json\n".into()),
                new_text: "import json\nWRONG_CONST = 'nope'\n".into(),
            }]),
        })
        .await;
    assert!(failed_patch.is_err());
    assert!(report.worktree_path.is_dir());

    let marker = report.worktree_path.join(".preflight-marker");
    fs::write(&marker, "reuse this worktree").unwrap();

    let patch = server
        .handle_propose_patch(ProposeWorktreePatchArgs {
            task_id: task_id.to_string(),
            patch_kind: "code_diff".into(),
            summary: "Define the smoke message".into(),
            dependency_spec: None,
            edits: Some(fix_edits()),
        })
        .await
        .unwrap();
    assert!(patch.smoke_verified);
    assert_eq!(patch.worktree_branch, report.worktree_branch);
    assert!(marker.is_file());
    let proposed = fixture.task(task_id).await;
    assert_eq!(proposed.state, TaskState::SelfHealingProposed);
    assert_eq!(proposed.active_proposal.as_ref(), Some(&patch));

    let pending = server
        .handle_approve_and_execute(approval(task_id, None, false), None)
        .await
        .unwrap();
    assert_eq!(
        pending,
        ApprovalExecutionResponse::ApprovalRequired {
            task_id: task_id.to_string(),
            patch_id: Some(patch.patch_id.clone()),
            summary: patch.summary.clone(),
            diff_preview: patch.diff_preview.clone(),
        }
    );
    assert_eq!(fixture.task(task_id).await, proposed);
    assert!(matches!(
        server.handle_approve_and_execute(approval(task_id, Some("wrong".into()), true), None).await,
        Err(RelayError::ProposalNotFound(_))
    ));

    // A new MCP server can use the pending proposal and existing Git worktree.
    drop(server);
    let server = fixture.server();
    let approved = approval(task_id, Some(patch.patch_id.clone()), true);
    let response = server.handle_approve_and_execute(approved.clone(), None).await.unwrap();
    assert_eq!(
        response,
        ApprovalExecutionResponse::ExecutionStarted {
            task_id: task_id.to_string(),
            worktree_branch: report.worktree_branch.clone(),
        }
    );
    let completed = fixture.task(task_id).await;
    assert_eq!(completed.state, TaskState::Completed);
    assert_eq!(
        server.handle_approve_and_execute(approved, None).await.unwrap(),
        response,
    );
    assert_eq!(fixture.task(task_id).await, completed);
    let node_id = completed.evidence_node_id.unwrap();
    assert!(!report.worktree_path.exists());
    assert!(server.context.active_worktrees.lock().await.is_empty());
    assert!(git(&fixture.repo, &["branch", "--list", &report.worktree_branch]).await.is_empty());
    assert!(!fixture.sandbox.is_working_tree_dirty().await.unwrap());
    assert_eq!(fixture.sandbox.head_commit_sha().await.unwrap(), fixture.sha);
    assert_eq!(fs::read_to_string(fixture.repo.join("src/app.py")).unwrap(), PYTHON);

    let snapshot = server.handle_get_dag_snapshot(GetDagSnapshotArgs::default()).await.unwrap();
    let artifact = snapshot
        .nodes
        .iter()
        .find(|node| {
            matches!(&node.payload, NodeKind::Artifact { git_commit_sha, .. } if git_commit_sha == &fixture.sha)
        })
        .unwrap();
    assert_eq!(snapshot.stale_count, 0);
    assert_eq!(snapshot.fresh_evidence_count, 1);
    assert_eq!(snapshot.stale_evidence_count, 0);
    assert!(snapshot.metric_authority_note.is_none());
    assert!(snapshot.edges.iter().any(|edge| {
        edge.kind == EdgeKind::ProducedFrom && edge.from == artifact.id && edge.to == node_id
    }));
    assert_eq!(
        server.handle_inspect_sync_status().await.unwrap().last_synced_commit,
        Some(fixture.sha.clone()),
    );

    let read = ReadEvidenceLogArgs { node_id: node_id.clone(), max_lines: Some(1) };
    let fresh = server.read_evidence_log(Parameters(read.clone())).await.unwrap();
    assert_eq!(fresh.is_error, Some(false));
    let fresh: EvidenceLog = serde_json::from_str(&fresh.content[0].as_text().unwrap().text).unwrap();
    assert_eq!(fresh.log_tail.lines().count(), 1);
    let full_log = server
        .handle_read_evidence_log(ReadEvidenceLogArgs {
            node_id: node_id.clone(),
            max_lines: None,
        })
        .await
        .unwrap();
    assert!(full_log.log_tail.contains("training done"));
    assert_eq!(full_log.log_tail.lines().last(), Some(fresh.log_tail.as_str()));
    assert_eq!(fresh.node.freshness, Freshness::Fresh);
    let NodeKind::Evidence { metrics_json, log_blob_hash, .. } = fresh.node.payload else {
        panic!("expected evidence payload");
    };
    assert_eq!(metrics_json, serde_json::json!({"alpha": 0.25, "smoke": "smoke passed"}).into());
    assert!(!log_blob_hash.is_empty());

    let mut next = artifact.clone();
    next.id = "artifact:new".into();
    let NodeKind::Artifact { git_commit_sha, .. } = &mut next.payload else {
        panic!("expected artifact payload");
    };
    *git_commit_sha = "new-commit".into();
    fixture.engine.supersede_artifact(&artifact.id, next, "upstream edit").await.unwrap();

    let stale = server.read_evidence_log(Parameters(read)).await.unwrap();
    assert_eq!(stale.is_error, Some(true));
    let error = &stale.content[0].as_text().unwrap().text;
    assert!(error.contains("STALE"));
    assert!(!error.contains("0.25"));

    let stale_snapshot = server
        .handle_get_dag_snapshot(GetDagSnapshotArgs::default())
        .await
        .unwrap();
    assert_eq!(stale_snapshot.stale_count, 1);
    assert_eq!(stale_snapshot.fresh_evidence_count, 0);
    assert_eq!(stale_snapshot.stale_evidence_count, 1);
    assert!(stale_snapshot.metric_authority_note.is_some());
    let stale_node = stale_snapshot.nodes.iter().find(|node| node.id == node_id).unwrap();
    assert!(matches!(stale_node.freshness, Freshness::Stale { .. }));
    let NodeKind::Evidence { metrics_json: redacted_metrics, .. } = &stale_node.payload else {
        panic!("expected evidence payload");
    };
    assert_eq!(
        *redacted_metrics,
        serde_json::json!({
            "quarantined": true,
            "reason": "stale_evidence_metrics_redacted",
        })
        .into()
    );
    let serialized_stale = serde_json::to_string(&stale_snapshot).unwrap();
    assert!(serialized_stale.contains("stale_evidence_metrics_redacted"));
    assert!(!serialized_stale.contains("0.25"));
    assert!(!serialized_stale.contains("smoke passed"));

    let raw_snapshot = fixture.engine.dag_snapshot(true).await.unwrap();
    let raw_node = raw_snapshot.nodes.iter().find(|node| node.id == node_id).unwrap();
    let NodeKind::Evidence { metrics_json: raw_metrics, .. } = &raw_node.payload else {
        panic!("expected evidence payload");
    };
    assert_eq!(*raw_metrics, serde_json::json!({"alpha": 0.25, "smoke": "smoke passed"}).into());
}

#[tokio::test]
async fn mcp_rejection_and_failed_preset_clean_up_worktrees() {
    let fixture = Fixture::new().await;
    let server = fixture.server();
    let rejected_id = fixture.submit("0.25", true).await;
    let report = server
        .handle_run_preflight(RunPreflightArgs { task_id: rejected_id.to_string() })
        .await
        .unwrap();
    let rejected = server
        .context
        .finish_approval(fixture.task(rejected_id).await, ApprovalDecision::Rejected)
        .await
        .unwrap();
    assert!(matches!(rejected, ApprovalExecutionResponse::Rejected { .. }));
    assert_eq!(fixture.task(rejected_id).await.state, TaskState::Rejected);
    assert!(!report.worktree_path.exists());

    let failed_id = fixture.submit("invalid-number", false).await;
    let error = server.handle_approve_and_execute(approval(failed_id, None, false), None).await;
    assert!(error.unwrap_err().to_string().contains("train_grid"));
    let failed = fixture.task(failed_id).await;
    assert_eq!(failed.state, TaskState::Failed);
    assert!(failed.error_message.unwrap().contains("invalid-number"));
    assert!(failed.evidence_node_id.is_none());
    assert!(!fixture.sandbox.task_worktree(failed_id).unwrap().0.exists());
    assert!(server.context.active_worktrees.lock().await.is_empty());
    assert!(!fixture.sandbox.is_working_tree_dirty().await.unwrap());
}
