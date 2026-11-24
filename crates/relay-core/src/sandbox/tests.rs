use super::*;
use crate::contracts::{JsonValue, PatchKind};
use std::collections::BTreeMap;
use std::fs;
use uuid::Uuid;

const MANIFEST: &str = r#"
[project]
repo_id = "forecast-bench"
name = "Forecasting Benchmark"
smoke_preset = "smoke"

[presets.smoke]
description = "3-second smoke check"
program = "uv"
args = ["run", "python", "src/app.py", "--smoke"]
timeout_secs = 30
allowed_params = ["window"]

[presets.train_grid]
description = "Run a parameter grid"
program = "uv"
args = ["run", "python", "src/app.py", "--windows", "7,14,28", "--alpha=0.5"]
timeout_secs = 30
allowed_params = ["windows", "alpha"]
"#;

const PYPROJECT: &str = r#"
[project]
name = "relay-sandbox-fixture"
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
parser.add_argument("--window")
parser.add_argument("--windows", action="append")
parser.add_argument("--alpha", type=float, action="append")
args = parser.parse_args()
if args.smoke:
    print(SMOKE_MESSAGE)
else:
    print("Starting training")
    print(json.dumps({"windows": args.windows, "alpha": args.alpha}, indent=2))
    print("Training finished")
"#;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("relay-sandbox-test-{}", Uuid::now_v7()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

async fn git(path: &Path, args: &[&str]) -> String {
    let output = git_output(git_command(path).args(args)).await.unwrap();
    output_text(&output.stdout)
}

async fn init_repo(path: &Path) {
    fs::create_dir_all(path).unwrap();
    git(path, &["init", "--initial-branch=main"]).await;
    git(path, &["config", "core.autocrlf", "false"]).await;
}

async fn create_initial_commit(path: &Path) -> String {
    git(path, &["add", "--all"]).await;
    let tree = git(path, &["write-tree"]).await;
    let sha = git(
        path,
        &[
            "-c",
            "user.name=Relay Test",
            "-c",
            "user.email=relay-test@example.invalid",
            "commit-tree",
            &tree,
            "-m",
            "Initial sandbox fixture",
        ],
    )
    .await;
    git(path, &["update-ref", "HEAD", &sha]).await;
    sha
}

async fn init_python_repo(path: &Path) -> String {
    init_repo(path).await;
    fs::create_dir(path.join("src")).unwrap();
    fs::write(path.join("src/app.py"), PYTHON).unwrap();
    fs::write(path.join("pyproject.toml"), PYPROJECT).unwrap();
    fs::write(path.join("relay.toml"), MANIFEST).unwrap();
    fs::write(path.join(".gitignore"), ".venv/\n__pycache__/\n").unwrap();
    create_initial_commit(path).await
}

#[test]
fn parses_relay_manifest_and_enforces_whitelist() {
    let manifest: RelayManifest = toml::from_str(MANIFEST).unwrap();
    assert!(resolve_preset(&manifest, "smoke").is_ok());
    assert!(matches!(
        resolve_preset(&manifest, "rm_rf"),
        Err(RelayError::UnknownPreset(_))
    ));
}

#[tokio::test]
async fn rejects_disallowed_preset_params_and_overrides_allowed_flags() {
    let temp = TempDir::new();
    let repo = temp.join("main");
    let base_commit = init_python_repo(&repo).await;
    let sandbox = WorktreeSandbox::new(&repo, temp.join("worktrees"));
    let manifest = load_manifest(&repo).unwrap();
    let task_id = Uuid::now_v7();
    let worktree = sandbox
        .prepare_worktree(task_id, &base_commit)
        .await
        .unwrap();

    let disallowed = BTreeMap::from([("output".into(), "unexpected".into())]);
    let error = sandbox
        .run_preset(worktree.worktree_path(), &manifest, "train_grid", &disallowed)
        .await
        .unwrap_err();
    assert!(matches!(error, RelayError::Internal(message) if message.contains("output")));

    let mut altered = manifest.clone();
    altered.presets.get_mut("train_grid").unwrap().args.clear();
    let error = sandbox
        .run_preset(worktree.worktree_path(), &altered, "train_grid", &BTreeMap::new())
        .await
        .unwrap_err();
    assert!(matches!(error, RelayError::Internal(message) if message.contains("host relay.toml")));

    let mut alternate_smoke = manifest.clone();
    alternate_smoke.project.smoke_preset = "train_grid".into();
    let error = sandbox.run_preflight(task_id, worktree.worktree_path(), worktree.branch_name(), &alternate_smoke)
        .await.unwrap_err();
    assert!(matches!(error, RelayError::Internal(message) if message.contains("smoke preset")));

    let overrides = BTreeMap::from([
        ("alpha".into(), "0.25".into()),
        ("windows".into(), "4,8".into()),
    ]);
    let output = sandbox
        .run_preset(worktree.worktree_path(), &manifest, "train_grid", &overrides)
        .await
        .unwrap();
    assert!(output.success, "{}", output.raw_log);
    assert_eq!(output.exit_code, Some(0));
    // The fixture collects every occurrence of each flag so duplicate defaults are visible.
    assert_eq!(
        output.metrics_json,
        Some(serde_json::json!({"windows": ["4,8"], "alpha": [0.25]}).into())
    );
    assert_eq!(
        output.log_blob_hash,
        format!("blake3:{}", blake3::hash(output.raw_log.as_bytes()).to_hex())
    );
    worktree.cleanup().await.unwrap();
}

#[tokio::test]
async fn git_bundle_roundtrip_transfers_commits_between_repos() {
    let temp = TempDir::new();
    let repo_a = temp.join("repo a");
    let repo_b = temp.join("repo b");
    init_repo(&repo_a).await;
    init_repo(&repo_b).await;
    fs::write(repo_a.join("data file.txt"), "initial contents\n").unwrap();
    let commit = create_initial_commit(&repo_a).await;
    let source = WorktreeSandbox::new(&repo_a, temp.join("source-worktrees"));
    let destination = WorktreeSandbox::new(&repo_b, temp.join("destination-worktrees"));
    let bundle = temp.join("sync bundle.git");

    let hash = source.create_git_bundle(&bundle).await.unwrap();
    let imported = destination.import_git_bundle(&bundle, "test").await.unwrap();

    assert_eq!(hash, format!("blake3:{}", blake3::hash(&fs::read(&bundle).unwrap()).to_hex()));
    assert_eq!(imported, commit);
    assert_eq!(git(&repo_b, &["rev-parse", "refs/relay/incoming/test"]).await, commit);
    assert_eq!(
        destination.changed_files_in_commit(&imported).await.unwrap(),
        vec!["data file.txt"]
    );
    assert_eq!(source.create_working_diff_patch().await.unwrap(), None);
    fs::write(repo_a.join("data file.txt"), "edited contents\n").unwrap();
    let delta = source.create_working_diff_patch().await.unwrap().unwrap();
    assert!(delta.contains("+edited contents"));
    assert!(source.is_working_tree_dirty().await.unwrap());
}

#[tokio::test]
async fn worktree_preflight_and_code_diff_self_healing_preserves_clean_main_tree() {
    let temp = TempDir::new();
    let repo = temp.join("main");
    let base_commit = init_python_repo(&repo).await;
    let sandbox = WorktreeSandbox::new(&repo, temp.join("worktrees"));
    let manifest = load_manifest(&repo).unwrap();
    let task_id = Uuid::now_v7();
    let worktree = sandbox.prepare_worktree(task_id, &base_commit).await.unwrap();
    let path = worktree.worktree_path();
    let branch = worktree.branch_name();

    let report = sandbox
        .run_preflight(task_id, path, branch, &manifest)
        .await
        .unwrap();
    assert!(!report.smoke_passed);
    assert_eq!(report.exit_code, Some(1));
    assert!(report.stderr_tail.contains("NameError"));
    assert!(report.detected_missing_module.is_none());

    let patch = r#"diff --git a/src/app.py b/src/app.py
--- a/src/app.py
+++ b/src/app.py
@@ -1,5 +1,7 @@
 import argparse
 import json
-
+
+SMOKE_MESSAGE = "Smoke passed"
+
 parser = argparse.ArgumentParser()
 parser.add_argument("--smoke", action="store_true")
diff --git a/fix-note.txt b/fix-note.txt
new file mode 100644
--- /dev/null
+++ b/fix-note.txt
@@ -0,0 +1 @@
+Smoke constant restored.
"#;
    let proposal = sandbox
        .apply_and_verify_patch(
            task_id,
            path,
            branch,
            &manifest,
            PatchKind::CodeDiff,
            "Define the smoke message",
            patch,
        )
        .await
        .unwrap();
    assert!(proposal.smoke_verified, "{}", proposal.smoke_stdout_tail);
    assert!(proposal.smoke_stdout_tail.contains("Smoke passed"));
    assert!(proposal.diff_preview.contains("diff --git a/src/app.py b/src/app.py"));
    assert!(proposal.diff_preview.contains("+SMOKE_MESSAGE = \"Smoke passed\""));
    assert!(proposal.diff_preview.contains("+Smoke constant restored."));
    assert!(!sandbox.is_working_tree_dirty().await.unwrap());
    assert_eq!(fs::read_to_string(repo.join("src/app.py")).unwrap(), PYTHON);

    let trained = sandbox
        .run_preset(path, &manifest, "train_grid", &BTreeMap::new())
        .await
        .unwrap();
    assert!(trained.success, "{}", trained.raw_log);
    assert_eq!(
        trained.metrics_json,
        Some(serde_json::json!({"windows": ["7,14,28"], "alpha": [0.5]}).into())
    );

    worktree.cleanup().await.unwrap();
    assert!(!path.exists());
    assert!(git(&repo, &["branch", "--list", branch]).await.is_empty());
    assert_eq!(sandbox.head_commit_sha().await.unwrap(), base_commit);
    assert!(!sandbox.is_working_tree_dirty().await.unwrap());
}

#[test]
fn detects_missing_python_module_from_traceback() {
    let stderr = "Traceback (most recent call last):\r\n  import scipy\r\n\
                  ModuleNotFoundError: No module named 'scipy'\r\n";
    assert_eq!(detect_missing_python_module(stderr).as_deref(), Some("scipy"));
    assert_eq!(detect_missing_python_module("ValueError: wrong input"), None);
}

#[tokio::test]
async fn compares_environment_inputs_at_a_commit_with_the_host() {
    let temp = TempDir::new();
    let repo = temp.join("host");
    init_repo(&repo).await;
    let sandbox = WorktreeSandbox::new(&repo, temp.join("worktrees"));
    let absent = compute_env_fingerprint(&repo).unwrap();
    let empty = sandbox.check_env_diff("HEAD").await.unwrap();
    assert_eq!(empty.host_env_fingerprint, absent);
    assert!(empty.in_sync);
    assert_eq!(absent.len(), "uv:blake3:".len() + 16);
    fs::write(repo.join("uv.lock"), "").unwrap();
    assert_ne!(compute_env_fingerprint(&repo).unwrap(), absent);
    fs::remove_file(repo.join("uv.lock")).unwrap();
    fs::write(
        repo.join("pyproject.toml"),
        "[project]\ndependencies = [\"Some_Package[extra]>=2\", \"Scipy>=1.14\"]\n",
    )
    .unwrap();
    let head = create_initial_commit(&repo).await;
    assert!(sandbox.check_env_diff("HEAD").await.unwrap().in_sync);

    fs::write(repo.join("pyproject.toml"), "[project]\ndependencies = []\n").unwrap();
    fs::write(repo.join("uv.lock"), "[[package]]\nname = \"some-package\"\n").unwrap();
    let report = sandbox.check_env_diff(&head).await.unwrap();
    assert_eq!(report.commit_sha, head);
    assert_eq!(report.host_env_fingerprint, compute_env_fingerprint(&repo).unwrap());
    assert!(!report.in_sync);
    assert_eq!(report.missing_packages, vec!["scipy"]);
}

#[test]
fn extracts_the_last_complete_metrics_object_from_stdout() {
    let stdout = "setup {broken\n{\"first\": 1}\n{\n  \"outer\": {\"inner\": 2}\n}\ntrailing log";
    assert_eq!(
        extract_metrics_json(stdout),
        Some(serde_json::json!({"outer": {"inner": 2}}).into())
    );
    assert_eq!(extract_metrics_json("true\n"), Some(JsonValue::Bool(true)));
}

#[tokio::test]
async fn sync_git_head_seeds_and_invalidates_evidence_on_new_commit() {
    use crate::contracts::{EdgeKind, GraphEdge};

    let temp = TempDir::new();
    let repo = temp.join("head-watch");
    init_repo(&repo).await;
    fs::write(repo.join("README.md"), "v1\n").unwrap();
    let first_sha = create_initial_commit(&repo).await;

    let engine = RelayEngine::in_memory("watch-repo", Some(repo.clone())).await.unwrap();
    let sandbox = WorktreeSandbox::new(&repo, temp.join("worktrees"));

    match sync_git_head_into_engine(&engine, &sandbox, "laptop").await.unwrap() {
        HeadSyncOutcome::Seeded { commit_sha } => assert_eq!(commit_sha, first_sha),
        other => panic!("expected Seeded, got {other:?}"),
    }
    assert!(matches!(
        sync_git_head_into_engine(&engine, &sandbox, "laptop").await.unwrap(),
        HeadSyncOutcome::Unchanged
    ));

    let artifact_id = format!("artifact:{first_sha}");
    let now = Utc::now();
    let evidence = GraphNode {
        id: "ev-1".into(),
        label: "Baseline".into(),
        payload: NodeKind::Evidence {
            task_id: "t-1".into(),
            preset: "smoke".into(),
            metrics_json: JsonValue::Number(1.0),
            log_blob_hash: "hash".into(),
        },
        freshness: Freshness::Fresh,
        device_origin: "host".into(),
        created_at: now,
        updated_at: now,
    };
    engine
        .upsert_node_and_edges(
            evidence,
            vec![GraphEdge {
                id: "edge-1".into(),
                from: artifact_id,
                to: "ev-1".into(),
                kind: EdgeKind::ProducedFrom,
            }],
        )
        .await
        .unwrap();

    fs::write(repo.join("README.md"), "v2\n").unwrap();
    git(&repo, &["add", "README.md"]).await;
    git(&repo, &["-c", "user.name=Relay Test", "-c", "user.email=relay-test@example.invalid",
        "commit", "-m", "Update forecast logic"]).await;
    let second_sha = sandbox.head_commit_sha().await.unwrap();

    match sync_git_head_into_engine(&engine, &sandbox, "laptop").await.unwrap() {
        HeadSyncOutcome::Superseded { commit_sha, report } => {
            assert_eq!(commit_sha, second_sha);
            assert_eq!(report.invalidated_evidence_ids, vec!["ev-1"]);
        }
        other => panic!("expected Superseded, got {other:?}"),
    }
    let snapshot = engine.dag_snapshot(true).await.unwrap();
    assert_eq!(snapshot.stale_count, 1);
    let second_node = snapshot
        .nodes
        .iter()
        .find(|n| n.id == format!("artifact:{second_sha}"))
        .unwrap();
    assert_eq!(second_node.label, "Update forecast logic");

    // Visiting the same two commits again must leave four ordered snapshots.
    let mut prior = second_node.id.clone();
    for sha in [&first_sha, &second_sha] {
        git(&repo, &["checkout", "--detach", sha]).await;
        assert!(matches!(sync_git_head_into_engine(&engine, &sandbox, "laptop").await.unwrap(),
            HeadSyncOutcome::Superseded { .. }));
        let head_id = engine.head_artifact_id().await.unwrap().unwrap();
        let snapshot = engine.dag_snapshot(true).await.unwrap();
        assert_eq!(snapshot.head_commit_sha.as_ref(), Some(sha));
        assert_ne!(head_id, format!("artifact:{sha}"));
        assert!(snapshot.edges.iter().any(|edge| edge.kind == EdgeKind::Supersedes
            && edge.from == head_id && edge.to == prior));
        assert!(!snapshot.edges.iter().any(|edge| edge.kind == EdgeKind::Supersedes && edge.to == head_id));
        prior = head_id;
    }
    assert_eq!(engine.dag_snapshot(true).await.unwrap().nodes.len(), 5);
}

#[tokio::test]
async fn passive_head_polling_preserves_a_peer_snapshot_across_restarts() {
    let temp = TempDir::new();
    let repo = temp.join("paired-host");
    init_repo(&repo).await;
    fs::write(repo.join("README.md"), "local\n").unwrap();
    let local_sha = create_initial_commit(&repo).await;
    let db = temp.join("host.db");
    let engine = RelayEngine::open("paired", &db, Some(repo.clone())).await.unwrap();
    let sandbox = WorktreeSandbox::new(&repo, temp.join("worktrees"));
    let mut peer = build_git_artifact_node(&sandbox, local_sha.clone(), "peer".into()).await.unwrap();
    let NodeKind::Artifact { git_commit_sha, .. } = &mut peer.payload else { unreachable!() };
    *git_commit_sha = "peer-commit".into();
    peer.id = "peer-artifact".into();
    engine.upsert_node_and_edges(peer, vec![]).await.unwrap();

    // A fresh binding (or an older DB) has no local observation yet.
    assert!(matches!(sync_git_head_into_engine(&engine, &sandbox, "host").await.unwrap(),
        HeadSyncOutcome::Unchanged));
    drop(engine);
    let engine = RelayEngine::open("paired", &db, Some(repo)).await.unwrap();
    assert!(matches!(sync_git_head_into_engine(&engine, &sandbox, "host").await.unwrap(),
        HeadSyncOutcome::Unchanged));
    assert_eq!(engine.head_commit_sha().await.unwrap().as_deref(), Some("peer-commit"));
    assert!(matches!(sync_git_head_explicitly(&engine, &sandbox, "host").await.unwrap(),
        HeadSyncOutcome::Superseded { commit_sha, .. } if commit_sha == local_sha));
}

#[tokio::test]
async fn read_repo_file_and_git_diff_enforce_repo_confinement() {
    let temp = TempDir::new();
    let repo = temp.join("safe-read");
    init_python_repo(&repo).await;
    fs::write(repo.join(".env"), "API_KEY=secret\n").unwrap();
    let sandbox = WorktreeSandbox::new(&repo, temp.join("worktrees"));

    let numbered = sandbox.read_repo_file("src/app.py", 4096).await.unwrap();
    assert!(numbered.contains("   1: import argparse"));
    fs::write(repo.join("large.txt"), format!("first line\n{}", "模型".repeat(10_000))).unwrap();
    assert_eq!(sandbox.read_repo_file("large.txt", 256).await.unwrap(), "   1: first line\n... [truncated]\n");
    fs::remove_file(repo.join("large.txt")).unwrap();
    assert!(sandbox.read_repo_file("../outside.txt", 4096).await.is_err());
    assert!(sandbox.read_repo_file(".git/config", 4096).await.is_err());
    assert!(sandbox.read_repo_file(".env", 4096).await.is_err());

    let clean = sandbox.git_status_and_diff(4096).await.unwrap();
    assert!(clean.contains("(no uncommitted diff)"));

    fs::write(repo.join("src/app.py"), format!("{PYTHON}\n# modified line\n")).unwrap();
    let dirty = sandbox.git_status_and_diff(4096).await.unwrap();
    assert!(dirty.contains("M src/app.py"));
    assert!(dirty.contains("+# modified line"));
}

#[test]
fn generates_valid_default_manifests_for_detected_project_types() {
    let temp = TempDir::new();

    let py_dir = temp.join("Py Sample App");
    fs::create_dir(&py_dir).unwrap();
    fs::write(
        py_dir.join("pyproject.toml"),
        "[project]\nname = \"py-sample\"\ndependencies = [\"pytest\"]\n",
    )
    .unwrap();
    let (py_toml, py_kind) = generate_default_manifest(&py_dir).unwrap();
    assert_eq!(py_kind, "python-uv");
    let py_manifest: RelayManifest = toml::from_str(&py_toml).unwrap();
    assert_eq!(py_manifest.project.repo_id, "py-sample-app");
    assert_eq!(py_manifest.project.smoke_preset, "smoke");
    assert_eq!(py_manifest.presets["smoke"].program, "uv");
    assert_eq!(py_manifest.presets["test"].program, "uv");

    let rs_dir = temp.join("rust-crate");
    fs::create_dir(&rs_dir).unwrap();
    fs::write(rs_dir.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
    let kind = write_default_manifest(&rs_dir).unwrap();
    assert_eq!(kind, "rust-cargo");
    let rs_manifest = load_manifest(&rs_dir).unwrap();
    assert_eq!(rs_manifest.project.smoke_preset, "check");
    assert_eq!(rs_manifest.presets["check"].program, "cargo");
    assert_eq!(rs_manifest.presets["test"].program, "cargo");
    assert_eq!(rs_manifest.presets["check"].timeout_secs, 900);
    assert_eq!(rs_manifest.presets["test"].timeout_secs, 1800);
    assert!(write_default_manifest(&rs_dir).is_err());

    let js_bun_dir = temp.join("web-bun");
    fs::create_dir(&js_bun_dir).unwrap();
    fs::write(
        js_bun_dir.join("package.json"),
        r#"{"scripts":{"lint":"oxlint","test":"bun test","build":"vite build"}}"#,
    )
    .unwrap();
    fs::write(js_bun_dir.join("bun.lock"), "").unwrap();
    let (bun_toml, bun_kind) = generate_default_manifest(&js_bun_dir).unwrap();
    assert_eq!(bun_kind, "js-ts");
    let bun_manifest: RelayManifest = toml::from_str(&bun_toml).unwrap();
    assert_eq!(bun_manifest.presets["smoke"].program, "bun");
    assert_eq!(bun_manifest.presets["smoke"].args, vec!["--version"]);
    assert_eq!(bun_manifest.presets["lint"].args, vec!["run", "lint"]);
    assert_eq!(bun_manifest.presets["build"].args, vec!["run", "build"]);

    let generic_dir = temp.join("misc");
    fs::create_dir(&generic_dir).unwrap();
    let (gen_toml, gen_kind) = generate_default_manifest(&generic_dir).unwrap();
    assert_eq!(gen_kind, "generic");
    let gen_manifest: RelayManifest = toml::from_str(&gen_toml).unwrap();
    assert_eq!(gen_manifest.presets["smoke"].program, "git");
}

#[test]
fn starter_creation_publishes_one_complete_manifest_and_preserves_existing_files() {
    let temp = TempDir::new();
    let repo = temp.join("实验仓库");
    fs::create_dir(&repo).unwrap();
    let expected = generate_default_manifest(&repo).unwrap().0;
    let start = std::sync::Barrier::new(4);
    std::thread::scope(|scope| {
        let writers: Vec<_> = (0..4).map(|_| scope.spawn(|| {
            start.wait();
            write_default_manifest(&repo)
        })).collect();
        let successes = writers.into_iter().map(|worker| worker.join().unwrap()).filter(Result::is_ok).count();
        assert_eq!(successes, 1);
    });
    assert_eq!(fs::read_to_string(repo.join("relay.toml")).unwrap(), expected);
    assert_eq!(fs::read_dir(&repo).unwrap().count(), 1);
    assert_ne!(slugify_repo_id("实验仓库"), slugify_repo_id("测评仓库"));
    fs::write(repo.join("relay.toml"), "unfinished user configuration").unwrap();
    assert!(write_default_manifest(&repo).is_err());
    assert_eq!(fs::read_to_string(repo.join("relay.toml")).unwrap(), "unfinished user configuration");
}

#[tokio::test]
async fn first_dispatch_can_use_a_starter_manifest_before_it_is_committed() {
    use crate::contracts::{ApproveAndExecuteArgs, RunPreflightArgs, TaskSpec, TaskState};
    use crate::mcp::RelayMcpServer;
    use std::sync::Arc;

    let temp = TempDir::new();
    let repo = temp.join("new-project");
    init_repo(&repo).await;
    fs::write(repo.join("README.md"), "Project\n").unwrap();
    let sha = create_initial_commit(&repo).await;
    write_default_manifest(&repo).unwrap();
    let sandbox = WorktreeSandbox::new(&repo, temp.join("worktrees"));
    let engine = Arc::new(RelayEngine::in_memory("new-project", Some(repo.clone())).await.unwrap());
    let server = RelayMcpServer::new(engine.clone(), Some(sandbox), None);
    let id = Uuid::now_v7();
    engine.submit_task(TaskSpec {
        task_id: id,
        repo_id: "new-project".into(),
        base_commit_sha: sha,
        command_preset: "smoke".into(),
        params: BTreeMap::new(),
        require_approval: false,
        requested_by_device: "onboarding".into(),
        user_note: None,
    }).await.unwrap();
    let preflight = server.handle_run_preflight(RunPreflightArgs { task_id: id.to_string() }).await.unwrap();
    assert!(preflight.smoke_passed);
    assert!(!preflight.worktree_path.join("relay.toml").exists());
    server.handle_approve_and_execute(ApproveAndExecuteArgs {
        task_id: id.to_string(), patch_id: None, approved_via_ui: None,
    }, None).await.unwrap();
    assert_eq!(engine.get_task(id).await.unwrap().unwrap().state, TaskState::Completed);
    assert_eq!(git(&repo, &["status", "--short"]).await, "?? relay.toml");
}
