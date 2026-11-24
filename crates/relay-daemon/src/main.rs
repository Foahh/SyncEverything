use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use relay_core::agent::{
    ContextStrategy, InferenceTier, LlmClient, LlmEndpointConfig, compose_context, load_llm_config,
    run_host_task_pipeline,
};
use relay_core::contracts::{RelayManifest, RuntimeMode, TaskSpec};
use relay_core::engine::RelayEngine;
use relay_core::mcp::RelayMcpServer;
use relay_core::sandbox::{
    HeadSyncOutcome, WorktreeSandbox, generate_manifest_for_repo, load_manifest,
    sync_git_head_explicitly, sync_git_head_into_engine, write_manifest_for_repo_with_options,
};
use relay_core::sync::{NetworkProfile, RelayNode, RelayNodeConfig, RelayNodeEvent};
use rmcp::ServiceExt;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::time::{self, MissedTickBehavior};
use tracing::{info, warn};
use uuid::Uuid;

const GIT_WATCH_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Parser)]
#[command(name = "relay-daemon", about = "Relay headless Host/Client daemon & MCP server")]
struct Cli {
    #[arg(long, global = true, default_value = ".relay")]
    data_dir: PathBuf,

    #[arg(long, global = true)]
    repo_root: Option<PathBuf>,

    #[arg(long, global = true)]
    repo_id: Option<String>,

    #[arg(long, global = true, env = "RELAY_LLM_BASE_URL")]
    llm_base_url: Option<String>,

    #[arg(long, global = true, env = "RELAY_LLM_MODEL")]
    llm_model: Option<String>,

    #[arg(long, global = true, env = "RELAY_LLM_API_KEY")]
    llm_api_key: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ContextStrategyArg {
    #[value(alias = "full_transcript")]
    FullTranscript,
    #[value(alias = "flat_summary")]
    FlatSummary,
    #[value(alias = "dag_bundle")]
    DagBundle,
}

impl From<ContextStrategyArg> for ContextStrategy {
    fn from(value: ContextStrategyArg) -> Self {
        match value {
            ContextStrategyArg::FullTranscript => Self::FullTranscript,
            ContextStrategyArg::FlatSummary => Self::FlatSummary,
            ContextStrategyArg::DagBundle => Self::DagBundle,
        }
    }
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Start the Relay Host with P2P sync, pairing ticket, Host executor, and Streamable HTTP MCP.
    Serve {
        #[arg(long, default_value = "127.0.0.1:7608")]
        mcp_addr: SocketAddr,
        /// Bind P2P to the IPv4 loopback interface for same-machine pairing.
        #[arg(long, conflicts_with = "lan_only")]
        local_only: bool,
        /// Pair using direct IP addresses with public discovery and relays disabled.
        #[arg(long, conflicts_with = "local_only")]
        lan_only: bool,
        /// Seconds to accept an incoming P2P pairing request (0 = 24 hours).
        #[arg(long, default_value_t = 600)]
        pairing_window_secs: u64,
    },
    /// Serve Relay MCP over stdio (compatible with Claude Code, Cursor, Codex, dsh, and any MCP client).
    McpStdio,
    /// Serve Relay MCP over Streamable HTTP at /mcp.
    McpHttp {
        #[arg(long, default_value = "127.0.0.1:7608")]
        addr: SocketAddr,
    },
    /// Start isolated Host and Client nodes and check the loopback route.
    SandboxDual,
    /// Detect project ecosystem and generate a starter `relay.toml` manifest.
    Init {
        /// Explicit package manager override (uv, cargo, bun, pnpm, yarn, npm, generic).
        #[arg(long)]
        package_manager: Option<String>,
        /// Overwrite an existing `relay.toml` atomically.
        #[arg(long)]
        force: bool,
        /// Print the detection summary and generated `relay.toml` without writing to disk.
        #[arg(long)]
        dry_run: bool,
    },
    /// Probe the configured Ollama or OpenAI-compatible Cloud LLM endpoint.
    LlmCheck,
    /// Check required external runtimes (git, uv), repository relay.toml, data dir, and optional LLM endpoint.
    Doctor {
        /// Output machine-readable JSON instead of human-readable status lines.
        #[arg(long)]
        json: bool,
        /// Skip probing the configured Ollama / Cloud LLM endpoint (useful offline).
        #[arg(long)]
        skip_llm: bool,
    },
    /// Sync the repository's current Git HEAD into the DAG (cascading STALE if HEAD advanced).
    SyncHead,
    /// Submit a whitelisted task preset bound to current Git HEAD and print its TaskRecord JSON.
    SubmitTask {
        /// Preset name defined in `<repo>/relay.toml`.
        #[arg(long)]
        preset: String,
        /// Optional key=value parameter overrides allowed by `relay.toml`.
        #[arg(long = "param")]
        params: Vec<String>,
        /// Optional note explaining why this run was submitted.
        #[arg(long)]
        note: Option<String>,
        /// Skip human approval when running this task.
        #[arg(long)]
        no_approval: bool,
        /// Optional explicit base commit SHA (defaults to current Git HEAD).
        #[arg(long)]
        base_commit: Option<String>,
    },
    /// Deterministically compose model context for Track 5 ablation (full_transcript / flat_summary / dag_bundle).
    ComposeContext {
        /// Context strategy to emit (ignored when --all-strategies is set).
        #[arg(long, value_enum, default_value_t = ContextStrategyArg::DagBundle)]
        strategy: ContextStrategyArg,
        /// Emit all three ablation strategies side-by-side in one JSON object.
        #[arg(long)]
        all_strategies: bool,
        /// Read the stored DAG as-is without syncing Git HEAD first.
        #[arg(long)]
        no_sync: bool,
    },
}

struct Repository {
    id: String,
    root: Option<PathBuf>,
}

impl Repository {
    fn resolve(root: Option<PathBuf>, id: Option<String>) -> Result<Self> {
        let root = if let Some(root) = root {
            let root = std::path::absolute(&root)
                .with_context(|| format!("resolve {}", root.display()))?;
            anyhow::ensure!(root.is_dir(), "repository root must be a directory");
            Some(root)
        } else {
            let cwd = std::env::current_dir()?;
            cwd.join("relay.toml").try_exists()?.then_some(cwd)
        };
        let id = match (id, root.as_deref()) {
            (Some(id), _) => Some(id),
            (None, Some(root)) => manifest_repo_id(root)?.or_else(|| {
                root.file_name()
                    .map(|name| relay_core::sandbox::slugify_repo_id(&name.to_string_lossy()))
            }),
            (None, None) => None,
        };
        Ok(Self {
            id: id.unwrap_or_else(|| "default-workspace".into()),
            root,
        })
    }

    async fn open(&self, data_dir: &Path) -> Result<(Arc<RelayEngine>, Option<WorktreeSandbox>)> {
        let engine =
            RelayEngine::open(&self.id, data_dir.join("relay.db"), self.root.clone()).await?;
        let sandbox = self
            .root
            .as_ref()
            .filter(|root| root.join(".git").exists() && load_manifest(root).is_ok())
            .map(|root| WorktreeSandbox::new(root, data_dir.join("worktrees")));
        Ok((Arc::new(engine), sandbox))
    }
}

fn manifest_repo_id(root: &Path) -> Result<Option<String>> {
    let path = root.join("relay.toml");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    let manifest: toml::Table =
        toml::from_str(&raw).with_context(|| format!("parse {}", path.display()))?;
    let Some(id) = manifest.get("project").and_then(|project| project.get("repo_id")) else {
        return Ok(None);
    };
    Ok(Some(
        id.as_str()
            .context("project.repo_id in relay.toml must be a string")?
            .to_owned(),
    ))
}

fn resolve_llm_config(cli: &Cli) -> Result<LlmEndpointConfig> {
    let mut cfg = load_llm_config(&cli.data_dir.join("llm-settings.json"))?;
    if let Some(base_url) = &cli.llm_base_url
        && !base_url.trim().is_empty()
    {
        cfg.base_url = base_url.trim().to_owned();
    }
    if let Some(model) = &cli.llm_model
        && !model.trim().is_empty()
    {
        cfg.model = model.trim().to_owned();
    }
    if let Some(api_key) = &cli.llm_api_key
        && !api_key.trim().is_empty()
    {
        cfg.api_key = Some(api_key.trim().to_owned());
        if cfg.tier == InferenceTier::HostLocalOllama {
            cfg.tier = InferenceTier::CloudOpenAiCompat;
        }
    }
    Ok(cfg)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,relay_core=debug,relay_daemon=debug".into()),
        )
        .with_writer(std::io::stderr)
        .init();

    if let Commands::Init {
        package_manager,
        force,
        dry_run,
    } = &cli.command
    {
        return run_init(
            cli.repo_root.as_deref(),
            package_manager.as_deref(),
            *force,
            *dry_run,
        );
    }

    if matches!(cli.command, Commands::LlmCheck) {
        let cfg = resolve_llm_config(&cli)?;
        let client = LlmClient::new(cfg, Duration::from_secs(15))?;
        let probe = client.probe().await;
        println!("{}", serde_json::to_string_pretty(&probe)?);
        anyhow::ensure!(probe.ok, "LLM probe failed: {}", probe.message);
        return Ok(());
    }

    if let Commands::Doctor {
        json: as_json,
        skip_llm,
    } = cli.command
    {
        return run_doctor(&cli, as_json, skip_llm).await;
    }

    let repo = Repository::resolve(cli.repo_root.clone(), cli.repo_id.clone())?;
    match cli.command {
        Commands::Serve {
            mcp_addr,
            local_only,
            lan_only,
            pairing_window_secs,
        } => {
            let llm_config = resolve_llm_config(&cli)?;
            let profile = if local_only {
                NetworkProfile::LocalOnly
            } else if lan_only {
                NetworkProfile::LanOnly
            } else {
                NetworkProfile::StandardN0
            };
            let listener = TcpListener::bind(mcp_addr).await?;
            let bound_addr = listener.local_addr()?;
            let (engine, sandbox) = repo.open(&cli.data_dir).await?;
            engine.set_runtime_mode(RuntimeMode::Host).await?;
            let (event_tx, event_rx) = mpsc::unbounded_channel();
            let node = Arc::new(
                RelayNode::start_with_events(
                    &cli.data_dir.join("blobs"),
                    engine.clone(),
                    RelayNodeConfig {
                        profile,
                        sandbox: sandbox.clone(),
                    },
                    event_tx,
                )
                .await?,
            );
            if profile == NetworkProfile::StandardN0 {
                // Give the relay address time to appear while allowing offline startup.
                let _ = tokio::time::timeout(Duration::from_secs(5), node.endpoint().online()).await;
            }
            let window = Duration::from_secs(if pairing_window_secs == 0 {
                86_400
            } else {
                pairing_window_secs
            });
            node.open_pairing_window(window);

            if let Some(sb) = &sandbox {
                match sync_git_head_into_engine(&engine, sb, node.id().to_string()).await {
                    Ok(HeadSyncOutcome::Seeded { commit_sha }) => {
                        info!(commit = %commit_sha, "Seeded repository HEAD artifact");
                    }
                    Ok(HeadSyncOutcome::Superseded { commit_sha, report }) => {
                        info!(
                            commit = %commit_sha,
                            invalidated = report.invalidated_evidence_ids.len(),
                            "Superseded DAG artifact from Git HEAD"
                        );
                    }
                    Ok(HeadSyncOutcome::Unchanged) => {}
                    Err(error) => warn!(%error, "Unable to read initial Git HEAD"),
                }
            }

            let addr = node.addr();
            let pairing_ticket = serde_json::to_string(&addr)?;
            println!(
                "{}",
                json!({
                    "node_id": node.id().to_string(),
                    "repo_id": engine.repo_id(),
                    "mcp_url": format!("http://{bound_addr}/mcp"),
                    "addr": addr,
                    "pairing_ticket": pairing_ticket,
                })
            );
            eprintln!("Relay Host pairing ticket (paste into Relay Desktop -> Pair Device):");
            eprintln!("{pairing_ticket}");

            let server = RelayMcpServer::new(engine.clone(), sandbox.clone(), Some(node.clone()));
            let events_task = spawn_host_event_loop(
                event_rx,
                engine.clone(),
                server.clone(),
                sandbox.clone(),
                llm_config,
            );
            let git_task = sandbox.clone().map(|sb| {
                spawn_git_head_watcher(engine.clone(), sb, node.id().to_string())
            });

            let http_result = serve_mcp_http(listener, server).await;
            events_task.abort();
            if let Some(task) = git_task {
                task.abort();
            }
            let node_result = node.shutdown_shared().await;
            http_result?;
            node_result?;
        }
        Commands::McpStdio | Commands::McpHttp { .. } => {
            let (device, addr) = match cli.command {
                Commands::McpHttp { addr } => ("mcp-http", Some(addr)),
                _ => ("mcp-stdio", None),
            };
            let (engine, sandbox) = repo.open(&cli.data_dir).await?;
            let git_task = if let Some(sb) = &sandbox {
                if let Err(error) = sync_git_head_into_engine(&engine, sb, device).await {
                    warn!(%error, device, "Unable to sync initial Git HEAD for MCP");
                }
                Some(spawn_git_head_watcher(engine.clone(), sb.clone(), device.into()))
            } else {
                None
            };
            let server = RelayMcpServer::new(engine, sandbox, None);
            if let Some(addr) = addr {
                serve_mcp_http(TcpListener::bind(addr).await?, server).await?;
            } else {
                let service = server.serve((tokio::io::stdin(), tokio::io::stdout())).await?;
                service.waiting().await?;
            }
            if let Some(task) = git_task {
                task.abort();
            }
        }
        Commands::SandboxDual => sandbox_dual(&repo, &cli.data_dir).await?,
        Commands::Init { .. } | Commands::LlmCheck | Commands::Doctor { .. } => unreachable!(),
        Commands::SyncHead => {
            let (engine, sandbox) = repo.open(&cli.data_dir).await?;
            let sb = sandbox.context("repository must contain both .git and relay.toml")?;
            let out = match sync_git_head_explicitly(&engine, &sb, "cli").await? {
                HeadSyncOutcome::Unchanged => json!({"status": "unchanged"}),
                HeadSyncOutcome::Seeded { commit_sha } => {
                    json!({"status": "seeded", "commit_sha": commit_sha})
                }
                HeadSyncOutcome::Superseded { commit_sha, report } => json!({
                    "status": "superseded",
                    "commit_sha": commit_sha,
                    "trigger_node_id": report.trigger_node_id,
                    "invalidated_evidence_ids": report.invalidated_evidence_ids,
                    "reason": report.reason,
                }),
            };
            println!("{out}");
        }
        Commands::SubmitTask {
            preset,
            params,
            note,
            no_approval,
            base_commit,
        } => {
            let (engine, sandbox) = repo.open(&cli.data_dir).await?;
            let sb = sandbox.context("repository must contain both .git and relay.toml")?;
            let _ = sync_git_head_into_engine(&engine, &sb, "cli").await?;
            let base_commit_sha = match base_commit {
                Some(rev) => sb.resolve_commit(&rev).await?,
                None => sb.head_commit_sha().await?,
            };
            let mut param_map = BTreeMap::new();
            for raw in params {
                let (k, v) = raw
                    .split_once('=')
                    .with_context(|| format!("expected --param KEY=VALUE, got '{raw}'"))?;
                param_map.insert(k.trim().to_owned(), v.trim().to_owned());
            }
            let spec = TaskSpec {
                task_id: Uuid::now_v7(),
                repo_id: engine.repo_id().to_owned(),
                base_commit_sha,
                command_preset: preset,
                params: param_map,
                require_approval: !no_approval,
                requested_by_device: "cli".into(),
                user_note: note,
            };
            let (record, _) = engine.submit_task(spec).await?;
            println!("{}", serde_json::to_string_pretty(&record)?);
        }
        Commands::ComposeContext {
            strategy,
            all_strategies,
            no_sync,
        } => {
            let (engine, sandbox) = repo.open(&cli.data_dir).await?;
            if !no_sync
                && let Some(sb) = &sandbox
                && let Err(error) = sync_git_head_into_engine(&engine, sb, "cli").await
            {
                warn!(%error, "Unable to sync Git HEAD before composing context");
            }
            let snapshot = engine.dag_snapshot(true).await?;
            let tasks = engine.list_tasks().await?;
            let handoffs = engine.list_handoffs(u32::MAX).await?;
            if all_strategies {
                let output = json!({
                    "full_transcript": compose_context(&snapshot, &tasks, &handoffs, ContextStrategy::FullTranscript),
                    "flat_summary": compose_context(&snapshot, &tasks, &handoffs, ContextStrategy::FlatSummary),
                    "dag_bundle": compose_context(&snapshot, &tasks, &handoffs, ContextStrategy::DagBundle),
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                let bundle = compose_context(&snapshot, &tasks, &handoffs, strategy.into());
                println!("{}", serde_json::to_string_pretty(&bundle)?);
            }
        }
    }

    Ok(())
}

async fn run_doctor(cli: &Cli, as_json: bool, skip_llm: bool) -> Result<()> {
    let mut checks = Vec::new();
    let mut required_ok = true;

    let git_check = probe_binary("git", &["--version"]).await;
    required_ok &= git_check.0;
    checks.push(json!({
        "name": "git",
        "required": true,
        "status": if git_check.0 { "ok" } else { "fail" },
        "detail": git_check.1,
    }));

    let uv_check = probe_binary("uv", &["--version"]).await;
    required_ok &= uv_check.0;
    checks.push(json!({
        "name": "uv",
        "required": true,
        "status": if uv_check.0 { "ok" } else { "fail" },
        "detail": uv_check.1,
    }));

    let data_dir_ok = std::fs::create_dir_all(&cli.data_dir).is_ok();
    required_ok &= data_dir_ok;
    checks.push(json!({
        "name": "data_dir",
        "required": true,
        "status": if data_dir_ok { "ok" } else { "fail" },
        "detail": cli.data_dir.display().to_string(),
    }));

    let explicit_repo = cli.repo_root.is_some();
    let repo_candidate = match &cli.repo_root {
        Some(path) => Some(path.clone()),
        None => std::env::current_dir().ok().filter(|cwd| cwd.join("relay.toml").is_file()),
    };
    let (repo_status, repo_detail) = match repo_candidate {
        Some(root) => {
            let has_git = root.join(".git").exists();
            match load_manifest(&root) {
                Ok(manifest) => {
                    let has_smoke = manifest.presets.contains_key(&manifest.project.smoke_preset);
                    if has_git && has_smoke {
                        (
                            "ok",
                            format!(
                                "repo_id='{}', presets={} (smoke='{}')",
                                manifest.project.repo_id,
                                manifest.presets.len(),
                                manifest.project.smoke_preset,
                            ),
                        )
                    } else if !has_git {
                        if explicit_repo {
                            required_ok = false;
                        }
                        (
                            if explicit_repo { "fail" } else { "warn" },
                            format!("relay.toml loaded ({}) but .git directory is missing", manifest.project.repo_id),
                        )
                    } else {
                        if explicit_repo {
                            required_ok = false;
                        }
                        (
                            if explicit_repo { "fail" } else { "warn" },
                            format!(
                                "relay.toml smoke_preset '{}' not found in [presets]",
                                manifest.project.smoke_preset
                            ),
                        )
                    }
                }
                Err(error) => {
                    if explicit_repo {
                        required_ok = false;
                    }
                    (
                        if explicit_repo { "fail" } else { "warn" },
                        format!("could not load relay.toml: {error}"),
                    )
                }
            }
        }
        None => (
            "warn",
            "no relay.toml in current directory (pass --repo-root <path> when starting daemon)".into(),
        ),
    };
    checks.push(json!({
        "name": "repository",
        "required": explicit_repo,
        "status": repo_status,
        "detail": repo_detail,
    }));

    if skip_llm {
        checks.push(json!({
            "name": "llm_endpoint",
            "required": false,
            "status": "skipped",
            "detail": "skipped via --skip-llm",
        }));
    } else {
        match resolve_llm_config(cli).and_then(|cfg| Ok(LlmClient::new(cfg, Duration::from_secs(6))?)) {
            Ok(client) => {
                let probe = client.probe().await;
                checks.push(json!({
                    "name": "llm_endpoint",
                    "required": false,
                    "status": if probe.ok { "ok" } else { "warn" },
                    "detail": format!("{} ({}) — {}", probe.endpoint, probe.model, probe.message),
                }));
            }
            Err(error) => {
                checks.push(json!({
                    "name": "llm_endpoint",
                    "required": false,
                    "status": "warn",
                    "detail": error.to_string(),
                }));
            }
        }
    }

    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "ok": required_ok,
                "runtime_architecture": "2 Rust binaries (relay-desktop, relay-daemon) + external runtimes (git, uv, optional Ollama/Cloud LLM)",
                "checks": checks,
            }))?
        );
    } else {
        println!("Relay Doctor — Runtime & Environment Check");
        for item in &checks {
            let status = item["status"].as_str().unwrap_or("unknown").to_ascii_uppercase();
            let name = item["name"].as_str().unwrap_or("check");
            let detail = item["detail"].as_str().unwrap_or("");
            println!("  [{status:<7}] {name:<14} {detail}");
        }
    }

    anyhow::ensure!(required_ok, "One or more required Relay runtime checks failed");
    Ok(())
}

async fn probe_binary(program: &str, args: &[&str]) -> (bool, String) {
    match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            let line = String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or("ok")
                .trim()
                .to_owned();
            (true, line)
        }
        Ok(output) => (
            false,
            format!("exited with {} ({})", output.status, String::from_utf8_lossy(&output.stderr).trim()),
        ),
        Err(error) => (false, format!("not found on PATH ({error})")),
    }
}

fn spawn_host_event_loop(
    mut events: mpsc::UnboundedReceiver<RelayNodeEvent>,
    engine: Arc<RelayEngine>,
    mcp: RelayMcpServer,
    sandbox: Option<WorktreeSandbox>,
    llm_config: LlmEndpointConfig,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = events.recv().await {
            match event {
                RelayNodeEvent::PeerPaired => {
                    info!("Remote peer paired with Relay Host");
                }
                RelayNodeEvent::DagChanged => {
                    info!("DAG updated via peer sync");
                }
                RelayNodeEvent::TaskSubmitted(task_id) => {
                    if sandbox.is_none() {
                        warn!(%task_id, "Task submitted but no Git + relay.toml sandbox is configured");
                        continue;
                    }
                    let engine = engine.clone();
                    let mcp = mcp.clone();
                    let llm_config = llm_config.clone();
                    tokio::spawn(async move {
                        match run_host_task_pipeline(&engine, &mcp, llm_config, task_id).await {
                            Ok(outcome) => info!(%task_id, ?outcome, "Host task pipeline finished"),
                            Err(error) => warn!(%task_id, %error, "Host task pipeline failed"),
                        }
                    });
                }
                RelayNodeEvent::TaskCancelled(task_id) => {
                    info!(%task_id, "Task cancellation requested via peer RPC");
                }
                RelayNodeEvent::ApprovalResolved(task_id, _, decision) => {
                    if sandbox.is_none() {
                        continue;
                    }
                    let mcp = mcp.clone();
                    tokio::spawn(async move {
                        match mcp.finish_ui_approval(task_id, decision).await {
                            Ok(record) => info!(
                                %task_id,
                                state = ?record.state,
                                "Host approval execution finished"
                            ),
                            Err(error) => warn!(%task_id, %error, "Host approval execution failed"),
                        }
                    });
                }
            }
        }
    })
}

fn spawn_git_head_watcher(
    engine: Arc<RelayEngine>,
    sandbox: WorktreeSandbox,
    device_origin: String,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = time::interval(GIT_WATCH_INTERVAL);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            match sync_git_head_into_engine(&engine, &sandbox, device_origin.clone()).await {
                Ok(HeadSyncOutcome::Superseded { commit_sha, report }) => {
                    info!(
                        commit = %commit_sha,
                        invalidated = report.invalidated_evidence_ids.len(),
                        "Detected Git commit; cascaded STALE freshness"
                    );
                }
                Ok(HeadSyncOutcome::Seeded { commit_sha }) => {
                    info!(commit = %commit_sha, "Seeded Git HEAD artifact");
                }
                Ok(HeadSyncOutcome::Unchanged) => {}
                Err(error) => warn!(%error, "Git HEAD watcher check failed"),
            }
        }
    })
}

async fn serve_mcp_http(listener: TcpListener, server: RelayMcpServer) -> Result<()> {
    let addr = listener.local_addr()?;
    let config = StreamableHttpServerConfig::default();
    let cancel_sessions = config.cancellation_token.clone();
    let service = StreamableHttpService::new(
        move || Ok(server.clone()),
        LocalSessionManager::default().into(),
        config,
    );
    let app = axum::Router::new().nest_service("/mcp", service);
    info!(%addr, "Relay Streamable HTTP MCP listening on /mcp");
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            if let Err(error) = tokio::signal::ctrl_c().await {
                tracing::error!(%error, "Ctrl-C handler failed");
            }
            cancel_sessions.cancel();
        })
        .await?;
    Ok(())
}

async fn sandbox_dual(repo: &Repository, data_dir: &Path) -> Result<()> {
    let host_dir = data_dir.join("sandbox-host");
    let client_dir = data_dir.join("sandbox-client");
    let (host_engine, sandbox) = repo.open(&host_dir).await?;
    let client_engine =
        Arc::new(RelayEngine::open(&repo.id, client_dir.join("relay.db"), None).await?);
    host_engine.set_runtime_mode(RuntimeMode::Host).await?;
    client_engine.set_runtime_mode(RuntimeMode::Client).await?;

    let profile = NetworkProfile::LocalOnly;
    let host =
        RelayNode::start_with_engine(&host_dir.join("blobs"), host_engine, sandbox, profile).await?;
    let client = match RelayNode::start_with_engine(
        &client_dir.join("blobs"),
        client_engine,
        None,
        profile,
    )
    .await
    {
        Ok(client) => client,
        Err(error) => {
            host.shutdown().await?;
            return Err(error.into());
        }
    };

    let result = async {
        let host_addr = host.addr();
        host.set_remote_peer(client.addr());
        client.set_remote_peer(host_addr.clone());
        client.ping_peer(host_addr, "sandbox-client").await?;
        info!(
            host_id = %host.id(),
            client_id = %client.id(),
            rtt_ms = ?client.peer_status().rtt_ms,
            "Local Host and Client ping succeeded"
        );
        tokio::signal::ctrl_c().await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    let (host_result, client_result) = tokio::join!(host.shutdown(), client.shutdown());
    result?;
    host_result?;
    client_result?;
    Ok(())
}

fn run_init(
    repo_root: Option<&Path>,
    package_manager: Option<&str>,
    force: bool,
    dry_run: bool,
) -> Result<()> {
    let raw_root = match repo_root {
        Some(path) => path.to_path_buf(),
        None => std::env::current_dir().context("resolve current working directory")?,
    };
    let target_root = std::path::absolute(&raw_root)
        .with_context(|| format!("resolve repository root {}", raw_root.display()))?;
    anyhow::ensure!(
        target_root.is_dir(),
        "repository path is not a directory: {}",
        target_root.display()
    );

    let manifest_path = target_root.join("relay.toml");
    let existed_before = manifest_path.exists();

    let generated = if dry_run {
        generate_manifest_for_repo(&target_root, package_manager)?
    } else {
        write_manifest_for_repo_with_options(&target_root, package_manager, force).map_err(
            |error| {
                let message = error.to_string();
                if !force && message.contains("relay.toml already exists") {
                    anyhow::anyhow!(
                        "{message} at {} (pass --force to overwrite)",
                        manifest_path.display()
                    )
                } else {
                    anyhow::Error::from(error)
                }
            },
        )?
    };

    let manifest: RelayManifest = toml::from_str(&generated.toml_content)
        .context("parse generated starter relay.toml")?;
    let mut preset_names = Vec::with_capacity(manifest.presets.len());
    if manifest.presets.contains_key(&generated.smoke_preset) {
        preset_names.push(generated.smoke_preset.as_str());
    }
    for name in manifest.presets.keys() {
        if name != &generated.smoke_preset {
            preset_names.push(name.as_str());
        }
    }

    let source_label = if package_manager.is_some() {
        "selected"
    } else {
        "detected"
    };
    let action = if dry_run {
        if existed_before {
            format!(
                "dry-run preview (not written; {} already exists — pass --force to overwrite)",
                manifest_path.display()
            )
        } else {
            format!(
                "dry-run preview (not written; target: {})",
                manifest_path.display()
            )
        }
    } else if existed_before {
        format!("overwrote {}", manifest_path.display())
    } else {
        format!("wrote {}", manifest_path.display())
    };

    println!("SyncEverything Init — Manifest Summary");
    println!("  Repository:      {}", target_root.display());
    println!("  Repo ID:         {}", manifest.project.repo_id);
    println!("  Project Name:    {}", manifest.project.name);
    println!(
        "  Package Manager: {} ({source_label}, ecosystem: {})",
        generated.package_manager, generated.ecosystem
    );
    println!("  Smoke Preset:    {}", generated.smoke_preset);
    println!("  Presets:         {}", preset_names.join(", "));
    println!("  Status:          {action}");

    if dry_run {
        println!();
        println!("--- relay.toml (dry-run preview) ---");
        print!("{}", generated.toml_content);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_repo_root_both_before_and_after_init_subcommand() {
        let before = Cli::try_parse_from([
            "relay-daemon",
            "--repo-root",
            "/tmp/sample-repo",
            "init",
            "--package-manager",
            "cargo",
            "--force",
            "--dry-run",
        ])
        .expect("parse global --repo-root before init");
        assert_eq!(before.repo_root, Some(PathBuf::from("/tmp/sample-repo")));
        match before.command {
            Commands::Init {
                package_manager,
                force,
                dry_run,
            } => {
                assert_eq!(package_manager.as_deref(), Some("cargo"));
                assert!(force);
                assert!(dry_run);
            }
            other => panic!("expected Init command, got {other:?}"),
        }

        let after = Cli::try_parse_from([
            "relay-daemon",
            "init",
            "--repo-root",
            "/tmp/sample-repo",
            "--dry-run",
        ])
        .expect("parse global --repo-root after init");
        assert_eq!(after.repo_root, Some(PathBuf::from("/tmp/sample-repo")));
        match after.command {
            Commands::Init {
                package_manager,
                force,
                dry_run,
            } => {
                assert!(package_manager.is_none());
                assert!(!force);
                assert!(dry_run);
            }
            other => panic!("expected Init command, got {other:?}"),
        }
    }
}
