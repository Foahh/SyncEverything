use anyhow::Result;
use clap::{Parser, Subcommand};
use relay_core::dag::DagEngine;
use relay_core::mcp::RelayMcpServer;
use relay_core::storage::RelayStore;
use relay_core::sync::RelayNode;
use rmcp::ServiceExt;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

#[derive(Debug, Parser)]
#[command(name = "relay-daemon", about = "Relay headless Host/Client daemon & MCP server")]
struct Cli {
    #[arg(long, default_value = ".relay")]
    data_dir: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Start the headless Relay host daemon (iroh 1.5 P2P + SQLite WAL + optional Streamable HTTP MCP)
    Serve {
        #[arg(long, default_value = "127.0.0.1:7608")]
        mcp_addr: SocketAddr,
    },
    /// Serve the frozen Relay MCP toolset over stdio (compatible with dsh 0.2.0-rc.2)
    McpStdio,
    /// Serve the frozen Relay MCP toolset over Streamable HTTP (/mcp)
    McpHttp {
        #[arg(long, default_value = "127.0.0.1:7608")]
        addr: SocketAddr,
    },
    /// Launch an isolated local dual-node sandbox (Host + Client) on one machine for fast dev testing
    SandboxDual,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // When serving MCP over stdio, logs MUST go to stderr so stdout stays pure JSON-RPC.
    let stderr_only = matches!(cli.command, Commands::McpStdio);
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,relay_core=debug,relay_daemon=debug".into()),
        );
    if stderr_only {
        subscriber.with_writer(std::io::stderr).init();
    } else {
        subscriber.init();
    }

    let dag = Arc::new(RwLock::new(DagEngine::new("default-repo")));

    match cli.command {
        Commands::Serve { mcp_addr } => {
            let db_path = cli.data_dir.join("relay.db");
            let blob_dir = cli.data_dir.join("blobs");
            let _store = RelayStore::connect(&db_path).await?;
            let node = RelayNode::start(&blob_dir).await?;
            info!(node_id = %node.id(), %mcp_addr, "Relay daemon ready");
            tokio::signal::ctrl_c().await?;
            node.shutdown().await?;
        }
        Commands::McpStdio => {
            let server = RelayMcpServer::new(dag);
            let transport = (tokio::io::stdin(), tokio::io::stdout());
            let service = server.serve(transport).await?;
            service.waiting().await?;
        }
        Commands::McpHttp { addr } => {
            let server = RelayMcpServer::new(dag);
            let service = StreamableHttpService::new(
                move || Ok(server.clone()),
                LocalSessionManager::default().into(),
                StreamableHttpServerConfig::default(),
            );
            let app = axum::Router::new().nest_service("/mcp", service);
            let listener = tokio::net::TcpListener::bind(addr).await?;
            info!(%addr, "Relay MCP Streamable HTTP server listening on /mcp");
            axum::serve(listener, app).await?;
        }
        Commands::SandboxDual => {
            let host_blobs = cli.data_dir.join("sandbox-host/blobs");
            let client_blobs = cli.data_dir.join("sandbox-client/blobs");
            let host = RelayNode::start(&host_blobs).await?;
            let client = RelayNode::start(&client_blobs).await?;
            info!(host_id = %host.id(), client_id = %client.id(), "Local dual-node sandbox started");
            tokio::signal::ctrl_c().await?;
            client.shutdown().await?;
            host.shutdown().await?;
        }
    }

    Ok(())
}
