# komodo-mcp

A Rust [Model Context Protocol](https://modelcontextprotocol.io/) server for
Komodo, using the official [rmcp 3.8.0 SDK](https://docs.rs/rmcp/3.8.0/rmcp/).
It connects to Komodo Core using the official **komodo_client 2.3.2**, pinned
exactly with optional features disabled. The tool router remains empty; public
inventory tools follow in issue #3.

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

All three environment variables are required. See [.env.example](.env.example)
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
servers, via a resource grant, group, global permission, base permission, or
transparent mode; admin, write, execute, and create privileges are unnecessary.
A permission-filtered empty inventory is a successful result.

The connection timeout is **5 seconds**, and each complete SDK operation has an
independent **15-second deadline**, including headers and response-body reading.
Tests inject shorter durations. Automatic redirects are disabled to keep API
credentials at the configured Core; point `KOMODO_ADDRESS` at the final URL.
Normal TLS certificate verification remains enabled.

All application and SDK diagnostics go to stderr, keeping stdout exclusively
for MCP messages. Logging defaults to `info` and accepts `RUST_LOG` filtering.
Upstream error bodies are discarded at the application boundary. Configuration
and client Debug output hide credentials, and the diagnostic writer redacts
credentials in complete log events, including verbose dependency trace fields:

```sh
RUST_LOG=komodo_mcp=debug,rmcp=warn ./target/release/komodo-mcp
```

## Client and error foundation

`CoreClient` in `src/client.rs` wraps one SDK instance and one configured reqwest
transport in an `Arc`; cloned handles retain the connection pool. Its generic
`read` method accepts official typed read requests and applies the same deadline
and safe error conversion. `detect_version` validates and retains the observed
SemVer response; `list_servers` reads one explicit page of at most 30 servers.
The executable shares this client and version through `KomodoServer`.

`AppError` contains only application-owned messages, a stable `kind`, the
operation, optional HTTP status, and configuration correction metadata:

| `kind` | Meaning |
| --- | --- |
| `configuration` | Missing or invalid environment/configuration field |
| `authentication` | Core rejected credentials or the user is disabled (401) |
| `permission_denied` | Forbidden (403), or the known Core 2.3.2 read-permission helper error (500) |
| `timeout` | Connection timeout or complete-operation deadline exceeded |
| `unreachable` | Connection, DNS, or TLS connection failure |
| `upstream` | Other unexpected HTTP or upstream failure |
| `invalid_response` | Malformed JSON, incompatible response shape, or invalid version data |

Future tools should return `error.into_tool_result()` for execution failures.
This produces rmcp's `isError: true` result with safe text and structured error
metadata. Dispatch/protocol errors remain SDK protocol errors. Do not report raw
SDK errors or response bodies. A narrow, isolated signature recognizes Core
2.3.2's resource permission helper; unrelated HTTP 500 responses stay `upstream`.

## Version evidence

The dependency is **komodo_client 2.3.2**; the local HTTP fixtures represent
**Komodo Core 2.3.2**. The observed remote version comes from `GetVersion`, not
from the dependency or fixture version. Other valid SemVer responses are retained
without a claim of compatibility. See the [fixture sources and limitations](tests/fixtures/core-2.3.2/README.md).
Verification used synthetic, enabled non-admin fixtures with resource read
access. **No running Core release was exercised**, because no test Core or
credentials were configured. Compatibility beyond these wire fixtures remains
unverified; changes to Core's HTTP 500 permission signature may require an update.

No optional `komodo_client` features are enabled. Its mandatory dependencies
enable reqwest's `json`, `stream`, `form`, `query`, and `rustls` features, plus
upstream's other mandatory dependencies. The direct reqwest requirement is
compatible with the SDK's `0.13.4` requirement; the lockfile resolves both to
**reqwest 0.13.5**, with one shared client type. No additional upstream feature
was needed.

## Development

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo tree --locked -e features -i komodo_client
cargo tree --locked -e features -i reqwest
```

CI runs formatting, Clippy, and tests on Linux for pull requests and pushes to
`main`, using only local fixtures and synthetic credentials.
The subprocess smoke test launches Cargo's compiled binary and explicitly uses
MCP **2025-11-25**: `initialize`, `notifications/initialized`, and `tools/list`.
It also checks the SDK's `INVALID_PARAMS` (`-32602`, `tool not found`) response
to an unknown tool, queries the list again, and closes stdin. Both output streams
are captured concurrently, every stdout frame is validated through EOF, and
exchanges, shutdown, and output draining have deadlines that accommodate the
startup probe. The test supplies a local mock Core and checks startup failures
and credential-safe stderr with `RUST_LOG=trace`. This empty router emits no
unsolicited MCP notifications.

HTTP integration tests exercise the real SDK, authenticated request envelopes,
official inventory response types, permission-filtered emptiness, typed failures,
delayed headers, stalled success/error bodies, TLS connection timeout, refused
connections, redirects, pool reuse, concurrency, and recovery after failure.
A test-only rmcp handler captures real MCP frames for execution failures without
adding a production tool. Fixture servers and subprocesses are bounded and
cleaned up on failure. Tests never mutate the parent process environment.

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
