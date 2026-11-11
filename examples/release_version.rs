//! Verify an explicitly selected executable through MCP, never a --version flag.
mod support;

use anyhow::{Context, Result, ensure};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let executable = PathBuf::from(
        args.next()
            .context("usage: release_version PATH_TO_KOMODO_MCP EXPECTED_VERSION")?,
    );
    let expected = args
        .next()
        .context("expected release version is required")?
        .into_string()
        .map_err(|_| anyhow::anyhow!("expected version must be UTF-8"))?;
    ensure!(args.next().is_none(), "unexpected release_version argument");
    println!(
        "{}",
        support::scenario::verify_release(&executable, &expected).await?
    );
    Ok(())
}
