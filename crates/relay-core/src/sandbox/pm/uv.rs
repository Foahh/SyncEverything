use super::security::{
    ensure_regular_or_absent, pm_error, stage_intent_to_add, validate_uv_dependency_spec,
};
use super::{
    BoxFuture, DependencyRunOptions, EnvFileSnapshot, PackageManagerAdapter, ParsedDependencies,
    StarterPresets,
};
use crate::contracts::CommandPreset;
use crate::error::{RelayError, RelayResult};
use crate::sandbox::execution::{run_dependency_command, worktree_command};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io;
use std::path::Path;

const PYPROJECT: &str = "pyproject.toml";
const UV_LOCK: &str = "uv.lock";

#[derive(Debug, Default)]
pub struct UvAdapter;

impl PackageManagerAdapter for UvAdapter {
    fn name(&self) -> &'static str {
        "uv"
    }

    fn ecosystem(&self) -> &'static str {
        "python"
    }

    fn priority(&self) -> u32 {
        10
    }

    fn detect(&self, repo_root: &Path) -> bool {
        repo_root.join(PYPROJECT).is_file() || repo_root.join(UV_LOCK).is_file()
    }

    fn generate_presets(&self, repo_root: &Path) -> RelayResult<StarterPresets> {
        let pyproject_raw = match std::fs::read_to_string(repo_root.join(PYPROJECT)) {
            Ok(raw) => raw,
            Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(err.into()),
        };
        let has_pytest = repo_root.join("tests").is_dir()
            || pyproject_raw.to_ascii_lowercase().contains("pytest");

        let mut presets = BTreeMap::new();
        presets.insert(
            "smoke".into(),
            CommandPreset {
                description: "Quick Python environment check via uv".into(),
                program: "uv".into(),
                args: vec![
                    "run".into(),
                    "python".into(),
                    "-c".into(),
                    "import sys; print(sys.version)".into(),
                ],
                timeout_secs: 60,
                allowed_params: Vec::new(),
            },
        );
        if has_pytest {
            presets.insert(
                "test".into(),
                CommandPreset {
                    description: "Run pytest suite in uv environment".into(),
                    program: "uv".into(),
                    args: vec!["run".into(), "pytest".into(), "-q".into()],
                    timeout_secs: 300,
                    allowed_params: Vec::new(),
                },
            );
        } else {
            presets.insert(
                "check".into(),
                CommandPreset {
                    description: "Verify uv Python runtime".into(),
                    program: "uv".into(),
                    args: vec!["run".into(), "python".into(), "--version".into()],
                    timeout_secs: 60,
                    allowed_params: Vec::new(),
                },
            );
        }

        Ok(StarterPresets {
            smoke_preset: "smoke".into(),
            presets,
        })
    }

    fn manifest_files(&self) -> &'static [&'static str] {
        &[PYPROJECT]
    }

    fn lockfile_paths(&self) -> &'static [&'static str] {
        &[UV_LOCK]
    }

    fn parse_dependencies(&self, files: &EnvFileSnapshot) -> RelayResult<ParsedDependencies> {
        let mut parsed = ParsedDependencies::default();
        if let Some(bytes) = files.get(PYPROJECT).and_then(Option::as_deref) {
            let project = parse_toml::<Pyproject>(bytes)?.project;
            for requirement in &project.dependencies {
                if let Some((name, spec)) = parse_pep508_requirement(requirement) {
                    parsed.declared.insert(name, spec);
                }
            }
        }
        if let Some(bytes) = files.get(UV_LOCK).and_then(Option::as_deref) {
            match parse_toml::<UvLockfile>(bytes) {
                Ok(lock) => {
                    for pkg in lock.package {
                        parsed.locked.insert(
                            normalize_python_name(&pkg.name),
                            pkg.version.unwrap_or_default(),
                        );
                    }
                }
                Err(error) => {
                    parsed
                        .notes
                        .push(format!("Could not read packages from {UV_LOCK}: {error}"));
                }
            }
        }
        Ok(parsed)
    }

    fn supports_dependency_add(&self) -> bool {
        true
    }

    fn add_dependency<'a>(
        &'a self,
        worktree_path: &'a Path,
        spec: &'a str,
        options: DependencyRunOptions,
    ) -> BoxFuture<'a, RelayResult<()>> {
        Box::pin(async move {
            let validated = validate_uv_dependency_spec(worktree_path, spec)?;
            ensure_regular_or_absent("uv", worktree_path, &[PYPROJECT, UV_LOCK], &[PYPROJECT])?;

            let mut command = worktree_command("uv", worktree_path, None);
            command.args(["add", "--no-sync", "--", &validated]);
            let output = run_dependency_command(command, "uv", options).await?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                let detail = if stderr.is_empty() {
                    format!("uv exited with {}", output.status)
                } else {
                    stderr
                };
                return Err(pm_error("uv", detail));
            }
            stage_intent_to_add("uv", worktree_path, self.lockfile_paths()).await?;
            Ok(())
        })
    }

    fn detect_missing_dependency(&self, stderr: &str) -> Option<String> {
        detect_missing_python_module(stderr)
    }

    fn fallback_package_spec(&self, missing_dependency: &str) -> Option<String> {
        python_package_for_module(missing_dependency)
    }
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

/// Map common Python import module names to their PyPI distribution names for offline fallback.
pub fn python_package_for_module(module: &str) -> Option<String> {
    let name = module.split('.').next()?;
    if !name.starts_with(|ch: char| ch.is_ascii_alphabetic())
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return None;
    }
    Some(match name {
        "sklearn" => "scikit-learn".into(),
        "yaml" => "pyyaml".into(),
        "cv2" => "opencv-python".into(),
        _ => name.to_ascii_lowercase().replace('_', "-"),
    })
}

#[derive(Deserialize)]
struct Pyproject {
    #[serde(default)]
    project: ProjectSection,
}

#[derive(Default, Deserialize)]
struct ProjectSection {
    #[serde(default)]
    dependencies: Vec<String>,
}

#[derive(Deserialize)]
struct UvLockfile {
    #[serde(default)]
    package: Vec<UvLockedPackage>,
}

#[derive(Deserialize)]
struct UvLockedPackage {
    name: String,
    #[serde(default)]
    version: Option<String>,
}

fn parse_toml<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> RelayResult<T> {
    let input = std::str::from_utf8(bytes)
        .map_err(|error| RelayError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    Ok(toml::from_str(input)?)
}

fn parse_pep508_requirement(requirement: &str) -> Option<(String, String)> {
    let requirement = requirement.trim();
    let end = requirement
        .find(|ch: char| !ch.is_ascii_alphanumeric() && !matches!(ch, '-' | '_' | '.'));
    let raw_name = &requirement[..end.unwrap_or(requirement.len())];
    if raw_name.is_empty() {
        return None;
    }
    let spec = requirement[raw_name.len()..].trim().to_owned();
    Some((normalize_python_name(raw_name), spec))
}

fn normalize_python_name(name: &str) -> String {
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
