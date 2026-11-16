use std::{fmt, process::ExitCode};

use komodo_mcp::{
    client::{CoreClient, RequestPolicy},
    config::Config,
    diagnostics::SafeDiagnostics,
    error::AppError,
    server::KomodoServer,
};
use rmcp::{ServiceExt, service::QuitReason, transport::stdio};
use tracing_subscriber::{EnvFilter, prelude::*};

// Keep this reporting boundary closed to raw SDK/protocol error chains.
#[derive(Debug)]
enum StartupError {
    Core(AppError),
    Runtime(&'static str),
}

impl From<AppError> for StartupError {
    fn from(error: AppError) -> Self {
        Self::Core(error)
    }
}

impl fmt::Display for StartupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Core(error) => error.fmt(f),
            Self::Runtime(message) => f.write_str(message),
        }
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("komodo-mcp failed: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), StartupError> {
    let config = Config::from_env()?;
    // Dependency Debug/Trace events include complete MCP requests and replies,
    // including newly supplied configuration secrets that are not API credentials.
    // Apply this hard filter after RUST_LOG, even for more specific directives.
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::filter::filter_fn(|metadata| {
            metadata.target().starts_with("komodo_mcp") || *metadata.level() <= tracing::Level::INFO
        }))
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(SafeDiagnostics::new(&config, std::io::stderr))
                .with_ansi(false)
                .without_time(),
        )
        .try_init()
        .map_err(|_| StartupError::Runtime("failed to configure diagnostics"))?;

    let client = CoreClient::new(config, RequestPolicy::default())?;
    let core_version = client.detect_version().await?;
    tracing::info!(version = %core_version, "connected to Komodo Core");

    tracing::info!("starting komodo-mcp over stdio");
    let outcome = async {
        let service = KomodoServer {
            client: client.clone(),
            core_version,
        }
        .serve(stdio())
        .await
        .map_err(|_| StartupError::Runtime("failed to initialize MCP service over stdio"))?;

        match service
            .waiting()
            .await
            .map_err(|_| StartupError::Runtime("MCP service task failed"))?
        {
            // rmcp reports stdin EOF as Closed after a session has initialized.
            QuitReason::Closed => tracing::info!("MCP session closed"),
            QuitReason::JoinError(_) => {
                return Err(StartupError::Runtime("MCP request task failed"));
            }
            _ => return Err(StartupError::Runtime("MCP service stopped unexpectedly")),
        }
        Ok(())
    }
    .await;
    client.shutdown_terminals().await;
    outcome
}
