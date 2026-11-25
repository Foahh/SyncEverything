use crate::error::{RelayError, RelayResult};
use crate::sandbox::{
    git_command, git_output, validate_confined_path_components, validate_confined_relative_path,
};
use std::io;
use std::path::{Component, Path, PathBuf};

const MAX_DEPENDENCY_SPEC_LEN: usize = 512;

const ALLOWED_PEP508_MARKER_VARS: &[&str] = &[
    "python_version",
    "sys_platform",
    "platform_system",
    "platform_machine",
    "os_name",
    "implementation_name",
];

/// Reject empty specs, oversized specs, option injection, control bytes, URL schemes,
/// `file:` references, and unconditionally unsafe shell expansion characters.
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
    if trimmed.contains("://") {
        return Err(pm_error(
            manager,
            format!("URL schemes are blocked in dependency_spec: '{trimmed}'"),
        ));
    }
    if trimmed.to_ascii_lowercase().starts_with("file:") {
        return Err(pm_error(
            manager,
            format!("file: references are blocked in dependency_spec: '{trimmed}'"),
        ));
    }
    for ch in trimmed.chars() {
        if ch == '\0' || ch.is_control() {
            return Err(pm_error(
                manager,
                "control characters are blocked in dependency_spec",
            ));
        }
        if matches!(
            ch,
            '&' | '|' | '$' | '`' | '(' | ')' | '{' | '}' | '\\' | '%'
        ) {
            return Err(pm_error(
                manager,
                format!("shell metacharacter '{ch}' is blocked in dependency_spec"),
            ));
        }
    }
    Ok(trimmed)
}

/// Validate a Python (`uv`) dependency specification: either a strict PEP-508 registry spec
/// (e.g. `scipy>=1.14`, `numpy >= 1.26, < 2.0`, `urllib3>=1.26,!=2.0.0`,
/// `foo; python_version >= "3.11"`) or a confined local archive/wheel inside `worktree_path`
/// (e.g. `./vendor/tiny_stats-0.1.0-py3-none-any.whl`).
pub fn validate_uv_dependency_spec(worktree_path: &Path, spec: &str) -> RelayResult<String> {
    let trimmed = reject_common_injection("uv", spec)?;

    let looks_like_path = trimmed.starts_with('.')
        || trimmed.contains('/')
        || trimmed.ends_with(".whl")
        || trimmed.ends_with(".tar.gz")
        || trimmed.ends_with(".zip");

    if looks_like_path {
        if trimmed.chars().any(|ch| {
            ch.is_whitespace() || matches!(ch, '<' | '>' | ';' | '\'' | '"' | '!' | '^')
        }) {
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
    let (req_part, marker_part) = match spec.split_once(';') {
        Some((req, marker)) => {
            if marker.contains(';') {
                return Err(pm_error(
                    "uv",
                    format!("multiple ';' markers are blocked in dependency_spec: '{spec}'"),
                ));
            }
            (req.trim(), Some(marker.trim()))
        }
        None => (spec, None),
    };

    let bytes = req_part.as_bytes();
    let name_end = bytes
        .iter()
        .position(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')))
        .unwrap_or(bytes.len());
    let name = &req_part[..name_end];
    if !is_valid_pep508_identifier(name) {
        return Err(pm_error(
            "uv",
            format!("invalid Python package name in dependency_spec: '{spec}'"),
        ));
    }

    let mut rest = req_part[name_end..].trim_start();
    if let Some(after_open) = rest.strip_prefix('[') {
        let Some((extras, after_close)) = after_open.split_once(']') else {
            return Err(pm_error(
                "uv",
                format!("unclosed extras bracket in dependency_spec: '{spec}'"),
            ));
        };
        if extras.trim().is_empty()
            || !extras
                .split(',')
                .all(|extra| is_valid_pep508_identifier(extra.trim()))
        {
            return Err(pm_error(
                "uv",
                format!("invalid extras list in dependency_spec: '{spec}'"),
            ));
        }
        rest = after_close.trim_start();
    }

    if !rest.is_empty() {
        for raw_clause in rest.split(',') {
            let clause = raw_clause.trim();
            if clause.is_empty() {
                return Err(pm_error(
                    "uv",
                    format!("empty version clause in dependency_spec: '{spec}'"),
                ));
            }
            let version = strip_pep508_operator(clause)
                .ok_or_else(|| {
                    pm_error(
                        "uv",
                        format!("invalid version specifier '{clause}' in dependency_spec: '{spec}'"),
                    )
                })?
                .trim_start();
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
    }

    if let Some(marker) = marker_part {
        if marker.is_empty() {
            return Err(pm_error(
                "uv",
                format!("empty environment marker in dependency_spec: '{spec}'"),
            ));
        }
        validate_pep508_marker(marker, spec)?;
    }

    Ok(())
}

fn validate_pep508_marker(marker: &str, full_spec: &str) -> RelayResult<()> {
    let mut cursor = marker;
    loop {
        cursor = cursor.trim_start();
        let var_end = cursor
            .bytes()
            .position(|b| !(b.is_ascii_alphanumeric() || b == b'_'))
            .unwrap_or(cursor.len());
        let var = &cursor[..var_end];
        if !ALLOWED_PEP508_MARKER_VARS.contains(&var) {
            return Err(pm_error(
                "uv",
                format!("disallowed environment marker '{var}' in dependency_spec: '{full_spec}'"),
            ));
        }
        cursor = cursor[var_end..].trim_start();
        let after_op = strip_pep508_operator(cursor).ok_or_else(|| {
            pm_error(
                "uv",
                format!("invalid marker operator in dependency_spec: '{full_spec}'"),
            )
        })?;
        cursor = after_op.trim_start();

        let quote = match cursor.as_bytes().first().copied() {
            Some(b'"') => '"',
            Some(b'\'') => '\'',
            _ => {
                return Err(pm_error(
                    "uv",
                    format!("marker value must be quoted in dependency_spec: '{full_spec}'"),
                ));
            }
        };
        let after_open = &cursor[1..];
        let Some((literal, after_close)) = after_open.split_once(quote) else {
            return Err(pm_error(
                "uv",
                format!("unclosed quote in environment marker: '{full_spec}'"),
            ));
        };
        if literal.is_empty()
            || !literal
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
        {
            return Err(pm_error(
                "uv",
                format!("invalid marker literal '{literal}' in dependency_spec: '{full_spec}'"),
            ));
        }

        cursor = after_close;
        if cursor.trim().is_empty() {
            return Ok(());
        }
        if !cursor.starts_with(' ') {
            return Err(pm_error(
                "uv",
                format!("invalid trailing marker syntax in dependency_spec: '{full_spec}'"),
            ));
        }
        cursor = cursor.trim_start();
        if let Some(rest) = cursor
            .strip_prefix("and ")
            .or_else(|| cursor.strip_prefix("or "))
        {
            cursor = rest;
        } else {
            return Err(pm_error(
                "uv",
                format!("only 'and'/'or' combinators are allowed in environment markers: '{full_spec}'"),
            ));
        }
    }
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
    for op in ["===", "==", "!=", "<=", ">=", "~=", "<", ">"] {
        if let Some(rest) = clause.strip_prefix(op) {
            return Some(rest);
        }
    }
    None
}

fn strip_semver_comparator(clause: &str) -> (bool, &str) {
    for op in ["<=", ">=", "^", "~", "=", "<", ">"] {
        if let Some(rest) = clause.strip_prefix(op) {
            return (true, rest);
        }
    }
    (false, clause)
}

fn is_valid_semver_version_atom(atom: &str, allow_bare_wildcard: bool, allow_x_wildcard: bool) -> bool {
    if atom.is_empty() || atom.starts_with('-') {
        return false;
    }
    if atom == "*" {
        return allow_bare_wildcard;
    }
    if !atom.as_bytes()[0].is_ascii_digit() {
        return false;
    }
    if !atom
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'+' | b'*'))
    {
        return false;
    }
    if atom.contains('*') {
        let mut parts = atom.split('.').peekable();
        while let Some(part) = parts.next() {
            if part == "*" {
                return parts.peek().is_none();
            }
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return false;
            }
        }
        return false;
    }
    if !allow_x_wildcard && atom.split('.').any(|part| matches!(part, "x" | "X")) {
        return false;
    }
    true
}

/// Validate a Cargo crate specification (`<crate>` or `<crate>@<semver-req>`),
/// supporting standard Cargo semver comparators (`^`, `~`, `=`, `>=`, `<=`, `<`, `>`, `,`, `*`).
pub fn validate_cargo_dependency_spec(spec: &str) -> RelayResult<String> {
    let trimmed = reject_common_injection("cargo", spec)?;
    for forbidden in ['/', ':', '[', ']', '?', ';', '\'', '"', '!'] {
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
        let ver = ver.trim();
        if ver.is_empty() {
            return Err(pm_error(
                "cargo",
                format!("invalid Cargo version constraint in dependency_spec: '{trimmed}'"),
            ));
        }
        for raw_clause in ver.split(',') {
            let clause = raw_clause.trim();
            if clause.is_empty() {
                return Err(pm_error(
                    "cargo",
                    format!("empty Cargo version clause in dependency_spec: '{trimmed}'"),
                ));
            }
            let (has_op, rest) = strip_semver_comparator(clause);
            let atom = rest.trim_start();
            if !is_valid_semver_version_atom(atom, !has_op, false) {
                return Err(pm_error(
                    "cargo",
                    format!("invalid Cargo version constraint in dependency_spec: '{trimmed}'"),
                ));
            }
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

/// Validate a JavaScript/TypeScript registry dependency spec (`<pkg>[@<version-or-range>]` or
/// `@<scope>/<pkg>[@<version-or-range>]`), accepting standard semver ranges (`^`, `~`,
/// `>=`, `<=`, `<`, `>`, `*`, space-separated comparators) and dist-tags.
pub fn validate_js_dependency_spec(manager: &str, spec: &str) -> RelayResult<String> {
    let trimmed = reject_common_injection(manager, spec)?;
    for forbidden in [':', '[', ']', ',', '?', ';', '\'', '"', '!'] {
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

    if let Some(ver) = version {
        let ver = ver.trim();
        if ver.is_empty() || ver.contains('@') || !is_valid_js_version_or_range(ver) {
            return Err(pm_error(
                manager,
                format!("invalid version or dist-tag in dependency_spec: '{trimmed}'"),
            ));
        }
    }

    Ok(trimmed.to_owned())
}

fn is_valid_js_version_or_range(ver: &str) -> bool {
    if !ver.contains(' ')
        && ver.as_bytes()[0].is_ascii_alphabetic()
        && ver
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return true;
    }

    let tokens: Vec<&str> = ver.split(' ').filter(|token| !token.is_empty()).collect();
    if tokens.is_empty() {
        return false;
    }

    let mut idx = 0;
    while idx < tokens.len() {
        let token = tokens[idx];
        if matches!(token, "<=" | ">=" | "^" | "~" | "=" | "<" | ">") {
            let Some(&next) = tokens.get(idx + 1) else {
                return false;
            };
            if !is_valid_semver_version_atom(next, false, true) {
                return false;
            }
            idx += 2;
        } else {
            let (has_op, atom) = strip_semver_comparator(token);
            if !is_valid_semver_version_atom(atom, !has_op, true) {
                return false;
            }
            idx += 1;
        }
    }
    true
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
/// Bare `~` is inert in `cmd.exe` when `%` is disallowed.
pub fn is_safe_cmd_arg(arg: &str) -> bool {
    !arg.is_empty()
        && arg.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'-' | b'_' | b'.' | b':' | b'/' | b'@' | b'+' | b'=' | b'~'
                )
        })
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsLauncher {
    pub program: PathBuf,
    pub prefix_args: Vec<std::ffi::OsString>,
}

#[cfg(windows)]
impl WindowsLauncher {
    pub fn direct(program: PathBuf) -> Self {
        Self {
            program,
            prefix_args: Vec::new(),
        }
    }

    pub fn with_script(node_exe: PathBuf, script: PathBuf) -> Self {
        Self {
            program: node_exe,
            prefix_args: vec![script.into_os_string()],
        }
    }
}

#[cfg(windows)]
/// Resolve an installed JS package manager executable on Windows.
///
/// Preference order (avoiding `cmd.exe` BatBadBut exposure whenever possible):
/// 1. Native `{program}.exe` on `PATH`
/// 2. Direct `node.exe` + JS manager CLI script (bypasses `cmd.exe` completely)
/// 3. Checked `{program}.cmd` shim with strict `is_safe_cmd_arg` validation
pub fn resolve_windows_package_launcher(
    program: &str,
    args: &[String],
    search_path: &std::ffi::OsStr,
) -> RelayResult<Option<WindowsLauncher>> {
    if !matches!(program, "npm" | "pnpm" | "yarn") {
        return Ok(None);
    }
    let mut cmd_candidate: Option<(PathBuf, PathBuf)> = None;
    for dir in std::env::split_paths(search_path).filter(|dir| dir.is_absolute()) {
        let exe = dir.join(format!("{program}.exe"));
        if exe.is_file() {
            return Ok(Some(WindowsLauncher::direct(exe)));
        }
        if cmd_candidate.is_none() {
            let cmd = dir.join(format!("{program}.cmd"));
            if cmd.is_file() {
                cmd_candidate = Some((dir, cmd));
            }
        }
    }
    let Some((cmd_dir, cmd_path)) = cmd_candidate else {
        return Ok(None);
    };
    if let Some(direct_node) =
        resolve_windows_node_cli_launcher(program, &cmd_dir, &cmd_path, search_path)
    {
        return Ok(Some(direct_node));
    }
    if let Some(unsafe_arg) = args.iter().find(|arg| !is_safe_cmd_arg(arg)) {
        return Err(RelayError::Internal(format!(
            "argument '{unsafe_arg}' contains characters not allowed for Windows .cmd launcher '{program}'"
        )));
    }
    Ok(Some(WindowsLauncher::direct(cmd_path)))
}

#[cfg(windows)]
fn resolve_windows_node_cli_launcher(
    program: &str,
    cmd_dir: &Path,
    cmd_path: &Path,
    search_path: &std::ffi::OsStr,
) -> Option<WindowsLauncher> {
    let node_exe = find_windows_node_exe(cmd_dir, search_path)?;
    let script = find_windows_js_cli_script(program, cmd_dir, cmd_path)?;
    Some(WindowsLauncher::with_script(node_exe, script))
}

#[cfg(windows)]
fn find_windows_node_exe(cmd_dir: &Path, search_path: &std::ffi::OsStr) -> Option<PathBuf> {
    let sibling = cmd_dir.join("node.exe");
    if sibling.is_file() {
        return Some(sibling);
    }
    for dir in std::env::split_paths(search_path).filter(|dir| dir.is_absolute()) {
        let candidate = dir.join("node.exe");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(windows)]
fn find_windows_js_cli_script(program: &str, cmd_dir: &Path, cmd_path: &Path) -> Option<PathBuf> {
    let known_rel_paths: &[&str] = match program {
        "npm" => &["node_modules/npm/bin/npm-cli.js"],
        "pnpm" => &[
            "node_modules/pnpm/bin/pnpm.cjs",
            "node_modules/pnpm/dist/pnpm.cjs",
            "node_modules/corepack/dist/pnpm.js",
        ],
        "yarn" => &[
            "node_modules/yarn/bin/yarn.js",
            "node_modules/corepack/dist/yarn.js",
        ],
        _ => &[],
    };
    for rel in known_rel_paths {
        let candidate = cmd_dir.join(rel);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    // Fallback: inspect standard npm/cmd-shim `%dp0%\...` or `%~dp0\...` JS entrypoint references.
    let meta = std::fs::metadata(cmd_path).ok()?;
    if meta.len() > 16 * 1024 {
        return None;
    }
    let raw = std::fs::read_to_string(cmd_path).ok()?;
    extract_cmd_shim_js_script(cmd_dir, &raw)
}

#[cfg(windows)]
fn extract_cmd_shim_js_script(cmd_dir: &Path, shim_content: &str) -> Option<PathBuf> {
    for raw_token in shim_content.split(['"', '\r', '\n']) {
        let token = raw_token.trim();
        let lower = token.to_ascii_lowercase();
        let rel = if let Some(idx) = lower.find("%~dp0\\") {
            &token[idx + "%~dp0\\".len()..]
        } else if let Some(idx) = lower.find("%~dp0/") {
            &token[idx + "%~dp0/".len()..]
        } else if let Some(idx) = lower.find("%dp0%\\") {
            &token[idx + "%dp0%\\".len()..]
        } else if let Some(idx) = lower.find("%dp0%/") {
            &token[idx + "%dp0%/".len()..]
        } else {
            continue;
        };
        let rel = rel.trim_start_matches(['\\', '/']);
        let rel_lower = rel.to_ascii_lowercase();
        if !(rel_lower.ends_with(".js") || rel_lower.ends_with(".cjs") || rel_lower.ends_with(".mjs"))
            || rel.contains("..")
            || rel.contains('%')
            || rel.contains(':')
            || rel.chars().any(char::is_control)
        {
            continue;
        }
        let normalized = rel.replace('\\', "/");
        let candidate = cmd_dir.join(&normalized);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

pub(crate) fn pm_error(manager: &str, message: impl Into<String>) -> RelayError {
    RelayError::PackageManager {
        manager: manager.to_owned(),
        message: message.into(),
    }
}
