use super::security::{
    ensure_regular_or_absent, is_safe_cmd_arg, validate_cargo_dependency_spec,
    validate_js_dependency_spec, validate_uv_dependency_spec,
};
use super::*;
use crate::contracts::{CommandPreset, PatchKind, ProposeWorktreePatchArgs, TaskSpec};
use crate::engine::RelayEngine;
use crate::mcp::RelayMcpServer;
use crate::sandbox::{
    PatchInput, WorktreeSandbox, compute_env_fingerprint, git_command, git_output, load_manifest,
    output_text,
};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(format!("relay-pm-test-{prefix}-{}", Uuid::now_v7()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn join(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

async fn git(path: &Path, args: &[&str]) -> String {
    let output = git_output(git_command(path).args(args)).await.unwrap();
    output_text(&output.stdout)
}

async fn init_git_repo(path: &Path) {
    fs::create_dir_all(path).unwrap();
    git(path, &["init", "--initial-branch=main"]).await;
    git(path, &["config", "core.autocrlf", "false"]).await;
}

async fn commit_all(path: &Path, msg: &str) -> String {
    git(path, &["add", "--all"]).await;
    let tree = git(path, &["write-tree"]).await;
    let parent = git_command(path)
        .args(["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])
        .output()
        .await
        .ok()
        .filter(|o| o.status.success())
        .map(|o| output_text(&o.stdout));
    let mut args = vec![
        "-c",
        "user.name=Relay PM Test",
        "-c",
        "user.email=relay-pm@example.invalid",
        "commit-tree",
        &tree,
    ];
    let parent_owned;
    if let Some(p) = parent {
        parent_owned = p;
        args.push("-p");
        args.push(&parent_owned);
    }
    args.push("-m");
    args.push(msg);
    let sha = git(path, &args).await;
    git(path, &["update-ref", "HEAD", &sha]).await;
    sha
}

/// Build a deterministic, uncompressed (STORE) ZIP wheel in pure Rust so offline L3 tests
/// run reliably with zero external dependencies or network access.
fn write_minimal_python_wheel(wheel_path: &Path, module_name: &str, dist_name: &str, version: &str) {
    let dist_info = format!("{dist_name}-{version}.dist-info");
    let entries: Vec<(String, Vec<u8>)> = vec![
        (
            format!("{module_name}.py"),
            b"def answer():\n    return 42\n".to_vec(),
        ),
        (
            format!("{dist_info}/METADATA"),
            format!("Metadata-Version: 2.1\nName: {dist_name}\nVersion: {version}\n").into_bytes(),
        ),
        (
            format!("{dist_info}/WHEEL"),
            b"Wheel-Version: 1.0\nGenerator: relay-test\nRoot-Is-Purelib: true\nTag: py3-none-any\n"
                .to_vec(),
        ),
        (format!("{dist_info}/RECORD"), Vec::new()),
    ];

    let mut zip = Vec::new();
    let mut central = Vec::new();
    for (name, data) in &entries {
        let offset = zip.len() as u32;
        let crc = crc32_ieee(data);
        let size = data.len() as u32;
        let name_bytes = name.as_bytes();
        let name_len = name_bytes.len() as u16;

        // Local file header
        zip.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        zip.extend_from_slice(&20u16.to_le_bytes()); // version needed
        zip.extend_from_slice(&0u16.to_le_bytes()); // flags
        zip.extend_from_slice(&0u16.to_le_bytes()); // compression: STORE
        zip.extend_from_slice(&0u16.to_le_bytes()); // mod time
        zip.extend_from_slice(&0x5421u16.to_le_bytes()); // mod date
        zip.extend_from_slice(&crc.to_le_bytes());
        zip.extend_from_slice(&size.to_le_bytes());
        zip.extend_from_slice(&size.to_le_bytes());
        zip.extend_from_slice(&name_len.to_le_bytes());
        zip.extend_from_slice(&0u16.to_le_bytes()); // extra len
        zip.extend_from_slice(name_bytes);
        zip.extend_from_slice(data);

        // Central directory header
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0u16.to_le_bytes()); // flags
        central.extend_from_slice(&0u16.to_le_bytes()); // compression: STORE
        central.extend_from_slice(&0u16.to_le_bytes()); // mod time
        central.extend_from_slice(&0x5421u16.to_le_bytes()); // mod date
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra len
        central.extend_from_slice(&0u16.to_le_bytes()); // comment len
        central.extend_from_slice(&0u16.to_le_bytes()); // disk start
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name_bytes);
    }

    let cd_offset = zip.len() as u32;
    let cd_size = central.len() as u32;
    let count = entries.len() as u16;
    zip.extend_from_slice(&central);

    // End of central directory record
    zip.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    zip.extend_from_slice(&0u16.to_le_bytes()); // disk num
    zip.extend_from_slice(&0u16.to_le_bytes()); // cd start disk
    zip.extend_from_slice(&count.to_le_bytes());
    zip.extend_from_slice(&count.to_le_bytes());
    zip.extend_from_slice(&cd_size.to_le_bytes());
    zip.extend_from_slice(&cd_offset.to_le_bytes());
    zip.extend_from_slice(&0u16.to_le_bytes()); // comment len

    if let Some(parent) = wheel_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(wheel_path, zip).unwrap();
}

fn crc32_ieee(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

#[test]
fn registry_detects_by_priority_and_subdetects_js_lockfiles() {
    let names = supported_package_manager_names();
    assert_eq!(names, vec!["uv", "cargo", "bun", "pnpm", "yarn", "npm", "generic"]);
    assert_eq!(list_supported_package_managers().len(), 7);

    let temp = TempDir::new("priority");

    // Empty dir -> generic
    let repo = temp.join("repo");
    fs::create_dir(&repo).unwrap();
    assert_eq!(detect_adapter(&repo).name(), "generic");

    // package.json only -> npm
    fs::write(repo.join("package.json"), r#"{"name":"demo"}"#).unwrap();
    assert_eq!(detect_adapter(&repo).name(), "npm");

    // yarn.lock beats npm
    fs::write(repo.join("yarn.lock"), "").unwrap();
    assert_eq!(detect_adapter(&repo).name(), "yarn");

    // pnpm-lock.yaml beats yarn
    fs::write(repo.join("pnpm-lock.yaml"), "").unwrap();
    assert_eq!(detect_adapter(&repo).name(), "pnpm");

    // bun.lock beats pnpm
    fs::write(repo.join("bun.lock"), "{}").unwrap();
    assert_eq!(detect_adapter(&repo).name(), "bun");

    // Cargo.toml beats JS
    fs::write(repo.join("Cargo.toml"), "[package]\nname = \"demo\"\n").unwrap();
    assert_eq!(detect_adapter(&repo).name(), "cargo");

    // pyproject.toml beats Cargo
    fs::write(repo.join("pyproject.toml"), "[project]\nname = \"demo\"\n").unwrap();
    assert_eq!(detect_adapter(&repo).name(), "uv");
}

#[test]
fn explicit_package_manager_override_and_invalid_override_rejection() {
    let temp = TempDir::new("override");
    let repo = temp.join("polyglot");
    fs::create_dir(&repo).unwrap();
    fs::write(repo.join("pyproject.toml"), "[project]\nname = \"py\"\n").unwrap();
    fs::write(repo.join("Cargo.toml"), "[package]\nname = \"rs\"\n").unwrap();
    fs::write(
        repo.join("package.json"),
        r#"{"name":"web","scripts":{"test":"vitest","lint":"eslint"}}"#,
    )
    .unwrap();

    // Auto-detect picks uv, explicit override switches cleanly to cargo or pnpm.
    assert_eq!(resolve_adapter(&repo, None).unwrap().name(), "uv");
    assert_eq!(resolve_adapter(&repo, Some("cargo")).unwrap().name(), "cargo");
    assert_eq!(resolve_adapter(&repo, Some("PNPM")).unwrap().name(), "pnpm");

    let pnpm_manifest = generate_manifest_for_repo(&repo, Some("pnpm")).unwrap();
    assert_eq!(pnpm_manifest.package_manager, "pnpm");
    assert_eq!(pnpm_manifest.ecosystem, "javascript");
    let parsed: RelayManifest = toml::from_str(&pnpm_manifest.toml_content).unwrap();
    assert_eq!(parsed.project.package_manager.as_deref(), Some("pnpm"));
    assert_eq!(parsed.presets["smoke"].program, "pnpm");
    assert_eq!(parsed.presets["test"].program, "pnpm");
    assert_eq!(parsed.presets["lint"].program, "pnpm");

    // Invalid overrides are rejected with clear errors.
    for invalid in ["", "   ", "poetry-unknown", "pip", "uv;rm -rf /"] {
        let err = resolve_adapter(&repo, Some(invalid)).unwrap_err();
        assert!(
            err.to_string().contains("unknown package_manager")
                && err.to_string().contains("supported:"),
            "unexpected error for '{invalid}': {err}"
        );
    }

    // load_manifest validates [project].package_manager in relay.toml.
    fs::write(
        repo.join("relay.toml"),
        "[project]\nrepo_id = \"polyglot\"\nname = \"Polyglot\"\npackage_manager = \"invalid-pm\"\n",
    )
    .unwrap();
    let err = load_manifest(&repo).unwrap_err();
    assert!(err.to_string().contains("unknown package_manager"), "{err}");

    // Valid override in relay.toml controls resolve_adapter_for_repo & fingerprinting.
    fs::write(
        repo.join("relay.toml"),
        "[project]\nrepo_id = \"polyglot\"\nname = \"Polyglot\"\npackage_manager = \"cargo\"\n",
    )
    .unwrap();
    assert_eq!(resolve_adapter_for_repo(&repo).unwrap().name(), "cargo");
    let fp = compute_env_fingerprint(&repo).unwrap();
    assert!(fp.starts_with("cargo:blake3:"), "{fp}");
    assert_eq!(fp.len(), "cargo:blake3:".len() + 16);
}

#[test]
fn custom_adapter_registers_into_registry_without_core_changes() {
    #[derive(Debug)]
    struct MockGoAdapter;

    impl PackageManagerAdapter for MockGoAdapter {
        fn name(&self) -> &'static str {
            "go"
        }
        fn ecosystem(&self) -> &'static str {
            "go"
        }
        fn priority(&self) -> u32 {
            15
        }
        fn detect(&self, repo_root: &Path) -> bool {
            repo_root.join("go.mod").is_file()
        }
        fn generate_presets(&self, _repo_root: &Path) -> RelayResult<StarterPresets> {
            let mut presets = BTreeMap::new();
            presets.insert(
                "test".into(),
                CommandPreset {
                    description: "Run go test".into(),
                    program: "go".into(),
                    args: vec!["test".into(), "./...".into()],
                    timeout_secs: 300,
                    allowed_params: Vec::new(),
                },
            );
            Ok(StarterPresets {
                smoke_preset: "test".into(),
                presets,
            })
        }
        fn manifest_files(&self) -> &'static [&'static str] {
            &["go.mod"]
        }
        fn lockfile_paths(&self) -> &'static [&'static str] {
            &["go.sum"]
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
            _spec: &'a str,
            _options: DependencyRunOptions,
        ) -> BoxFuture<'a, RelayResult<()>> {
            Box::pin(async { Ok(()) })
        }
    }

    let registry = PackageManagerRegistry::with_adapters(vec![
        Box::new(generic::GenericAdapter),
        Box::new(MockGoAdapter),
        Box::new(cargo::CargoAdapter),
    ]);
    assert_eq!(registry.supported_names(), vec!["go", "cargo", "generic"]);

    let temp = TempDir::new("custom-go");
    fs::write(temp.join("go.mod"), "module example.com/demo\n\ngo 1.24\n").unwrap();
    let detected = registry.detect(temp.path());
    assert_eq!(detected.name(), "go");
    assert_eq!(detected.ecosystem(), "go");

    let mut snap = EnvFileSnapshot::new();
    snap.insert("go.mod".into(), Some(b"module example.com/demo\n".to_vec()));
    snap.insert("go.sum".into(), None);
    let fp = detected.compute_fingerprint(&snap);
    assert!(fp.starts_with("go:blake3:"));
    assert_eq!(fp.len(), "go:blake3:".len() + 16);
}

#[tokio::test]
async fn env_diff_supports_cargo_ecosystem_missing_and_changed_deps() {
    let temp = TempDir::new("cargo-env");
    let repo = temp.join("rs-app");
    init_git_repo(&repo).await;
    fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname = \"rs-app\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1.0.210\"\ntokio = { version = \"1.40\" }\nmy_dep = { package = \"real_crate\", version = \"0.2\" }\n",
    )
    .unwrap();
    fs::write(
        repo.join("Cargo.lock"),
        "[[package]]\nname = \"serde\"\nversion = \"1.0.210\"\n",
    )
    .unwrap();
    let target_sha = commit_all(&repo, "Initial cargo deps").await;

    let sandbox = WorktreeSandbox::new(&repo, temp.join("worktrees"));
    let clean = sandbox.check_env_diff("HEAD").await.unwrap();
    assert!(clean.in_sync);
    assert_eq!(clean.ecosystem, "rust");
    assert_eq!(clean.package_manager, "cargo");
    assert!(clean.repo_env_fingerprint.starts_with("cargo:blake3:"));
    assert!(clean.missing_packages.is_empty());

    // Host drops tokio + real-crate and pins an older serde version.
    fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname = \"rs-app\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1.0.190\"\n",
    )
    .unwrap();
    fs::write(
        repo.join("Cargo.lock"),
        "[[package]]\nname = \"serde\"\nversion = \"1.0.190\"\n",
    )
    .unwrap();

    let drifted = sandbox.check_env_diff(&target_sha).await.unwrap();
    assert!(!drifted.in_sync);
    assert_eq!(drifted.ecosystem, "rust");
    assert_eq!(drifted.package_manager, "cargo");
    assert_eq!(drifted.missing_packages, vec!["real-crate", "tokio"]);
    assert!(
        drifted
            .notes
            .iter()
            .any(|note| note.contains("serde") && note.contains("1.0.210") && note.contains("1.0.190")),
        "expected version change note, got {:?}",
        drifted.notes
    );
}

#[tokio::test]
async fn env_diff_supports_js_lockfile_variants_bun_pnpm_yarn_npm() {
    let temp = TempDir::new("js-env");

    // 1. Bun (JSONC bun.lock with comments & trailing commas)
    let bun_repo = temp.join("bun-app");
    init_git_repo(&bun_repo).await;
    fs::write(
        bun_repo.join("package.json"),
        r#"{"name":"bun-app","dependencies":{"zod":"3.23.8","react":"19.0.0"}}"#,
    )
    .unwrap();
    fs::write(
        bun_repo.join("bun.lock"),
        "{\n  // bun text lockfile\n  \"lockfileVersion\": 1,\n  \"packages\": {\n    \"react\": [\"react@19.0.0\", \"\", {}, \"sha512-x\"],\n  },\n}\n",
    )
    .unwrap();
    let bun_head = commit_all(&bun_repo, "bun initial").await;
    fs::write(
        bun_repo.join("package.json"),
        r#"{"name":"bun-app","dependencies":{"react":"18.3.1"}}"#,
    )
    .unwrap();
    let bun_diff = WorktreeSandbox::new(&bun_repo, temp.join("wt-bun"))
        .check_env_diff(&bun_head)
        .await
        .unwrap();
    assert!(!bun_diff.in_sync);
    assert_eq!(bun_diff.ecosystem, "javascript");
    assert_eq!(bun_diff.package_manager, "bun");
    assert!(bun_diff.host_env_fingerprint.starts_with("bun:blake3:"));
    assert_eq!(bun_diff.missing_packages, vec!["zod"]);
    assert!(bun_diff.notes.iter().any(|n| n.contains("react") && n.contains("19.0.0")));

    // 2. pnpm (pnpm-lock.yaml)
    let pnpm_repo = temp.join("pnpm-app");
    init_git_repo(&pnpm_repo).await;
    fs::write(
        pnpm_repo.join("package.json"),
        r#"{"name":"pnpm-app","dependencies":{"zod":"3.23.8","@types/node":"22.10.0"}}"#,
    )
    .unwrap();
    fs::write(
        pnpm_repo.join("pnpm-lock.yaml"),
        "lockfileVersion: '9.0'\npackages:\n  '@types/node@22.10.0':\n    resolution: {}\n",
    )
    .unwrap();
    let pnpm_head = commit_all(&pnpm_repo, "pnpm initial").await;
    fs::write(pnpm_repo.join("package.json"), r#"{"name":"pnpm-app","dependencies":{}}"#).unwrap();
    let pnpm_diff = WorktreeSandbox::new(&pnpm_repo, temp.join("wt-pnpm"))
        .check_env_diff(&pnpm_head)
        .await
        .unwrap();
    assert_eq!(pnpm_diff.package_manager, "pnpm");
    assert!(pnpm_diff.host_env_fingerprint.starts_with("pnpm:blake3:"));
    assert_eq!(pnpm_diff.missing_packages, vec!["zod"]);

    // 3. yarn (yarn.lock)
    let yarn_repo = temp.join("yarn-app");
    init_git_repo(&yarn_repo).await;
    fs::write(
        yarn_repo.join("package.json"),
        r#"{"name":"yarn-app","dependencies":{"lodash":"4.17.21","dayjs":"1.11.13"}}"#,
    )
    .unwrap();
    fs::write(
        yarn_repo.join("yarn.lock"),
        "lodash@^4.17.21:\n  version \"4.17.21\"\n",
    )
    .unwrap();
    let yarn_head = commit_all(&yarn_repo, "yarn initial").await;
    fs::write(yarn_repo.join("package.json"), r#"{"name":"yarn-app","dependencies":{}}"#).unwrap();
    let yarn_diff = WorktreeSandbox::new(&yarn_repo, temp.join("wt-yarn"))
        .check_env_diff(&yarn_head)
        .await
        .unwrap();
    assert_eq!(yarn_diff.package_manager, "yarn");
    assert!(yarn_diff.host_env_fingerprint.starts_with("yarn:blake3:"));
    assert_eq!(yarn_diff.missing_packages, vec!["dayjs"]);

    // 4. npm (package-lock.json v3)
    let npm_repo = temp.join("npm-app");
    init_git_repo(&npm_repo).await;
    fs::write(
        npm_repo.join("package.json"),
        r#"{"name":"npm-app","dependencies":{"lodash":"4.17.21","zod":"3.23.8"}}"#,
    )
    .unwrap();
    fs::write(
        npm_repo.join("package-lock.json"),
        r#"{"lockfileVersion":3,"packages":{"node_modules/lodash":{"version":"4.17.21"}}}"#,
    )
    .unwrap();
    let npm_head = commit_all(&npm_repo, "npm initial").await;
    fs::write(npm_repo.join("package.json"), r#"{"name":"npm-app","dependencies":{}}"#).unwrap();
    let npm_diff = WorktreeSandbox::new(&npm_repo, temp.join("wt-npm"))
        .check_env_diff(&npm_head)
        .await
        .unwrap();
    assert_eq!(npm_diff.package_manager, "npm");
    assert!(npm_diff.host_env_fingerprint.starts_with("npm:blake3:"));
    assert_eq!(npm_diff.missing_packages, vec!["zod"]);
}

#[test]
fn commit_detection_requires_the_selected_javascript_manager_lockfile() {
    let registry = PackageManagerRegistry::builtin();
    for (lockfile, expected) in [
        ("bun.lock", "bun"),
        ("pnpm-lock.yaml", "pnpm"),
        ("yarn.lock", "yarn"),
        ("package-lock.json", "npm"),
    ] {
        let adapter = registry.detect_from_file_presence(|file| {
            file == "package.json" || file == lockfile
        });
        assert_eq!(adapter.name(), expected);
    }
}

#[tokio::test]
async fn env_diff_tracks_the_package_manager_of_each_checkout() {
    let repo = TempDir::new("env-switch-manager");
    init_git_repo(repo.path()).await;
    fs::write(
        repo.join("package.json"),
        r#"{"name":"example","dependencies":{"zod":"3.23.8"}}"#,
    )
    .unwrap();
    fs::write(
        repo.join("package-lock.json"),
        r#"{"lockfileVersion":3,"packages":{"node_modules/zod":{"version":"3.23.8"}}}"#,
    )
    .unwrap();
    let npm_head = commit_all(repo.path(), "npm initial").await;
    let npm_fingerprint = compute_env_fingerprint(repo.path()).unwrap();
    fs::remove_file(repo.join("package-lock.json")).unwrap();
    fs::write(
        repo.join("pnpm-lock.yaml"),
        "lockfileVersion: '9.0'\npackages:\n  zod@3.24.0:\n    resolution: {}\n",
    )
    .unwrap();

    let sandbox = WorktreeSandbox::new(repo.path(), repo.join("worktrees"));
    let diff = sandbox.check_env_diff("HEAD").await.unwrap();
    assert_eq!(diff.package_manager, "npm");
    assert_eq!(diff.repo_env_fingerprint, npm_fingerprint);
    assert_eq!(diff.host_env_fingerprint, compute_env_fingerprint(repo.path()).unwrap());
    assert_eq!(sandbox.env_fingerprint_at_commit(&npm_head).await.unwrap(), npm_fingerprint);
    assert!(!diff.in_sync);
    assert!(diff.notes.iter().any(|note| note.contains("commit 'npm' vs host 'pnpm'")));
}

#[test]
fn security_blocks_option_injection_shell_metachars_cmd_hazards_and_traversal() {
    let temp = TempDir::new("sec");
    let wt = temp.join("wt");
    fs::create_dir_all(wt.join("vendor")).unwrap();
    fs::write(wt.join("vendor/good.whl"), "placeholder").unwrap();
    fs::write(temp.join("outside.whl"), "outside").unwrap();

    // Valid specs pass across Python (uv), Cargo, and JS/TS (npm/pnpm/yarn/bun).
    for valid_uv in [
        "scipy",
        "scipy>=1.14",
        "requests[security]>=2.32,<3.0",
        "requests[socks, security] >= 2.31, != 2.32.0",
        "numpy >= 1.26, < 2.0",
        "urllib3>=1.26,!=2.0.0",
        "tiny_stats==0.1.0",
        "foo; python_version >= \"3.11\"",
        "foo>=1.0; python_version >= \"3.11\" and sys_platform == 'linux'",
        "bar; os_name == \"nt\" or platform_machine == \"x86_64\"",
        "baz; platform_system != \"Windows\" and implementation_name == \"cpython\"",
        "./vendor/good.whl",
        "vendor/good.whl",
    ] {
        assert!(
            validate_uv_dependency_spec(&wt, valid_uv).is_ok(),
            "expected valid uv spec '{valid_uv}'"
        );
    }
    for valid_cargo in [
        "serde",
        "serde@1.0.210",
        "tokio@=1.40.0",
        "serde@^1.0",
        "tokio@~1.40",
        "serde@>=1.0,<2.0",
        "serde@>=1.0, <2.0",
        "serde@1.*",
        "serde@*",
        "my_crate",
    ] {
        assert!(
            validate_cargo_dependency_spec(valid_cargo).is_ok(),
            "expected valid cargo spec '{valid_cargo}'"
        );
    }
    for manager in ["bun", "pnpm", "yarn", "npm"] {
        for valid_js in [
            "lodash",
            "zod@3.23.8",
            "@types/node",
            "@types/node@22.10.0",
            "react@^18.3.0",
            "@types/node@^22",
            "zod@~3.23.0",
            "pkg@>=1.0.0 <2.0.0",
            "pkg@>= 1.0.0 < 2.0.0",
            "react@18.*",
            "react@latest",
        ] {
            assert!(
                validate_js_dependency_spec(manager, valid_js).is_ok(),
                "expected valid {manager} spec '{valid_js}'"
            );
        }
    }

    // Injection & traversal vectors blocked across ALL ecosystems.
    let bad_specs = [
        "",
        "   ",
        "-e",
        "--pre",
        "--editable=.",
        "--index-url=https://evil.invalid/simple",
        "--git=https://evil.invalid/repo.git",
        "--path=../outside",
        "pkg;rm -rf /",
        "pkg; rm -rf /",
        "pkg&&calc",
        "pkg && calc",
        "pkg|whoami",
        "pkg$(id)",
        "pkg`id`",
        "pkg>out.txt",
        "pkg<in.txt",
        "pkg\n--flag",
        "pkg\r--flag",
        "pkg\0null",
        "%COMSPEC%",
        "pkg^evil",
        "pkg!bang!",
        "pkg --extra-arg",
        "../outside.whl",
        "./vendor/../../outside.whl",
        "/etc/passwd",
        ".git/config",
        ".env",
        "https://evil.invalid/pkg.whl",
        "file:../outside",
    ];

    for bad in bad_specs {
        assert!(
            validate_uv_dependency_spec(&wt, bad).is_err(),
            "uv must reject '{bad}'"
        );
        assert!(
            validate_cargo_dependency_spec(bad).is_err(),
            "cargo must reject '{bad}'"
        );
        assert!(
            validate_js_dependency_spec("pnpm", bad).is_err(),
            "js must reject '{bad}'"
        );
    }

    // Ecosystem-specific malformed specs and marker injection attempts remain blocked.
    for bad_uv in [
        "foo; ",
        "foo; extra == \"dev\"",
        "foo; python_version >= \"3.11\"; rm -rf /",
        "foo; python_version >= 3.11",
        "foo; python_version >= \"3.11\" and",
        "foo; python_version >= \"$(id)\"",
        "foo; python_version >= \"3.11\" or evil == \"1\"",
        "foo>=1.0 --pre",
    ] {
        assert!(
            validate_uv_dependency_spec(&wt, bad_uv).is_err(),
            "uv must reject '{bad_uv}'"
        );
    }
    for bad_cargo in [
        "serde@",
        "serde@>=*",
        "serde@1.0,",
        "serde@1.0 --offline",
        "serde@>out.txt",
        "serde@1.*.0",
    ] {
        assert!(
            validate_cargo_dependency_spec(bad_cargo).is_err(),
            "cargo must reject '{bad_cargo}'"
        );
    }
    for bad_js in [
        "react@",
        "react@>=*",
        "react@18.0 --registry=https://evil.invalid",
        "react@latest --ignore-scripts",
        "react@>out.txt",
        "react@1.0;rm -rf /",
    ] {
        assert!(
            validate_js_dependency_spec("npm", bad_js).is_err(),
            "js must reject '{bad_js}'"
        );
    }

    // Non-regular directory masquerading as a manifest or lockfile is rejected.
    fs::create_dir(wt.join("Cargo.toml")).unwrap();
    let dir_err = ensure_regular_or_absent("cargo", &wt, &["Cargo.toml", "Cargo.lock"], &["Cargo.toml"])
        .unwrap_err();
    assert!(dir_err.to_string().contains("regular file"), "{dir_err}");

    // Cross-platform cmd.exe arg allowlist check (bare ~ is inert without %).
    assert!(is_safe_cmd_arg("@scope/pkg@1.2.3"));
    assert!(is_safe_cmd_arg("zod@~3.23.0"));
    assert!(!is_safe_cmd_arg("pkg^1.0.0"));
    assert!(!is_safe_cmd_arg("%PATH%"));
    assert!(!is_safe_cmd_arg("a&b"));
    assert!(!is_safe_cmd_arg("pkg!1"));
    assert!(!is_safe_cmd_arg("pkg>out"));
    assert!(!is_safe_cmd_arg("two words"));

    // Symlink escape test when the host OS allows creating symlinks without elevation.
    let link_path = wt.join("vendor/escape.whl");
    #[cfg(unix)]
    let symlink_created = std::os::unix::fs::symlink(temp.join("outside.whl"), &link_path).is_ok();
    #[cfg(windows)]
    let symlink_created =
        std::os::windows::fs::symlink_file(temp.join("outside.whl"), &link_path).is_ok();
    if symlink_created {
        let err = validate_uv_dependency_spec(&wt, "./vendor/escape.whl").unwrap_err();
        assert!(err.to_string().contains("symlink"), "expected symlink rejection, got {err}");
    }
}

#[tokio::test]
async fn l3_uv_offline_wheel_add_updates_pyproject_and_stages_new_uv_lock() {
    let temp = TempDir::new("uv-l3");
    let repo = temp.join("py-wheel-repo");
    init_git_repo(&repo).await;
    fs::create_dir_all(repo.join("src")).unwrap();
    fs::write(
        repo.join("pyproject.toml"),
        "[project]\nname = \"wheel-demo\"\nversion = \"0.1.0\"\nrequires-python = \">=3.9\"\ndependencies = []\n\n[tool.uv]\npackage = false\n",
    )
    .unwrap();
    fs::write(
        repo.join("relay.toml"),
        "[project]\nrepo_id = \"wheel-demo\"\nname = \"Wheel Demo\"\nsmoke_preset = \"smoke\"\npackage_manager = \"uv\"\n\n[presets.smoke]\ndescription = \"Verify mini_pkg import\"\nprogram = \"uv\"\nargs = [\"run\", \"--offline\", \"python\", \"src/check.py\"]\ntimeout_secs = 60\nallowed_params = []\n",
    )
    .unwrap();
    fs::write(
        repo.join("src/check.py"),
        "import mini_pkg\nprint(f'answer={mini_pkg.answer()}')\n",
    )
    .unwrap();
    fs::write(repo.join(".gitignore"), ".venv/\n__pycache__/\n").unwrap();
    write_minimal_python_wheel(
        &repo.join("vendor/mini_pkg-0.1.0-py3-none-any.whl"),
        "mini_pkg",
        "mini_pkg",
        "0.1.0",
    );
    let sha = commit_all(&repo, "Initial wheel fixture").await;

    let sandbox = WorktreeSandbox::new(&repo, temp.join("worktrees"));
    let manifest = load_manifest(&repo).unwrap();
    let task_id = Uuid::now_v7();
    let wt = sandbox.prepare_worktree(task_id, &sha).await.unwrap();

    // Preflight fails and detects missing Python module `mini_pkg`.
    let preflight = sandbox
        .run_preflight(task_id, wt.worktree_path(), wt.branch_name(), &manifest)
        .await
        .unwrap();
    assert!(!preflight.smoke_passed);
    assert_eq!(preflight.detected_missing_module.as_deref(), Some("mini_pkg"));

    // Option injection is rejected BEFORE touching the worktree.
    assert!(
        sandbox
            .apply_and_verify_patch(
                task_id,
                wt.worktree_path(),
                wt.branch_name(),
                &manifest,
                PatchInput::DependencySpec("--index-url=https://evil.invalid"),
                "Injected flag",
            )
            .await
            .is_err()
    );

    // Valid local wheel dependency_spec passes smoke verification and captures uv.lock in diff_preview.
    let proposal = sandbox
        .apply_and_verify_patch(
            task_id,
            wt.worktree_path(),
            wt.branch_name(),
            &manifest,
            PatchInput::DependencySpec("./vendor/mini_pkg-0.1.0-py3-none-any.whl"),
            "Install vendored mini_pkg wheel",
        )
        .await
        .unwrap();
    assert_eq!(proposal.kind, PatchKind::DependencySpec);
    assert!(proposal.smoke_verified, "{}", proposal.smoke_stdout_tail);
    assert!(proposal.smoke_stdout_tail.contains("answer=42"));
    assert!(proposal.diff_preview.contains("pyproject.toml"));
    assert!(
        proposal.diff_preview.contains("uv.lock"),
        "expected newly generated uv.lock in diff_preview:\n{}",
        proposal.diff_preview
    );
    assert!(!sandbox.is_working_tree_dirty().await.unwrap());

    wt.cleanup().await.unwrap();
}

#[tokio::test]
async fn l3_generic_cargo_js_adapters_and_mcp_reject_legacy_patch_kinds() {
    let temp = TempDir::new("l3-adapters");

    // 1. Generic adapter rejects dependency_spec with actionable code_diff guidance.
    let generic_dir = temp.join("generic");
    fs::create_dir(&generic_dir).unwrap();
    let err = adapter_by_name("generic")
        .unwrap()
        .add_dependency(&generic_dir, "foo", DependencyRunOptions::default())
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("code_diff"),
        "expected actionable code_diff hint, got: {err}"
    );

    // 2. Cargo & JS adapters enforce security checks before invoking CLI.
    let cargo_dir = temp.join("cargo");
    fs::create_dir(&cargo_dir).unwrap();
    let cargo_missing = adapter_by_name("cargo")
        .unwrap()
        .add_dependency(&cargo_dir, "serde@1.0", DependencyRunOptions::default())
        .await
        .unwrap_err();
    assert!(cargo_missing.to_string().contains("Cargo.toml"), "{cargo_missing}");

    let js_dir = temp.join("js");
    fs::create_dir(&js_dir).unwrap();
    let js_missing = adapter_by_name("bun")
        .unwrap()
        .add_dependency(&js_dir, "zod@3.23.8", DependencyRunOptions::default())
        .await
        .unwrap_err();
    assert!(js_missing.to_string().contains("package.json"), "{js_missing}");

    // 3. MCP server strictly rejects removed legacy patch_kind names (`uv_spec`, `file_edits`).
    let engine = Arc::new(RelayEngine::in_memory("legacy-check", None).await.unwrap());
    let task_id = Uuid::now_v7();
    engine
        .submit_task(TaskSpec {
            task_id,
            repo_id: "legacy-check".into(),
            base_commit_sha: "abc1234".into(),
            command_preset: "smoke".into(),
            params: BTreeMap::new(),
            require_approval: true,
            requested_by_device: "test".into(),
            user_note: None,
        })
        .await
        .unwrap();
    let server = RelayMcpServer::new(engine, None, None);
    for legacy_kind in ["uv_spec", "file_edits"] {
        let err = server
            .handle_propose_patch(ProposeWorktreePatchArgs {
                task_id: task_id.to_string(),
                patch_kind: legacy_kind.into(),
                summary: "legacy".into(),
                dependency_spec: Some("scipy".into()),
                edits: None,
            })
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("unknown patch kind")
                && err.to_string().contains("dependency_spec")
                && err.to_string().contains("code_diff"),
            "unexpected error for '{legacy_kind}': {err}"
        );
    }
}

#[test]
fn write_manifest_for_repo_with_options_respects_force_flag() {
    let dir = TempDir::new("force-write");
    fs::write(
        dir.join("pyproject.toml"),
        "[project]\nname = \"demo\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();

    let first = write_manifest_for_repo_with_options(dir.path(), None, false).unwrap();
    assert_eq!(first.package_manager, "uv");

    let err = write_manifest_for_repo_with_options(dir.path(), Some("cargo"), false).unwrap_err();
    assert!(
        err.to_string().contains("relay.toml already exists"),
        "expected already-exists error, got: {err}"
    );
    let kept = load_manifest(dir.path()).unwrap();
    assert_eq!(kept.project.package_manager.as_deref(), Some("uv"));

    let overwritten =
        write_manifest_for_repo_with_options(dir.path(), Some("cargo"), true).unwrap();
    assert_eq!(overwritten.package_manager, "cargo");
    assert_eq!(overwritten.ecosystem, "rust");
    let reloaded = load_manifest(dir.path()).unwrap();
    assert_eq!(reloaded.project.package_manager.as_deref(), Some("cargo"));
}
