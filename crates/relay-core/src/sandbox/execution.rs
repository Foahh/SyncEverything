use super::{
    WorktreeSandbox, check_git_output, git_command, git_output, load_manifest, resolve_preset,
    working_diff,
};
use crate::contracts::{CommandPreset, JsonValue, PatchKind, PatchProposal, RelayManifest, TaskId};
use crate::error::{RelayError, RelayResult};
use chrono::Utc;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use uuid::Uuid;

const SMOKE_TAIL_LINES: usize = 30;

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
    pub raw_log: String,
}

impl WorktreeSandbox {
    fn authorized_smoke_preset<'a>(&self, manifest: &'a RelayManifest) -> RelayResult<&'a str> {
        if load_manifest(self.repo_root())?.project.smoke_preset != manifest.project.smoke_preset {
            return Err(RelayError::Internal("smoke preset differs from the host relay.toml".into()));
        }
        Ok(&manifest.project.smoke_preset)
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
        let mut command = worktree_command(&preset.program, &worktree_path);
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
        Ok(PreflightReport {
            task_id,
            worktree_path: worktree_path.to_owned(),
            worktree_branch: worktree_branch.into(),
            smoke_passed: output.success,
            exit_code: output.exit_code,
            stdout_tail: tail_lines(&output.stdout),
            stderr_tail: tail_lines(&output.stderr),
            detected_missing_module: detect_missing_python_module(&output.stderr),
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
        kind: PatchKind,
        summary: &str,
        content: &str,
    ) -> RelayResult<PatchProposal> {
        self.validate_task_worktree(task_id, worktree_path, worktree_branch)
            .await?;
        let worktree_path = std::path::absolute(worktree_path)?;
        let smoke_preset = self.authorized_smoke_preset(manifest)?;
        let diff_before = working_diff(&worktree_path).await?;
        match kind {
            PatchKind::UvSpec => add_uv_dependency(&worktree_path, content).await?,
            PatchKind::CodeDiff => apply_code_diff(&worktree_path, content).await?,
        }
        let diff_preview = working_diff(&worktree_path).await?;
        if diff_preview.is_empty() || diff_preview == diff_before {
            return Err(RelayError::Internal("patch produced no worktree diff".into()));
        }
        let smoke = self
            .run_preset(
                &worktree_path,
                manifest,
                smoke_preset,
                &BTreeMap::new(),
            )
            .await?;
        // Smoke can reconcile a tracked lockfile. Approval must cover the final diff.
        let diff_preview = working_diff(&worktree_path).await?;
        if diff_preview.is_empty() || diff_preview == diff_before {
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

fn worktree_command(program: &str, worktree_path: &Path) -> Command {
    let mut command = Command::new(program);
    command.current_dir(worktree_path).stdin(Stdio::null());
    for key in ["VIRTUAL_ENV", "CONDA_PREFIX", "UV_PROJECT", "UV_WORKING_DIRECTORY"] {
        command.env_remove(key);
    }
    command.env("UV_PROJECT_ENVIRONMENT", worktree_path.join(".venv"));
    command
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

    let output = worktree_command("uv", worktree_path)
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

async fn apply_code_diff(worktree_path: &Path, diff: &str) -> RelayResult<()> {
    let mut child = git_command(worktree_path)
        .args(["apply", "--whitespace=nowarn", "--intent-to-add", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| RelayError::Git("git apply stdin was not captured".into()))?;
    let send_patch = async move {
        // Dropping stdin signals EOF even if writing the patch fails.
        stdin.write_all(diff.as_bytes()).await
    };
    let (write_result, output) = tokio::join!(send_patch, child.wait_with_output());
    check_git_output(&output?)?;
    write_result?;
    Ok(())
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
