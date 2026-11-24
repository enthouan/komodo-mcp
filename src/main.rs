mod listener_health;

use std::{fmt, process::ExitCode};

use komodo_mcp::{
    client::{CoreClient, RequestPolicy},
    config::{Config, Transport},
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
    if std::env::args_os()
        .skip(1)
        .eq([std::ffi::OsStr::new("--healthcheck")])
    {
        return if listener_health::check().await {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
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
    let transport = config.transport().clone();
    let over_http = matches!(transport, Transport::Http(_));
    // Dependency Debug/Trace events include complete MCP requests and replies,
    // including newly supplied configuration secrets that are not API credentials.
    // Apply this hard filter after RUST_LOG, even for more specific directives.
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(tracing_subscriber::filter::filter_fn(move |metadata| {
            // HTTP dependency events can include arbitrary incoming headers and session IDs.
            metadata.target().starts_with("komodo_mcp")
                || (!over_http && *metadata.level() <= tracing::Level::INFO)
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

    if let Transport::Http(http) = transport {
        let listener = tokio::net::TcpListener::bind(http.bind())
            .await
            .map_err(|_| StartupError::Runtime("failed to bind the configured HTTP listener"))?;
        let address = listener
            .local_addr()
            .map_err(|_| StartupError::Runtime("failed to inspect the HTTP listener"))?;
        tracing::info!(%address, "starting komodo-mcp over HTTP");
        return komodo_mcp::http::serve(listener, http, client, core_version, shutdown_signal())
            .await
            .map_err(|_| StartupError::Runtime("MCP service stopped on an HTTP transport error"));
    }

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

async fn shutdown_signal() {
    #[cfg(unix)]
    if let Ok(mut terminate) =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
        return;
    }
    let _ = tokio::signal::ctrl_c().await;
}
