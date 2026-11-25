use super::security::pm_error;
use super::{
    BoxFuture, DependencyRunOptions, EnvFileSnapshot, PackageManagerAdapter, ParsedDependencies,
    StarterPresets,
};
use crate::contracts::CommandPreset;
use crate::error::RelayResult;
use std::collections::BTreeMap;
use std::path::Path;

const RELAY_TOML: &str = "relay.toml";

#[derive(Debug, Default)]
pub struct GenericAdapter;

impl PackageManagerAdapter for GenericAdapter {
    fn name(&self) -> &'static str {
        "generic"
    }

    fn ecosystem(&self) -> &'static str {
        "generic"
    }

    fn priority(&self) -> u32 {
        1000
    }

    fn detect(&self, _repo_root: &Path) -> bool {
        true
    }

    fn generate_presets(&self, _repo_root: &Path) -> RelayResult<StarterPresets> {
        let mut presets = BTreeMap::new();
        presets.insert(
            "smoke".into(),
            CommandPreset {
                description: "Inspect repository Git status".into(),
                program: "git".into(),
                args: vec!["status".into(), "--short".into()],
                timeout_secs: 30,
                allowed_params: Vec::new(),
            },
        );
        Ok(StarterPresets {
            smoke_preset: "smoke".into(),
            presets,
        })
    }

    fn manifest_files(&self) -> &'static [&'static str] {
        &[RELAY_TOML]
    }

    fn lockfile_paths(&self) -> &'static [&'static str] {
        &[]
    }

    fn parse_dependencies(&self, _files: &EnvFileSnapshot) -> RelayResult<ParsedDependencies> {
        Ok(ParsedDependencies::default())
    }

    fn supports_dependency_add(&self) -> bool {
        false
    }

    fn add_dependency<'a>(
        &'a self,
        _worktree_path: &'a Path,
        spec: &'a str,
        _options: DependencyRunOptions,
    ) -> BoxFuture<'a, RelayResult<()>> {
        Box::pin(async move {
            Err(pm_error(
                "generic",
                format!(
                    "cannot add dependency '{spec}' in a generic repository with no managed package manifest; use patch_kind='code_diff' with explicit file edits or configure project.package_manager in relay.toml"
                ),
            ))
        })
    }
}
