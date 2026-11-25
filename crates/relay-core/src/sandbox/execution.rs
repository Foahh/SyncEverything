use super::{
    WorktreeSandbox, git_command, git_output, load_manifest, resolve_preset,
    validate_confined_path_components, validate_confined_relative_path, working_diff,
};
use crate::contracts::{
    CommandPreset, FileEdit, JsonValue, PatchKind, PatchProposal, RelayManifest, TaskId,
};
use crate::error::{RelayError, RelayResult};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use uuid::Uuid;

const SMOKE_TAIL_LINES: usize = 30;
const ERROR_SNIPPET_RADIUS: u32 = 10;
const MAX_ERROR_SNIPPETS: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ErrorSourceSnippet {
    pub file_path: String,
    pub line: u32,
    pub snippet: String,
}

#[derive(Debug, Clone, Copy)]
pub enum PatchInput<'a> {
    UvSpec(&'a str),
    FileEdits(&'a [FileEdit]),
}

impl PatchInput<'_> {
    pub fn kind(&self) -> PatchKind {
        match self {
            Self::UvSpec(_) => PatchKind::UvSpec,
            Self::FileEdits(_) => PatchKind::FileEdits,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PresetRunOutput {
    pub stdout: String,
    pub stderr: String,
    pub raw_log: String,
    pub exit_code: Option<i32>,
    pub success: bool,
    pub duration_ms: u64,
    pub log_blob_hash: String,
    pub metrics_json: Option<JsonValue>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreflightReport {
    pub task_id: TaskId,
    pub worktree_path: PathBuf,
    pub worktree_branch: String,
    pub smoke_passed: bool,
    pub exit_code: Option<i32>,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub detected_missing_module: Option<String>,
    pub error_snippets: Vec<ErrorSourceSnippet>,
    pub raw_log: String,
}

impl WorktreeSandbox {
    fn authorized_smoke_preset<'a>(&self, manifest: &'a RelayManifest) -> RelayResult<&'a str> {
        if load_manifest(self.repo_root())?.project.smoke_preset != manifest.project.smoke_preset {
            return Err(RelayError::Internal("smoke preset differs from the host relay.toml".into()));
        }
        Ok(&manifest.project.smoke_preset)
    }

    fn shared_cargo_target_dir(&self) -> RelayResult<PathBuf> {
        Ok(std::path::absolute(&self.worktrees_base)?.join("cargo-target"))
    }

    pub async fn run_preset(
        &self,
        worktree_path: &Path,
        manifest: &RelayManifest,
        preset_name: &str,
        params: &BTreeMap<String, String>,
    ) -> RelayResult<PresetRunOutput> {
        let preset = resolve_preset(manifest, preset_name)?;
        let args = preset_args(preset, params)?;

        // Authorize the command using the host checkout, even if a candidate patch edits relay.toml.
        let host_manifest = load_manifest(self.repo_root())?;
        if resolve_preset(&host_manifest, preset_name)? != preset {
            return Err(RelayError::Internal(format!(
                "preset '{preset_name}' differs from the host relay.toml"
            )));
        }

        let start = Instant::now();
        let worktree_path = std::path::absolute(worktree_path)?;
        let program = Path::new(&preset.program);
        #[cfg(windows)]
        let launcher = match std::env::var_os("PATH") {
            Some(path) => resolve_windows_package_launcher(&preset.program, &args, &path)?,
            None => None,
        };
        #[cfg(windows)]
        let program = launcher.as_deref().unwrap_or(program);
        let cargo_target_dir = self.shared_cargo_target_dir()?;
        let mut command = worktree_command(program, &worktree_path, Some(&cargo_target_dir));
        let is_uv = Path::new(&preset.program)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| stem.eq_ignore_ascii_case("uv"));
        if is_uv && !tokio::fs::try_exists(worktree_path.join("pyproject.toml")).await? {
            // With no project here, prevent uv from synchronizing a parent checkout.
            command.env("UV_NO_PROJECT", "1");
        }
        let mut child = command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let mut stdout_pipe = child
            .stdout
            .take()
            .ok_or_else(|| RelayError::Internal("preset stdout was not captured".into()))?;
        let mut stderr_pipe = child
            .stderr
            .take()
            .ok_or_else(|| RelayError::Internal("preset stderr was not captured".into()))?;
        let mut stdout_bytes = Vec::new();
        let mut stderr_bytes = Vec::new();
        let timeout = Duration::from_secs(preset.timeout_secs.into());
        let completed = tokio::time::timeout(timeout, async {
            tokio::try_join!(
                child.wait(),
                stdout_pipe.read_to_end(&mut stdout_bytes),
                stderr_pipe.read_to_end(&mut stderr_bytes),
            )
        })
        .await;
        let (exit_code, success) = match completed {
            Ok(result) => {
                let (status, _, _) = result?;
                (status.code(), status.success())
            }
            Err(_) => {
                let _ = child.start_kill();
                if !stderr_bytes.is_empty() && !stderr_bytes.ends_with(b"\n") {
                    stderr_bytes.push(b'\n');
                }
                stderr_bytes.extend_from_slice(
                    format!(
                        "preset '{preset_name}' timed out after {} seconds\n",
                        preset.timeout_secs
                    )
                    .as_bytes(),
                );
                (None, false)
            }
        };
        let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
        let stdout = String::from_utf8_lossy(&stdout_bytes).into_owned();
        let stderr = String::from_utf8_lossy(&stderr_bytes).into_owned();
        let separator = if !stdout.is_empty() && !stdout.ends_with('\n') && !stderr.is_empty() {
            "\n"
        } else {
            ""
        };
        let raw_log = format!("{stdout}{separator}{stderr}");
        let log_blob_hash = format!("blake3:{}", blake3::hash(raw_log.as_bytes()).to_hex());
        let metrics_json = extract_metrics_json(&stdout);
        Ok(PresetRunOutput {
            stdout,
            stderr,
            raw_log,
            exit_code,
            success,
            duration_ms,
            log_blob_hash,
            metrics_json,
        })
    }

    pub async fn run_preflight(
        &self,
        task_id: TaskId,
        worktree_path: &Path,
        worktree_branch: &str,
        manifest: &RelayManifest,
    ) -> RelayResult<PreflightReport> {
        self.validate_task_worktree(task_id, worktree_path, worktree_branch)
            .await?;
        let output = self
            .run_preset(
                worktree_path,
                manifest,
                self.authorized_smoke_preset(manifest)?,
                &BTreeMap::new(),
            )
            .await?;
        let error_snippets = if output.success {
            Vec::new()
        } else {
            let root = worktree_path.to_owned();
            let stderr = output.stderr.clone();
            tokio::task::spawn_blocking(move || extract_error_snippets(&root, &stderr))
                .await
                .map_err(|err| RelayError::Internal(format!("snippet reader failed: {err}")))?
        };
        Ok(PreflightReport {
            task_id,
            worktree_path: worktree_path.to_owned(),
            worktree_branch: worktree_branch.into(),
            smoke_passed: output.success,
            exit_code: output.exit_code,
            stdout_tail: tail_lines(&output.stdout),
            stderr_tail: tail_lines(&output.stderr),
            detected_missing_module: detect_missing_python_module(&output.stderr),
            error_snippets,
            raw_log: output.raw_log,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn apply_and_verify_patch(
        &self,
        task_id: TaskId,
        worktree_path: &Path,
        worktree_branch: &str,
        manifest: &RelayManifest,
        patch: PatchInput<'_>,
        summary: &str,
    ) -> RelayResult<PatchProposal> {
        self.apply_patch_for_review(
            task_id,
            worktree_path,
            worktree_branch,
            manifest,
            patch,
            summary,
        )
        .await
        .map(|(proposal, _)| proposal)
    }

    pub(crate) async fn apply_patch_for_review(
        &self,
        task_id: TaskId,
        worktree_path: &Path,
        worktree_branch: &str,
        manifest: &RelayManifest,
        patch: PatchInput<'_>,
        summary: &str,
    ) -> RelayResult<(PatchProposal, WorktreeCheckpoint)> {
        self.validate_task_worktree(task_id, worktree_path, worktree_branch)
            .await?;
        let worktree_path = std::path::absolute(worktree_path)?;
        let smoke_preset = self.authorized_smoke_preset(manifest)?;
        let kind = patch.kind();
        let diff_before = working_diff(&worktree_path).await?;
        let staged = match patch {
            PatchInput::FileEdits(edits) => prepare_file_edits(&worktree_path, edits).await?,
            PatchInput::UvSpec(_) => BTreeMap::new(),
        };
        let paths = match patch {
            PatchInput::FileEdits(_) => staged.keys().cloned().collect(),
            PatchInput::UvSpec(_) => {
                vec![PathBuf::from("pyproject.toml"), PathBuf::from("uv.lock")]
            }
        };
        let checkpoint = WorktreeCheckpoint::capture(&worktree_path, paths).await?;
        let result = async {
            match patch {
                PatchInput::UvSpec(spec) => add_uv_dependency(&worktree_path, spec).await?,
                PatchInput::FileEdits(_) => write_file_edits(&worktree_path, &staged).await?,
            }
            let diff_preview = working_diff(&worktree_path).await?;
            if diff_preview.is_empty() || diff_preview == diff_before {
                return Err(RelayError::Internal(
                    "patch produced no worktree diff".into(),
                ));
            }
            let smoke = self
                .run_preset(&worktree_path, manifest, smoke_preset, &BTreeMap::new())
                .await?;
            // Smoke may update tracked files, including the lockfile.
            let diff_preview = working_diff(&worktree_path).await?;
            if smoke.success && (diff_preview.is_empty() || diff_preview == diff_before) {
                return Err(RelayError::Internal(
                    "patch produced no lasting worktree diff".into(),
                ));
            }
            Ok(PatchProposal {
                patch_id: Uuid::now_v7().to_string(),
                task_id,
                kind,
                summary: summary.into(),
                diff_preview,
                worktree_branch: worktree_branch.into(),
                smoke_verified: smoke.success,
                smoke_stdout_tail: tail_lines(&smoke.raw_log),
                created_at: Utc::now(),
            })
        }
        .await;
        if !matches!(&result, Ok(proposal) if proposal.smoke_verified) {
            checkpoint
                .restore(&worktree_path)
                .await
                .map_err(|restore| {
                    let reason = result
                        .as_ref()
                        .err()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "patch failed smoke verification".into());
                    RelayError::Internal(format!(
                        "{reason}; could not restore the task worktree: {restore}"
                    ))
                })?;
        }
        result.map(|proposal| (proposal, checkpoint))
    }
}

#[cfg(windows)]
fn is_safe_cmd_arg(arg: &str) -> bool {
    !arg.is_empty()
        && arg.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@' | b'+' | b'=')
        })
}

#[cfg(windows)]
fn resolve_windows_package_launcher(
    program: &str,
    args: &[String],
    search_path: &std::ffi::OsStr,
) -> RelayResult<Option<std::path::PathBuf>> {
    // Standard Windows npm/pnpm/yarn installs often expose only a .cmd shim.
    // Prefer a native .exe if available; otherwise allow .cmd only when every
    // resolved argument belongs to a strict cmd.exe-inert character allowlist.
    if !matches!(program, "npm" | "pnpm" | "yarn") {
        return Ok(None);
    }
    let mut cmd_candidate = None;
    for dir in std::env::split_paths(search_path).filter(|dir| dir.is_absolute()) {
        let exe = dir.join(format!("{program}.exe"));
        if exe.is_file() {
            return Ok(Some(exe));
        }
        if cmd_candidate.is_none() {
            let cmd = dir.join(format!("{program}.cmd"));
            if cmd.is_file() {
                cmd_candidate = Some(cmd);
            }
        }
    }
    let Some(cmd) = cmd_candidate else {
        return Ok(None);
    };
    if let Some(unsafe_arg) = args.iter().find(|arg| !is_safe_cmd_arg(arg)) {
        return Err(RelayError::Internal(format!(
            "argument '{unsafe_arg}' contains characters not allowed for Windows .cmd launcher '{program}'"
        )));
    }
    Ok(Some(cmd))
}

#[cfg(all(windows, test))]
#[test]
fn package_manager_shims_allow_safe_custom_scripts_and_reject_unsafe_chars() {
    let dir = tempfile::tempdir().unwrap();

    for (program, args) in [
        ("npm", vec!["run".into(), "test:unit".into()]),
        (
            "pnpm",
            vec!["--filter".into(), "@app/web".into(), "run".into(), "check:ci".into()],
        ),
        (
            "yarn",
            vec!["run".into(), "test".into(), "--".into(), "--reporter=verbose".into()],
        ),
    ] {
        let other = dir.path().join(format!("{program}.cmd"));
        std::fs::write(&other, "").unwrap();
        assert_eq!(
            resolve_windows_package_launcher(program, &args, dir.path().as_os_str()).unwrap(),
            Some(other)
        );
    }

    assert_eq!(
        resolve_windows_package_launcher(
            "python",
            &["--version".into()],
            dir.path().as_os_str()
        )
        .unwrap(),
        None
    );

    for bad_arg in [
        "foo&calc",
        "foo|dir",
        "%COMSPEC%",
        "foo^bar",
        "\"quoted\"",
        "two words",
        "line\nbreak",
        "",
    ] {
        let err = resolve_windows_package_launcher(
            "pnpm",
            &["run".into(), bad_arg.into()],
            dir.path().as_os_str(),
        )
        .unwrap_err();
        assert!(err.to_string().contains(".cmd"), "unexpected error: {err}");
    }
}

#[cfg(all(windows, test))]
#[test]
fn package_manager_launcher_prefers_exe_over_cmd() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("npm.exe");
    let cmd = dir.path().join("npm.cmd");
    std::fs::write(&exe, "").unwrap();
    std::fs::write(&cmd, "").unwrap();

    // Native .exe runs without cmd.exe shell parsing even when args contain spaces.
    let resolved = resolve_windows_package_launcher(
        "npm",
        &["run".into(), "hello world".into()],
        dir.path().as_os_str(),
    )
    .unwrap();
    assert_eq!(resolved, Some(exe));
}

fn worktree_command(
    program: impl AsRef<std::ffi::OsStr>,
    worktree_path: &Path,
    cargo_target_dir: Option<&Path>,
) -> Command {
    let program = program.as_ref();
    let mut command = Command::new(program);
    command.current_dir(worktree_path).stdin(Stdio::null());
    for key in ["VIRTUAL_ENV", "CONDA_PREFIX", "UV_PROJECT", "UV_WORKING_DIRECTORY"] {
        command.env_remove(key);
    }
    command.env("UV_PROJECT_ENVIRONMENT", worktree_path.join(".venv"));
    if Path::new(program)
        .file_stem()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("cargo"))
    {
        let target_dir = cargo_target_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(|| worktree_path.join("target"));
        command.env("CARGO_TARGET_DIR", target_dir);
    }
    command
}

#[cfg(test)]
#[test]
fn cargo_outputs_share_isolated_worktree_target_dir() {
    use std::ffi::OsStr;
    let base = std::env::temp_dir().join("relay-cargo-sandbox");
    let repo_root = base.join("repo");
    let worktrees_base = base.join("worktrees");
    let checkout = worktrees_base.join("task-1");
    let sandbox = WorktreeSandbox::new(&repo_root, &worktrees_base);
    let shared_target = sandbox.shared_cargo_target_dir().unwrap();
    assert_eq!(
        shared_target,
        std::path::absolute(&worktrees_base).unwrap().join("cargo-target")
    );
    assert_ne!(shared_target, repo_root.join("target"));
    assert_ne!(shared_target, checkout.join("target"));
    for program in ["cargo", "cargo.exe"] {
        let cmd = worktree_command(program, &checkout, Some(&shared_target));
        assert!(cmd.as_std().get_envs().any(|(key, value)|
            key == OsStr::new("CARGO_TARGET_DIR") && value == Some(shared_target.as_os_str())));
    }
}

fn preset_args(preset: &CommandPreset, params: &BTreeMap<String, String>) -> RelayResult<Vec<String>> {
    for key in params.keys() {
        if !preset.allowed_params.contains(key) {
            return Err(RelayError::Internal(format!(
                "parameter '{key}' is not allowed for this preset"
            )));
        }
    }

    let mut args = Vec::new();
    let mut replaced = BTreeSet::new();
    let mut defaults = preset.args.iter().peekable();
    while let Some(arg) = defaults.next() {
        if let Some(flag) = arg.strip_prefix("--") {
            let key = flag.split_once('=').map_or(flag, |(key, _)| key);
            if let Some(value) = params.get(key) {
                if flag == key
                    && defaults.peek().is_some_and(|next| !next.starts_with("--"))
                {
                    defaults.next();
                }
                if replaced.insert(key) {
                    args.extend([format!("--{key}"), value.clone()]);
                }
                continue;
            }
        }
        args.push(arg.clone());
    }
    for (key, value) in params {
        if !replaced.contains(key.as_str()) {
            args.extend([format!("--{key}"), value.clone()]);
        }
    }
    Ok(args)
}

async fn add_uv_dependency(worktree_path: &Path, spec: &str) -> RelayResult<()> {
    for filename in ["pyproject.toml", "uv.lock"] {
        match tokio::fs::symlink_metadata(worktree_path.join(filename)).await {
            Ok(metadata) if !metadata.is_file() => {
                return Err(RelayError::Uv(format!("{filename} must be a regular file")));
            }
            Ok(_) => {}
            Err(error) if filename == "uv.lock" && error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }

    let output = worktree_command("uv", worktree_path, None)
        .args(["add", "--no-sync", "--", spec])
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await?;
    if !output.status.success() {
        return Err(RelayError::Uv(
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    // A lockfile generated from scratch must also appear in the proposal.
    git_output(git_command(worktree_path).args([
        "add",
        "--intent-to-add",
        "--",
        "uv.lock",
    ]))
    .await?;
    Ok(())
}

/// Keep the baseline working tree and index private to this attempt. A preflight
/// may have left both tracked changes and new files that the next retry needs.
pub(crate) struct WorktreeCheckpoint {
    scratch: tempfile::TempDir,
    index: PathBuf,
    tree: String,
    original_files: Vec<(PathBuf, Option<PathBuf>)>,
}

impl WorktreeCheckpoint {
    async fn capture(root: &Path, paths_to_edit: Vec<PathBuf>) -> RelayResult<Self> {
        let paths = git_output(git_command(root).args([
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "index",
            "--git-path",
            "objects",
        ]))
        .await?;
        let paths = std::str::from_utf8(&paths.stdout)
            .map_err(|err| RelayError::Git(format!("invalid git metadata path: {err}")))?;
        let mut paths = paths.lines();
        let index = PathBuf::from(
            paths
                .next()
                .ok_or_else(|| RelayError::Git("missing git index path".into()))?,
        );
        let objects = PathBuf::from(
            paths
                .next()
                .ok_or_else(|| RelayError::Git("missing git objects path".into()))?,
        );
        let scratch = tempfile::tempdir()?;
        let info = scratch.path().join("objects/info");
        tokio::fs::create_dir_all(&info).await?;
        tokio::fs::write(info.join("alternates"), format!("{}\n", objects.display())).await?;
        let canonical_root = std::fs::canonicalize(root)?;
        let mut original_files = Vec::new();
        for rel in paths_to_edit {
            let original =
                ensure_no_symlink_escape(&canonical_root, &rel, &rel_display_path(&rel))?;
            let backup = match tokio::fs::symlink_metadata(&original).await {
                Ok(meta) if meta.is_file() => {
                    let backup = scratch
                        .path()
                        .join(format!("source-{}", original_files.len()));
                    tokio::fs::copy(&original, &backup).await?;
                    Some(backup)
                }
                Ok(_) => {
                    return Err(RelayError::Internal(format!(
                        "'{}' must be a regular file",
                        rel.display()
                    )));
                }
                Err(err) if err.kind() == io::ErrorKind::NotFound => None,
                Err(err) => return Err(err.into()),
            };
            original_files.push((rel, backup));
        }
        tokio::fs::copy(&index, scratch.path().join("original-index")).await?;
        tokio::fs::copy(&index, scratch.path().join("snapshot-index")).await?;
        let mut checkpoint = Self {
            scratch,
            index,
            original_files,
            tree: String::new(),
        };
        git_output(checkpoint.git(root).args(["add", "--all", "--", "."])).await?;
        let tree = git_output(checkpoint.git(root).arg("write-tree")).await?;
        checkpoint.tree = super::output_text(&tree.stdout);
        Ok(checkpoint)
    }

    fn git(&self, root: &Path) -> Command {
        let mut cmd = git_command(root);
        cmd.env("GIT_INDEX_FILE", self.scratch.path().join("snapshot-index"))
            .env("GIT_OBJECT_DIRECTORY", self.scratch.path().join("objects"))
            .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES");
        cmd
    }

    pub(crate) async fn restore(&self, root: &Path) -> RelayResult<()> {
        // Index the attempted edits privately so read-tree also removes newly
        // created files. Ignored dependency caches stay outside this operation.
        git_output(self.git(root).args(["add", "--all", "--", "."])).await?;
        git_output(
            self.git(root)
                .args(["read-tree", "--reset", "-u", &self.tree]),
        )
        .await?;
        // Git checkout filters can change file bytes (for example LF to CRLF).
        // Restore the exact input files and remove newly created inputs even if
        // the attempted edit also changed their ignore rules.
        let canonical_root = std::fs::canonicalize(root)?;
        for (rel, backup) in &self.original_files {
            let path = ensure_no_symlink_escape(&canonical_root, rel, &rel_display_path(rel))?;
            if let Some(backup) = backup {
                let parent = path.parent().expect("validated worktree-relative path");
                tokio::fs::create_dir_all(parent).await?;
                let temp = tempfile::NamedTempFile::new_in(parent)?;
                tokio::fs::copy(backup, temp.path()).await?;
                temp.persist(&path).map_err(|err| err.error)?;
            } else if let Err(err) = tokio::fs::remove_file(path).await
                && err.kind() != io::ErrorKind::NotFound
            {
                return Err(err.into());
            }
        }
        let temp = tempfile::NamedTempFile::new_in(self.index.parent().expect("git index parent"))?;
        tokio::fs::copy(self.scratch.path().join("original-index"), temp.path()).await?;
        temp.persist(&self.index).map_err(|err| err.error)?;
        Ok(())
    }
}

struct StagedEditFile {
    target: PathBuf,
    content: String,
    permissions: Option<std::fs::Permissions>,
}

async fn prepare_file_edits(
    worktree_path: &Path,
    edits: &[FileEdit],
) -> RelayResult<BTreeMap<PathBuf, StagedEditFile>> {
    if edits.is_empty() {
        return Err(RelayError::Internal(
            "file_edits requires at least one edit entry".into(),
        ));
    }
    let canonical_root = std::fs::canonicalize(worktree_path)?;
    let mut staged = BTreeMap::new();
    for (idx, edit) in edits.iter().enumerate() {
        let edit_num = idx + 1;
        let rel = validate_confined_relative_path(&edit.file_path)?;
        let display_path = rel_display_path(&rel);
        if edit.old_text.as_deref() == Some("") {
            return Err(RelayError::Internal(format!(
                "edit #{edit_num} for '{display_path}': old_text must not be empty; omit old_text to overwrite or create the file"
            )));
        }
        let file = match staged.entry(rel.clone()) {
            std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::btree_map::Entry::Vacant(entry) => {
                let target = ensure_no_symlink_escape(&canonical_root, &rel, &display_path)?;
                let metadata = match tokio::fs::symlink_metadata(&target).await {
                    Ok(meta) if meta.is_file() => Some(meta),
                    Ok(_) => {
                        return Err(RelayError::Internal(format!(
                            "'{display_path}' is not a regular file"
                        )));
                    }
                    Err(err) if err.kind() == io::ErrorKind::NotFound => None,
                    Err(err) => return Err(err.into()),
                };
                let ignored = git_command(worktree_path)
                    .args(["check-ignore", "--quiet", "--"])
                    .arg(&rel)
                    .output()
                    .await?;
                match ignored.status.code() {
                    Some(0) => {
                        return Err(RelayError::Internal(format!(
                            "'{display_path}' is ignored by Git and cannot be reviewed"
                        )));
                    }
                    Some(1) => {}
                    _ => super::check_git_output(&ignored)?,
                }
                let content = if edit.old_text.is_some() {
                    if metadata.is_none() {
                        return Err(RelayError::Internal(format!(
                            "edit #{edit_num} for '{display_path}': file does not exist; omit old_text to create it"
                        )));
                    }
                    let text = tokio::fs::read_to_string(&target).await?;
                    if text.contains('\0') {
                        return Err(RelayError::Internal(format!(
                            "'{display_path}' is a binary file"
                        )));
                    }
                    text
                } else {
                    String::new()
                };
                entry.insert(StagedEditFile {
                    target,
                    content,
                    permissions: metadata.map(|m| m.permissions()),
                })
            }
        };
        if let Some(old) = &edit.old_text {
            file.content = apply_unique_replacement(
                &display_path,
                edit_num,
                &file.content,
                old,
                &edit.new_text,
            )?;
        } else {
            file.content.clone_from(&edit.new_text);
        }
    }
    Ok(staged)
}

async fn write_file_edits(
    worktree_path: &Path,
    staged: &BTreeMap<PathBuf, StagedEditFile>,
) -> RelayResult<()> {
    // Write only after every requested replacement has matched. Replacing a
    // directory entry keeps other hard links to its old contents untouched.
    for file in staged.values() {
        let parent = file
            .target
            .parent()
            .expect("validated worktree-relative path");
        tokio::fs::create_dir_all(parent).await?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        std::io::Write::write_all(&mut temp, file.content.as_bytes())?;
        if let Some(permissions) = &file.permissions {
            temp.as_file().set_permissions(permissions.clone())?;
        }
        temp.persist(&file.target).map_err(|err| err.error)?;
    }
    // Existing untracked files must also appear in the approval diff.
    git_output(
        git_command(worktree_path)
            .args(["--literal-pathspecs", "add", "--intent-to-add", "--"])
            .args(staged.keys()),
    )
    .await?;
    Ok(())
}

fn ensure_no_symlink_escape(canonical_root: &Path, rel: &Path, raw: &str) -> RelayResult<PathBuf> {
    let mut current = canonical_root.to_path_buf();
    let components: Vec<_> = rel.components().collect();
    for (idx, component) in components.iter().enumerate() {
        let Component::Normal(part) = component else {
            return Err(RelayError::Internal(format!(
                "invalid worktree relative path '{raw}'"
            )));
        };
        current.push(part);
        let is_last = idx + 1 == components.len();
        match std::fs::symlink_metadata(&current) {
            Ok(meta) => {
                #[cfg(windows)]
                let is_link = {
                    use std::os::windows::fs::MetadataExt;
                    meta.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT
                };
                #[cfg(not(windows))]
                let is_link = meta.file_type().is_symlink();
                if is_link {
                    return Err(RelayError::Internal(format!(
                        "symlink path '{raw}' is not allowed inside worktree edits"
                    )));
                }
                if !is_last && !meta.is_dir() {
                    return Err(RelayError::Internal(format!(
                        "parent component of '{raw}' is not a directory"
                    )));
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => break,
            Err(err) => return Err(err.into()),
        }
    }
    let target = canonical_root.join(rel);
    if target.exists() {
        let canonical_target = std::fs::canonicalize(&target)?;
        let resolved = canonical_target.strip_prefix(canonical_root).map_err(|_| {
            RelayError::Internal(format!("path '{raw}' resolves outside the task worktree"))
        })?;
        validate_confined_path_components(resolved, raw)?;
    }
    Ok(target)
}

fn apply_unique_replacement(
    display_path: &str,
    edit_num: usize,
    content: &str,
    old_text: &str,
    new_text: &str,
) -> RelayResult<String> {
    // Preserve unrelated mixed line endings. Normalize only the replacement
    // to the local line ending and match model-provided LF in CRLF source.
    let crlf_old = old_text.replace("\r\n", "\n").replace('\n', "\r\n");
    let needle = if !content.contains(old_text) && content.contains("\r\n") {
        crlf_old.as_str()
    } else {
        old_text
    };
    let mut offsets = Vec::new();
    let mut count = 0usize;
    let mut start = 0;
    let step = needle
        .chars()
        .next()
        .expect("validated nonempty old_text")
        .len_utf8();
    while let Some(pos) = content[start..].find(needle) {
        let pos = start + pos;
        if offsets.len() < 3 {
            offsets.push(pos);
        }
        count += 1;
        start = pos + step;
    }
    if count == 1 {
        let pos = offsets[0];
        let line_ending = content[pos..]
            .find('\n')
            .map(|n| pos + n)
            .or_else(|| content[..pos].rfind('\n'));
        let replacement = if line_ending.is_some_and(|n| content[..n].ends_with('\r')) {
            new_text.replace("\r\n", "\n").replace('\n', "\r\n")
        } else {
            new_text.to_owned()
        };
        let mut updated = content.to_owned();
        updated.replace_range(pos..pos + needle.len(), &replacement);
        return Ok(updated);
    }
    if count > 1 {
        let context = format_multi_match_context(display_path, content, &offsets);
        return Err(RelayError::Internal(format!(
            "edit #{edit_num} for '{display_path}': old_text matched {count} times (must match uniquely). Include more surrounding context lines.\n{context}"
        )));
    }
    let context = format_zero_match_context(display_path, content, old_text);
    Err(RelayError::Internal(format!(
        "edit #{edit_num} for '{display_path}': old_text did not match any text in the file. Inspect the numbered lines below and retry with exact text.\n{context}"
    )))
}

fn byte_offset_to_line(content: &str, byte_offset: usize) -> usize {
    content[..byte_offset].bytes().filter(|&b| b == b'\n').count() + 1
}

#[cfg(test)]
#[test]
fn replacements_preserve_local_line_endings_and_reject_overlapping_matches() {
    let original = "header\r\nalpha\r\nbeta\r\nfooter\n";
    let updated =
        apply_unique_replacement("notes", 1, original, "alpha\nbeta", "one\ntwo").unwrap();
    assert_eq!(updated, "header\r\none\r\ntwo\r\nfooter\n");
    let updated = apply_unique_replacement("notes", 1, original, "alpha", "one\ntwo").unwrap();
    assert_eq!(updated, "header\r\none\r\ntwo\r\nbeta\r\nfooter\n");
    assert!(apply_unique_replacement("notes", 1, "aaa", "aa", "x").is_err());
}

fn format_numbered_window(lines: &[&str], start_1based: usize, end_1based: usize) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let start = start_1based.max(1).min(lines.len());
    let end = end_1based.max(start).min(lines.len());
    let mut out = String::new();
    for line_no in start..=end {
        let line = lines[line_no - 1];
        let end = line.floor_char_boundary(384);
        let suffix = if end < line.len() { "..." } else { "" };
        out.push_str(&format!("{line_no:>4}: {}{suffix}\n", &line[..end]));
    }
    out
}

fn format_multi_match_context(display_path: &str, content: &str, offsets: &[usize]) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let match_lines: Vec<usize> = offsets
        .iter()
        .map(|&off| byte_offset_to_line(content, off))
        .collect();
    let line_list = match_lines
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let mut out = format!("Matches in {display_path} at lines: {line_list}\n");
    for &line_no in match_lines.iter().take(3) {
        out.push_str(&format!("--- around line {line_no} ---\n"));
        out.push_str(&format_numbered_window(
            &lines,
            line_no.saturating_sub(3),
            line_no.saturating_add(3),
        ));
    }
    out
}

fn format_zero_match_context(display_path: &str, content: &str, old_text: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    if lines.is_empty() {
        return format!("({display_path} is empty)");
    }
    let needle_lines: Vec<&str> = old_text
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .take(4)
        .map(|s| &s[..s.floor_char_boundary(384)])
        .collect();
    let mut best_line: Option<(usize, usize)> = None;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let mut score = 0usize;
        for needle in &needle_lines {
            if trimmed == *needle {
                score = score.max(needle.len() * 3);
            } else if trimmed.contains(needle) || needle.contains(trimmed) {
                score = score.max(trimmed.len().min(needle.len()) * 2);
            } else {
                let common_tokens: usize = needle
                    .split(|c: char| !c.is_alphanumeric() && c != '_')
                    .filter(|tok| tok.len() >= 3 && trimmed.contains(tok))
                    .map(str::len)
                    .sum();
                score = score.max(common_tokens);
            }
        }
        if score > 0 && best_line.is_none_or(|(best_score, _)| score > best_score) {
            best_line = Some((score, idx + 1));
        }
    }

    if let Some((_, line_no)) = best_line {
        let start = line_no.saturating_sub(8).max(1);
        let end = line_no.saturating_add(8).min(lines.len());
        format!(
            "Closest lines in {display_path} (lines {start}-{end} of {}):\n{}",
            lines.len(),
            format_numbered_window(&lines, start, end)
        )
    } else {
        let end = lines.len().min(40);
        format!(
            "Top lines of {display_path} (lines 1-{end} of {}):\n{}",
            lines.len(),
            format_numbered_window(&lines, 1, end)
        )
    }
}

fn rel_display_path(rel: &Path) -> String {
    rel.components()
        .filter_map(|c| match c {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Parse worktree-local file/line references from stderr and extract ±10 lines of numbered source.
pub fn extract_error_snippets(worktree_path: &Path, stderr: &str) -> Vec<ErrorSourceSnippet> {
    let Ok(canonical_root) = std::fs::canonicalize(worktree_path) else {
        return Vec::new();
    };
    let mut raw_refs: Vec<(String, u32)> = Vec::new();
    let mut recent_quoted_path: Option<(usize, String)> = None;

    for (idx, line) in stderr.lines().enumerate() {
        // 1. Python traceback format: File "<path>", line <N>
        if let Some((path, line_no)) = parse_python_traceback_line(line) {
            raw_refs.push((path, line_no));
            continue;
        }
        // 2. Generic `path:line(:col)?` format (Rust/Cargo, GCC, TS/JS, Python linters, etc.)
        let mut found_inline = false;
        for (path, line_no) in parse_colon_file_line_refs(line) {
            raw_refs.push((path, line_no));
            found_inline = true;
        }
        if found_inline {
            continue;
        }
        // 3. Multi-line parser diagnostics (e.g. `Failed to parse: \`uv.lock\`` followed by `at line 4`)
        if let Some(quoted) = extract_backtick_or_quoted_path(line) {
            recent_quoted_path = Some((idx, quoted));
        }
        if let Some((ref_idx, ref quoted_path)) = recent_quoted_path
            && idx <= ref_idx + 4
            && let Some(line_no) = parse_at_line_number(line)
        {
            raw_refs.push((quoted_path.clone(), line_no));
        }
    }

    // Prefer the innermost valid frames, then present them in traceback order.
    let mut snippets = Vec::new();
    let mut seen = BTreeSet::new();
    for (raw_path, line_no) in raw_refs.into_iter().rev() {
        let Some((path, full)) = resolve_snippet_target(&canonical_root, &raw_path) else {
            continue;
        };
        if line_no == 0 || !seen.insert((path.clone(), line_no)) {
            continue;
        }
        let start = line_no.saturating_sub(ERROR_SNIPPET_RADIUS).max(1);
        let end = line_no.saturating_add(ERROR_SNIPPET_RADIUS);
        let Ok(view) = super::read_source_window(&full, path, start, end, 8192) else {
            continue;
        };
        if line_no > view.end_line {
            continue;
        }
        snippets.push(ErrorSourceSnippet {
            file_path: view.file_path,
            line: line_no,
            snippet: view.content.trim_end_matches('\n').to_owned(),
        });
        if snippets.len() == MAX_ERROR_SNIPPETS {
            break;
        }
    }
    snippets.reverse();
    snippets
}

fn parse_python_traceback_line(line: &str) -> Option<(String, u32)> {
    let after_file = line.split_once("File \"")?.1;
    let (path, rest) = after_file.split_once("\", line ")?;
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let line_no: u32 = digits.parse().ok()?;
    (!path.is_empty() && line_no > 0).then(|| (path.to_owned(), line_no))
}

fn parse_colon_file_line_refs(line: &str) -> Vec<(String, u32)> {
    let mut results = Vec::new();
    for raw_token in line.split_whitespace() {
        let token = raw_token.trim_matches(|c: char| {
            matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | '\'' | '"' | '`')
        });
        if token.is_empty() || token.contains("://") {
            continue;
        }
        if let Some((path, line_no)) = split_path_and_line(token) {
            results.push((path.to_owned(), line_no));
        }
    }
    results
}

fn split_path_and_line(token: &str) -> Option<(&str, u32)> {
    // Support `path:line` and `path:line:col`, including Windows drive letters like `C:\dir\a.py:12:4`.
    let token = token.trim_end_matches(':');
    let bytes = token.as_bytes();
    let min_colon = if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        2
    } else {
        0
    };
    let rest = &token[min_colon..];
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.len() < 2 {
        return None;
    }
    let last = parts[parts.len() - 1];
    let prev = parts[parts.len() - 2];
    let (path_end_parts, line_str) = if parts.len() >= 3
        && !last.is_empty()
        && last.bytes().all(|b| b.is_ascii_digit())
        && !prev.is_empty()
        && prev.bytes().all(|b| b.is_ascii_digit())
    {
        (parts.len() - 2, prev)
    } else if !last.is_empty() && last.bytes().all(|b| b.is_ascii_digit()) {
        (parts.len() - 1, last)
    } else {
        return None;
    };

    let path_len = min_colon
        + parts[..path_end_parts].iter().map(|p| p.len()).sum::<usize>()
        + path_end_parts.saturating_sub(1);
    let path = &token[..path_len];
    if path.is_empty() || (!path.contains('/') && !path.contains('\\') && !path.contains('.')) {
        return None;
    }
    let line_no: u32 = line_str.parse().ok()?;
    (line_no > 0).then_some((path, line_no))
}

fn extract_backtick_or_quoted_path(line: &str) -> Option<String> {
    for quote in ['`', '\'', '"'] {
        let mut rest = line;
        while let Some((_, after_open)) = rest.split_once(quote) {
            let Some((candidate, after_close)) = after_open.split_once(quote) else {
                break;
            };
            let trimmed = candidate.trim();
            if !trimmed.is_empty()
                && !trimmed.contains(char::is_whitespace)
                && !trimmed.contains("://")
                && (trimmed.contains('.') || trimmed.contains('/') || trimmed.contains('\\'))
            {
                return Some(trimmed.to_owned());
            }
            rest = after_close;
        }
    }
    None
}

fn parse_at_line_number(line: &str) -> Option<u32> {
    let lower = line.to_ascii_lowercase();
    let idx = lower.find("line ")?;
    let after = &lower[idx + 5..];
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    let line_no: u32 = digits.parse().ok()?;
    (line_no > 0).then_some(line_no)
}

fn resolve_snippet_target(canonical_root: &Path, raw_path: &str) -> Option<(String, PathBuf)> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return None;
    }
    let candidate = Path::new(trimmed);
    let full = if candidate.is_absolute() {
        std::fs::canonicalize(candidate).ok()?
    } else {
        let normalized_sep = trimmed.replace('\\', "/");
        let rel = normalized_sep.strip_prefix("./").unwrap_or(&normalized_sep);
        std::fs::canonicalize(canonical_root.join(rel)).ok()?
    };
    let resolved = full.strip_prefix(canonical_root).ok()?;
    for component in resolved.components() {
        let Component::Normal(part) = component else {
            return None;
        };
        let name = part.to_string_lossy().to_ascii_lowercase();
        if matches!(
            name.as_str(),
            ".git"
                | ".venv"
                | "target"
                | ".relay"
                | ".relay-desktop"
                | "node_modules"
                | "__pycache__"
        ) || name.starts_with(".env")
            || name.ends_with(".db")
            || name.ends_with(".db-wal")
            || name.ends_with(".db-shm")
            || name.ends_with(".pem")
            || name.ends_with(".key")
        {
            return None;
        }
    }
    if !full.is_file() {
        return None;
    }
    let display = rel_display_path(resolved);
    (!display.is_empty()).then_some((display, full))
}

/// Extract the last missing-module diagnostic from a Python traceback.
pub fn detect_missing_python_module(stderr: &str) -> Option<String> {
    stderr.lines().rev().find_map(|line| {
        let module = line
            .trim_start()
            .strip_prefix("ModuleNotFoundError: No module named '")?
            .split_once('\'')?
            .0;
        (!module.is_empty()).then(|| module.to_owned())
    })
}

pub fn extract_metrics_json(stdout: &str) -> Option<JsonValue> {
    if let Ok(value) = serde_json::from_str::<Value>(stdout) {
        return Some(value.into());
    }

    let mut rest = stdout;
    let mut last_object = None;
    while let Some(start) = rest.find('{') {
        rest = &rest[start..];
        let mut values = serde_json::Deserializer::from_str(rest).into_iter::<Value>();
        if let Some(Ok(value)) = values.next() {
            last_object = Some(value.into());
            rest = &rest[values.byte_offset()..];
        } else {
            rest = &rest[1..];
        }
    }
    last_object
}

fn tail_lines(log: &str) -> String {
    let mut lines: Vec<_> = log.lines().rev().take(SMOKE_TAIL_LINES).collect();
    lines.reverse();
    lines.join("\n")
}
