# Building and testing the native server

## Fresh-checkout demo (synthetic Core)

Requires Git and Rust **1.99.1** with Cargo. Run these commands in a terminal;
all Cargo commands run from the cloned repository root. With
[rustup](https://rustup.rs/):

```sh
git clone https://github.com/enthouan/komodo-mcp.git
cd komodo-mcp
rustup toolchain install 1.99.1 --profile minimal --component rustfmt,clippy
cargo build --locked --release --bin komodo-mcp
cargo build --locked --examples
cargo run --locked --example stacks_on_server -- ./target/release/komodo-mcp
```

The included `rust-toolchain.toml` selects 1.99.1. With asdf-rust, use
`asdf install` instead of the rustup command; `.tool-versions` selects the same
version. No Node, real Core, real API credentials, or model service is needed.
If you override Cargo's target directory, pass the actual binary path in place
of `./target/release/komodo-mcp`.

The last command starts an ephemeral loopback mock, launches the **compiled
production executable**, initializes an official rmcp stdio client using MCP
`2025-11-25`, discovers the read-only tools, resolves `fixture-server` by exact name,
and follows `list_stacks` continuation with `page_size: 2`. It selects only
`target.kind == "server"` and `target.id == get_server(...).id` from MCP responses.
Representative output (Cargo progress and the Core read trace omitted):

```text
Initialized MCP 2025-11-25; discovered read-only tools; called get_server and list_stacks over stdio.
Stacks on fixture-server (000000000000000000000001):
  alpha-web (000000000000000000000002) — running
  gamma-jobs (000000000000000000000005) — stopped
  zeta-metrics (000000000000000000000008) — paused
Scanned 3 pages (6 visible stacks).
```

The client closes MCP stdin, checks a successful child exit, and stops the
mock. Stderr confirms `Cleanup: MCP child exited exit status: 0; mock stopped.`
(The exit-status wording varies by OS.) Failures exit nonzero and print captured
server stderr. Ctrl-C and SIGTERM also run cleanup. Exchanges, the overall
scenario, and shutdown have deadlines; repeated IDs or invalid continuation
fail explicitly with an incomplete answer instead of looping indefinitely.
Inherited `KOMODO_ADDRESS`, `KOMODO_API_KEY`, and `KOMODO_API_SECRET` are replaced
with explicit synthetic configuration in the child process.

Pass another exact synthetic server name as the final argument:

```sh
cargo run --locked --example stacks_on_server -- ./target/release/komodo-mcp empty-server
```

This reports `No matching stacks in the 6 visible stacks.` for `empty-server`.
`other-server` has `beta-other`; the Swarm and unassigned stacks do not match a
Server. This is fixture-based verification of the SDK and MCP path; it does not
run Core's authorization engine or contact deployed infrastructure.

For a standalone mock and a verified MCP Inspector configuration, see
[the interactive walkthrough](../mock-stdio.md). For real Core, continue below.

## Build and launch

Install stable Rust **1.99.1** with Cargo, rustfmt, and Clippy (for example, using
[rustup](https://rustup.rs/)). The included `rust-toolchain.toml` selects this
toolchain for rustup. With asdf-rust, run `asdf install`; `.tool-versions` selects
the same version for this checkout.

```sh
cargo build --locked --release
export KOMODO_ADDRESS=https://komodo.example.test
export KOMODO_API_KEY=replace-with-your-api-key
export KOMODO_API_SECRET=replace-with-your-api-secret
./target/release/komodo-mcp
```

Configure an MCP client to launch the compiled executable using its absolute
path. The client sends newline-delimited JSON-RPC messages on the process's
stdin and reads MCP responses from stdout. There is no interactive shell prompt.
After initialization, the client can query `tools/list`; closing stdin ends the
session and exits successfully.

All three environment variables are required. See [.env.example](../../.env.example)
for synthetic placeholders; the executable reads its environment once and does
not load `.env` files. Configure these variables in your MCP client's process
environment. Use an absolute HTTP or HTTPS address, optionally with a base path
such as `https://komodo.example.test/komodo/`. A trailing slash is normalized;
the path prefix is preserved. User information, query strings, fragments, empty
credentials, and invalid header characters are rejected without printing values.

Startup sends an authenticated `GetVersion` read before starting MCP and retains
the actual Core version alongside the shared client. Invalid configuration or a
failed version probe exits nonzero with a safe stderr diagnostic and no stdout.
The user must be enabled. Inventory reads need `Read` access to the relevant
Servers, Stacks, and Deployments being inspected, via a resource grant, group,
global permission, base permission, or transparent mode; admin, write, execute, and create privileges are unnecessary.
A permission-filtered empty inventory is a successful result.
