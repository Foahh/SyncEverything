use super::{WorktreeSandbox, check_git_output, git_command, git_output};
use crate::contracts::EnvDiffReport;
use crate::error::{RelayError, RelayResult};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::io;
use std::path::Path;

const PYPROJECT: &str = "pyproject.toml";
const UV_LOCK: &str = "uv.lock";

/// Fingerprint the dependency inputs in the given checkout, including missing files.
pub fn compute_env_fingerprint(repo_root: &Path) -> RelayResult<String> {
    let files = EnvFiles::read(repo_root)?;
    Ok(files.fingerprint())
}

impl WorktreeSandbox {
    pub async fn check_env_diff(&self, commit_sha: &str) -> RelayResult<EnvDiffReport> {
        let head = self.env_diff_head().await?;
        let target_commit = if commit_sha == "HEAD" {
            head.clone()
        } else {
            Some(self.resolve_commit(commit_sha).await?)
        };
        let host = EnvFiles::read(self.repo_root())?;
        let mut target = if let Some(sha) = target_commit.as_deref() {
            self.env_files_at_commit(sha).await?
        } else {
            EnvFiles {
                pyproject: None,
                uv_lock: None,
            }
        };
        let mut notes = Vec::new();
        if target_commit == head {
            for (name, target_file, host_file) in [
                (PYPROJECT, &mut target.pyproject, &host.pyproject),
                (UV_LOCK, &mut target.uv_lock, &host.uv_lock),
            ] {
                if target_file.is_none() && host_file.is_some() {
                    target_file.clone_from(host_file);
                    notes.push(format!("{name} is absent from HEAD. Used the working copy."));
                }
            }
        }

        let target_packages = project_dependencies(target.pyproject.as_deref())?;
        let mut host_packages = project_dependencies(host.pyproject.as_deref())?;
        if let Some(lock) = host.uv_lock.as_deref() {
            match parse_toml::<Lockfile>(lock) {
                Ok(lock) => host_packages.extend(
                    lock.package
                        .into_iter()
                        .map(|package| normalize_name(&package.name)),
                ),
                Err(error) => notes.push(format!("Could not read packages from {UV_LOCK}: {error}")),
            }
        }
        let repo_env_fingerprint = target.fingerprint();
        let host_env_fingerprint = host.fingerprint();
        Ok(EnvDiffReport {
            commit_sha: target_commit.unwrap_or_else(|| commit_sha.to_owned()),
            in_sync: repo_env_fingerprint == host_env_fingerprint,
            missing_packages: target_packages.difference(&host_packages).cloned().collect(),
            repo_env_fingerprint,
            host_env_fingerprint,
            notes,
        })
    }

    pub(crate) async fn env_fingerprint_at_commit(&self, commit: &str) -> RelayResult<String> {
        let sha = self.resolve_commit(commit).await?;
        Ok(self.env_files_at_commit(&sha).await?.fingerprint())
    }

    async fn env_files_at_commit(&self, sha: &str) -> RelayResult<EnvFiles> {
        let (pyproject, uv_lock) = tokio::try_join!(
            self.read_env_file_at_commit(sha, PYPROJECT),
            self.read_env_file_at_commit(sha, UV_LOCK),
        )?;
        Ok(EnvFiles { pyproject, uv_lock })
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

struct EnvFiles {
    pyproject: Option<Vec<u8>>,
    uv_lock: Option<Vec<u8>>,
}

impl EnvFiles {
    fn read(root: &Path) -> RelayResult<Self> {
        Ok(Self {
            pyproject: read_optional(&root.join(PYPROJECT))?,
            uv_lock: read_optional(&root.join(UV_LOCK))?,
        })
    }

    fn fingerprint(&self) -> String {
        let mut hasher = blake3::Hasher::new();
        for (file, contents) in [(PYPROJECT, &self.pyproject), (UV_LOCK, &self.uv_lock)] {
            hasher.update(file.as_bytes());
            match contents {
                Some(bytes) => {
                    hasher.update(&[1]);
                    hasher.update(&(bytes.len() as u64).to_le_bytes());
                    hasher.update(bytes);
                }
                None => {
                    hasher.update(&[0]);
                }
            }
        }
        let digest = hasher.finalize().to_hex();
        format!("uv:blake3:{}", &digest.as_str()[..16])
    }
}

fn read_optional(path: &Path) -> RelayResult<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[derive(Deserialize)]
struct Pyproject {
    #[serde(default)]
    project: Project,
}

#[derive(Default, Deserialize)]
struct Project {
    #[serde(default)]
    dependencies: Vec<String>,
}

#[derive(Deserialize)]
struct Lockfile {
    #[serde(default)]
    package: Vec<LockedPackage>,
}

#[derive(Deserialize)]
struct LockedPackage {
    name: String,
}

fn parse_toml<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> RelayResult<T> {
    let input = std::str::from_utf8(bytes)
        .map_err(|error| RelayError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    Ok(toml::from_str(input)?)
}

fn project_dependencies(pyproject: Option<&[u8]>) -> RelayResult<BTreeSet<String>> {
    let Some(bytes) = pyproject else {
        return Ok(BTreeSet::new());
    };
    let project = parse_toml::<Pyproject>(bytes)?.project;
    Ok(project
        .dependencies
        .iter()
        .filter_map(|requirement| {
            // The PEP 508 name precedes extras, version constraints, URLs and markers.
            let requirement = requirement.trim_start();
            let end = requirement
                .find(|ch: char| !ch.is_ascii_alphanumeric() && !matches!(ch, '-' | '_' | '.'));
            let name = &requirement[..end.unwrap_or(requirement.len())];
            (!name.is_empty()).then(|| normalize_name(name))
        })
        .collect())
}

fn normalize_name(name: &str) -> String {
    let mut normalized = String::with_capacity(name.len());
    for ch in name.chars() {
        if matches!(ch, '-' | '_' | '.') {
            if !normalized.ends_with('-') {
                normalized.push('-');
            }
        } else {
            normalized.push(ch.to_ascii_lowercase());
        }
    }
    normalized
}
