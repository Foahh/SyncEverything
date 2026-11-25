use chrono::{DateTime, Utc};
use relay_core::error::RelayResult;
use relay_core::sandbox::{
    detect_adapter, load_manifest, resolve_adapter_for_repo, slugify_repo_id,
};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::HashSet;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

pub const MAX_RECENT_WORKSPACES: usize = 10;
const RECENT_WORKSPACES_FILE: &str = "recent_workspaces.json";
const FALLBACK_REPO_NAME: &str = "default-workspace";

/// Specta-safe recent workspace descriptor returned to the desktop UI.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Type)]
pub struct RecentWorkspaceEntry {
    pub path: String,
    pub repo_id: String,
    pub name: String,
    pub package_manager: Option<String>,
    pub last_opened_at: DateTime<Utc>,
    pub exists: bool,
    pub has_manifest: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredRecentWorkspace {
    path: String,
    repo_id: String,
    name: String,
    package_manager: Option<String>,
    last_opened_at: DateTime<Utc>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RecentWorkspacesFile {
    #[serde(default)]
    entries: Vec<StoredRecentWorkspace>,
}

#[derive(Debug, Clone)]
struct ProbedWorkspace {
    repo_id: String,
    name: String,
    package_manager: Option<String>,
    has_manifest: bool,
}

/// Detect or read the active package manager name for a repository directory.
pub fn detect_repo_package_manager(repo_root: &Path) -> Option<String> {
    if !repo_root.is_dir() {
        return None;
    }
    match resolve_adapter_for_repo(repo_root) {
        Ok(adapter) => Some(adapter.name().to_owned()),
        Err(_) => Some(detect_adapter(repo_root).name().to_owned()),
    }
}

/// List persisted recent workspaces ordered by `last_opened_at DESC`, dynamically verifying
/// on-disk existence, `relay.toml` presence, and package manager metadata.
pub fn list_recent_workspaces(base_data_dir: &Path) -> RelayResult<Vec<RecentWorkspaceEntry>> {
    let mut stored = load_store(base_data_dir)?;
    normalize_and_cap(&mut stored);
    Ok(stored.into_iter().map(materialize_entry).collect())
}

/// Record or touch a workspace repository using the current UTC timestamp, atomically
/// persisting up to [`MAX_RECENT_WORKSPACES`] deduplicated entries.
pub fn record_recent_workspace(
    base_data_dir: &Path,
    repo_path: &Path,
) -> RelayResult<Vec<RecentWorkspaceEntry>> {
    record_recent_workspace_at(base_data_dir, repo_path, Utc::now())
}

/// Record or touch a workspace repository with an explicit timestamp (useful for deterministic tests).
pub fn record_recent_workspace_at(
    base_data_dir: &Path,
    repo_path: &Path,
    opened_at: DateTime<Utc>,
) -> RelayResult<Vec<RecentWorkspaceEntry>> {
    let display = normalize_display_path(repo_path);
    let key = canonical_path_key(&display);
    let _guard = lock_store_for_update(base_data_dir)?;
    let mut stored = load_store(base_data_dir)?;

    let previous = stored
        .iter()
        .find(|entry| canonical_path_key(Path::new(&entry.path)) == key)
        .cloned();
    stored.retain(|entry| canonical_path_key(Path::new(&entry.path)) != key);

    let updated = match probe_workspace(&display) {
        Some(probed) => StoredRecentWorkspace {
            path: display.to_string_lossy().into_owned(),
            repo_id: probed.repo_id,
            name: probed.name,
            package_manager: probed.package_manager,
            last_opened_at: opened_at,
        },
        None => {
            let fallback_name = previous
                .as_ref()
                .map(|entry| entry.name.clone())
                .unwrap_or_else(|| fallback_name_for_path(&display));
            let fallback_id = previous
                .as_ref()
                .map(|entry| entry.repo_id.clone())
                .unwrap_or_else(|| slugify_repo_id(&fallback_name));
            StoredRecentWorkspace {
                path: display.to_string_lossy().into_owned(),
                repo_id: fallback_id,
                name: fallback_name,
                package_manager: previous.and_then(|entry| entry.package_manager),
                last_opened_at: opened_at,
            }
        }
    };

    stored.push(updated);
    normalize_and_cap(&mut stored);
    save_store_atomic(base_data_dir, &stored)?;
    Ok(stored.into_iter().map(materialize_entry).collect())
}

/// Remove a workspace entry matching `repo_path` (works even if the directory was already deleted)
/// and atomically persist the updated list.
pub fn remove_recent_workspace(
    base_data_dir: &Path,
    repo_path: &Path,
) -> RelayResult<Vec<RecentWorkspaceEntry>> {
    let raw_trimmed = repo_path.to_string_lossy().trim().to_owned();
    let target_key = canonical_path_key(Path::new(&raw_trimmed));
    let _guard = lock_store_for_update(base_data_dir)?;
    let mut stored = load_store(base_data_dir)?;
    stored.retain(|entry| {
        entry.path != raw_trimmed && canonical_path_key(Path::new(&entry.path)) != target_key
    });
    normalize_and_cap(&mut stored);
    save_store_atomic(base_data_dir, &stored)?;
    Ok(stored.into_iter().map(materialize_entry).collect())
}

fn load_store(base_data_dir: &Path) -> RelayResult<Vec<StoredRecentWorkspace>> {
    let path = base_data_dir.join(RECENT_WORKSPACES_FILE);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let parsed: RecentWorkspacesFile = serde_json::from_str(&raw)?;
    Ok(parsed.entries)
}

fn lock_store_for_update(base_data_dir: &Path) -> RelayResult<std::fs::File> {
    std::fs::create_dir_all(base_data_dir)?;
    // Lock a stable file rather than the JSON file, which is replaced on each write.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(base_data_dir.join("recent_workspaces.lock"))?;
    lock.lock()?;
    Ok(lock)
}

fn save_store_atomic(base_data_dir: &Path, entries: &[StoredRecentWorkspace]) -> RelayResult<()> {
    let payload = serde_json::to_vec_pretty(&RecentWorkspacesFile {
        entries: entries.to_vec(),
    })?;
    let target = base_data_dir.join(RECENT_WORKSPACES_FILE);
    let mut temp = tempfile::NamedTempFile::new_in(base_data_dir)?;
    temp.write_all(&payload)?;
    temp.as_file().sync_all()?;
    temp.persist(target).map_err(|error| error.error)?;
    Ok(())
}

fn normalize_and_cap(entries: &mut Vec<StoredRecentWorkspace>) {
    entries.sort_by(|a, b| {
        b.last_opened_at
            .cmp(&a.last_opened_at)
            .then_with(|| a.path.cmp(&b.path))
    });
    let mut seen = HashSet::new();
    entries.retain(|entry| seen.insert(canonical_path_key(Path::new(&entry.path))));
    entries.truncate(MAX_RECENT_WORKSPACES);
}

fn materialize_entry(stored: StoredRecentWorkspace) -> RecentWorkspaceEntry {
    let path = PathBuf::from(&stored.path);
    match probe_workspace(&path) {
        Some(probed) => RecentWorkspaceEntry {
            path: stored.path,
            repo_id: probed.repo_id,
            name: probed.name,
            package_manager: probed.package_manager,
            last_opened_at: stored.last_opened_at,
            exists: true,
            has_manifest: probed.has_manifest,
        },
        None => RecentWorkspaceEntry {
            path: stored.path,
            repo_id: stored.repo_id,
            name: stored.name,
            package_manager: stored.package_manager,
            last_opened_at: stored.last_opened_at,
            exists: false,
            has_manifest: false,
        },
    }
}

fn probe_workspace(repo_root: &Path) -> Option<ProbedWorkspace> {
    if !repo_root.is_dir() {
        return None;
    }
    let has_manifest = repo_root.join("relay.toml").is_file();
    if has_manifest && let Ok(manifest) = load_manifest(repo_root) {
        let package_manager = manifest
            .project
            .package_manager
            .or_else(|| Some(detect_adapter(repo_root).name().to_owned()));
        return Some(ProbedWorkspace {
            repo_id: manifest.project.repo_id,
            name: manifest.project.name,
            package_manager,
            has_manifest: true,
        });
    }
    let name = fallback_name_for_path(repo_root);
    let repo_id = slugify_repo_id(&name);
    Some(ProbedWorkspace {
        repo_id,
        name,
        package_manager: Some(detect_adapter(repo_root).name().to_owned()),
        has_manifest,
    })
}

fn fallback_name_for_path(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| FALLBACK_REPO_NAME.to_owned())
}

fn normalize_display_path(path: &Path) -> PathBuf {
    let resolved = std::fs::canonicalize(path)
        .or_else(|_| std::path::absolute(path))
        .unwrap_or_else(|_| path.to_path_buf());
    strip_windows_verbatim_prefix(&resolved)
}

fn strip_windows_verbatim_prefix(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let raw = path.to_string_lossy();
        if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{unc}"));
        }
        if let Some(stripped) = raw.strip_prefix(r"\\?\") {
            return PathBuf::from(stripped);
        }
    }
    path.to_path_buf()
}

fn canonical_path_key(path: &Path) -> String {
    let normalized = normalize_display_path(path);
    let mut text = normalized.to_string_lossy().into_owned();
    #[cfg(windows)]
    {
        text = text.replace('/', "\\");
        while text.len() > 3 && text.ends_with('\\') {
            text.pop();
        }
        text.to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        while text.len() > 1 && text.ends_with('/') {
            text.pop();
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use relay_core::sandbox::write_manifest_for_repo;
    use std::fs;
    use tempfile::TempDir;

    fn make_repo(parent: &Path, name: &str) -> PathBuf {
        let dir = parent.join(name);
        fs::create_dir_all(dir.join(".git")).unwrap();
        dir
    }

    #[test]
    fn persists_and_reloads_recent_workspaces_round_trip() {
        let data_dir = TempDir::new().unwrap();
        let repos_root = TempDir::new().unwrap();
        let py_repo = make_repo(repos_root.path(), "forecast-lab");
        fs::write(
            py_repo.join("pyproject.toml"),
            "[project]\nname = \"forecast-lab\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();

        let opened_at = DateTime::parse_from_rfc3339("2026-11-25T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let recorded = record_recent_workspace_at(data_dir.path(), &py_repo, opened_at).unwrap();
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].repo_id, "forecast-lab");
        assert_eq!(recorded[0].name, "forecast-lab");
        assert_eq!(recorded[0].package_manager.as_deref(), Some("uv"));
        assert_eq!(recorded[0].last_opened_at, opened_at);
        assert!(recorded[0].exists);
        assert!(!recorded[0].has_manifest);

        assert!(data_dir.path().join(RECENT_WORKSPACES_FILE).is_file());

        let reloaded = list_recent_workspaces(data_dir.path()).unwrap();
        assert_eq!(reloaded, recorded);
    }

    #[test]
    fn concurrent_workspace_updates_preserve_both_entries() {
        let data_dir = TempDir::new().unwrap();
        let first = make_repo(data_dir.path(), "first");
        let second = make_repo(data_dir.path(), "second");
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            for repo in [&first, &second] {
                let data_dir = data_dir.path();
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    record_recent_workspace(data_dir, repo).unwrap();
                });
            }
        });
        let entries = list_recent_workspaces(data_dir.path()).unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn deduplicates_normalized_paths_and_updates_lru_order() {
        let data_dir = TempDir::new().unwrap();
        let repos_root = TempDir::new().unwrap();
        let first = make_repo(repos_root.path(), "alpha-repo");
        let second = make_repo(repos_root.path(), "beta-repo");
        let t0 = DateTime::parse_from_rfc3339("2026-11-25T08:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        record_recent_workspace_at(data_dir.path(), &first, t0).unwrap();
        record_recent_workspace_at(data_dir.path(), &second, t0 + Duration::minutes(5)).unwrap();

        // Re-record `first` via a non-normalized relative segment (`alpha-repo/.`).
        let messy_first = first.join(".");
        let entries =
            record_recent_workspace_at(data_dir.path(), &messy_first, t0 + Duration::minutes(10))
                .unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].repo_id, "alpha-repo");
        assert_eq!(entries[0].last_opened_at, t0 + Duration::minutes(10));
        assert_eq!(entries[1].repo_id, "beta-repo");
        assert_eq!(entries[1].last_opened_at, t0 + Duration::minutes(5));
    }

    #[test]
    fn caps_recent_workspaces_at_ten_entries_evicting_oldest() {
        let data_dir = TempDir::new().unwrap();
        let repos_root = TempDir::new().unwrap();
        let base_time = DateTime::parse_from_rfc3339("2026-11-25T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        for index in 0..12 {
            let repo = make_repo(repos_root.path(), &format!("repo-{index:02}"));
            record_recent_workspace_at(
                data_dir.path(),
                &repo,
                base_time + Duration::minutes(index as i64),
            )
            .unwrap();
        }

        let listed = list_recent_workspaces(data_dir.path()).unwrap();
        assert_eq!(listed.len(), MAX_RECENT_WORKSPACES);
        let ids: Vec<&str> = listed.iter().map(|entry| entry.repo_id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "repo-11", "repo-10", "repo-09", "repo-08", "repo-07", "repo-06", "repo-05",
                "repo-04", "repo-03", "repo-02",
            ]
        );
    }

    #[test]
    fn flags_deleted_workspace_directory_as_missing_dynamically() {
        let data_dir = TempDir::new().unwrap();
        let repos_root = TempDir::new().unwrap();
        let ephemeral = make_repo(repos_root.path(), "ephemeral-project");
        fs::write(
            ephemeral.join("Cargo.toml"),
            "[package]\nname = \"ephemeral-project\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        write_manifest_for_repo(&ephemeral, None).unwrap();

        let before = record_recent_workspace(data_dir.path(), &ephemeral).unwrap();
        assert!(before[0].exists);
        assert!(before[0].has_manifest);
        assert_eq!(before[0].package_manager.as_deref(), Some("cargo"));

        fs::remove_dir_all(&ephemeral).unwrap();

        let after = list_recent_workspaces(data_dir.path()).unwrap();
        assert_eq!(after.len(), 1);
        assert!(!after[0].exists);
        assert!(!after[0].has_manifest);
        assert_eq!(after[0].repo_id, "ephemeral-project");
        assert_eq!(after[0].package_manager.as_deref(), Some("cargo"));
    }

    #[test]
    fn detects_and_refreshes_manifest_and_package_manager_metadata() {
        let data_dir = TempDir::new().unwrap();
        let repos_root = TempDir::new().unwrap();
        let repo = make_repo(repos_root.path(), "web-toolkit");
        fs::write(
            repo.join("package.json"),
            r#"{"name":"web-toolkit","scripts":{"test":"bun test"}}"#,
        )
        .unwrap();
        fs::write(repo.join("bun.lock"), "{}").unwrap();

        let initial = record_recent_workspace(data_dir.path(), &repo).unwrap();
        assert_eq!(initial[0].package_manager.as_deref(), Some("bun"));
        assert!(!initial[0].has_manifest);
        assert_eq!(initial[0].repo_id, "web-toolkit");

        write_manifest_for_repo(&repo, Some("pnpm")).unwrap();

        let refreshed = list_recent_workspaces(data_dir.path()).unwrap();
        assert!(refreshed[0].has_manifest);
        assert_eq!(refreshed[0].package_manager.as_deref(), Some("pnpm"));
        assert_eq!(refreshed[0].repo_id, "web-toolkit");
        assert_eq!(refreshed[0].name, "web-toolkit");
    }

    #[test]
    fn removes_existing_and_deleted_recent_workspace_entries() {
        let data_dir = TempDir::new().unwrap();
        let repos_root = TempDir::new().unwrap();
        let kept = make_repo(repos_root.path(), "kept-repo");
        let gone = make_repo(repos_root.path(), "deleted-repo");
        let t0 = DateTime::parse_from_rfc3339("2026-11-25T09:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        record_recent_workspace_at(data_dir.path(), &kept, t0).unwrap();
        let recorded =
            record_recent_workspace_at(data_dir.path(), &gone, t0 + Duration::minutes(1)).unwrap();
        let gone_path = recorded[0].path.clone();

        fs::remove_dir_all(&gone).unwrap();

        let after_remove_gone =
            remove_recent_workspace(data_dir.path(), Path::new(&gone_path)).unwrap();
        assert_eq!(after_remove_gone.len(), 1);
        assert_eq!(after_remove_gone[0].repo_id, "kept-repo");

        let after_remove_kept = remove_recent_workspace(data_dir.path(), &kept).unwrap();
        assert!(after_remove_kept.is_empty());
        assert!(list_recent_workspaces(data_dir.path()).unwrap().is_empty());
    }
}
