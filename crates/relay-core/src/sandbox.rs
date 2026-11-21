use crate::contracts::{CommandPreset, RelayManifest};
use crate::error::{RelayError, RelayResult};
use std::path::{Path, PathBuf};
use tokio::process::Command;

/// Load and validate project-level command whitelist from `<repo>/relay.toml`.
pub fn load_manifest(repo_root: &Path) -> RelayResult<RelayManifest> {
    let manifest_path = repo_root.join("relay.toml");
    let raw = std::fs::read_to_string(&manifest_path)?;
    let manifest: RelayManifest = toml::from_str(&raw)?;
    Ok(manifest)
}

/// Resolve a named preset against the whitelist in `relay.toml`.
pub fn resolve_preset<'a>(
    manifest: &'a RelayManifest,
    preset_name: &str,
) -> RelayResult<&'a CommandPreset> {
    manifest
        .presets
        .get(preset_name)
        .ok_or_else(|| RelayError::UnknownPreset(preset_name.to_string()))
}

/// Async CLI wrapper around system `git` and `uv` (never libgit2).
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
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.repo_root)
            .args(["rev-parse", "HEAD"])
            .output()
            .await?;
        if !output.status.success() {
            return Err(RelayError::Git(
                String::from_utf8_lossy(&output.stderr).trim().to_string(),
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_relay_manifest_and_enforces_whitelist() {
        let raw = r#"
[project]
repo_id = "forecast-bench"
name = "Forecasting Benchmark"
smoke_preset = "smoke"

[presets.smoke]
description = "3-second smoke check"
program = "uv"
args = ["run", "python", "src/forecast.py", "--smoke"]
timeout_secs = 30
allowed_params = ["window"]
"#;
        let manifest: RelayManifest = toml::from_str(raw).unwrap();
        assert!(resolve_preset(&manifest, "smoke").is_ok());
        assert!(matches!(
            resolve_preset(&manifest, "rm_rf"),
            Err(RelayError::UnknownPreset(_))
        ));
    }
}
