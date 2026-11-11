# Interactive stdio walkthrough

Run these commands from the repository root after the README's production and
example builds. Every credential below is synthetic. The executable reads its
**process environment**; it does not load an `.env` file.

## Standalone mock

In terminal 1:

```sh
cargo run --locked --example mock_core -- 127.0.0.1:8089
```

Wait for the readiness line, emitted only after binding:

```text
MOCK_CORE_READY http://127.0.0.1:8089/komodo
```

An explicit loopback IP and port are accepted, including `127.0.0.1:0` for an
ephemeral port. Use the **actual address in the readiness line** in terminal 2;
no sleep or guessed startup interval is needed. The mock requires loopback and
supports repeated requests and independent client connections until Ctrl-C or
SIGTERM. It validates both `x-api-key` and `x-api-secret`, returning a Core-shaped
HTTP 401 for rejection. Its diagnostics never print credential values.

## MCP Inspector 2.14.0

Requires **Node.js ^22.23.4 || ^24.22.1 || >=26.12.1** and npm/npx
(tested with Node 24.22.1). The pinned
[official configuration guide](https://github.com/modelcontextprotocol/inspector/blob/2.14.0/docs/mcp-server-configuration.md)
documents `mcpServers`, `type: "stdio"`, `command`, `args`, `env`, `--config`,
`--server`, and CLI tool calls; its
[package manifest](https://github.com/modelcontextprotocol/inspector/blob/2.14.0/package.json)
specifies the Node prerequisite. Use the explicit version below for reproducibility.
The template explicitly selects `protocolEra: "legacy"` for the server's
initialize-based MCP `2025-11-25` lifecycle and sets 25-second connection/request
timeouts. The [CLI guide](https://github.com/modelcontextprotocol/inspector/blob/2.14.0/clients/cli/README.md)
documents the tool commands.

In terminal 2, generate a configuration from the checked-in
[template](../examples/mcp-inspector.json), using the actual build output:

```sh
cargo run --locked --example inspector_config -- ./target/release/komodo-mcp \
  http://127.0.0.1:8089/komodo > target/mcp-inspector.json
npx --yes @modelcontextprotocol/inspector@2.14.0 --cli \
  --config target/mcp-inspector.json --server komodo-mock --method tools/list
```

The helper resolves the executable's **absolute path** and serializes JSON;
paths containing spaces remain a single executable argument. It sets all three
Core variables explicitly. The template's placeholder command is not runnable
until substituted; the helper performs that substitution portably. Replace the
address above if terminal 1 reports a different port. The CLI command connects,
initializes, discovers all six inventory tools, then closes the session. Inspector
2.14.0 reports schema portability advisories for the existing tool schemas
(0 errors, 40 warnings); discovery and the calls below were verified successfully.

Resolve the exact name, and copy the returned stable server ID:

```sh
npx --yes @modelcontextprotocol/inspector@2.14.0 --cli \
  --config target/mcp-inspector.json --server komodo-mock \
  --method tools/call --tool-name get_server --tool-arg name=fixture-server
```

List stacks with a small page size:

```sh
npx --yes @modelcontextprotocol/inspector@2.14.0 --cli \
  --config target/mcp-inspector.json --server komodo-mock \
  --method tools/call --tool-name list_stacks --tool-arg page=0 --tool-arg page_size=2
```

Repeat with `page=1`, then `page=2`, following `next_page` with `page_size=2`
until `has_more` is false and `next_page` is null. Among all pages, select only
objects whose `target.kind` is `server` and `target.id` equals the ID returned
by `get_server`. The answer is `alpha-web`, `gamma-jobs`, and `zeta-metrics`.
`beta-other` targets another server; `omega-swarm` has a Swarm target even
though its source fixture also contains a Server ID; `unassigned` has no target.

For the browser UI instead of CLI:

```sh
npx --yes @modelcontextprotocol/inspector@2.14.0 \
  --config target/mcp-inspector.json
```

Open the local URL Inspector prints, select `komodo-mock` and **Connect**, then
use the Tools panel to discover and call the tools above. `--server` selects
only in CLI mode; the browser lists the entries in the configuration file. Closing/disconnecting the client closes server
stdin; stop Inspector and the standalone mock with Ctrl-C in their terminals.
Each CLI invocation is independent, so the mock accepts a new version probe
and tool sequence for every connection.

## Minimum real-Core permissions

Use an **enabled identity** with a valid API key and secret. Answering the
question requires **Read access to the selected Server** and **Read access to
the Stacks the caller should see**. List/get Deployment tools similarly require
Read access to those Deployments; Server and Stack tools require their own
family's Read access. Resource grants, user-group grants, or other configured
Read grants (including applicable global/base permissions or transparent mode)
can supply this visibility. Admin, write, execute, and create grants are
unnecessary. Inventory describes the resources visible to that identity;
a successful empty result does not establish that no other resources exist.
The startup `GetVersion` probe requires valid credentials and an enabled user.

## Troubleshooting

The production server emits MCP only on stdout and safe diagnostics on stderr.
Clients may report only early EOF or an initialization failure because the
version probe happens **before** MCP initialization. In the automated demo,
look below `Captured komodo-mcp stderr:`. In Inspector, inspect its terminal
server-stderr output (or the UI's stderr/log pane). With another client, enable
its child-stderr capture. To isolate a startup problem in a shell using the
same explicit environment, run the compiled server with stdin closed and
redirect stderr to a local file, then inspect that file; successful startup with
no MCP initialize will itself end with an initialization diagnostic.

| Symptom / actual safe diagnostic | Corrective action |
| --- | --- |
| `configuration (configure)` and `KOMODO_API_SECRET is required` (similarly for either other required field) | Set the named variable in the launched process environment. An `.env` file alone has no effect. Restart the client/server after changing it. |
| `authentication (GetVersion)` / `Core rejected authentication; check the API key, secret, and enabled user.` / `HTTP 401` | For the mock, use `fixture-API-KEY-7d42` and `fixture-API-SECRET-a913` together. For real Core, check the key, secret, and enabled identity. |
| `unreachable (GetVersion)` / `Unable to reach Core; check KOMODO_ADDRESS, connectivity, and TLS certificates.` | Start the mock, wait for `MOCK_CORE_READY`, and copy its actual address (including `/komodo`) into the client configuration. Check the port and real-Core TLS/network reachability. |
| `launch compiled komodo-mcp; build it first and pass its executable path`, Inspector `ENOENT`, or helper `compiled executable not found` | Build the binary, supply its real path, and regenerate the configuration. Keep paths with spaces quoted in shell commands. |
| `get_server failed (not_found)` or tool `isError: true` with `kind: not_found` | Check exact spelling/case, list the family for a current ID, and check Read visibility. Missing and inaccessible resources can both be absent from lists. |
| Tool `permission_denied` / `Core denied access; grant the user the required resource read permission.` | Grant the relevant Read access. The tagged Core's detail permission denial is HTTP 500, recognized from its specific helper response. |
| Session closes during startup; stderr reports `timeout (GetVersion)`, `upstream (GetVersion)`, or `invalid_response (GetVersion)` | Inspect captured stderr, confirm the address is the Core API endpoint and the proxy returns the expected Core version response, and check Core health/version. Then reconnect. The server's request deadline is 15 seconds; the example allows 25 seconds per MCP exchange. |

The demo has a 90-second overall bound, 32-page inventory bound, and 5-second
shutdown/drain bounds. It reports an incomplete answer on a bound or inconsistent
continuation. It closes input and waits for the child, killing and reaping it
if shutdown stalls. Stderr is drained concurrently and retained up to 1 MiB.

## Evidence and mock limits

Both entry points share [the mock implementation](../examples/support/mock_core.rs).
It embeds the existing Core 2.3.2 wire fixtures and the
[demo manifest](../tests/fixtures/core-2.3.2/demo.json), independent of the caller's
working directory. Summaries, details, associations, and version responses use
consistent synthetic IDs. [Read grants](../tests/fixtures/core-2.3.2/demo-permissions.json)
belong to the existing enabled non-admin fixture user. See
[fixture provenance](../tests/fixtures/core-2.3.2/README.md) for tagged sources.

The mock serves only authenticated `POST /komodo/read`: `GetVersion`,
`ListServers`/`GetServer`, `ListStacks`/`GetStack`, and
`ListDeployments`/`GetDeployment`. Official request types validate the envelopes.
List handling covers the filters emitted by production (exact names/IDs,
case-insensitive literal terms), template/tag filtering for this untagged
inventory, Name ordering in either direction, and Core's `page * limit` behavior
(default limit 30, zero unlimited). Other sorts/family-specific filters, writes,
executes, and unknown reads are rejected and recorded for the harness. This is
a fixture server, not an implementation of Core's permission engine.

CI explicitly builds the production executable and examples, then **executes**
the README command. Integration tests assert the actual computed answer,
initialization/discovery/tool transcript, supported read operations and page
requests, later-page matches, exclusions, safe diagnostics, authentication,
empty results, and cleanup after failures. Cargo's test compilation of examples
alone does not execute their demonstration main functions.
