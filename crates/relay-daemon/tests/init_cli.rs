use relay_core::contracts::RelayManifest;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use uuid::Uuid;

struct TempRepo(PathBuf);

impl TempRepo {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir()
            .join(format!("relay-init-cli-{}", Uuid::now_v7()))
            .join(name);
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        if let Some(parent) = self.0.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }
}

fn run_daemon(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_relay-daemon"))
        .args(args)
        .output()
        .expect("execute relay-daemon binary")
}

#[test]
fn init_dry_run_prints_summary_and_valid_preview_without_writing_file() {
    let repo = TempRepo::new("python-ml-demo");
    fs::write(
        repo.path().join("pyproject.toml"),
        "[project]\nname = \"python-ml-demo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let repo_arg = repo.path().to_string_lossy().into_owned();
    let output = run_daemon(&["init", "--repo-root", &repo_arg, "--dry-run"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !repo.path().join("relay.toml").exists(),
        "--dry-run must not write relay.toml"
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Repo ID:"), "stdout:\n{stdout}");
    assert!(stdout.contains("python-ml-demo"), "stdout:\n{stdout}");
    assert!(stdout.contains("Package Manager: uv"), "stdout:\n{stdout}");
    assert!(stdout.contains("ecosystem: python"), "stdout:\n{stdout}");
    assert!(stdout.contains("Smoke Preset:"), "stdout:\n{stdout}");
    assert!(stdout.contains("Presets:"), "stdout:\n{stdout}");
    assert!(stdout.contains("dry-run preview"), "stdout:\n{stdout}");

    let (_, preview_toml) = stdout
        .split_once("--- relay.toml (dry-run preview) ---\n")
        .expect("dry-run output must include preview marker");
    let manifest: RelayManifest = toml::from_str(preview_toml).expect("preview must parse");
    assert_eq!(manifest.project.repo_id, "python-ml-demo");
    assert_eq!(manifest.project.package_manager.as_deref(), Some("uv"));
    assert!(manifest.presets.contains_key(&manifest.project.smoke_preset));
}

#[test]
fn init_accepts_repo_root_before_and_after_subcommand() {
    let repo_before = TempRepo::new("rust-before");
    fs::write(
        repo_before.path().join("Cargo.toml"),
        "[package]\nname = \"rust-before\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    let arg_before = repo_before.path().to_string_lossy().into_owned();
    let out_before = run_daemon(&["--repo-root", &arg_before, "init"]);
    assert!(
        out_before.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out_before.stderr)
    );
    let manifest_before: RelayManifest =
        toml::from_str(&fs::read_to_string(repo_before.path().join("relay.toml")).unwrap())
            .unwrap();
    assert_eq!(
        manifest_before.project.package_manager.as_deref(),
        Some("cargo")
    );

    let repo_after = TempRepo::new("bun-after");
    fs::write(
        repo_after.path().join("package.json"),
        r#"{"name":"bun-after","scripts":{"test":"bun test"}}"#,
    )
    .unwrap();
    fs::write(repo_after.path().join("bun.lock"), "{}").unwrap();
    let arg_after = repo_after.path().to_string_lossy().into_owned();
    let out_after = run_daemon(&["init", "--repo-root", &arg_after]);
    assert!(
        out_after.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out_after.stderr)
    );
    let manifest_after: RelayManifest =
        toml::from_str(&fs::read_to_string(repo_after.path().join("relay.toml")).unwrap()).unwrap();
    assert_eq!(
        manifest_after.project.package_manager.as_deref(),
        Some("bun")
    );
}

#[test]
fn init_package_manager_override_and_invalid_manager_error() {
    let repo = TempRepo::new("override-demo");
    fs::write(
        repo.path().join("pyproject.toml"),
        "[project]\nname = \"override-demo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let repo_arg = repo.path().to_string_lossy().into_owned();

    let invalid = run_daemon(&[
        "init",
        "--repo-root",
        &repo_arg,
        "--package-manager",
        "not-a-pm",
    ]);
    assert!(!invalid.status.success());
    let stderr = String::from_utf8_lossy(&invalid.stderr);
    assert!(
        stderr.contains("unknown package_manager")
            && stderr.contains("uv")
            && stderr.contains("cargo")
            && stderr.contains("bun")
            && stderr.contains("generic"),
        "expected supported manager list in stderr, got:\n{stderr}"
    );
    assert!(!repo.path().join("relay.toml").exists());

    let valid = run_daemon(&[
        "init",
        "--repo-root",
        &repo_arg,
        "--package-manager",
        "cargo",
    ]);
    assert!(
        valid.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&valid.stderr)
    );
    let stdout = String::from_utf8(valid.stdout).unwrap();
    assert!(stdout.contains("Package Manager: cargo (selected, ecosystem: rust)"));

    let manifest: RelayManifest =
        toml::from_str(&fs::read_to_string(repo.path().join("relay.toml")).unwrap()).unwrap();
    assert_eq!(manifest.project.package_manager.as_deref(), Some("cargo"));
    assert!(manifest.presets.contains_key(&manifest.project.smoke_preset));
}

#[test]
fn init_refuses_overwrite_without_force_and_overwrites_with_force() {
    let repo = TempRepo::new("force-demo");
    let repo_arg = repo.path().to_string_lossy().into_owned();
    let manifest_path = repo.path().join("relay.toml");

    let first = run_daemon(&["init", "--repo-root", &repo_arg]);
    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let original_raw = fs::read_to_string(&manifest_path).unwrap();
    let original: RelayManifest = toml::from_str(&original_raw).unwrap();
    assert_eq!(original.project.package_manager.as_deref(), Some("generic"));

    let refused = run_daemon(&[
        "init",
        "--repo-root",
        &repo_arg,
        "--package-manager",
        "uv",
    ]);
    assert!(!refused.status.success());
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("relay.toml already exists") && stderr.contains("--force"),
        "expected refusal error mentioning --force, got:\n{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&manifest_path).unwrap(),
        original_raw,
        "existing relay.toml must not be modified without --force"
    );

    let forced = run_daemon(&[
        "init",
        "--repo-root",
        &repo_arg,
        "--package-manager",
        "uv",
        "--force",
    ]);
    assert!(
        forced.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&forced.stderr)
    );
    let stdout = String::from_utf8(forced.stdout).unwrap();
    assert!(stdout.contains("overwrote"), "stdout:\n{stdout}");

    let updated: RelayManifest =
        toml::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
    assert_eq!(updated.project.package_manager.as_deref(), Some("uv"));
    assert!(updated.presets.contains_key(&updated.project.smoke_preset));
}
