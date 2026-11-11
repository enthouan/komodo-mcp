mod support;
use anyhow::{Context, Result, ensure};
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let executable = PathBuf::from(
        args.next()
            .context("usage: stacks_on_server PATH_TO_KOMODO_MCP [SERVER_NAME]")?,
    );
    let server = args
        .next()
        .map(|s| {
            s.into_string()
                .map_err(|_| anyhow::anyhow!("server name must be UTF-8"))
        })
        .transpose()?
        .unwrap_or_else(|| support::mock_core::DEFAULT_SERVER.into());
    ensure!(
        args.next().is_none(),
        "usage: stacks_on_server PATH_TO_KOMODO_MCP [SERVER_NAME]"
    );
    let answer = support::scenario::demonstrate(&executable, &server).await?;
    println!("{answer}");
    Ok(())
}
