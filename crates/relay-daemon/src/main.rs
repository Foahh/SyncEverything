use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use relay_core::contracts::RuntimeMode;
use relay_core::engine::RelayEngine;
use relay_core::mcp::RelayMcpServer;
use relay_core::sandbox::WorktreeSandbox;
use relay_core::sync::{NetworkProfile, RelayNode};
use rmcp::ServiceExt;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use serde_json::json;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tracing::info;

#[derive(Debug, Parser)]
#[command(name = "relay-daemon", about = "Relay headless Host/Client daemon & MCP server")]
struct Cli {
    #[arg(long, global = true, default_value = ".relay")]
    data_dir: PathBuf,

    #[arg(long, global = true)]
    repo_root: Option<PathBuf>,

    #[arg(long, global = true)]
    repo_id: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Start the Relay Host with P2P sync and Streamable HTTP MCP.
    Serve {
        #[arg(long, default_value = "127.0.0.1:7608")]
        mcp_addr: SocketAddr,
        #[arg(long)]
        local_only: bool,
    },
    /// Serve Relay MCP over stdio (compatible with dsh 0.2.0-rc.2).
    McpStdio,
    /// Serve Relay MCP over Streamable HTTP at /mcp.
    McpHttp {
        #[arg(long, default_value = "127.0.0.1:7608")]
        addr: SocketAddr,
    },
    /// Start isolated Host and Client nodes and check the loopback route.
    SandboxDual,
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

    let repo = Repository::resolve(cli.repo_root, cli.repo_id)?;
    match cli.command {
        Commands::Serve {
            mcp_addr,
            local_only,
        } => {
            let profile = if local_only {
                NetworkProfile::LocalOnly
            } else {
                NetworkProfile::StandardN0
            };
            let listener = TcpListener::bind(mcp_addr).await?;
            let (engine, sandbox) = repo.open(&cli.data_dir).await?;
            engine.set_runtime_mode(RuntimeMode::Host).await?;
            let node = Arc::new(
                RelayNode::start_with_engine(
                    &cli.data_dir.join("blobs"),
                    engine.clone(),
                    sandbox.clone(),
                    profile,
                )
                .await?,
            );
            if !local_only {
                // Give the relay address time to appear while allowing offline startup.
                let _ = tokio::time::timeout(Duration::from_secs(5), node.endpoint().online()).await;
            }
            println!("{}", json!({"node_id": node.id().to_string(), "addr": node.addr()}));
            let server = RelayMcpServer::new(engine, sandbox, Some(node.clone()));
            let http_result = serve_mcp_http(listener, server).await;
            let node_result = node.shutdown_shared().await;
            http_result?;
            node_result?;
        }
        Commands::McpStdio => {
            let (engine, sandbox) = repo.open(&cli.data_dir).await?;
            let server = RelayMcpServer::new(engine, sandbox, None);
            let service = server.serve((tokio::io::stdin(), tokio::io::stdout())).await?;
            service.waiting().await?;
        }
        Commands::McpHttp { addr } => {
            let (engine, sandbox) = repo.open(&cli.data_dir).await?;
            let server = RelayMcpServer::new(engine, sandbox, None);
            serve_mcp_http(TcpListener::bind(addr).await?, server).await?;
        }
        Commands::SandboxDual => sandbox_dual(&repo, &cli.data_dir).await?,
    }

    Ok(())
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
