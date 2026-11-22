mod env;
mod execution;

pub use env::compute_env_fingerprint;
pub use execution::{
    PreflightReport, PresetRunOutput, detect_missing_python_module, extract_metrics_json,
};

use crate::contracts::{
    CommandPreset, Freshness, GraphNode, NodeKind, RelayManifest, StalePropagationReport, TaskId,
};
use crate::engine::RelayEngine;
use crate::error::{RelayError, RelayResult};
use chrono::Utc;
use std::path::{Path, PathBuf};
use std::process::Output;
use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// Load the project-level command whitelist from the repository's relay.toml.
pub fn load_manifest(repo_root: &Path) -> RelayResult<RelayManifest> {
    let raw = std::fs::read_to_string(repo_root.join("relay.toml"))?;
    Ok(toml::from_str(&raw)?)
}

/// Resolve a named preset against the whitelist in relay.toml.
pub fn resolve_preset<'a>(
    manifest: &'a RelayManifest,
    preset_name: &str,
) -> RelayResult<&'a CommandPreset> {
    manifest
        .presets
        .get(preset_name)
        .ok_or_else(|| RelayError::UnknownPreset(preset_name.to_string()))
}

/// Async CLI wrapper around system Git and uv.
#[derive(Debug, Clone)]
pub struct WorktreeSandbox {
    repo_root: PathBuf,
    worktrees_base: PathBuf,
}

impl WorktreeSandbox {
    pub fn new(repo_root: impl Into<PathBuf>, worktrees_base: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
            worktrees_base: worktrees_base.into(),
        }
    }

    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    pub fn worktrees_base(&self) -> &Path {
        &self.worktrees_base
    }

    pub async fn head_commit_sha(&self) -> RelayResult<String> {
        self.resolve_commit("HEAD").await
    }

    pub async fn commit_subject(&self, commit_sha: &str) -> RelayResult<String> {
        let sha = self.resolve_commit(commit_sha).await?;
        let output = git_output(git_command(&self.repo_root).args([
            "log",
            "-1",
            "--format=%s",
            &sha,
        ]))
        .await?;
        Ok(output_text(&output.stdout))
    }

    pub(crate) async fn branch_name(&self) -> RelayResult<String> {
        let output = git_output(
            git_command(&self.repo_root).args(["rev-parse", "--abbrev-ref", "HEAD"]),
        )
        .await?;
        Ok(output_text(&output.stdout))
    }

    pub(crate) async fn git_bundle_head(&self, bundle_path: &Path) -> RelayResult<String> {
        let path = std::path::absolute(bundle_path)?;
        let output = git_output(
            git_command(&self.repo_root)
                .args(["bundle", "list-heads"])
                .arg(path)
                .arg("HEAD"),
        )
        .await?;
        let heads = output_text(&output.stdout);
        let sha = heads
            .strip_suffix(" HEAD")
            .filter(|sha| {
                matches!(sha.len(), 40 | 64) && sha.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            .ok_or_else(|| RelayError::Git("bundle is missing a valid HEAD commit".into()))?;
        Ok(sha.to_owned())
    }

    async fn resolve_commit(&self, rev: &str) -> RelayResult<String> {
        let output = git_output(git_command(&self.repo_root).args([
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{rev}^{{commit}}"),
        ]))
        .await?;
        Ok(output_text(&output.stdout))
    }

    pub async fn create_git_bundle(&self, output_bundle_path: &Path) -> RelayResult<String> {
        let path = std::path::absolute(output_bundle_path)?;
        git_output(
            git_command(&self.repo_root)
                .args(["bundle", "create"])
                .arg(&path)
                .arg("HEAD"),
        )
        .await?;
        let mut file = tokio::fs::File::open(path).await?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = [0; 8192];
        loop {
            let count = file.read(&mut buffer).await?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        Ok(format!("blake3:{}", hasher.finalize().to_hex()))
    }

    pub async fn create_working_diff_patch(&self) -> RelayResult<Option<String>> {
        let diff = working_diff(&self.repo_root).await?;
        Ok((!diff.is_empty()).then_some(diff))
    }

    /// Short names are stored under refs/relay/incoming.
    pub async fn import_git_bundle(&self, bundle_path: &Path, target_ref: &str) -> RelayResult<String> {
        let path = std::path::absolute(bundle_path)?;
        let target_ref = if target_ref.starts_with("refs/") {
            target_ref.to_owned()
        } else {
            format!("refs/relay/incoming/{target_ref}")
        };
        git_output(git_command(&self.repo_root).args(["check-ref-format", &target_ref])).await?;
        git_output(
            git_command(&self.repo_root)
                .args(["fetch", "--no-tags", "--"])
                .arg(path)
                .arg(format!("HEAD:{target_ref}")),
        )
        .await?;
        self.resolve_commit(&target_ref).await
    }

    pub async fn changed_files_in_commit(&self, commit_sha: &str) -> RelayResult<Vec<String>> {
        let sha = self.resolve_commit(commit_sha).await?;
        let output = git_output(git_command(&self.repo_root).args([
            "diff-tree",
            "--root",
            "--diff-merges=first-parent",
            "--no-commit-id",
            "--name-only",
            "--no-renames",
            "-r",
            "-z",
            &sha,
            "--",
        ]))
        .await?;
        Ok(output
            .stdout
            .split(|byte| *byte == 0)
            .filter(|path| !path.is_empty())
            .map(|path| String::from_utf8_lossy(path).into_owned())
            .collect())
    }

    pub async fn is_working_tree_dirty(&self) -> RelayResult<bool> {
        let output = git_output(git_command(&self.repo_root).args([
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
        ]))
        .await?;
        Ok(!output.stdout.is_empty())
    }

    pub async fn prepare_worktree(
        &self,
        task_id: TaskId,
        base_commit_sha: &str,
    ) -> RelayResult<WorktreeHandle> {
        let base_commit = self.resolve_commit(base_commit_sha).await?;
        let (path, branch) = self.task_worktree(task_id)?;
        let root =
            git_output(git_command(&self.repo_root).args(["rev-parse", "--show-toplevel"])).await?;
        let root = std::fs::canonicalize(output_text(&root.stdout))?;
        if is_within_checkout(&path, &root)? {
            let ignored = git_command(&self.repo_root)
                .args(["check-ignore", "-q", "--"])
                .arg(&path)
                .output()
                .await?;
            if !ignored.status.success() {
                return Err(RelayError::Internal(format!(
                    "worktree location {} must be outside the checkout or Git-ignored",
                    path.display()
                )));
            }
        }
        tokio::fs::create_dir_all(&self.worktrees_base).await?;
        git_output(
            git_command(&self.repo_root)
                .args(["worktree", "add", "-B", &branch, "--"])
                .arg(&path)
                .arg(base_commit),
        )
        .await?;
        Ok(WorktreeHandle {
            repo_root: std::path::absolute(&self.repo_root)?,
            path,
            branch,
        })
    }

    pub(crate) async fn reopen_worktree(
        &self,
        task_id: TaskId,
        base_commit_sha: &str,
    ) -> RelayResult<Option<WorktreeHandle>> {
        let (path, branch) = self.task_worktree(task_id)?;
        if !tokio::fs::try_exists(&path).await? {
            return Ok(None);
        }
        let handle = WorktreeHandle {
            repo_root: std::path::absolute(&self.repo_root)?,
            path,
            branch,
        };
        self.validate_worktree_base(task_id, &handle, base_commit_sha).await?;
        Ok(Some(handle))
    }

    pub(crate) async fn validate_worktree_base(
        &self,
        task_id: TaskId,
        handle: &WorktreeHandle,
        base_commit_sha: &str,
    ) -> RelayResult<()> {
        let path = handle.worktree_path();
        self.validate_task_worktree(task_id, path, handle.branch_name()).await?;
        let actual =
            git_output(git_command(path).args(["rev-parse", "--verify", "HEAD"])).await?;
        if output_text(&actual.stdout) != self.resolve_commit(base_commit_sha).await? {
            return Err(RelayError::Git(
                "task worktree no longer points to its base commit".into(),
            ));
        }
        Ok(())
    }

    pub(crate) fn task_worktree(&self, task_id: TaskId) -> RelayResult<(PathBuf, String)> {
        let id = task_id.simple().to_string();
        // The first half of a UUIDv7 is mostly a timestamp.
        let short_id = &id[16..];
        let path = std::path::absolute(&self.worktrees_base)?.join(short_id);
        Ok((path, format!("relay/worktree/{short_id}")))
    }

    async fn validate_task_worktree(
        &self,
        task_id: TaskId,
        worktree_path: &Path,
        worktree_branch: &str,
    ) -> RelayResult<()> {
        let (expected_path, expected_branch) = self.task_worktree(task_id)?;
        let path = std::fs::canonicalize(worktree_path)?;
        if worktree_branch != expected_branch || path != std::fs::canonicalize(expected_path)? {
            return Err(RelayError::Internal(
                "worktree does not belong to this task".into(),
            ));
        }

        let output = git_output(git_command(worktree_path).args([
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-common-dir",
            "--git-dir",
        ]))
        .await?;
        let dirs = String::from_utf8_lossy(&output.stdout);
        let mut dirs = dirs.lines();
        let (Some(root), Some(common_dir), Some(git_dir)) = (dirs.next(), dirs.next(), dirs.next())
        else {
            return Err(RelayError::Git("could not identify task worktree".into()));
        };
        let host_dir = git_output(git_command(&self.repo_root).args([
            "rev-parse",
            "--path-format=absolute",
            "--git-common-dir",
        ]))
        .await?;
        let common_dir = std::fs::canonicalize(common_dir)?;
        let branch = git_output(git_command(worktree_path).args(["symbolic-ref", "HEAD"])).await?;
        if path != std::fs::canonicalize(root)?
            || common_dir == std::fs::canonicalize(git_dir)?
            || common_dir != std::fs::canonicalize(output_text(&host_dir.stdout))?
            || output_text(&branch.stdout) != format!("refs/heads/{expected_branch}")
        {
            return Err(RelayError::Internal(
                "worktree does not belong to this task".into(),
            ));
        }
        Ok(())
    }
}

/// A task checkout and its dedicated branch. Call cleanup when the task finishes.
#[derive(Debug, Clone)]
pub struct WorktreeHandle {
    repo_root: PathBuf,
    path: PathBuf,
    branch: String,
}

impl WorktreeHandle {
    pub fn worktree_path(&self) -> &Path {
        &self.path
    }

    pub fn branch_name(&self) -> &str {
        &self.branch
    }

    pub(crate) async fn diff(&self) -> RelayResult<String> {
        working_diff(&self.path).await
    }

    pub async fn cleanup(&self) -> RelayResult<()> {
        git_output(
            git_command(&self.repo_root)
                .args(["worktree", "remove", "--force", "--"])
                .arg(&self.path),
        )
        .await?;
        let _ = git_command(&self.repo_root)
            .args(["branch", "-D", "--", &self.branch])
            .output()
            .await;
        Ok(())
    }
}

fn is_within_checkout(path: &Path, root: &Path) -> RelayResult<bool> {
    // Resolve the nearest existing ancestor before creating any directories.
    for ancestor in path.ancestors() {
        match std::fs::canonicalize(ancestor) {
            Ok(canonical) => return Ok(canonical.starts_with(root)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(false)
}

fn git_command(repo_root: &Path) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo_root).kill_on_drop(true);
    // Local Git variables from an invoking hook can redirect worktree operations.
    for key in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR"] {
        command.env_remove(key);
    }
    command
}

async fn git_output(command: &mut Command) -> RelayResult<Output> {
    let output = command.output().await?;
    check_git_output(&output)?;
    Ok(output)
}

fn check_git_output(output: &Output) -> RelayResult<()> {
    if output.status.success() {
        Ok(())
    } else {
        let stderr = output_text(&output.stderr);
        Err(RelayError::Git(if stderr.is_empty() {
            format!("git exited with {}", output.status)
        } else {
            stderr
        }))
    }
}

fn output_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches(['\r', '\n'])
        .to_owned()
}

async fn working_diff(repo_root: &Path) -> RelayResult<String> {
    let output = git_output(git_command(repo_root).args([
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--no-color",
        "--no-relative",
        "--binary",
        "--src-prefix=a/",
        "--dst-prefix=b/",
        "HEAD",
        "--",
    ]))
    .await?;
    String::from_utf8(output.stdout)
        .map_err(|error| RelayError::Git(format!("working diff is not valid UTF-8: {error}")))
}

/// Result of checking a Git repository's `HEAD` against the engine DAG.
#[derive(Debug, Clone)]
pub enum HeadSyncOutcome {
    Unchanged,
    Seeded {
        commit_sha: String,
    },
    Superseded {
        commit_sha: String,
        report: StalePropagationReport,
    },
}

/// Build an `Artifact` DAG node from a real Git commit in `sandbox`.
pub async fn build_git_artifact_node(
    sandbox: &WorktreeSandbox,
    commit_sha: String,
    device_origin: String,
) -> RelayResult<GraphNode> {
    let fingerprint = compute_env_fingerprint(sandbox.repo_root())?;
    let (changed_files, dirty, subject) = tokio::join!(
        sandbox.changed_files_in_commit(&commit_sha),
        sandbox.is_working_tree_dirty(),
        sandbox.commit_subject(&commit_sha),
    );
    let short_sha: String = commit_sha.chars().take(7).collect();
    let label = subject
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .unwrap_or_else(|| format!("Commit {short_sha}"));
    let now = Utc::now();
    Ok(GraphNode {
        id: format!("artifact:{commit_sha}"),
        label,
        payload: NodeKind::Artifact {
            git_commit_sha: commit_sha,
            env_fingerprint: fingerprint,
            working_tree_dirty: dirty.unwrap_or_default(),
            changed_files: changed_files.unwrap_or_default(),
        },
        freshness: Freshness::Fresh,
        device_origin,
        created_at: now,
        updated_at: now,
    })
}

/// Inspect the repository's live Git `HEAD` and seed or supersede the DAG if a new commit exists.
pub async fn sync_git_head_into_engine(
    engine: &RelayEngine,
    sandbox: &WorktreeSandbox,
    device_origin: impl Into<String>,
) -> RelayResult<HeadSyncOutcome> {
    let head_sha = sandbox.head_commit_sha().await?;
    let snapshot = engine.dag_snapshot(true).await;
    if snapshot.head_commit_sha.as_deref() == Some(head_sha.as_str()) {
        return Ok(HeadSyncOutcome::Unchanged);
    }

    // Find the current active artifact (prefer the node matching head_commit_sha, else newest artifact).
    let previous_artifact_id = snapshot
        .head_commit_sha
        .as_deref()
        .and_then(|sha| {
            snapshot.nodes.iter().find_map(|node| match &node.payload {
                NodeKind::Artifact { git_commit_sha, .. } if git_commit_sha == sha => {
                    Some(node.id.clone())
                }
                _ => None,
            })
        })
        .or_else(|| {
            snapshot
                .nodes
                .iter()
                .filter(|node| matches!(node.payload, NodeKind::Artifact { .. }))
                .max_by_key(|node| node.updated_at)
                .map(|node| node.id.clone())
        });

    let artifact = build_git_artifact_node(sandbox, head_sha.clone(), device_origin.into()).await?;
    let short_sha: String = head_sha.chars().take(7).collect();
    if let Some(old_id) = previous_artifact_id
        && old_id != artifact.id
    {
        let reason = format!("Git commit {short_sha}: {}", artifact.label);
        let report = engine.supersede_artifact(&old_id, artifact, reason).await?;
        Ok(HeadSyncOutcome::Superseded {
            commit_sha: head_sha,
            report,
        })
    } else {
        engine.upsert_node_and_edges(artifact, Vec::new()).await?;
        Ok(HeadSyncOutcome::Seeded {
            commit_sha: head_sha,
        })
    }
}

#[cfg(test)]
mod tests;
