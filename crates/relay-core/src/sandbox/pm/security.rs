use crate::error::{RelayError, RelayResult};
use crate::sandbox::{
    git_command, git_output, validate_confined_path_components, validate_confined_relative_path,
};
use std::io;
use std::path::{Component, Path, PathBuf};

const MAX_DEPENDENCY_SPEC_LEN: usize = 512;

/// Reject empty specs, option injection, whitespace, control bytes, shell metacharacters,
/// and Windows `.cmd` shim metacharacter hazards (`%`, `^`, `!`).
pub fn reject_common_injection<'a>(manager: &str, raw: &'a str) -> RelayResult<&'a str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(pm_error(manager, "dependency_spec must not be empty"));
    }
    if trimmed.len() > MAX_DEPENDENCY_SPEC_LEN {
        return Err(pm_error(
            manager,
            format!("dependency_spec exceeds maximum length of {MAX_DEPENDENCY_SPEC_LEN} bytes"),
        ));
    }
    if trimmed.starts_with('-') {
        return Err(pm_error(
            manager,
            format!("option injection is blocked in dependency_spec: '{trimmed}'"),
        ));
    }
    for ch in trimmed.chars() {
        if ch == '\0' || ch.is_control() {
            return Err(pm_error(
                manager,
                "control characters are blocked in dependency_spec",
            ));
        }
        if ch.is_whitespace() {
            return Err(pm_error(
                manager,
                format!("whitespace is blocked in dependency_spec: '{trimmed}'"),
            ));
        }
        if matches!(
            ch,
            '&' | '|' | ';' | '$' | '`' | '\'' | '"' | '(' | ')' | '{' | '}' | '\\'
        ) {
            return Err(pm_error(
                manager,
                format!("shell metacharacter '{ch}' is blocked in dependency_spec"),
            ));
        }
        if matches!(ch, '%' | '^' | '!') {
            return Err(pm_error(
                manager,
                format!("Windows .cmd metacharacter '{ch}' is blocked in dependency_spec"),
            ));
        }
    }
    Ok(trimmed)
}

/// Validate a Python (`uv`) dependency specification: either a strict PEP-508 registry spec
/// (e.g. `scipy>=1.14`, `requests[security]>=2.32,<3.0`) or a confined local archive/wheel
/// inside `worktree_path` (e.g. `./vendor/tiny_stats-0.1.0-py3-none-any.whl`).
pub fn validate_uv_dependency_spec(worktree_path: &Path, spec: &str) -> RelayResult<String> {
    let trimmed = reject_common_injection("uv", spec)?;
    if trimmed.contains("://") {
        return Err(pm_error(
            "uv",
            format!("URL schemes are blocked in dependency_spec: '{trimmed}'"),
        ));
    }

    let looks_like_path = trimmed.starts_with('.')
        || trimmed.contains('/')
        || trimmed.ends_with(".whl")
        || trimmed.ends_with(".tar.gz")
        || trimmed.ends_with(".zip");

    if looks_like_path {
        if trimmed.contains('<') || trimmed.contains('>') {
            return Err(pm_error(
                "uv",
                format!("shell metacharacter is blocked in local path dependency_spec: '{trimmed}'"),
            ));
        }
        let rel = ensure_confined_local_artifact("uv", worktree_path, trimmed)?;
        let display = rel
            .components()
            .filter_map(|c| match c {
                Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/");
        return Ok(format!("./{display}"));
    }

    if trimmed.contains("..") {
        return Err(pm_error(
            "uv",
            format!("path traversal sequence '..' is blocked in dependency_spec: '{trimmed}'"),
        ));
    }

    validate_pep508_registry_spec(trimmed)?;
    Ok(trimmed.to_owned())
}

fn validate_pep508_registry_spec(spec: &str) -> RelayResult<()> {
    let bytes = spec.as_bytes();
    let name_end = bytes
        .iter()
        .position(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')))
        .unwrap_or(bytes.len());
    let name = &spec[..name_end];
    if !is_valid_pep508_identifier(name) {
        return Err(pm_error(
            "uv",
            format!("invalid Python package name in dependency_spec: '{spec}'"),
        ));
    }

    let mut rest = &spec[name_end..];
    if let Some(after_open) = rest.strip_prefix('[') {
        let Some((extras, after_close)) = after_open.split_once(']') else {
            return Err(pm_error(
                "uv",
                format!("unclosed extras bracket in dependency_spec: '{spec}'"),
            ));
        };
        if extras.is_empty()
            || !extras
                .split(',')
                .all(|extra| !extra.is_empty() && is_valid_pep508_identifier(extra))
        {
            return Err(pm_error(
                "uv",
                format!("invalid extras list in dependency_spec: '{spec}'"),
            ));
        }
        rest = after_close;
    }

    if rest.is_empty() {
        return Ok(());
    }

    for clause in rest.split(',') {
        if clause.is_empty() {
            return Err(pm_error(
                "uv",
                format!("empty version clause in dependency_spec: '{spec}'"),
            ));
        }
        let version = strip_pep508_operator(clause).ok_or_else(|| {
            pm_error(
                "uv",
                format!("invalid version specifier '{clause}' in dependency_spec: '{spec}'"),
            )
        })?;
        if version.is_empty()
            || !version.as_bytes()[0].is_ascii_digit()
            || !version.bytes().all(|b| {
                b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-' | b'+' | b'*')
            })
        {
            return Err(pm_error(
                "uv",
                format!("invalid version constraint '{clause}' in dependency_spec: '{spec}'"),
            ));
        }
    }

    Ok(())
}

fn is_valid_pep508_identifier(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    if !bytes[0].is_ascii_alphanumeric() || !bytes[bytes.len() - 1].is_ascii_alphanumeric() {
        return false;
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

fn strip_pep508_operator(clause: &str) -> Option<&str> {
    for op in ["===", "==", "<=", ">=", "~=", "<", ">"] {
        if let Some(rest) = clause.strip_prefix(op) {
            return Some(rest);
        }
    }
    None
}

/// Validate a Cargo crate specification (`<crate>` or `<crate>@<version>`).
pub fn validate_cargo_dependency_spec(spec: &str) -> RelayResult<String> {
    let trimmed = reject_common_injection("cargo", spec)?;
    for forbidden in ['<', '>', '/', ':', '[', ']', ',', '*', '?', '~'] {
        if trimmed.contains(forbidden) {
            return Err(pm_error(
                "cargo",
                format!("character '{forbidden}' is blocked in cargo dependency_spec: '{trimmed}'"),
            ));
        }
    }
    if trimmed.contains("..") {
        return Err(pm_error(
            "cargo",
            format!("path traversal sequence '..' is blocked in dependency_spec: '{trimmed}'"),
        ));
    }

    let (name, version) = match trimmed.split_once('@') {
        Some((name, version)) => {
            if version.contains('@') {
                return Err(pm_error(
                    "cargo",
                    format!("invalid cargo dependency_spec: '{trimmed}'"),
                ));
            }
            (name, Some(version))
        }
        None => (trimmed, None),
    };

    if !is_valid_cargo_crate_name(name) {
        return Err(pm_error(
            "cargo",
            format!("invalid Cargo crate name in dependency_spec: '{trimmed}'"),
        ));
    }

    if let Some(ver) = version {
        let ver = ver.strip_prefix('=').unwrap_or(ver);
        if ver.is_empty()
            || !ver.as_bytes()[0].is_ascii_digit()
            || !ver
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+'))
        {
            return Err(pm_error(
                "cargo",
                format!("invalid Cargo version constraint in dependency_spec: '{trimmed}'"),
            ));
        }
    }

    Ok(trimmed.to_owned())
}

fn is_valid_cargo_crate_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    if !(bytes[0].is_ascii_alphabetic() || bytes[0] == b'_') {
        return false;
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
}

/// Validate a JavaScript/TypeScript registry dependency spec (`<pkg>[@<version>]` or
/// `@<scope>/<pkg>[@<version>]`) ensuring every character is cmd.exe-inert on Windows.
pub fn validate_js_dependency_spec(manager: &str, spec: &str) -> RelayResult<String> {
    let trimmed = reject_common_injection(manager, spec)?;
    for forbidden in ['<', '>', ':', '[', ']', ',', '*', '?', '~', '='] {
        if trimmed.contains(forbidden) {
            return Err(pm_error(
                manager,
                format!("character '{forbidden}' is blocked in {manager} dependency_spec: '{trimmed}'"),
            ));
        }
    }
    if trimmed.contains("..") {
        return Err(pm_error(
            manager,
            format!("path traversal sequence '..' is blocked in dependency_spec: '{trimmed}'"),
        ));
    }

    let (pkg_name, version) = if let Some(after_at) = trimmed.strip_prefix('@') {
        let Some((scope, rest)) = after_at.split_once('/') else {
            return Err(pm_error(
                manager,
                format!("scoped package must match '@scope/name[@version]': '{trimmed}'"),
            ));
        };
        if !is_valid_js_segment(scope) {
            return Err(pm_error(
                manager,
                format!("invalid package scope in dependency_spec: '{trimmed}'"),
            ));
        }
        match rest.split_once('@') {
            Some((name, ver)) => (name, Some(ver)),
            None => (rest, None),
        }
    } else {
        match trimmed.split_once('@') {
            Some((name, ver)) => (name, Some(ver)),
            None => (trimmed, None),
        }
    };

    if !is_valid_js_segment(pkg_name) {
        return Err(pm_error(
            manager,
            format!("invalid package name in dependency_spec: '{trimmed}'"),
        ));
    }

    if let Some(ver) = version
        && (ver.is_empty()
            || !ver.as_bytes()[0].is_ascii_alphanumeric()
            || !ver
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'+')))
    {
        return Err(pm_error(
            manager,
            format!("invalid version or dist-tag in dependency_spec: '{trimmed}'"),
        ));
    }

    if !is_safe_cmd_arg(trimmed) {
        return Err(pm_error(
            manager,
            format!("dependency_spec '{trimmed}' contains characters disallowed for safe launcher execution"),
        ));
    }

    Ok(trimmed.to_owned())
}

fn is_valid_js_segment(segment: &str) -> bool {
    let bytes = segment.as_bytes();
    if bytes.is_empty() || segment.contains('/') || segment.contains('@') {
        return false;
    }
    if !bytes[0].is_ascii_alphanumeric() {
        return false;
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

/// Ensure a local artifact path (e.g. a vendored `.whl`) stays strictly inside `worktree_path`
/// with zero symlink escapes, path traversal, or protected directory access.
pub fn ensure_confined_local_artifact(
    manager: &str,
    worktree_path: &Path,
    raw: &str,
) -> RelayResult<PathBuf> {
    let rel = validate_confined_relative_path(raw)
        .map_err(|err| pm_error(manager, err.to_string()))?;
    let canonical_root = std::fs::canonicalize(worktree_path)?;
    let target = ensure_no_symlink_components(manager, &canonical_root, &rel, raw)?;
    let meta = std::fs::symlink_metadata(&target).map_err(|err| {
        if err.kind() == io::ErrorKind::NotFound {
            pm_error(
                manager,
                format!("local dependency artifact '{raw}' does not exist in the worktree"),
            )
        } else {
            err.into()
        }
    })?;
    if meta.file_type().is_symlink() {
        return Err(pm_error(
            manager,
            format!("symlink dependency path '{raw}' is blocked"),
        ));
    }
    if !meta.is_file() {
        return Err(pm_error(
            manager,
            format!("local dependency artifact '{raw}' must be a regular file"),
        ));
    }
    let canonical_target = std::fs::canonicalize(&target)?;
    let resolved = canonical_target.strip_prefix(&canonical_root).map_err(|_| {
        pm_error(
            manager,
            format!("local dependency artifact '{raw}' resolves outside the worktree"),
        )
    })?;
    validate_confined_path_components(resolved, raw)
        .map_err(|err| pm_error(manager, err.to_string()))?;
    Ok(rel)
}

/// Verify that each manifest/lockfile in `all_files` is either absent (unless listed in
/// `required_files`) or a non-symlink regular file strictly confined inside `worktree_path`.
pub fn ensure_regular_or_absent(
    manager: &str,
    worktree_path: &Path,
    all_files: &[&str],
    required_files: &[&str],
) -> RelayResult<()> {
    let canonical_root = std::fs::canonicalize(worktree_path)?;
    for &file in all_files {
        let rel = validate_confined_relative_path(file)
            .map_err(|err| pm_error(manager, err.to_string()))?;
        let target = ensure_no_symlink_components(manager, &canonical_root, &rel, file)?;
        match std::fs::symlink_metadata(&target) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(pm_error(
                        manager,
                        format!("symlink '{file}' is blocked inside the worktree"),
                    ));
                }
                if !meta.is_file() {
                    return Err(pm_error(
                        manager,
                        format!("{file} must be a regular file"),
                    ));
                }
                let canonical_target = std::fs::canonicalize(&target)?;
                if !canonical_target.starts_with(&canonical_root) {
                    return Err(pm_error(
                        manager,
                        format!("{file} resolves outside the worktree"),
                    ));
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                if required_files.contains(&file) {
                    return Err(pm_error(
                        manager,
                        format!("{file} is required in the worktree"),
                    ));
                }
            }
            Err(err) => return Err(err.into()),
        }
    }
    Ok(())
}

fn ensure_no_symlink_components(
    manager: &str,
    canonical_root: &Path,
    rel: &Path,
    raw: &str,
) -> RelayResult<PathBuf> {
    let mut current = canonical_root.to_path_buf();
    let components: Vec<_> = rel.components().collect();
    for (idx, component) in components.iter().enumerate() {
        let Component::Normal(part) = component else {
            return Err(pm_error(
                manager,
                format!("invalid relative path '{raw}'"),
            ));
        };
        current.push(part);
        let is_last = idx + 1 == components.len();
        match std::fs::symlink_metadata(&current) {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(pm_error(
                        manager,
                        format!("symlink path '{raw}' is blocked inside the worktree"),
                    ));
                }
                if !is_last && !meta.is_dir() {
                    return Err(pm_error(
                        manager,
                        format!("parent component of '{raw}' is not a directory"),
                    ));
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => break,
            Err(err) => return Err(err.into()),
        }
    }
    Ok(canonical_root.join(rel))
}

/// Run `git add --intent-to-add` on any existing lockfiles so newly generated lockfiles
/// always appear in `git diff` and human approval previews.
pub async fn stage_intent_to_add(
    manager: &str,
    worktree_path: &Path,
    paths: &[&str],
) -> RelayResult<()> {
    if paths.is_empty() {
        return Ok(());
    }
    let canonical_root = std::fs::canonicalize(worktree_path)?;
    let mut existing = Vec::new();
    for &path in paths {
        let rel = validate_confined_relative_path(path)
            .map_err(|err| pm_error(manager, err.to_string()))?;
        let target = ensure_no_symlink_components(manager, &canonical_root, &rel, path)?;
        match tokio::fs::symlink_metadata(&target).await {
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(pm_error(
                        manager,
                        format!("symlink lockfile '{path}' is blocked"),
                    ));
                }
                if meta.is_file() {
                    existing.push(path);
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    if existing.is_empty() {
        return Ok(());
    }
    let mut cmd = git_command(worktree_path);
    cmd.args(["add", "--intent-to-add", "--"]);
    for path in existing {
        cmd.arg(path);
    }
    git_output(&mut cmd).await?;
    Ok(())
}

/// Strict allowlist of characters safe to pass through a Windows `.cmd` shim without
/// triggering `cmd.exe` metacharacter interpretation or argument splitting.
pub fn is_safe_cmd_arg(arg: &str) -> bool {
    !arg.is_empty()
        && arg.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@' | b'+' | b'=')
        })
}

#[cfg(windows)]
pub(crate) fn resolve_windows_package_launcher(
    program: &str,
    args: &[String],
    search_path: &std::ffi::OsStr,
) -> RelayResult<Option<PathBuf>> {
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

pub(crate) fn pm_error(manager: &str, message: impl Into<String>) -> RelayError {
    RelayError::PackageManager {
        manager: manager.to_owned(),
        message: message.into(),
    }
}
