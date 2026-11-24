use super::security::{
    ensure_regular_or_absent, pm_error, stage_intent_to_add, validate_cargo_dependency_spec,
};
use super::{BoxFuture, EnvFileSnapshot, PackageManagerAdapter, ParsedDependencies, StarterPresets};
use crate::contracts::CommandPreset;
use crate::error::{RelayError, RelayResult};
use crate::sandbox::execution::worktree_command;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::process::Stdio;

const CARGO_TOML: &str = "Cargo.toml";
const CARGO_LOCK: &str = "Cargo.lock";

#[derive(Debug, Default)]
pub struct CargoAdapter;

impl PackageManagerAdapter for CargoAdapter {
    fn name(&self) -> &'static str {
        "cargo"
    }

    fn ecosystem(&self) -> &'static str {
        "rust"
    }

    fn priority(&self) -> u32 {
        20
    }

    fn detect(&self, repo_root: &Path) -> bool {
        repo_root.join(CARGO_TOML).is_file()
    }

    fn generate_presets(&self, _repo_root: &Path) -> RelayResult<StarterPresets> {
        let mut presets = BTreeMap::new();
        presets.insert(
            "check".into(),
            CommandPreset {
                description: "Compile-check the Cargo workspace".into(),
                program: "cargo".into(),
                args: vec!["check".into()],
                timeout_secs: 900,
                allowed_params: Vec::new(),
            },
        );
        presets.insert(
            "test".into(),
            CommandPreset {
                description: "Run Cargo tests".into(),
                program: "cargo".into(),
                args: vec!["test".into()],
                timeout_secs: 1800,
                allowed_params: Vec::new(),
            },
        );
        Ok(StarterPresets {
            smoke_preset: "check".into(),
            presets,
        })
    }

    fn manifest_files(&self) -> &'static [&'static str] {
        &[CARGO_TOML]
    }

    fn lockfile_paths(&self) -> &'static [&'static str] {
        &[CARGO_LOCK]
    }

    fn parse_dependencies(&self, files: &EnvFileSnapshot) -> RelayResult<ParsedDependencies> {
        let mut parsed = ParsedDependencies::default();
        if let Some(bytes) = files.get(CARGO_TOML).and_then(Option::as_deref) {
            let manifest: toml::Value = parse_toml(bytes)?;
            for section in ["dependencies", "dev-dependencies", "build-dependencies"] {
                if let Some(table) = manifest.get(section).and_then(toml::Value::as_table) {
                    collect_cargo_deps(table, &mut parsed.declared);
                }
            }
            if let Some(table) = manifest
                .get("workspace")
                .and_then(|ws| ws.get("dependencies"))
                .and_then(toml::Value::as_table)
            {
                collect_cargo_deps(table, &mut parsed.declared);
            }
        }
        if let Some(bytes) = files.get(CARGO_LOCK).and_then(Option::as_deref) {
            match parse_toml::<CargoLockfile>(bytes) {
                Ok(lock) => {
                    for pkg in lock.package {
                        parsed.locked.insert(
                            normalize_cargo_name(&pkg.name),
                            pkg.version.unwrap_or_default(),
                        );
                    }
                }
                Err(error) => {
                    parsed
                        .notes
                        .push(format!("Could not read packages from {CARGO_LOCK}: {error}"));
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
    ) -> BoxFuture<'a, RelayResult<()>> {
        Box::pin(async move {
            let validated = validate_cargo_dependency_spec(spec)?;
            ensure_regular_or_absent("cargo", worktree_path, &[CARGO_TOML, CARGO_LOCK], &[CARGO_TOML])?;

            let mut args = vec!["add".to_owned()];
            if env_flag_enabled("CARGO_NET_OFFLINE") || env_flag_enabled("RELAY_OFFLINE") {
                args.push("--offline".to_owned());
            }
            args.push("--".to_owned());
            args.push(validated);

            let output = worktree_command("cargo", worktree_path, None)
                .args(&args)
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .output()
                .await?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                let detail = if stderr.is_empty() {
                    format!("cargo exited with {}", output.status)
                } else {
                    stderr
                };
                return Err(pm_error("cargo", detail));
            }
            stage_intent_to_add("cargo", worktree_path, self.lockfile_paths()).await?;
            Ok(())
        })
    }

    fn detect_missing_dependency(&self, stderr: &str) -> Option<String> {
        detect_missing_cargo_crate(stderr)
    }
}

fn env_flag_enabled(key: &str) -> bool {
    std::env::var(key)
        .ok()
        .is_some_and(|val| matches!(val.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
}

fn collect_cargo_deps(table: &toml::map::Map<String, toml::Value>, out: &mut BTreeMap<String, String>) {
    for (dep_key, value) in table {
        let (pkg_name, version) = match value {
            toml::Value::String(ver) => (dep_key.as_str(), ver.trim().to_owned()),
            toml::Value::Table(spec) => {
                let actual_name = spec
                    .get("package")
                    .and_then(toml::Value::as_str)
                    .unwrap_or(dep_key.as_str());
                let ver = spec
                    .get("version")
                    .and_then(toml::Value::as_str)
                    .map(str::trim)
                    .unwrap_or("")
                    .to_owned();
                (actual_name, ver)
            }
            _ => (dep_key.as_str(), String::new()),
        };
        let normalized = normalize_cargo_name(pkg_name);
        if !normalized.is_empty() {
            out.entry(normalized)
                .and_modify(|existing| {
                    if existing.is_empty() && !version.is_empty() {
                        existing.clone_from(&version);
                    }
                })
                .or_insert(version);
        }
    }
}

#[derive(Deserialize)]
struct CargoLockfile {
    #[serde(default)]
    package: Vec<CargoLockedPackage>,
}

#[derive(Deserialize)]
struct CargoLockedPackage {
    name: String,
    #[serde(default)]
    version: Option<String>,
}

fn parse_toml<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> RelayResult<T> {
    let input = std::str::from_utf8(bytes)
        .map_err(|error| RelayError::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
    Ok(toml::from_str(input)?)
}

fn normalize_cargo_name(name: &str) -> String {
    name.trim().to_ascii_lowercase().replace('_', "-")
}

fn detect_missing_cargo_crate(stderr: &str) -> Option<String> {
    for line in stderr.lines().rev() {
        let trimmed = line.trim();
        for prefix in [
            "error[E0432]: unresolved import `",
            "error[E0433]: failed to resolve: use of undeclared crate or module `",
            "error[E0463]: can't find crate for `",
        ] {
            if let Some(rest) = trimmed.strip_prefix(prefix)
                && let Some((ident, _)) = rest.split_once('`')
            {
                let root = ident.split("::").next().unwrap_or(ident);
                if !root.is_empty()
                    && !matches!(root, "crate" | "self" | "super" | "std" | "core" | "alloc")
                {
                    return Some(root.to_owned());
                }
            }
        }
    }
    None
}
