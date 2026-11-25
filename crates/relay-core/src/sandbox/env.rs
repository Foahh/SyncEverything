use super::pm::{
    EnvFileSnapshot, PackageManagerAdapter, default_registry, diff_dependencies,
    resolve_adapter_for_repo,
};
use super::{WorktreeSandbox, check_git_output, git_command, git_output};
use crate::contracts::{EnvDiffReport, RelayManifest};
use crate::error::RelayResult;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::Path;

/// Fingerprint the resolved package manager's manifest and lockfile inputs in `repo_root`.
/// Format: `<pm>:blake3:<16hex>`.
pub fn compute_env_fingerprint(repo_root: &Path) -> RelayResult<String> {
    let adapter = resolve_adapter_for_repo(repo_root)?;
    let files = read_env_files_on_disk(repo_root, &adapter.manifest_and_lock_files())?;
    Ok(adapter.compute_fingerprint(&files))
}

impl WorktreeSandbox {
    pub async fn check_env_diff(&self, commit_sha: &str) -> RelayResult<EnvDiffReport> {
        let target_commit = if commit_sha == "HEAD" {
            self.env_diff_head().await?
        } else {
            Some(self.resolve_commit(commit_sha).await?)
        };

        let host_adapter = resolve_adapter_for_repo(self.repo_root())?;
        let host_files =
            read_env_files_on_disk(self.repo_root(), &host_adapter.manifest_and_lock_files())?;
        let (adapter, target_files) = if let Some(sha) = target_commit.as_deref() {
            let adapter = self.env_adapter_at_commit(sha).await?;
            let files = self.env_files_at_commit(sha, &adapter.manifest_and_lock_files()).await?;
            (adapter, files)
        } else {
            (host_adapter, EnvFileSnapshot::new())
        };

        let mut notes = Vec::new();
        if adapter.name() != host_adapter.name() {
            notes.push(format!(
                "Package manager changed: commit '{}' vs host '{}'",
                adapter.name(),
                host_adapter.name(),
            ));
        }

        let target_deps = adapter.parse_dependencies(&target_files)?;
        let host_deps = host_adapter.parse_dependencies(&host_files)?;
        let mut diff = diff_dependencies(&target_deps, &host_deps);
        if adapter.ecosystem() != host_adapter.ecosystem() {
            // Dependencies with the same name in different registries are unrelated.
            diff.missing_packages = target_deps.declared.keys().cloned().collect();
            diff.notes = target_deps.notes.into_iter().chain(host_deps.notes).collect();
        }
        notes.extend(diff.notes);

        let repo_env_fingerprint = adapter.compute_fingerprint(&target_files);
        let host_env_fingerprint = host_adapter.compute_fingerprint(&host_files);
        Ok(EnvDiffReport {
            commit_sha: target_commit.unwrap_or_else(|| commit_sha.to_owned()),
            ecosystem: adapter.ecosystem().to_owned(),
            package_manager: adapter.name().to_owned(),
            in_sync: repo_env_fingerprint == host_env_fingerprint,
            missing_packages: diff.missing_packages,
            repo_env_fingerprint,
            host_env_fingerprint,
            notes,
        })
    }

    pub(crate) async fn env_fingerprint_at_commit(&self, commit: &str) -> RelayResult<String> {
        let sha = self.resolve_commit(commit).await?;
        let adapter = self.env_adapter_at_commit(&sha).await?;
        let tracked_files = adapter.manifest_and_lock_files();
        let files = self.env_files_at_commit(&sha, &tracked_files).await?;
        Ok(adapter.compute_fingerprint(&files))
    }

    async fn env_adapter_at_commit(&self, sha: &str) -> RelayResult<&'static dyn PackageManagerAdapter> {
        if let Some(raw) = self.read_env_file_at_commit(sha, "relay.toml").await? {
            let raw = std::str::from_utf8(&raw)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
            let manifest: RelayManifest = toml::from_str(raw)?;
            if let Some(explicit) = manifest.project.package_manager.as_deref() {
                return super::pm::adapter_by_name(explicit);
            }
        } else if let Some(explicit) = self.host_package_manager_override()? {
            return super::pm::adapter_by_name(&explicit);
        }
        let root_files = self.root_files_at_commit(sha).await?;
        Ok(default_registry().detect_from_file_presence(|file| root_files.contains(file)))
    }

    fn host_package_manager_override(&self) -> RelayResult<Option<String>> {
        let manifest_path = self.repo_root().join("relay.toml");
        match std::fs::symlink_metadata(&manifest_path) {
            Ok(_) => Ok(super::load_manifest(self.repo_root())?.project.package_manager),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    async fn root_files_at_commit(&self, sha: &str) -> RelayResult<BTreeSet<String>> {
        let output = git_output(
            git_command(self.repo_root()).args(["ls-tree", "--name-only", sha]),
        )
        .await?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
    }

    async fn env_files_at_commit(
        &self,
        sha: &str,
        files: &[&str],
    ) -> RelayResult<EnvFileSnapshot> {
        let mut snapshot = BTreeMap::new();
        for &file in files {
            let contents = self.read_env_file_at_commit(sha, file).await?;
            snapshot.insert(file.to_owned(), contents);
        }
        Ok(snapshot)
    }

    async fn env_diff_head(&self) -> RelayResult<Option<String>> {
        let output = git_command(self.repo_root())
            .args(["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
            .output()
            .await?;
        if output.status.code() == Some(1) {
            return Ok(None);
        }
        check_git_output(&output)?;
        Ok(Some(super::output_text(&output.stdout)))
    }

    async fn read_env_file_at_commit(&self, sha: &str, file: &str) -> RelayResult<Option<Vec<u8>>> {
        let output = git_command(self.repo_root())
            .args(["show", &format!("{sha}:{file}")])
            .output()
            .await?;
        if output.status.success() {
            return Ok(Some(output.stdout));
        }

        // A missing file is a valid fingerprint input. Other git failures should be surfaced.
        let tree = git_output(git_command(self.repo_root()).args(["ls-tree", sha, "--", file]))
            .await?;
        if tree.stdout.is_empty() {
            return Ok(None);
        }
        check_git_output(&output).map(|()| Some(output.stdout))
    }
}

fn read_env_files_on_disk(root: &Path, files: &[&str]) -> RelayResult<EnvFileSnapshot> {
    let mut snapshot = BTreeMap::new();
    for &file in files {
        snapshot.insert(file.to_owned(), read_optional(&root.join(file))?);
    }
    Ok(snapshot)
}

fn read_optional(path: &Path) -> RelayResult<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
