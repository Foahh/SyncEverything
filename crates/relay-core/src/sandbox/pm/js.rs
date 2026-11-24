use super::security::{
    ensure_regular_or_absent, pm_error, stage_intent_to_add, validate_js_dependency_spec,
};
use super::{BoxFuture, EnvFileSnapshot, PackageManagerAdapter, ParsedDependencies, StarterPresets};
use crate::contracts::CommandPreset;
use crate::error::{RelayError, RelayResult};
use crate::sandbox::execution::worktree_command;
use serde_json::Value;
use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::process::Stdio;

const PACKAGE_JSON: &str = "package.json";
const BUN_LOCK: &str = "bun.lock";
const BUN_LOCKB: &str = "bun.lockb";
const PNPM_LOCK: &str = "pnpm-lock.yaml";
const YARN_LOCK: &str = "yarn.lock";
const NPM_LOCK: &str = "package-lock.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsVariant {
    Bun,
    Pnpm,
    Yarn,
    Npm,
}

#[derive(Debug, Clone, Copy)]
pub struct JsAdapter {
    variant: JsVariant,
}

impl JsAdapter {
    pub const fn new(variant: JsVariant) -> Self {
        Self { variant }
    }

    pub const fn bun() -> Self {
        Self::new(JsVariant::Bun)
    }

    pub const fn pnpm() -> Self {
        Self::new(JsVariant::Pnpm)
    }

    pub const fn yarn() -> Self {
        Self::new(JsVariant::Yarn)
    }

    pub const fn npm() -> Self {
        Self::new(JsVariant::Npm)
    }

    pub const fn variant(&self) -> JsVariant {
        self.variant
    }

    fn add_args(&self, validated_spec: &str) -> Vec<String> {
        match self.variant {
            JsVariant::Bun => vec![
                "add".into(),
                "--ignore-scripts".into(),
                validated_spec.to_owned(),
            ],
            JsVariant::Pnpm => vec![
                "add".into(),
                "--ignore-scripts".into(),
                validated_spec.to_owned(),
            ],
            JsVariant::Yarn => vec![
                "add".into(),
                "--ignore-scripts".into(),
                validated_spec.to_owned(),
            ],
            JsVariant::Npm => vec![
                "install".into(),
                "--ignore-scripts".into(),
                validated_spec.to_owned(),
            ],
        }
    }
}

impl PackageManagerAdapter for JsAdapter {
    fn name(&self) -> &'static str {
        match self.variant {
            JsVariant::Bun => "bun",
            JsVariant::Pnpm => "pnpm",
            JsVariant::Yarn => "yarn",
            JsVariant::Npm => "npm",
        }
    }

    fn ecosystem(&self) -> &'static str {
        "javascript"
    }

    fn priority(&self) -> u32 {
        match self.variant {
            JsVariant::Bun => 30,
            JsVariant::Pnpm => 31,
            JsVariant::Yarn => 32,
            JsVariant::Npm => 33,
        }
    }

    fn detect(&self, repo_root: &Path) -> bool {
        if !repo_root.join(PACKAGE_JSON).is_file() {
            return false;
        }
        match self.variant {
            JsVariant::Bun => repo_root.join(BUN_LOCK).is_file() || repo_root.join(BUN_LOCKB).is_file(),
            JsVariant::Pnpm => repo_root.join(PNPM_LOCK).is_file(),
            JsVariant::Yarn => repo_root.join(YARN_LOCK).is_file(),
            JsVariant::Npm => true,
        }
    }

    fn generate_presets(&self, repo_root: &Path) -> RelayResult<StarterPresets> {
        let pm = self.name();
        let scripts = match std::fs::read(repo_root.join(PACKAGE_JSON)) {
            Ok(bytes) => {
                let package: Value = serde_json::from_slice(&bytes)?;
                package
                    .get("scripts")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default()
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => serde_json::Map::new(),
            Err(err) => return Err(err.into()),
        };

        let mut presets = BTreeMap::new();
        // A new worktree does not contain the main checkout's node_modules. User scripts
        // that need dependencies must prepare them inside that worktree.
        presets.insert(
            "smoke".into(),
            CommandPreset {
                description: "Verify the project package manager is available".into(),
                program: pm.into(),
                args: vec!["--version".into()],
                timeout_secs: 120,
                allowed_params: Vec::new(),
            },
        );
        for (name, description) in [
            ("lint", "Run project lint checks"),
            ("typecheck", "Run project type checks"),
            ("test", "Run project tests"),
            ("build", "Build the project"),
        ] {
            if scripts
                .get(name)
                .and_then(Value::as_str)
                .is_some_and(|command| !command.trim().is_empty())
            {
                presets.insert(
                    name.into(),
                    CommandPreset {
                        description: description.into(),
                        program: pm.into(),
                        args: vec!["run".into(), name.into()],
                        timeout_secs: 300,
                        allowed_params: Vec::new(),
                    },
                );
            }
        }

        Ok(StarterPresets {
            smoke_preset: "smoke".into(),
            presets,
        })
    }

    fn manifest_files(&self) -> &'static [&'static str] {
        &[PACKAGE_JSON]
    }

    fn lockfile_paths(&self) -> &'static [&'static str] {
        match self.variant {
            JsVariant::Bun => &[BUN_LOCK, BUN_LOCKB],
            JsVariant::Pnpm => &[PNPM_LOCK],
            JsVariant::Yarn => &[YARN_LOCK],
            JsVariant::Npm => &[NPM_LOCK],
        }
    }

    fn parse_dependencies(&self, files: &EnvFileSnapshot) -> RelayResult<ParsedDependencies> {
        let mut parsed = ParsedDependencies::default();
        if let Some(bytes) = files.get(PACKAGE_JSON).and_then(Option::as_deref) {
            let pkg: Value = serde_json::from_slice(bytes)?;
            for section in ["dependencies", "devDependencies"] {
                if let Some(deps) = pkg.get(section).and_then(Value::as_object) {
                    for (name, ver) in deps {
                        let normalized = normalize_js_name(name);
                        if !normalized.is_empty() {
                            parsed.declared.insert(
                                normalized,
                                ver.as_str().map(str::trim).unwrap_or("").to_owned(),
                            );
                        }
                    }
                }
            }
        }

        match self.variant {
            JsVariant::Npm => {
                if let Some(bytes) = files.get(NPM_LOCK).and_then(Option::as_deref) {
                    match parse_npm_lock(bytes) {
                        Ok(locked) => parsed.locked.extend(locked),
                        Err(err) => parsed
                            .notes
                            .push(format!("Could not read packages from {NPM_LOCK}: {err}")),
                    }
                }
            }
            JsVariant::Bun => {
                if let Some(bytes) = files.get(BUN_LOCK).and_then(Option::as_deref) {
                    match parse_bun_text_lock(bytes) {
                        Ok(locked) => parsed.locked.extend(locked),
                        Err(err) => parsed
                            .notes
                            .push(format!("Could not read packages from {BUN_LOCK}: {err}")),
                    }
                } else if files.get(BUN_LOCKB).and_then(Option::as_deref).is_some() {
                    parsed.notes.push(
                        "bun.lockb is binary; fingerprint covers raw bytes and dependency diff uses package.json".into(),
                    );
                }
            }
            JsVariant::Pnpm => {
                if let Some(bytes) = files.get(PNPM_LOCK).and_then(Option::as_deref) {
                    match std::str::from_utf8(bytes) {
                        Ok(text) => parsed.locked.extend(parse_pnpm_lock(text)),
                        Err(err) => parsed
                            .notes
                            .push(format!("Could not read packages from {PNPM_LOCK}: {err}")),
                    }
                }
            }
            JsVariant::Yarn => {
                if let Some(bytes) = files.get(YARN_LOCK).and_then(Option::as_deref) {
                    match std::str::from_utf8(bytes) {
                        Ok(text) => parsed.locked.extend(parse_yarn_lock(text)),
                        Err(err) => parsed
                            .notes
                            .push(format!("Could not read packages from {YARN_LOCK}: {err}")),
                    }
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
            let manager = self.name();
            let validated = validate_js_dependency_spec(manager, spec)?;
            let check_files = self.manifest_and_lock_files();
            ensure_regular_or_absent(manager, worktree_path, &check_files, &[PACKAGE_JSON])?;

            let args = self.add_args(&validated);
            let program = Path::new(manager);
            #[cfg(windows)]
            let launcher = match std::env::var_os("PATH") {
                Some(path) => super::security::resolve_windows_package_launcher(manager, &args, &path)?,
                None => None,
            };
            #[cfg(windows)]
            let program = launcher.as_deref().unwrap_or(program);

            let output = worktree_command(program, worktree_path, None)
                .args(&args)
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .output()
                .await?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                let detail = if !stderr.is_empty() {
                    stderr
                } else if !stdout.is_empty() {
                    stdout
                } else {
                    format!("{manager} exited with {}", output.status)
                };
                return Err(pm_error(manager, detail));
            }
            stage_intent_to_add(manager, worktree_path, self.lockfile_paths()).await?;
            Ok(())
        })
    }

    fn detect_missing_dependency(&self, stderr: &str) -> Option<String> {
        detect_missing_js_package(stderr)
    }
}

fn normalize_js_name(name: &str) -> String {
    name.trim().to_ascii_lowercase()
}

fn parse_npm_lock(bytes: &[u8]) -> RelayResult<BTreeMap<String, String>> {
    let lock: Value = serde_json::from_slice(bytes)?;
    let mut out = BTreeMap::new();
    if let Some(packages) = lock.get("packages").and_then(Value::as_object) {
        for (path, entry) in packages {
            if let Some((_, pkg_name)) = path.rsplit_once("node_modules/") {
                let normalized = normalize_js_name(pkg_name);
                if !normalized.is_empty() {
                    let version = entry
                        .get("version")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_owned();
                    out.entry(normalized).or_insert(version);
                }
            }
        }
    }
    if out.is_empty()
        && let Some(deps) = lock.get("dependencies").and_then(Value::as_object)
    {
        for (name, entry) in deps {
            let normalized = normalize_js_name(name);
            if !normalized.is_empty() {
                let version = entry
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim()
                    .to_owned();
                out.insert(normalized, version);
            }
        }
    }
    Ok(out)
}

fn parse_bun_text_lock(bytes: &[u8]) -> RelayResult<BTreeMap<String, String>> {
    let raw = std::str::from_utf8(bytes)
        .map_err(|err| RelayError::Io(io::Error::new(io::ErrorKind::InvalidData, err)))?;
    let cleaned = strip_jsonc(raw);
    let lock: Value = serde_json::from_str(&cleaned)?;
    let mut out = BTreeMap::new();
    if let Some(packages) = lock.get("packages").and_then(Value::as_object) {
        for (key, value) in packages {
            let normalized = normalize_js_name(key);
            if normalized.is_empty() {
                continue;
            }
            let version = value
                .as_array()
                .and_then(|arr| arr.first())
                .and_then(Value::as_str)
                .and_then(|spec| split_pkg_at_version(spec).map(|(_, v)| v.to_owned()))
                .unwrap_or_default();
            out.insert(normalized, version);
        }
    }
    Ok(out)
}

fn strip_jsonc(input: &str) -> String {
    // 1. Strip `//` and `/* ... */` comments outside quoted strings.
    let mut without_comments = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut in_string = false;
    let mut escaped = false;
    while i < chars.len() {
        let ch = chars[i];
        if in_string {
            without_comments.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if ch == '"' {
            in_string = true;
            without_comments.push(ch);
            i += 1;
        } else if ch == '/' && i + 1 < chars.len() && chars[i + 1] == '/' {
            i += 2;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if ch == '/' && i + 1 < chars.len() && chars[i + 1] == '*' {
            i += 2;
            while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                i += 1;
            }
            i = (i + 2).min(chars.len());
        } else {
            without_comments.push(ch);
            i += 1;
        }
    }

    // 2. Strip trailing commas before `}` or `]` outside quoted strings.
    let mut out = String::with_capacity(without_comments.len());
    let cleaned: Vec<char> = without_comments.chars().collect();
    let mut idx = 0;
    in_string = false;
    escaped = false;
    while idx < cleaned.len() {
        let ch = cleaned[idx];
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            idx += 1;
            continue;
        }
        if ch == '"' {
            in_string = true;
            out.push(ch);
            idx += 1;
        } else if ch == ',' {
            let mut lookahead = idx + 1;
            while lookahead < cleaned.len() && cleaned[lookahead].is_whitespace() {
                lookahead += 1;
            }
            if lookahead < cleaned.len() && matches!(cleaned[lookahead], '}' | ']') {
                idx += 1;
            } else {
                out.push(ch);
                idx += 1;
            }
        } else {
            out.push(ch);
            idx += 1;
        }
    }
    out
}

fn parse_pnpm_lock(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut in_packages = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if !line.starts_with(' ') && !line.starts_with('\t') {
            in_packages = trimmed == "packages:";
            continue;
        }
        if !in_packages {
            continue;
        }
        // Top-level entries under `packages:` are indented by 2 spaces.
        if line.starts_with("  ") && !line.starts_with("   ") && trimmed.ends_with(':') {
            let key = trimmed
                .trim_end_matches(':')
                .trim_matches(|c| matches!(c, '\'' | '"'))
                .trim_start_matches('/');
            let base = key.split('(').next().unwrap_or(key);
            if let Some((name, version)) = split_pkg_at_version(base) {
                let normalized = normalize_js_name(name);
                if !normalized.is_empty() {
                    out.entry(normalized).or_insert_with(|| version.to_owned());
                }
            }
        }
    }
    out
}

fn parse_yarn_lock(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut active_names: Vec<String> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if !line.starts_with(' ') && !line.starts_with('\t') {
            active_names.clear();
            if let Some(header) = trimmed.strip_suffix(':') {
                for selector in header.split(',') {
                    let selector = selector.trim().trim_matches(|c| matches!(c, '"' | '\''));
                    if selector.is_empty() || selector == "__metadata" {
                        continue;
                    }
                    if let Some((name, _)) = split_pkg_at_version(selector) {
                        let normalized = normalize_js_name(name);
                        if !normalized.is_empty() {
                            active_names.push(normalized);
                        }
                    }
                }
            }
        } else if !active_names.is_empty() {
            let version = trimmed
                .strip_prefix("version:")
                .or_else(|| trimmed.strip_prefix("version "))
                .map(|raw| raw.trim().trim_matches(|c| matches!(c, '"' | '\'')));
            if let Some(ver) = version {
                for name in &active_names {
                    out.entry(name.clone()).or_insert_with(|| ver.to_owned());
                }
            }
        }
    }
    out
}

fn split_pkg_at_version(spec: &str) -> Option<(&str, &str)> {
    let search_from = usize::from(spec.starts_with('@'));
    let rel_idx = spec[search_from..].find('@')?;
    let idx = search_from + rel_idx;
    let name = &spec[..idx];
    let ver = &spec[idx + 1..];
    (!name.is_empty()).then_some((name, ver))
}

fn detect_missing_js_package(stderr: &str) -> Option<String> {
    for line in stderr.lines().rev() {
        for marker in ["Cannot find module '", "Cannot find package '", "Cannot find module \""] {
            if let Some((_, after)) = line.split_once(marker) {
                let quote = if marker.ends_with('"') { '"' } else { '\'' };
                if let Some((pkg, _)) = after.split_once(quote) {
                    let pkg = pkg.trim();
                    if !pkg.is_empty() && !pkg.starts_with('.') && !pkg.starts_with('/') {
                        return Some(pkg.to_owned());
                    }
                }
            }
        }
    }
    None
}
