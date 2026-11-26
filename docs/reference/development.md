# Development and version evidence

## Version evidence

The [v0.9.0 compatibility matrix](../v0-9-0-compatibility.md) identifies the
package, client, transport and service versions for each workflow. Compare the
MCP tools and their grants with the [Komodo API](../v0-9-0-api-coverage.md)
and the [official `km` CLI](../v0-9-0-cli-coverage.md).

The Rust dependency and synthetic HTTP fixtures target Komodo Core 2.3.2.
The fixtures exercise scoped non-admin reads and Core's permission errors.
Running Core instances report their version through `GetVersion`. On Linux
x86-64, the packaged v0.9 milestone candidate completed container diagnostics,
configuration preview/apply and interrupted Deployment recovery against Core
and Periphery 2.3.2 over native stdio and authenticated Streamable HTTP.
[Disposable regression evidence](../releases/v0.9.0/disposable-regression.md)

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
It also verifies release metadata and initializes the actual release executable
to check its MCP application version. Release notes live in [CHANGELOG.md](../../CHANGELOG.md);
maintainers use [the release skill](../../.agents/skills/komodo-mcp-release/SKILL.md)
for validation, publication, and recovery.
The subprocess smoke test launches Cargo's compiled binary and explicitly uses
MCP **2025-11-25**: configured startup, initialization, read-only tool discovery, list
and ID/name detail calls in every family, inventory continuation to an exactly
full final page, actionable failures, recovery, and stdin EOF shutdown. It also
checks the SDK's unknown-tool protocol error. Both output streams are captured
concurrently, every stdout frame is validated through EOF, and all phases have
deadlines. Verbose `RUST_LOG=trace` exercises credential redaction; populated
sensitive fixture fields must never appear in MCP content or captured stderr.

HTTP integration tests exercise the real SDK, exact typed authenticated read
requests, permission-filtered emptiness, pagination offsets and request counts,
bounded exact/ambiguous/name-shaped-ID resolution, all response field allowlists,
target semantics, typed failures, delayed headers/bodies, TLS timeouts,
connectivity, redirects, pool reuse, concurrency, and recovery. The actual
production `KomodoServer` is exercised through rmcp, alongside the foundation's
error-frame tests. Fixtures and subprocesses are bounded and cleaned up on
failure. Tests never mutate the parent process environment.

No dependency features were added: rmcp's existing `server` feature already
enables the re-exported schemars schema/derive support. Komodo optional features
remain disabled.

To build an optimized executable and inspect the source package contents:

```sh
cargo build --locked --release
cargo package --locked --allow-dirty --list
```
