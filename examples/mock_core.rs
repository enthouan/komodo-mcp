#[path = "support/mock_core.rs"]
mod mock_core;

use anyhow::{Context, Result, ensure};

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let bind = args.next().unwrap_or_else(|| "127.0.0.1:8089".into());
    ensure!(args.next().is_none(), "usage: mock_core [127.0.0.1:PORT]");
    let core =
        mock_core::MockCore::start(bind.parse().context("expected loopback IP:port")?).await?;
    println!("MOCK_CORE_READY {}", core.address);
    use std::io::Write;
    std::io::stdout().flush()?;
    eprintln!("Synthetic Core is ready; Ctrl-C stops it.");
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! { result = tokio::signal::ctrl_c() => result?, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    tokio::signal::ctrl_c().await?;
    let verified = core.verify_reads();
    let stopped = core.shutdown().await;
    stopped?;
    verified
}
