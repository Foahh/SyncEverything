mod env;
mod execution;
pub mod pm;

pub use env::compute_env_fingerprint;
pub use execution::{
    ErrorSourceSnippet, PatchInput, PreflightReport, PresetRunOutput, detect_missing_python_module,
    extract_error_snippets, extract_metrics_json,
};
pub use pm::{
    DependencyChange, DependencyDiff, EnvFileSnapshot, GeneratedManifest, PackageManagerAdapter,
    PackageManagerInfo, PackageManagerRegistry, ParsedDependencies, StarterPresets,
    adapter_by_name, detect_adapter, diff_dependencies, generate_manifest_for_repo,
    list_supported_package_managers, resolve_adapter, resolve_adapter_for_repo,
    supported_package_manager_names, validate_package_manager_name, write_manifest_for_repo,
};

use crate::contracts::{
    CommandPreset, Freshness, GraphNode, NodeKind, RelayManifest, StalePropagationReport, TaskId,
    WorktreeFileView,
};
use crate::engine::RelayEngine;
use crate::error::{RelayError, RelayResult};
use chrono::Utc;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};
use std::process::{Output, Stdio};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

/// Validate parsed `relay.toml` metadata, including any explicit `project.package_manager`.
pub fn validate_manifest(manifest: &RelayManifest) -> RelayResult<()> {
    if let Some(pm) = manifest.project.package_manager.as_deref() {
        pm::validate_package_manager_name(pm)?;
    }
    Ok(())
}

/// Load and validate the project-level command whitelist from the repository's `relay.toml`.
pub fn load_manifest(repo_root: &Path) -> RelayResult<RelayManifest> {
    let raw = std::fs::read_to_string(repo_root.join("relay.toml"))?;
    let manifest: RelayManifest = toml::from_str(&raw)?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

/// Generate a starter `relay.toml` manifest by inspecting project files in `repo_root`.
/// Returns `(toml_content, detected_package_manager)`.
pub fn generate_default_manifest(repo_root: &Path) -> RelayResult<(String, String)> {
    let generated = pm::generate_manifest_for_repo(repo_root, None)?;
    Ok((generated.toml_content, generated.package_manager))
}

/// Write a starter `relay.toml` into `repo_root` without overwriting an existing file.
/// Returns the detected package manager name (`uv`, `cargo`, `bun`, `pnpm`, `yarn`, `npm`, or `generic`).
pub fn write_default_manifest(repo_root: &Path) -> RelayResult<String> {
    let generated = pm::write_manifest_for_repo(repo_root, None)?;
    Ok(generated.package_manager)
}

/// Derive the same starter repository ID during onboarding and manifest generation.
pub fn slugify_repo_id(raw: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;
    for ch in raw.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !slug.is_empty() {
            slug.push('-');
            prev_dash = true;
        }
    }
    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() || !raw.is_ascii() {
        let prefix = if trimmed.is_empty() { "workspace" } else { trimmed };
        let digest = blake3::hash(raw.as_bytes()).to_hex();
        format!("{prefix}-{}", &digest.as_str()[..12])
    } else {
        trimmed.to_owned()
    }
}

pub(crate) fn toml_string_literal(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            c if c.is_control() => {
                use std::fmt::Write;
                let _ = write!(escaped, "\\u{:04X}", c as u32);
            }
            c => escaped.push(c),
        }
    }
    escaped.push('"');
    escaped
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
    pub(crate) fn task_manifest(&self, worktree: &Path) -> RelayResult<RelayManifest> {
        match std::fs::symlink_metadata(worktree.join("relay.toml")) {
            Ok(_) => load_manifest(worktree),
            // A freshly generated manifest may not yet be present in the task's base commit.
            Err(error) if error.kind() == ErrorKind::NotFound => load_manifest(self.repo_root()),
            Err(error) => Err(error.into()),
        }
    }

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

    /// Resolve a revision once so a queued task can be pinned to its immutable Git object.
    pub async fn resolve_commit(&self, rev: &str) -> RelayResult<String> {
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

    /// Read a UTF-8 file strictly inside the repository root with line numbers.
    /// Blocks path traversal (`..`), symlinks escaping the repo, `.git`/`.venv`/`.relay` internals, and `.env*` secrets.
    pub async fn read_repo_file(&self, relative_path: &str, max_bytes: usize) -> RelayResult<String> {
        let view = read_confined_file(&self.repo_root, relative_path, None, None, max_bytes).await?;
        Ok(view.content)
    }

    /// Read a UTF-8 file strictly inside a task's isolated Git worktree with 1-based line numbers.
    /// Blocks path traversal (`..`), symlink escapes, `.git`/`.venv`/`.relay`, `.env*`, and binary files.
    pub async fn read_worktree_file(
        &self,
        worktree_path: &Path,
        relative_path: &str,
        start_line: Option<u32>,
        end_line: Option<u32>,
        max_bytes: usize,
    ) -> RelayResult<WorktreeFileView> {
        read_confined_file(worktree_path, relative_path, start_line, end_line, max_bytes).await
    }

    /// Return `git status --short` and uncommitted `git diff HEAD` (strictly read-only).
    pub async fn git_status_and_diff(&self, max_bytes: usize) -> RelayResult<String> {
        let status_out =
            git_output(git_command(&self.repo_root).args(["status", "--short"])).await?;
        let status_text = output_text(&status_out.stdout);
        let diff_text = working_diff(&self.repo_root).await?;

        let mut out = String::from("[git status --short]\n");
        if status_text.is_empty() {
            out.push_str("(clean working tree)\n");
        } else {
            out.push_str(&status_text);
            out.push('\n');
        }
        out.push_str("\n[git diff HEAD]\n");
        if diff_text.is_empty() {
            out.push_str("(no uncommitted diff)\n");
        } else {
            out.push_str(&diff_text);
        }

        let cap = max_bytes.max(512);
        if out.len() > cap {
            let mut end = cap;
            while end > 0 && !out.is_char_boundary(end) {
                end -= 1;
            }
            out.truncate(end);
            out.push_str("\n... [diff truncated]");
        }
        Ok(out)
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
    ) -> RelayResult<String> {
        let path = handle.worktree_path();
        self.validate_task_worktree(task_id, path, handle.branch_name()).await?;
        let actual =
            git_output(git_command(path).args(["rev-parse", "--verify", "HEAD"])).await?;
        let sha = output_text(&actual.stdout);
        if sha != self.resolve_commit(base_commit_sha).await? {
            return Err(RelayError::Git(
                "task worktree no longer points to its base commit".into(),
            ));
        }
        Ok(sha)
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

const MAX_CONFINED_FILE_READ_BYTES: u64 = 2 * 1024 * 1024;
const DEFAULT_WORKTREE_READ_WINDOW: u32 = 200;

async fn read_confined_file(
    root: &Path,
    relative_path: &str,
    start_line: Option<u32>,
    end_line: Option<u32>,
    max_bytes: usize,
) -> RelayResult<WorktreeFileView> {
    let trimmed = relative_path.trim();
    let normalized = validate_confined_relative_path(trimmed)?;
    let canonical_root = std::fs::canonicalize(root)?;
    let candidate = canonical_root.join(&normalized);
    let canonical_file = std::fs::canonicalize(&candidate).map_err(|err| {
        RelayError::Internal(format!("could not open '{trimmed}': {err}"))
    })?;
    let resolved = canonical_file.strip_prefix(&canonical_root).map_err(|_| {
        RelayError::Internal(format!(
            "path '{trimmed}' resolves outside the active repository"
        ))
    })?;
    validate_confined_path_components(resolved, trimmed)?;
    let metadata = tokio::fs::metadata(&canonical_file).await?;
    if !metadata.is_file() {
        return Err(RelayError::Internal(format!(
            "'{trimmed}' is not a regular file"
        )));
    }

    let cap = max_bytes.max(256);
    let ranged = start_line.is_some() || end_line.is_some();
    let read_limit = if ranged {
        MAX_CONFINED_FILE_READ_BYTES
    } else {
        (cap.saturating_add(1).max(4096) as u64).min(MAX_CONFINED_FILE_READ_BYTES)
    };
    let file = tokio::fs::File::open(&canonical_file).await?;
    let mut bytes = Vec::new();
    file.take(read_limit).read_to_end(&mut bytes).await?;
    if bytes.iter().take(4096).any(|b| *b == 0) {
        return Err(RelayError::Internal(format!(
            "'{trimmed}' appears to be a binary file"
        )));
    }
    let read_truncated = (bytes.len() as u64) < metadata.len();
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(error) if read_truncated && error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).expect("validated UTF-8 prefix")
        }
        Err(_) => return Err(RelayError::Internal(format!("'{trimmed}' is not valid UTF-8 text"))),
    };

    let all_lines: Vec<&str> = text.lines().collect();
    let total_lines = u32::try_from(all_lines.len()).unwrap_or(u32::MAX);
    let display_path = normalized
        .components()
        .filter_map(|c| match c {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/");

    if total_lines == 0 {
        return Ok(WorktreeFileView {
            file_path: display_path,
            start_line: 0,
            end_line: 0,
            total_lines: 0,
            truncated: false,
            content: String::new(),
        });
    }

    let start = start_line.unwrap_or(1).max(1);
    let requested_end = match (start_line, end_line) {
        (_, Some(end)) => end.max(start),
        (Some(_), None) => start.saturating_add(DEFAULT_WORKTREE_READ_WINDOW - 1),
        (None, None) => total_lines,
    };
    let end = requested_end.min(total_lines);

    let mut formatted = String::new();
    let mut truncated = read_truncated;
    let mut last_emitted = 0u32;
    if start <= total_lines {
        for line_no in start..=end {
            let line = all_lines[(line_no - 1) as usize];
            let entry = format!("{line_no:>4}: {line}\n");
            if formatted.len() + entry.len() > cap {
                truncated = true;
                break;
            }
            formatted.push_str(&entry);
            last_emitted = line_no;
        }
    }
    if truncated {
        formatted.push_str("... [truncated]\n");
    }
    let effective_start = if last_emitted == 0 { 0 } else { start };
    Ok(WorktreeFileView {
        file_path: display_path,
        start_line: effective_start,
        end_line: last_emitted,
        total_lines,
        truncated,
        content: formatted,
    })
}

pub(crate) fn validate_confined_relative_path(raw: &str) -> RelayResult<PathBuf> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(RelayError::Internal("file path must not be empty".into()));
    }
    let normalized_sep = trimmed.replace('\\', "/");
    let rel = Path::new(&normalized_sep);
    if rel.is_absolute() {
        return Err(RelayError::Internal(format!(
            "path '{trimmed}' must be relative to the repository/worktree root"
        )));
    }
    validate_confined_path_components(rel, trimmed)
}

pub(crate) fn validate_confined_path_components(path: &Path, raw: &str) -> RelayResult<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => {
                let name = part.to_string_lossy().to_ascii_lowercase();
                if matches!(name.as_str(), ".git" | ".venv" | ".relay" | ".relay-desktop")
                    || name.starts_with(".env")
                    || name.ends_with(".db")
                    || name.ends_with(".db-wal")
                    || name.ends_with(".db-shm")
                    || name.ends_with(".pem")
                    || name.ends_with(".key")
                {
                    return Err(RelayError::Internal(format!(
                        "access to protected path '{raw}' is blocked"
                    )));
                }
                normalized.push(part);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(RelayError::Internal(format!(
                    "path traversal outside repository is blocked: '{raw}'"
                )));
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        return Err(RelayError::Internal("invalid repository file path".into()));
    }
    Ok(normalized)
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
    // Explicitly null stdin so spawning git inside `mcp-stdio` never deadlocks on Windows stdin pipes.
    command
        .arg("-C")
        .arg(repo_root)
        .stdin(Stdio::null())
        .kill_on_drop(true);
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
    sync_checkout(engine, sandbox, device_origin.into(), false).await
}

/// An explicit sync-head command selects the physical checkout even if a peer updated the DAG.
pub async fn sync_git_head_explicitly(
    engine: &RelayEngine,
    sandbox: &WorktreeSandbox,
    device_origin: impl Into<String>,
) -> RelayResult<HeadSyncOutcome> {
    sync_checkout(engine, sandbox, device_origin.into(), true).await
}

async fn sync_checkout(
    engine: &RelayEngine,
    sandbox: &WorktreeSandbox,
    device_origin: String,
    explicit: bool,
) -> RelayResult<HeadSyncOutcome> {
    let checkout = tokio::fs::canonicalize(sandbox.repo_root()).await?;
    let prior = engine.observed_git_head(&checkout).await?;
    let head_sha = sandbox.head_commit_sha().await?;
    if !explicit && prior.as_deref() == Some(head_sha.as_str()) {
        return Ok(HeadSyncOutcome::Unchanged);
    }

    let artifact = build_git_artifact_node(sandbox, head_sha, device_origin).await?;
    engine.observe_git_artifact(&checkout, artifact, prior.as_deref(), explicit).await
}

#[cfg(test)]
mod tests;
