use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use relay_core::agent::{
    InferenceTier, LlmClient, LlmEndpointConfig, load_llm_config, run_host_task_pipeline,
};
use relay_core::contracts::{RuntimeMode, TaskSpec};
use relay_core::engine::RelayEngine;
use relay_core::mcp::RelayMcpServer;
use relay_core::sandbox::{HeadSyncOutcome, WorktreeSandbox, load_manifest, sync_git_head_into_engine};
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
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
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

#[derive(Debug, Subcommand)]
enum Commands {
    /// Start the Relay Host with P2P sync, pairing ticket, Host executor, and Streamable HTTP MCP.
    Serve {
        #[arg(long, default_value = "127.0.0.1:7608")]
        mcp_addr: SocketAddr,
        #[arg(long)]
        local_only: bool,
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
    /// Probe the configured Ollama or OpenAI-compatible Cloud LLM endpoint.
    LlmCheck,
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
            (None, Some(root)) => manifest_repo_id(root)?,
            (None, None) => None,
        };
        Ok(Self {
            id: id.unwrap_or_else(|| "forecast-bench".into()),
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

    if matches!(cli.command, Commands::LlmCheck) {
        let cfg = resolve_llm_config(&cli)?;
        let client = LlmClient::new(cfg, Duration::from_secs(15))?;
        let probe = client.probe().await;
        println!("{}", serde_json::to_string_pretty(&probe)?);
        anyhow::ensure!(probe.ok, "LLM probe failed: {}", probe.message);
        return Ok(());
    }

    let repo = Repository::resolve(cli.repo_root.clone(), cli.repo_id.clone())?;
    let llm_config = resolve_llm_config(&cli)?;
    match cli.command {
        Commands::Serve {
            mcp_addr,
            local_only,
            pairing_window_secs,
        } => {
            let profile = if local_only {
                NetworkProfile::LocalOnly
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
            if !local_only {
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
        Commands::McpStdio => {
            let (engine, sandbox) = repo.open(&cli.data_dir).await?;
            if let Some(sb) = &sandbox
                && let Err(error) = sync_git_head_into_engine(&engine, sb, "mcp-stdio").await
            {
                warn!(%error, "Unable to sync initial Git HEAD for mcp-stdio");
            }
            let server = RelayMcpServer::new(engine, sandbox, None);
            let service = server.serve((tokio::io::stdin(), tokio::io::stdout())).await?;
            service.waiting().await?;
        }
        Commands::McpHttp { addr } => {
            let (engine, sandbox) = repo.open(&cli.data_dir).await?;
            if let Some(sb) = &sandbox
                && let Err(error) = sync_git_head_into_engine(&engine, sb, "mcp-http").await
            {
                warn!(%error, "Unable to sync initial Git HEAD for mcp-http");
            }
            let server = RelayMcpServer::new(engine, sandbox, None);
            serve_mcp_http(TcpListener::bind(addr).await?, server).await?;
        }
        Commands::SandboxDual => sandbox_dual(&repo, &cli.data_dir).await?,
        Commands::LlmCheck => unreachable!(),
        Commands::SyncHead => {
            let (engine, sandbox) = repo.open(&cli.data_dir).await?;
            let sb = sandbox.context("repository must contain both .git and relay.toml")?;
            let out = match sync_git_head_into_engine(&engine, &sb, "cli").await? {
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
                Some(sha) => sha,
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
    }

    Ok(())
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
                RelayNodeEvent::ApprovalResolved(task_id, decision) => {
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
