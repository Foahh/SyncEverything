use crate::{RuntimeManager, WorkspaceInfo};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use specta::Type;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, State};
use tempfile::NamedTempFile;

pub const DEFAULT_MCP_HTTP_URL: &str = "http://127.0.0.1:7608/mcp";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ProjectMcpClient {
    ClaudeCode,
    Cursor,
}

impl ProjectMcpClient {
    pub fn relative_path(self) -> &'static str {
        match self {
            Self::ClaudeCode => ".mcp.json",
            Self::Cursor => ".cursor/mcp.json",
        }
    }

    fn parent_subdir(self) -> Option<&'static str> {
        match self {
            Self::ClaudeCode => None,
            Self::Cursor => Some(".cursor"),
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            Self::ClaudeCode => ".mcp.json",
            Self::Cursor => "mcp.json",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum McpTransportMode {
    Stdio,
    StreamableHttp,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ProjectMcpConfigRequest {
    pub client: ProjectMcpClient,
    pub transport: McpTransportMode,
    /// Optional Streamable HTTP endpoint URL. When `None`, defaults to `http://127.0.0.1:7608/mcp`.
    pub http_url: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ProjectMcpFileStatus {
    CreateNew,
    AddServer,
    UpdateServer,
    Unchanged,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ProjectMcpConfigPreview {
    pub client: ProjectMcpClient,
    pub transport: McpTransportMode,
    pub relative_path: String,
    pub target_path: String,
    pub file_exists: bool,
    pub status: ProjectMcpFileStatus,
    pub existing_content: Option<String>,
    pub merged_content: String,
    pub preserved_server_names: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct ProjectMcpConfigWriteResult {
    pub client: ProjectMcpClient,
    pub transport: McpTransportMode,
    pub relative_path: String,
    pub target_path: String,
    pub created: bool,
    pub status: ProjectMcpFileStatus,
    pub written_content: String,
}

struct TargetLayout {
    canonical_root: PathBuf,
    display_target_path: PathBuf,
    parent_dir: PathBuf,
    parent_exists: bool,
    target_path: PathBuf,
    file_exists: bool,
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn preview_project_mcp_config(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
    request: ProjectMcpConfigRequest,
) -> Result<ProjectMcpConfigPreview, String> {
    let workspace = state.get_or_init(&app).await?.workspace_info();
    tokio::task::spawn_blocking(move || preview_for_workspace(&workspace, &request))
        .await
        .map_err(|err| format!("Preview task failed: {err}"))?
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn write_project_mcp_config(
    app: AppHandle,
    state: State<'_, RuntimeManager>,
    request: ProjectMcpConfigRequest,
) -> Result<ProjectMcpConfigWriteResult, String> {
    let workspace = state.get_or_init(&app).await?.workspace_info();
    tokio::task::spawn_blocking(move || write_for_workspace(&workspace, &request))
        .await
        .map_err(|err| format!("Write task failed: {err}"))?
}

pub fn validate_http_url(raw: &str) -> Result<String, String> {
    if raw.chars().any(|ch| ch.is_control()) {
        return Err("MCP HTTP endpoint URL cannot contain control characters.".into());
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("MCP HTTP endpoint URL cannot be empty.".into());
    }
    if trimmed.chars().any(|ch| ch.is_whitespace()) {
        return Err("MCP HTTP endpoint URL cannot contain whitespace.".into());
    }
    let rest = trimmed
        .strip_prefix("http://")
        .or_else(|| trimmed.strip_prefix("https://"))
        .ok_or_else(|| "MCP HTTP endpoint URL must start with http:// or https://.".to_owned())?;

    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err("MCP HTTP endpoint URL must include a valid host.".into());
    }
    if authority.contains('@') {
        return Err("MCP HTTP endpoint URL must not include embedded credentials.".into());
    }

    let (host, port_opt) = if let Some(after_bracket) = authority.strip_prefix('[') {
        let Some((ipv6_host, suffix)) = after_bracket.split_once(']') else {
            return Err("MCP HTTP endpoint URL has an unclosed IPv6 host bracket.".into());
        };
        if ipv6_host.is_empty() {
            return Err("MCP HTTP endpoint URL must include a valid host.".into());
        }
        let port = if suffix.is_empty() {
            None
        } else if let Some(port_str) = suffix.strip_prefix(':') {
            Some(port_str)
        } else {
            return Err("MCP HTTP endpoint URL has an invalid host/port format.".into());
        };
        (ipv6_host, port)
    } else if let Some((host_part, port_part)) = authority.rsplit_once(':') {
        if host_part.contains(':') {
            return Err("IPv6 hosts in MCP HTTP endpoint URL must be enclosed in brackets.".into());
        }
        (host_part, Some(port_part))
    } else {
        (authority, None)
    };

    if host.is_empty()
        || !host
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | ':'))
    {
        return Err("MCP HTTP endpoint URL contains an invalid host.".into());
    }
    if let Some(port_str) = port_opt {
        let port: u16 = port_str
            .parse()
            .map_err(|_| "MCP HTTP endpoint URL contains an invalid port number.".to_owned())?;
        if port == 0 {
            return Err("MCP HTTP endpoint URL port must be between 1 and 65535.".into());
        }
    }

    Ok(trimmed.to_owned())
}

pub fn build_relay_server_entry(
    workspace: &WorkspaceInfo,
    request: &ProjectMcpConfigRequest,
) -> Result<Value, String> {
    match request.transport {
        McpTransportMode::Stdio => {
            let repo_root = workspace
                .repo_root
                .as_deref()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "Open a local Git repository first.".to_owned())?;
            let data_dir = workspace
                .mcp_data_dir
                .as_deref()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    "Active workspace data directory is unavailable for stdio MCP configuration."
                        .to_owned()
                })?;

            let args = vec![
                "--repo-root".to_owned(),
                repo_root.to_owned(),
                "--repo-id".to_owned(),
                workspace.repo_id.clone(),
                "--data-dir".to_owned(),
                data_dir.to_owned(),
                "mcp-stdio".to_owned(),
            ];

            Ok(json!({
                "command": "relay-daemon",
                "args": args,
            }))
        }
        McpTransportMode::StreamableHttp => {
            let raw_url = request
                .http_url
                .as_deref()
                .unwrap_or(DEFAULT_MCP_HTTP_URL);
            let url = validate_http_url(raw_url)?;
            match request.client {
                ProjectMcpClient::ClaudeCode => Ok(json!({
                    "type": "http",
                    "url": url,
                })),
                ProjectMcpClient::Cursor => Ok(json!({
                    "url": url,
                })),
            }
        }
    }
}

pub fn preview_for_workspace(
    workspace: &WorkspaceInfo,
    request: &ProjectMcpConfigRequest,
) -> Result<ProjectMcpConfigPreview, String> {
    let repo_root = require_repo_root(workspace)?;
    let layout = resolve_target_layout(repo_root, request.client)?;
    let relay_entry = build_relay_server_entry(workspace, request)?;
    let existing_content = if layout.file_exists {
        Some(
            fs::read_to_string(&layout.target_path).map_err(|err| {
                format!(
                    "Unable to read {}: {err}",
                    request.client.relative_path()
                )
            })?,
        )
    } else {
        None
    };

    let (merged_content, status, preserved_server_names) = merge_mcp_json(
        existing_content.as_deref(),
        relay_entry,
        request.client.relative_path(),
    )?;

    Ok(ProjectMcpConfigPreview {
        client: request.client,
        transport: request.transport,
        relative_path: request.client.relative_path().to_owned(),
        target_path: layout.display_target_path.to_string_lossy().into_owned(),
        file_exists: layout.file_exists,
        status,
        existing_content,
        merged_content,
        preserved_server_names,
    })
}

pub fn write_for_workspace(
    workspace: &WorkspaceInfo,
    request: &ProjectMcpConfigRequest,
) -> Result<ProjectMcpConfigWriteResult, String> {
    let repo_root = require_repo_root(workspace)?;
    let layout = resolve_target_layout(repo_root, request.client)?;
    let relay_entry = build_relay_server_entry(workspace, request)?;

    let existing_content = if layout.file_exists {
        Some(
            fs::read_to_string(&layout.target_path).map_err(|err| {
                format!(
                    "Unable to read {}: {err}",
                    request.client.relative_path()
                )
            })?,
        )
    } else {
        None
    };

    let (merged_content, status, _) = merge_mcp_json(
        existing_content.as_deref(),
        relay_entry,
        request.client.relative_path(),
    )?;

    if !layout.parent_exists {
        if let Some(subdir) = request.client.parent_subdir() {
            let subdir_path = layout.canonical_root.join(subdir);
            match fs::create_dir(&subdir_path) {
                Ok(()) => {}
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {}
                Err(err) => {
                    return Err(format!("Unable to create `{subdir}` directory: {err}"));
                }
            }
        }
    }

    // Re-verify target containment and symlink safety right before atomic replacement.
    let verified = resolve_target_layout(repo_root, request.client)?;
    if !verified.parent_exists {
        return Err(format!(
            "Target directory for {} is not available.",
            request.client.relative_path()
        ));
    }

    let mut temp = NamedTempFile::new_in(&verified.parent_dir).map_err(|err| {
        format!(
            "Unable to create temporary file next to {}: {err}",
            request.client.relative_path()
        )
    })?;
    temp.write_all(merged_content.as_bytes()).map_err(|err| {
        format!(
            "Unable to write temporary file for {}: {err}",
            request.client.relative_path()
        )
    })?;
    temp.as_file().sync_all().map_err(|err| {
        format!(
            "Unable to flush temporary file for {}: {err}",
            request.client.relative_path()
        )
    })?;
    temp.persist(&verified.target_path).map_err(|err| {
        format!(
            "Unable to atomically replace {}: {}",
            request.client.relative_path(),
            err.error
        )
    })?;

    Ok(ProjectMcpConfigWriteResult {
        client: request.client,
        transport: request.transport,
        relative_path: request.client.relative_path().to_owned(),
        target_path: verified.display_target_path.to_string_lossy().into_owned(),
        created: !layout.file_exists,
        status,
        written_content: merged_content,
    })
}

fn require_repo_root(workspace: &WorkspaceInfo) -> Result<&Path, String> {
    let raw = workspace
        .repo_root
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Open a local Git repository first.".to_owned())?;
    Ok(Path::new(raw))
}

fn resolve_target_layout(repo_root: &Path, client: ProjectMcpClient) -> Result<TargetLayout, String> {
    let canonical_root = fs::canonicalize(repo_root)
        .map_err(|err| format!("Active repository root is not accessible: {err}"))?;
    if !canonical_root.is_dir() {
        return Err("Active repository root must be a directory.".into());
    }

    let display_target_path = match client.parent_subdir() {
        Some(subdir) => repo_root.join(subdir).join(client.file_name()),
        None => repo_root.join(client.file_name()),
    };

    let (parent_dir, parent_exists) = match client.parent_subdir() {
        None => (canonical_root.clone(), true),
        Some(subdir) => {
            let candidate = canonical_root.join(subdir);
            match fs::symlink_metadata(&candidate) {
                Ok(meta) => {
                    reject_if_symlink(&meta, subdir)?;
                    if !meta.is_dir() {
                        return Err(format!(
                            "Refusing to write {}: `{subdir}` exists and is not a directory.",
                            client.relative_path()
                        ));
                    }
                    let canonical_subdir = verify_contained_child(
                        &canonical_root,
                        &candidate,
                        subdir,
                        subdir,
                    )?;
                    (canonical_subdir, true)
                }
                Err(err) if err.kind() == ErrorKind::NotFound => (candidate, false),
                Err(err) => {
                    return Err(format!("Unable to inspect `{subdir}` directory: {err}"));
                }
            }
        }
    };

    let candidate_file = parent_dir.join(client.file_name());
    if !parent_exists {
        return Ok(TargetLayout {
            canonical_root,
            display_target_path,
            parent_dir,
            parent_exists: false,
            target_path: candidate_file,
            file_exists: false,
        });
    }

    match fs::symlink_metadata(&candidate_file) {
        Ok(meta) => {
            reject_if_symlink(&meta, client.relative_path())?;
            if !meta.is_file() {
                return Err(format!(
                    "Refusing to overwrite {}: target exists and is not a regular file.",
                    client.relative_path()
                ));
            }
            let canonical_file = verify_contained_child(
                &parent_dir,
                &candidate_file,
                client.file_name(),
                client.relative_path(),
            )?;
            if !canonical_file.starts_with(&canonical_root) {
                return Err(format!(
                    "Refusing to access {}: path resolves outside the active repository root.",
                    client.relative_path()
                ));
            }
            Ok(TargetLayout {
                canonical_root,
                display_target_path,
                parent_dir,
                parent_exists: true,
                target_path: canonical_file,
                file_exists: true,
            })
        }
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(TargetLayout {
            canonical_root,
            display_target_path,
            parent_dir,
            parent_exists: true,
            target_path: candidate_file,
            file_exists: false,
        }),
        Err(err) => Err(format!(
            "Unable to inspect {}: {err}",
            client.relative_path()
        )),
    }
}

fn reject_if_symlink(meta: &fs::Metadata, label: &str) -> Result<(), String> {
    if meta.file_type().is_symlink() {
        return Err(format!(
            "Refusing to access `{label}` because it is a symbolic link."
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        if (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0 {
            return Err(format!(
                "Refusing to access `{label}` because it is a symbolic link or directory junction."
            ));
        }
    }
    Ok(())
}

fn verify_contained_child(
    expected_parent: &Path,
    candidate: &Path,
    expected_name: &str,
    label: &str,
) -> Result<PathBuf, String> {
    let canonical = fs::canonicalize(candidate).map_err(|err| {
        format!("Refusing to access `{label}`: unable to verify path containment ({err}).")
    })?;
    let parent_matches = canonical.parent() == Some(expected_parent);
    let name_matches = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            if cfg!(windows) {
                name.eq_ignore_ascii_case(expected_name)
            } else {
                name == expected_name
            }
        });
    if !parent_matches || !name_matches {
        return Err(format!(
            "Refusing to access `{label}` because it resolves outside the active repository root."
        ));
    }
    Ok(canonical)
}

fn merge_mcp_json(
    existing_raw: Option<&str>,
    relay_entry: Value,
    relative_path: &str,
) -> Result<(String, ProjectMcpFileStatus, Vec<String>), String> {
    let (mut root_obj, existed) = match existing_raw {
        None => (Map::new(), false),
        Some(raw) => {
            let parsed: Value = serde_json::from_str(raw).map_err(|err| {
                format!(
                    "Refusing to overwrite {relative_path}: existing file contains invalid JSON ({err}). Fix or remove the file before writing."
                )
            })?;
            let Value::Object(map) = parsed else {
                return Err(format!(
                    "Refusing to overwrite {relative_path}: root JSON value must be an object."
                ));
            };
            (map, true)
        }
    };

    let (status, preserved_server_names) = match root_obj.get_mut("mcpServers") {
        None => {
            let mut servers = Map::new();
            servers.insert("relay".to_owned(), relay_entry);
            root_obj.insert("mcpServers".to_owned(), Value::Object(servers));
            let status = if existed {
                ProjectMcpFileStatus::AddServer
            } else {
                ProjectMcpFileStatus::CreateNew
            };
            (status, Vec::new())
        }
        Some(servers_val) => {
            let Some(servers) = servers_val.as_object_mut() else {
                return Err(format!(
                    "Refusing to overwrite {relative_path}: \"mcpServers\" must be a JSON object."
                ));
            };
            let preserved: Vec<String> = servers
                .keys()
                .filter(|key| key.as_str() != "relay")
                .cloned()
                .collect();
            let existing_relay = servers.get("relay").cloned();
            let status = match existing_relay.as_ref() {
                None => ProjectMcpFileStatus::AddServer,
                Some(prev) if prev == &relay_entry => ProjectMcpFileStatus::Unchanged,
                Some(_) => ProjectMcpFileStatus::UpdateServer,
            };
            servers.insert("relay".to_owned(), relay_entry);
            (status, preserved)
        }
    };

    let pretty = serde_json::to_string_pretty(&Value::Object(root_obj))
        .map_err(|err| format!("Unable to format {relative_path} JSON: {err}"))?;
    Ok((format!("{pretty}\n"), status, preserved_server_names))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_workspace(root: &Path) -> WorkspaceInfo {
        WorkspaceInfo {
            repo_id: "demo-repo".into(),
            name: "demo-repo".into(),
            repo_root: Some(root.to_string_lossy().into_owned()),
            mcp_data_dir: Some(root.join(".relay").to_string_lossy().into_owned()),
            is_git_repo: true,
            has_manifest: true,
            package_manager: None,
            smoke_preset: "smoke".into(),
            presets: Vec::new(),
        }
    }

    #[test]
    fn creates_new_claude_code_stdio_config_and_preview_is_read_only() {
        let dir = TempDir::new().expect("temp dir");
        let workspace = sample_workspace(dir.path());
        let request = ProjectMcpConfigRequest {
            client: ProjectMcpClient::ClaudeCode,
            transport: McpTransportMode::Stdio,
            http_url: None,
        };

        let preview = preview_for_workspace(&workspace, &request).expect("preview");
        assert!(!preview.file_exists);
        assert_eq!(preview.status, ProjectMcpFileStatus::CreateNew);
        assert_eq!(preview.relative_path, ".mcp.json");
        assert!(preview.existing_content.is_none());
        assert!(preview.preserved_server_names.is_empty());
        assert!(!dir.path().join(".mcp.json").exists());

        let parsed: Value = serde_json::from_str(&preview.merged_content).expect("valid json");
        assert_eq!(parsed["mcpServers"]["relay"]["command"], "relay-daemon");
        assert_eq!(
            parsed["mcpServers"]["relay"]["args"],
            json!([
                "--repo-root",
                dir.path().to_string_lossy(),
                "--repo-id",
                "demo-repo",
                "--data-dir",
                dir.path().join(".relay").to_string_lossy(),
                "mcp-stdio",
            ])
        );

        let written = write_for_workspace(&workspace, &request).expect("write");
        assert!(written.created);
        assert_eq!(written.status, ProjectMcpFileStatus::CreateNew);
        assert_eq!(written.written_content, preview.merged_content);
        assert_eq!(
            fs::read_to_string(dir.path().join(".mcp.json")).expect("read written"),
            preview.merged_content
        );

        let after = preview_for_workspace(&workspace, &request).expect("second preview");
        assert!(after.file_exists);
        assert_eq!(after.status, ProjectMcpFileStatus::Unchanged);
    }

    #[test]
    fn creates_cursor_config_and_does_not_create_dir_during_preview() {
        let dir = TempDir::new().expect("temp dir");
        let workspace = sample_workspace(dir.path());
        let request = ProjectMcpConfigRequest {
            client: ProjectMcpClient::Cursor,
            transport: McpTransportMode::StreamableHttp,
            http_url: Some("http://127.0.0.1:7608/mcp".into()),
        };

        let preview = preview_for_workspace(&workspace, &request).expect("preview");
        assert!(!preview.file_exists);
        assert_eq!(preview.relative_path, ".cursor/mcp.json");
        assert!(!dir.path().join(".cursor").exists());

        let parsed: Value = serde_json::from_str(&preview.merged_content).expect("valid json");
        assert_eq!(
            parsed["mcpServers"]["relay"],
            json!({ "url": "http://127.0.0.1:7608/mcp" })
        );

        let written = write_for_workspace(&workspace, &request).expect("write");
        assert!(written.created);
        assert!(dir.path().join(".cursor").is_dir());
        assert!(dir.path().join(".cursor").join("mcp.json").is_file());
    }

    #[test]
    fn merges_while_preserving_top_level_keys_and_existing_servers() {
        let dir = TempDir::new().expect("temp dir");
        let target = dir.path().join(".mcp.json");
        fs::write(
            &target,
            r#"{
  "$schema": "https://example.com/mcp-schema.json",
  "customFlag": true,
  "mcpServers": {
    "github": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-github"]
    },
    "postgres": {
      "type": "http",
      "url": "http://127.0.0.1:9000/mcp"
    }
  }
}"#,
        )
        .expect("seed existing .mcp.json");

        let workspace = sample_workspace(dir.path());
        let request = ProjectMcpConfigRequest {
            client: ProjectMcpClient::ClaudeCode,
            transport: McpTransportMode::StreamableHttp,
            http_url: Some("http://127.0.0.1:8800/mcp".into()),
        };

        let preview = preview_for_workspace(&workspace, &request).expect("preview");
        assert!(preview.file_exists);
        assert_eq!(preview.status, ProjectMcpFileStatus::AddServer);
        assert_eq!(
            preview.preserved_server_names,
            vec!["github".to_owned(), "postgres".to_owned()]
        );

        let written = write_for_workspace(&workspace, &request).expect("write");
        assert!(!written.created);
        assert_eq!(written.status, ProjectMcpFileStatus::AddServer);

        let on_disk: Value =
            serde_json::from_str(&fs::read_to_string(&target).expect("read")).expect("json");
        assert_eq!(on_disk["$schema"], "https://example.com/mcp-schema.json");
        assert_eq!(on_disk["customFlag"], true);
        assert_eq!(on_disk["mcpServers"]["github"]["command"], "npx");
        assert_eq!(
            on_disk["mcpServers"]["postgres"]["url"],
            "http://127.0.0.1:9000/mcp"
        );
        assert_eq!(
            on_disk["mcpServers"]["relay"],
            json!({
                "type": "http",
                "url": "http://127.0.0.1:8800/mcp"
            })
        );
    }

    #[test]
    fn updates_existing_relay_entry_without_touching_siblings() {
        let dir = TempDir::new().expect("temp dir");
        let target = dir.path().join(".mcp.json");
        fs::write(
            &target,
            r#"{
  "mcpServers": {
    "filesystem": { "command": "fs-mcp" },
    "relay": { "command": "old-relay", "args": ["old"] }
  }
}"#,
        )
        .expect("seed");

        let workspace = sample_workspace(dir.path());
        let request = ProjectMcpConfigRequest {
            client: ProjectMcpClient::ClaudeCode,
            transport: McpTransportMode::StreamableHttp,
            http_url: None,
        };

        let preview = preview_for_workspace(&workspace, &request).expect("preview");
        assert_eq!(preview.status, ProjectMcpFileStatus::UpdateServer);
        assert_eq!(preview.preserved_server_names, vec!["filesystem".to_owned()]);

        let written = write_for_workspace(&workspace, &request).expect("write");
        assert_eq!(written.status, ProjectMcpFileStatus::UpdateServer);
        let on_disk: Value =
            serde_json::from_str(&fs::read_to_string(&target).expect("read")).expect("json");
        assert_eq!(on_disk["mcpServers"]["filesystem"]["command"], "fs-mcp");
        assert_eq!(
            on_disk["mcpServers"]["relay"],
            json!({
                "type": "http",
                "url": DEFAULT_MCP_HTTP_URL
            })
        );
    }

    #[test]
    fn rejects_corrupted_json_and_leaves_file_untouched() {
        let dir = TempDir::new().expect("temp dir");
        let target = dir.path().join(".mcp.json");
        let corrupted = "{ \"mcpServers\": ";
        fs::write(&target, corrupted).expect("write corrupted");

        let workspace = sample_workspace(dir.path());
        let request = ProjectMcpConfigRequest {
            client: ProjectMcpClient::ClaudeCode,
            transport: McpTransportMode::Stdio,
            http_url: None,
        };

        let preview_err = preview_for_workspace(&workspace, &request).unwrap_err();
        assert!(preview_err.contains("invalid JSON"), "got: {preview_err}");

        let write_err = write_for_workspace(&workspace, &request).unwrap_err();
        assert!(write_err.contains("invalid JSON"), "got: {write_err}");
        assert_eq!(fs::read_to_string(&target).expect("read"), corrupted);
    }

    #[test]
    fn rejects_non_object_root_and_non_object_mcp_servers() {
        let dir = TempDir::new().expect("temp dir");
        let workspace = sample_workspace(dir.path());
        let request = ProjectMcpConfigRequest {
            client: ProjectMcpClient::ClaudeCode,
            transport: McpTransportMode::Stdio,
            http_url: None,
        };

        let target = dir.path().join(".mcp.json");
        fs::write(&target, "[\"not-an-object\"]").expect("write array root");
        let root_err = write_for_workspace(&workspace, &request).unwrap_err();
        assert!(root_err.contains("root JSON value must be an object"), "got: {root_err}");
        assert_eq!(fs::read_to_string(&target).expect("read"), "[\"not-an-object\"]");

        let bad_servers = r#"{ "mcpServers": ["wrong-type"] }"#;
        fs::write(&target, bad_servers).expect("write bad mcpServers");
        let servers_err = write_for_workspace(&workspace, &request).unwrap_err();
        assert!(
            servers_err.contains("\"mcpServers\" must be a JSON object"),
            "got: {servers_err}"
        );
        assert_eq!(fs::read_to_string(&target).expect("read"), bad_servers);
    }

    #[test]
    fn validates_http_urls_strictly() {
        assert_eq!(
            validate_http_url("  http://127.0.0.1:7608/mcp  ").unwrap(),
            "http://127.0.0.1:7608/mcp"
        );
        assert_eq!(
            validate_http_url("https://relay.example.com:8443/mcp").unwrap(),
            "https://relay.example.com:8443/mcp"
        );
        assert_eq!(
            validate_http_url("http://[::1]:7608/mcp").unwrap(),
            "http://[::1]:7608/mcp"
        );

        for invalid in [
            "",
            "   ",
            "127.0.0.1:7608/mcp",
            "file:///etc/passwd",
            "http://",
            "http://127.0.0.1:0/mcp",
            "http://127.0.0.1:99999/mcp",
            "http://user:pass@127.0.0.1:7608/mcp",
            "http://127.0.0.1:7608/mcp\n",
            "http://127.0.0.1:7608/mcp bad",
        ] {
            assert!(
                validate_http_url(invalid).is_err(),
                "expected invalid URL to fail: {invalid:?}"
            );
        }
    }

    #[test]
    fn rejects_symlink_or_junction_escape_for_cursor_directory() {
        let repo = TempDir::new().expect("repo dir");
        let outside = TempDir::new().expect("outside dir");
        let outside_mcp = outside.path().join("mcp.json");
        fs::write(&outside_mcp, r#"{"mcpServers":{"secret":{"command":"keep"}}}"#).expect("seed");

        let cursor_link = repo.path().join(".cursor");
        let linked = create_dir_link(outside.path(), &cursor_link);
        assert!(linked, "expected directory symlink or junction creation to succeed");

        let workspace = sample_workspace(repo.path());
        let request = ProjectMcpConfigRequest {
            client: ProjectMcpClient::Cursor,
            transport: McpTransportMode::Stdio,
            http_url: None,
        };

        let preview_err = preview_for_workspace(&workspace, &request).unwrap_err();
        assert!(
            preview_err.contains("symbolic link")
                || preview_err.contains("junction")
                || preview_err.contains("outside"),
            "unexpected error: {preview_err}"
        );
        let write_err = write_for_workspace(&workspace, &request).unwrap_err();
        assert!(
            write_err.contains("symbolic link")
                || write_err.contains("junction")
                || write_err.contains("outside"),
            "unexpected error: {write_err}"
        );
        // Outside file must remain untouched.
        assert_eq!(
            fs::read_to_string(&outside_mcp).expect("read outside"),
            r#"{"mcpServers":{"secret":{"command":"keep"}}}"#
        );
    }

    #[test]
    fn rejects_file_symlink_escape_when_supported() {
        let repo = TempDir::new().expect("repo dir");
        let outside = TempDir::new().expect("outside dir");
        let outside_file = outside.path().join("escaped.json");
        fs::write(&outside_file, r#"{"mcpServers":{}}"#).expect("seed outside");

        let mcp_link = repo.path().join(".mcp.json");
        if !try_create_file_symlink(&outside_file, &mcp_link) {
            // Windows without Developer Mode or SeCreateSymbolicLinkPrivilege cannot create file symlinks.
            return;
        }

        let workspace = sample_workspace(repo.path());
        let request = ProjectMcpConfigRequest {
            client: ProjectMcpClient::ClaudeCode,
            transport: McpTransportMode::StreamableHttp,
            http_url: None,
        };

        let err = write_for_workspace(&workspace, &request).unwrap_err();
        assert!(err.contains("symbolic link"), "got: {err}");
        assert_eq!(
            fs::read_to_string(&outside_file).expect("read outside"),
            r#"{"mcpServers":{}}"#
        );
    }

    #[test]
    fn preserves_top_level_keys_when_mcp_servers_key_is_absent() {
        let dir = TempDir::new().expect("temp dir");
        let target = dir.path().join(".mcp.json");
        fs::write(&target, r#"{"$schema": "https://example.com/schema.json", "inputs": []}"#)
            .expect("seed");

        let workspace = sample_workspace(dir.path());
        let request = ProjectMcpConfigRequest {
            client: ProjectMcpClient::ClaudeCode,
            transport: McpTransportMode::StreamableHttp,
            http_url: None,
        };

        let preview = preview_for_workspace(&workspace, &request).expect("preview");
        assert!(preview.file_exists);
        assert_eq!(preview.status, ProjectMcpFileStatus::AddServer);
        assert!(preview.preserved_server_names.is_empty());

        write_for_workspace(&workspace, &request).expect("write");
        let on_disk: Value =
            serde_json::from_str(&fs::read_to_string(&target).expect("read")).expect("json");
        assert_eq!(on_disk["$schema"], "https://example.com/schema.json");
        assert_eq!(on_disk["inputs"], json!([]));
        assert_eq!(
            on_disk["mcpServers"]["relay"],
            json!({ "type": "http", "url": DEFAULT_MCP_HTTP_URL })
        );
    }

    #[test]
    fn rejects_preview_and_write_when_no_repo_is_open() {
        let workspace = WorkspaceInfo {
            repo_id: "default-workspace".into(),
            name: String::new(),
            repo_root: None,
            mcp_data_dir: None,
            is_git_repo: false,
            has_manifest: false,
            package_manager: None,
            smoke_preset: String::new(),
            presets: Vec::new(),
        };
        let request = ProjectMcpConfigRequest {
            client: ProjectMcpClient::ClaudeCode,
            transport: McpTransportMode::StreamableHttp,
            http_url: None,
        };
        assert!(preview_for_workspace(&workspace, &request).is_err());
        assert!(write_for_workspace(&workspace, &request).is_err());
    }

    fn create_dir_link(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            if std::os::windows::fs::symlink_dir(target, link).is_ok() {
                return true;
            }
            // Fall back to an unprivileged NTFS directory junction on Windows.
            std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()
                .is_ok_and(|out| out.status.success())
        }
    }

    fn try_create_file_symlink(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(target, link).is_ok()
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(target, link).is_ok()
        }
    }
}
