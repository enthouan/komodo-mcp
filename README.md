# komodo-mcp

A Rust [Model Context Protocol](https://modelcontextprotocol.io/) server for
Komodo, using the official [rmcp 3.8.0 SDK](https://docs.rs/rmcp/3.8.0/rmcp/).
This bootstrap advertises tools and returns an empty `tools/list` result. No
Komodo Core instance, credentials, or network access is needed to run it.

## Build and launch

Install stable Rust **1.99.1** with Cargo, rustfmt, and Clippy (for example, using
[rustup](https://rustup.rs/)). The included `rust-toolchain.toml` selects this
toolchain for rustup. With asdf-rust, run `asdf install`; `.tool-versions` selects
the same version for this checkout.

```sh
cargo build --locked --release
./target/release/komodo-mcp
```

Configure an MCP client to launch the compiled executable using its absolute
path. The client sends newline-delimited JSON-RPC messages on the process's
stdin and reads MCP responses from stdout. There is no interactive shell prompt.
After initialization, the client can query `tools/list`; closing stdin ends the
session and exits successfully.

All application and SDK diagnostics go to stderr, keeping stdout exclusively
for MCP messages. Logging defaults to `info` and accepts `RUST_LOG` filtering:

```sh
RUST_LOG=komodo_mcp=debug,rmcp=warn ./target/release/komodo-mcp
```

## Development

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

CI runs these commands on Linux for pull requests and pushes to `main`.
The subprocess smoke test launches Cargo's compiled binary and explicitly uses
MCP **2025-11-25**: `initialize`, `notifications/initialized`, and `tools/list`.
It also checks the SDK's `INVALID_PARAMS` (`-32602`, `tool not found`) response
to an unknown tool, queries the list again, and closes stdin. Both output streams
are captured concurrently, every stdout frame is validated through EOF, and
exchanges, shutdown, and output draining have deadlines. This empty server emits
no unsolicited MCP notifications.

The SDK also supports newer discovery-based startup; the smoke test deliberately
selects the initialize lifecycle. Add future tools to the router in `src/server.rs`.

To build an optimized executable and inspect the source package contents:

```sh
cargo build --locked --release
cargo package --locked --allow-dirty --list
```

## License

Licensed under **GPL-3.0-or-later**. See [LICENSE](LICENSE) for the declaration and
full license text. The source package includes this license. Include `LICENSE`
alongside any redistributed executable; this project does not yet build binary
distribution archives.
