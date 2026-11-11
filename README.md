# komodo-mcp

A Rust [Model Context Protocol](https://modelcontextprotocol.io/) server for
Komodo, using the official [rmcp 3.8.0 SDK](https://docs.rs/rmcp/3.8.0/rmcp/).
It connects to Komodo Core using the official **komodo_client 2.3.2**, pinned
exactly with optional features disabled. Six read-only tools list and inspect
Servers, Stacks, and Deployments.

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
Servers, Stacks, and Deployments being inspected, via a resource grant, group,
global permission, base permission, or transparent mode; admin, write, execute, and create privileges are unnecessary.
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

## Inventory tools

| Tool | Purpose |
| --- | --- |
| `list_servers` | Page through visible Server summaries |
| `get_server` | Inspect one Server and its enabled setting |
| `list_stacks` | Page through visible Stack summaries and active targets |
| `get_stack` | Inspect one Stack, including its effective project name |
| `list_deployments` | Page through visible Deployment summaries and active targets |
| `get_deployment` | Inspect one Deployment by a stable identity |

Each tool advertises a JSON input schema, output schema, and read-only,
idempotent, non-destructive annotations through MCP `tools/list`. No tool issues
Core write or execute requests. Discovery pagination is handled by rmcp and is
separate from the inventory page fields below.

List input is an object with only these optional integer fields:

```json
{"page": 0, "page_size": 30}
```

`page` defaults to **0**; `page_size` defaults to **30** and supports **1–100**.
Negative, fractional, null, unknown-field, and overflowing inputs fail before
Core is called. Both the requested offset and possible next-page offset are
checked. All requests send a positive upstream limit and sort by ascending name
(Core normally guarantees unique names within each family).

A list success contains exactly `items`, `page`, `page_size`, `has_more`, and
`next_page`. For example, after `list_servers({"page_size": 2})`:

```json
{
  "items": [
    {"id":"000000000000000000000001","name":"alpha","template":false,"state":"ok","region":"west","periphery_version":"2.3.2"},
    {"id":"000000000000000000000002","name":"beta","template":false,"state":"disabled","region":null,"periphery_version":null}
  ],
  "page": 0,
  "page_size": 2,
  "has_more": true,
  "next_page": 1
}
```

Continue with `list_servers({"page": 1, "page_size": 2})`. Keep `page_size`
unchanged. A partial page is exhausted; a full page causes one lookahead with
the **same page size**, filters, and ordering. Thus Core's `page * limit` offset
stays consistent, including when the final page is exactly full. `has_more`
reflects the observed probe, and `next_page` is null when exhausted. A failed
probe fails the tool call instead of claiming exhaustion. Lists use **at most
two reads**, each returning at most `page_size` summaries.

Pages view mutable inventory: renames, creation/deletion, permission changes,
and state refreshes can cause repeats, omissions, or different observations
between calls. Continuation is not a snapshot, total count, or durable cursor.

Detail input requires exactly one of these selectors, with no other fields:

```json
{"id": "000000000000000000000001"}
```

```json
{"name": "alpha"}
```

IDs are 24 hexadecimal characters returned by list tools; hex letter casing
is insignificant. Names are exact and case-sensitive, are not trimmed or
normalized, and are scoped to the selected family. Empty/whitespace-only,
missing, null, or conflicting selectors fail before upstream calls.

Core's `ResourceQuery.names` normally filters exact names, but interprets valid
ObjectId-shaped strings as IDs. An explicit `name` selector of that shape uses
Core's escaped name-term search and verifies exact equality, so it cannot
silently resolve an unrelated resource with that ID. Resolution requires one
exact visible match. Ambiguity returns at most five visible IDs and names with
guidance to retry by ID, even though normal Core creation prevents duplicate
names. A bounded search that cannot establish uniqueness returns
`incomplete_resolution`, with guidance to list and retry by ID.

Ordinary ID/name details use **one targeted list read** (limit 100) and **one
Get read by the resolved stable ID**, reusing the summary's computed state.
An ObjectId-shaped name searches **at most four pages of 100 summaries**, then
one Get read: **at most five reads / 400 searched summaries**. At the search
bound, even one observed match is incomplete unless exhaustion is established.
Ambiguous, missing, and incomplete resolutions do not perform a Get read.
Detail identity, name, template, and active workload target are checked against
the summary; inconsistent observations fail safely and advise retrying. Each
read has its own deadline, so a five-read path can take up to 75 seconds.

### Response fields and semantics

All fields below are always emitted; unavailable optional values are explicit
JSON nulls. The project owns these response types in `src/inventory/types.rs`.
Every resource has exactly the common fields **`id`, `name`, `template`, `state`**,
plus the following fields:

| Response | Additional complete field set |
| --- | --- |
| Server list item | `region` (string/null), `periphery_version` (string/null) |
| Server detail | Server list fields plus `enabled` (boolean) |
| Stack list item | `target` (object/null), `service_count` (nonnegative integer) |
| Stack detail | Stack list fields plus `project_name` (string/null) |
| Deployment list item and detail | `target` (object/null), `image` (string/null), `runtime_name` (string/null), `update_available` (boolean) |

`target` has exactly `kind` (`server` or `swarm`), `id`, and `name` (string/null).
It is null for an unassigned workload. Swarm takes precedence when both target
IDs are configured; a Swarm target does not identify an individual Server. A
missing display name keeps the stable target ID and emits `name: null`. Target
display names come from Core's computed summaries; no extra association reads
or permissions on the target are required by these tools.

Server state comes from the computed summary: `ok` (health check passing),
`not_ok` (unreachable), or `disabled`. `enabled` is the stored setting from
GetServer, not an inferred runtime state. Region and Periphery version come
from the summary; empty/unavailable strings become null.

Stack states preserve Core's `deploying`, `running`, `paused`, `stopped`,
`created`, `restarting`, `dead`, `removing`, `unhealthy`, `down`, and `unknown`.
Deployment states preserve `deploying`, `running`, `created`, `restarting`,
`stopping`, `removing`, `paused`, `exited`, `dead`, `unhealthy`, `not_deployed`,
and `unknown`. Unknown status never implies stopped or running; workload state
may be unknown when its target is disabled or unreachable. Template status is
reported separately from observed runtime state.

`service_count` counts the summary's deployed services, or latest services when
not deployed; it is not a count of healthy/running containers. `project_name`
uses Core's effective project-name helper: deployed name, configured name, then
resource name. Deployment image, update availability, and container/service
`runtime_name` come from Core's computed summary (deployed name, configured
custom name, then resource name). These reflect Core's current observation,
which can lag Docker or stored configuration. Free-form Docker status and
upstream errors are excluded.

After MCP initialization, a minimal usage sequence is:

```json
{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_stacks","arguments":{"page_size":30}}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_stack","arguments":{"id":"000000000000000000000002"}}}
```

Use an ID actually returned by your list result for the last call. Minimum
permissions are an enabled Core identity and `Read` access to the requested
Server, Stack, or Deployment. Lists contain only visible resources and may be
empty. Details preserve explicit authentication/permission errors, while absent
or permission-filtered targets return safe `not_found` guidance.

## Client and error foundation

`CoreClient` in `src/client.rs` wraps one SDK instance and one configured reqwest
transport in an `Arc`; cloned handles retain the connection pool. Its generic
`read` method accepts official typed read requests and applies the same deadline
and safe error conversion. `detect_version` validates and retains the observed
SemVer response. The executable shares this client and version through
`KomodoServer`; every tool uses official typed requests through `read`.

`AppError` contains only application-owned messages, a stable `kind`, the
operation, optional HTTP status, correction metadata, and up to five visible
`{id, name}` candidates for ambiguity:

| `kind` | Meaning |
| --- | --- |
| `invalid_arguments` | Invalid JSON types, selector, page size, or pagination arithmetic; no upstream request |
| `not_found` | No exact visible target, or supported Core missing-resource error; list and retry with a current ID |
| `ambiguous_target` | Multiple exact visible matches; retry with an explicit candidate ID |
| `incomplete_resolution` | A finite name search reached its bound; list the family and retry by ID |
| `configuration` | Missing or invalid environment/configuration field |
| `authentication` | Core rejected credentials or the user is disabled (401) |
| `permission_denied` | Forbidden (403), or the known Core 2.3.2 read-permission helper error (500) |
| `timeout` | Connection timeout or complete-operation deadline exceeded |
| `unreachable` | Connection, DNS, or TLS connection failure |
| `upstream` | Other unexpected HTTP or upstream failure |
| `invalid_response` | Malformed/incompatible data, invalid resource identity, or a resource changed between reads; retry inspection |

Tools return `error.into_tool_result()` for argument and execution failures.
This produces rmcp's `isError: true` result with safe text and structured error
metadata. Dispatch/protocol errors remain SDK protocol errors. Do not report raw
SDK errors or response bodies. Narrow signatures recognize Core 2.3.2's resource
permission helper and missing-resource helper for supported ID-based detail
reads before discarding upstream text. Unrelated HTTP 500 responses and proxy
404 responses stay `upstream`; authentication and permission failures retain
their categories. A list lookup cannot distinguish a missing resource from an
invisible one. This does not reveal inaccessible candidates.

Each output schema accepts either the documented success object or the safe
`AppError` object, so strict clients can validate both structured result shapes.
Success text is JSON generated from the same projection as `structuredContent`.
Failures retain rmcp's `isError: true` convention and the shared error conversion.
Argument parsing discards SDK/serde diagnostics that could echo tool arguments. All selected output strings, including candidate names,
are redacted for the configured API credentials. Raw configuration, environment
blocks, Compose content, stored files, descriptions, and upstream traces never
enter these projections.

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
MCP **2025-11-25**: configured startup, initialization, six-tool discovery, list
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

## License

Licensed under **GPL-3.0-or-later**. See [LICENSE](LICENSE) for the declaration and
full license text. The source package includes this license. Include `LICENSE`
alongside any redistributed executable; this project does not yet build binary
distribution archives.
