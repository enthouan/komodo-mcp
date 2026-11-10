mod server;

use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use rmcp::{ServiceExt, service::QuitReason, transport::stdio};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("komodo-mcp failed: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .without_time()
        .try_init()
        .map_err(|error| anyhow::anyhow!("failed to configure diagnostics: {error}"))?;

    tracing::info!("starting komodo-mcp over stdio");
    let service = server::KomodoServer
        .serve(stdio())
        .await
        .context("failed to initialize MCP service over stdio")?;

    match service.waiting().await.context("MCP service task failed")? {
        // rmcp reports stdin EOF as Closed after a session has initialized.
        QuitReason::Closed => tracing::info!("MCP session closed"),
        QuitReason::JoinError(error) => return Err(error).context("MCP request task failed"),
        reason => bail!("MCP service stopped unexpectedly: {reason:?}"),
    }
    Ok(())
}
