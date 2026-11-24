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
const ERROR_SNIPPET_RADIUS: usize = 10;
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
            extract_error_snippets(worktree_path, &output.stderr)
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
        self.validate_task_worktree(task_id, worktree_path, worktree_branch)
            .await?;
        let worktree_path = std::path::absolute(worktree_path)?;
        let smoke_preset = self.authorized_smoke_preset(manifest)?;
        let kind = patch.kind();
        let diff_before = working_diff(&worktree_path).await?;
        let apply_result = match patch {
            PatchInput::UvSpec(spec) => add_uv_dependency(&worktree_path, spec).await,
            PatchInput::FileEdits(edits) => apply_file_edits(&worktree_path, edits).await,
        };
        if let Err(error) = apply_result {
            if matches!(patch, PatchInput::UvSpec(_)) {
                let _ = rollback_worktree(&worktree_path).await;
            }
            return Err(error);
        }
        let diff_preview = working_diff(&worktree_path).await?;
        if diff_preview.is_empty() || diff_preview == diff_before {
            let _ = rollback_worktree(&worktree_path).await;
            return Err(RelayError::Internal("patch produced no worktree diff".into()));
        }
        let smoke = match self
            .run_preset(
                &worktree_path,
                manifest,
                smoke_preset,
                &BTreeMap::new(),
            )
            .await
        {
            Ok(smoke) => smoke,
            Err(error) => {
                let _ = rollback_worktree(&worktree_path).await;
                return Err(error);
            }
        };
        // Smoke can reconcile a tracked lockfile. Approval must cover the final diff.
        let diff_preview = working_diff(&worktree_path).await?;
        if !smoke.success {
            rollback_worktree(&worktree_path).await?;
        } else if diff_preview.is_empty() || diff_preview == diff_before {
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

async fn rollback_worktree(worktree_path: &Path) -> RelayResult<()> {
    git_output(git_command(worktree_path).args(["reset", "--hard", "HEAD"])).await?;
    git_output(git_command(worktree_path).args(["clean", "-fd"])).await?;
    Ok(())
}

struct StagedEditFile {
    display_path: String,
    target_path: PathBuf,
    content: String,
    existed: bool,
}

async fn apply_file_edits(worktree_path: &Path, edits: &[FileEdit]) -> RelayResult<()> {
    if edits.is_empty() {
        return Err(RelayError::Internal(
            "file_edits requires at least one edit entry".into(),
        ));
    }
    let canonical_root = std::fs::canonicalize(worktree_path)?;
    let mut staged: BTreeMap<PathBuf, StagedEditFile> = BTreeMap::new();

    // Phase 1: Validate every path and apply every edit atomically in memory.
    for (idx, edit) in edits.iter().enumerate() {
        let edit_num = idx + 1;
        let rel = validate_confined_relative_path(&edit.file_path)?;
        let display_path = rel_display_path(&rel);
        let target_path = ensure_no_symlink_escape(&canonical_root, &rel, &edit.file_path)?;

        match edit.old_text.as_deref() {
            Some("") => {
                return Err(RelayError::Internal(format!(
                    "edit #{edit_num} for '{display_path}': old_text must not be empty; omit old_text or set it to null to overwrite or create the entire file"
                )));
            }
            Some(old_text) => {
                if !staged.contains_key(&rel) {
                    let existed = match tokio::fs::symlink_metadata(&target_path).await {
                        Ok(meta) => {
                            if meta.file_type().is_symlink() {
                                return Err(RelayError::Internal(format!(
                                    "symlink target '{display_path}' is not allowed in file_edits"
                                )));
                            }
                            if !meta.is_file() {
                                return Err(RelayError::Internal(format!(
                                    "'{display_path}' is not a regular file"
                                )));
                            }
                            true
                        }
                        Err(err) if err.kind() == io::ErrorKind::NotFound => false,
                        Err(err) => return Err(err.into()),
                    };
                    if !existed {
                        return Err(RelayError::Internal(format!(
                            "edit #{edit_num} for '{display_path}': file does not exist; omit old_text to create a new file"
                        )));
                    }
                    let raw = tokio::fs::read(&target_path).await?;
                    if raw.iter().take(4096).any(|b| *b == 0) {
                        return Err(RelayError::Internal(format!(
                            "edit #{edit_num} for '{display_path}': cannot edit binary file"
                        )));
                    }
                    let text = String::from_utf8(raw).map_err(|_| {
                        RelayError::Internal(format!(
                            "edit #{edit_num} for '{display_path}': file is not valid UTF-8"
                        ))
                    })?;
                    staged.insert(
                        rel.clone(),
                        StagedEditFile {
                            display_path: display_path.clone(),
                            target_path,
                            content: text,
                            existed: true,
                        },
                    );
                }

                let entry = staged.get_mut(&rel).expect("staged file loaded");
                let updated = apply_unique_replacement(
                    &entry.display_path,
                    edit_num,
                    &entry.content,
                    old_text,
                    &edit.new_text,
                )?;
                entry.content = updated;
            }
            None => {
                if let Some(entry) = staged.get_mut(&rel) {
                    entry.content.clone_from(&edit.new_text);
                } else {
                    let existed = match tokio::fs::symlink_metadata(&target_path).await {
                        Ok(meta) => {
                            if meta.file_type().is_symlink() {
                                return Err(RelayError::Internal(format!(
                                    "symlink target '{display_path}' is not allowed in file_edits"
                                )));
                            }
                            if meta.is_dir() {
                                return Err(RelayError::Internal(format!(
                                    "'{display_path}' is a directory, not a file"
                                )));
                            }
                            true
                        }
                        Err(err) if err.kind() == io::ErrorKind::NotFound => false,
                        Err(err) => return Err(err.into()),
                    };
                    staged.insert(
                        rel.clone(),
                        StagedEditFile {
                            display_path,
                            target_path,
                            content: edit.new_text.clone(),
                            existed,
                        },
                    );
                }
            }
        }
    }

    // Phase 2: Commit all validated changes to disk; rollback if an I/O error interrupts halfway.
    let write_result: RelayResult<()> = async {
        let mut created_paths = Vec::new();
        for file in staged.values() {
            if let Some(parent) = file.target_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&file.target_path, file.content.as_bytes()).await?;
            if !file.existed {
                created_paths.push(file.display_path.as_str());
            }
        }
        if !created_paths.is_empty() {
            let mut cmd = git_command(worktree_path);
            cmd.args(["add", "--intent-to-add", "--"]);
            for path in created_paths {
                cmd.arg(path);
            }
            git_output(&mut cmd).await?;
        }
        Ok(())
    }
    .await;

    if let Err(error) = write_result {
        let _ = rollback_worktree(worktree_path).await;
        return Err(error);
    }
    Ok(())
}

fn ensure_no_symlink_escape(
    canonical_root: &Path,
    rel: &Path,
    raw: &str,
) -> RelayResult<PathBuf> {
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
                if meta.file_type().is_symlink() {
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
    let exact_matches: Vec<usize> = content.match_indices(old_text).map(|(pos, _)| pos).collect();
    if exact_matches.len() == 1 {
        let pos = exact_matches[0];
        let mut out = String::with_capacity(content.len() - old_text.len() + new_text.len());
        out.push_str(&content[..pos]);
        out.push_str(new_text);
        out.push_str(&content[pos + old_text.len()..]);
        return Ok(out);
    }
    if exact_matches.len() > 1 {
        let context = format_multi_match_context(display_path, content, &exact_matches);
        return Err(RelayError::Internal(format!(
            "edit #{edit_num} for '{display_path}': old_text matched {} times (must match uniquely). Include more surrounding context lines.\n{context}",
            exact_matches.len()
        )));
    }

    // Handle CRLF files when the model provides LF strings.
    if content.contains("\r\n") && !old_text.contains("\r\n") {
        let normalized = content.replace("\r\n", "\n");
        let lf_matches: Vec<usize> = normalized.match_indices(old_text).map(|(pos, _)| pos).collect();
        if lf_matches.len() == 1 {
            let pos = lf_matches[0];
            let mut replaced =
                String::with_capacity(normalized.len() - old_text.len() + new_text.len());
            replaced.push_str(&normalized[..pos]);
            replaced.push_str(&new_text.replace("\r\n", "\n"));
            replaced.push_str(&normalized[pos + old_text.len()..]);
            return Ok(replaced.replace('\n', "\r\n"));
        }
        if lf_matches.len() > 1 {
            let context = format_multi_match_context(display_path, &normalized, &lf_matches);
            return Err(RelayError::Internal(format!(
                "edit #{edit_num} for '{display_path}': old_text matched {} times (must match uniquely). Include more surrounding context lines.\n{context}",
                lf_matches.len()
            )));
        }
    }

    let context = format_zero_match_context(display_path, content, old_text);
    Err(RelayError::Internal(format!(
        "edit #{edit_num} for '{display_path}': old_text did not match any text in the file. Inspect the numbered lines below and retry with exact text.\n{context}"
    )))
}

fn byte_offset_to_line(content: &str, byte_offset: usize) -> usize {
    content[..byte_offset].bytes().filter(|&b| b == b'\n').count() + 1
}

fn format_numbered_window(lines: &[&str], start_1based: usize, end_1based: usize) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let start = start_1based.max(1).min(lines.len());
    let end = end_1based.max(start).min(lines.len());
    let mut out = String::new();
    for line_no in start..=end {
        out.push_str(&format!("{line_no:>4}: {}\n", lines[line_no - 1]));
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
    let lines: Vec<&str> = stderr.lines().collect();
    let mut recent_quoted_path: Option<(usize, String)> = None;

    for (idx, line) in lines.iter().enumerate() {
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

    // Resolve against worktree, filter excluded/sensitive dirs, deduplicate keeping latest occurrence.
    let mut resolved_order: Vec<(String, u32, PathBuf)> = Vec::new();
    for (raw_path, line_no) in raw_refs {
        if line_no == 0 {
            continue;
        }
        let Some((display_path, full_path)) = resolve_snippet_target(&canonical_root, &raw_path)
        else {
            continue;
        };
        if let Some(pos) = resolved_order
            .iter()
            .position(|(p, l, _)| p == &display_path && *l == line_no)
        {
            resolved_order.remove(pos);
        }
        resolved_order.push((display_path, line_no, full_path));
    }

    // Keep the innermost/last MAX_ERROR_SNIPPETS locations (closest to the crash site).
    if resolved_order.len() > MAX_ERROR_SNIPPETS {
        let drop_count = resolved_order.len() - MAX_ERROR_SNIPPETS;
        resolved_order.drain(0..drop_count);
    }

    let mut snippets = Vec::new();
    for (display_path, line_no, full_path) in resolved_order {
        let Ok(bytes) = std::fs::read(&full_path) else {
            continue;
        };
        if bytes.iter().take(4096).any(|b| *b == 0) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let file_lines: Vec<&str> = text.lines().collect();
        if file_lines.is_empty() {
            continue;
        }
        let clamped_line = (line_no as usize).min(file_lines.len()).max(1);
        let start = clamped_line.saturating_sub(ERROR_SNIPPET_RADIUS).max(1);
        let end = clamped_line
            .saturating_add(ERROR_SNIPPET_RADIUS)
            .min(file_lines.len());
        let snippet = format_numbered_window(&file_lines, start, end)
            .trim_end_matches('\n')
            .to_owned();
        snippets.push(ErrorSourceSnippet {
            file_path: display_path,
            line: line_no,
            snippet,
        });
    }
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
