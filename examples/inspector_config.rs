use anyhow::{Context, Result, ensure};
use std::path::PathBuf;

fn configuration(executable: PathBuf, address: Option<String>) -> Result<serde_json::Value> {
    let executable = executable
        .canonicalize()
        .context("compiled executable not found; build --release --bin komodo-mcp first")?;
    ensure!(executable.is_file(), "server path must be a file");
    let executable = executable
        .to_str()
        .context("Inspector requires a UTF-8 executable path")?;
    let mut config: serde_json::Value = serde_json::from_str(include_str!("mcp-inspector.json"))?;
    config["mcpServers"]["komodo-mock"]["command"] = executable.into();
    if let Some(address) = address {
        config["mcpServers"]["komodo-mock"]["env"]["KOMODO_ADDRESS"] = address.into();
    }
    Ok(config)
}
fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let executable = PathBuf::from(
        args.next()
            .context("usage: inspector_config PATH_TO_KOMODO_MCP [MOCK_ADDRESS]")?,
    );
    let address = args
        .next()
        .map(|s| {
            s.into_string()
                .map_err(|_| anyhow::anyhow!("address must be UTF-8"))
        })
        .transpose()?;
    ensure!(
        args.next().is_none(),
        "usage: inspector_config PATH_TO_KOMODO_MCP [MOCK_ADDRESS]"
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&configuration(executable, address)?)?
    );
    Ok(())
}

#[test]
fn serializes_executable_paths_with_spaces() -> Result<()> {
    let directory = std::env::temp_dir().join(format!("komodo inspector {}", std::process::id()));
    std::fs::create_dir_all(&directory)?;
    let executable = directory.join("server with spaces");
    std::fs::write(&executable, [])?;
    let expected = executable.canonicalize()?;
    let result = configuration(executable.clone(), None);
    std::fs::remove_dir_all(directory)?;
    let config = result?;
    let text = serde_json::to_string(&config)?;
    let parsed: serde_json::Value = serde_json::from_str(&text)?;
    assert_eq!(
        parsed["mcpServers"]["komodo-mock"]["command"]
            .as_str()
            .unwrap(),
        expected.to_str().unwrap()
    );
    Ok(())
}
