# komodo-mcp

[Repository maintenance](docs/maintenance.md) documents dependency updates,
required checks, main protection, and release compatibility.

The [inventory benchmark](benchmarks/README.md) compares the release executable
with a pinned TypeScript stdio reference using synthetic Core fixtures. See the
[archived measurements and interpretation](benchmarks/results/README.md) for
provisional results and repeatable commands.

A Rust [Model Context Protocol](https://modelcontextprotocol.io/) server for
Komodo, using the official [rmcp 3.9.0 SDK](https://docs.rs/rmcp/3.9.0/rmcp/).
It connects to Komodo Core using the official **komodo_client 2.3.2**, pinned
exactly with optional features disabled. Twenty-five read-only tools list and inspect
Servers, Stacks, Deployments, Repos, Builders, Builds and Procedures, summarize fleet health, inspect alerts/update
history, read bounded logs, and diagnose a stack in one call.
Optional operate-enabled tools deploy one existing workload or clone/pull a saved
Repo, or run/cancel a saved Server-backed Build, and return its Core update receipt.
See [workload operations](docs/operations.md) and [Build operations](docs/builds.md#run-one-saved-build).

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
[the interactive walkthrough](docs/mock-stdio.md). For real Core, continue below.

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

### Local capabilities and Core permissions

The process reads two independent optional grants at startup:

| Variable | Default | Enables |
| --- | --- | --- |
| `KOMODO_ALLOW_OPERATE` | `false` | Single-workload operations and explicit Repo clone/pull |
| `KOMODO_ALLOW_CONFIGURATION_EDIT` | `false` | Typed configuration preview/apply tools |

Each value must be exactly `true` or `false`. An unset variable means `false`;
an empty, misspelled, whitespace-padded, or non-Unicode value fails startup with
a safe configuration error before contacting Core. Restart the MCP process after
changing a grant. For example, set `KOMODO_ALLOW_OPERATE=true` in the MCP client's
server environment to opt into operations while configuration editing stays off.
Enabling configuration editing alone never enables operations. Neither grant
changes the caller's Core credentials or Core permissions.

Discovery, tool lookup and direct dispatch use the same restricted router.
Disabled, unknown and unclassified tools are rejected before argument handling,
target reads or execution; constructing a call manually cannot bypass discovery.
All twenty-five read tools remain locally available in every grant
combination, including `get_update` and `list_updates` for recovery. Core still
controls which resources and history that identity can read. Configuration editing
adds `preview_stack_create`/`apply_stack_create` and
`preview_stack_update`/`apply_stack_update`. Core `Write` on an existing Stack,
creation permission for new Stacks, and Read/Attach on new Server/Swarm/Repo
references remain required. See the [configuration workflow](docs/configuration.md)
for inline/Git/host sources, partial fields, secret handling, stale previews,
Core auto-deploy effects, no-op and uncertain submissions.
Applying a supplied Stack source requires both local grants because Core can
auto-deploy; configuration-only access still permits creation and ordinary
settings/attachment edits.
Deployment definitions add `preview_deployment_create`/`apply_deployment_create`
and `preview_deployment_update`/`apply_deployment_update` through the same contract.
Image/Build updates also require both local grants; Core can inspect registries
and auto-deploy under its policy. See [Deployment definitions](docs/deployments.md)
for typed fields, Server/Swarm targets, Build versions, preservation and the
separate saved/runtime observations.
Server definitions add `preview_server_create`/`apply_server_create` and
`preview_server_update`/`apply_server_update`. Connection-field applies and
enabled creation require both local grants; ordinary alert/monitoring edits and
disabled creation need configuration access. Initial pinned trust is explicit,
existing keys are preserved, and connectivity is observed separately from a
verified save. See [Server records](docs/servers.md) for fields, Core permissions,
background-policy effects and the absence of provisioning/key-rotation fallback.

Builders and Builds add `list_builders`/`get_builder`, `list_builds`/`get_build`
and typed create/update preview/apply tools. Direct writes support explicit
existing-Server Builders only and preserve Server fallback order. Core retains
admin-only Builder creation and attachment permissions. Build saves can refresh
Git/host Dockerfile caches; nonempty Build edits and existing Builder Server-list
edits require both grants. These definition tools submit zero build executions.
See [Builders and Build definitions](docs/builds.md) for credential evidence,
registry/redeployment effects, supported sources and read-back limits.

`run_build` requires operate and Core Execute, with prior acknowledgement of the
effects shown by `get_build`. It submits the saved Build once, rechecking its
actual Server Builder before execution. `cancel_build` is an explicit resource-level
request with a local original-update check; Core cannot enforce that update guard.
Bounded shared receipts, history and read-only recovery keep submission, observed
image results, cancellation and deployment health separate. See [Build operations](docs/builds.md#run-one-saved-build).

Procedures add bounded list/get, typed create/update preview/apply and explicit
saved run/cancel tools. Preserve sequential stage order, parallel groups and
disabled entries; existing nonempty updates require both grants. Saved children
run under Core's built-in administrator Procedure identity, with explicit effect
acknowledgement. Cancellation is resource-level and waits for the current stage;
parent/child reports, acknowledgement, terminal state and health stay separate.
See [trusted Procedures](docs/procedures.md) for permissions, limits and recovery.

Deliberate deletion adds `preview_resource_delete` for trusted configuration
access and `apply_resource_delete` only when both local grants are enabled.
Select one exact Stack/Deployment/Server kind and stable ID with `action=delete`.
Preview identifies visible workloads/references and incomplete coverage; apply
revalidates before one typed write. Core can tear down live workloads inside that
write. Definition removal, fresh runtime observations, cleanup errors and visible
reference changes are reported separately. See [deletion](docs/deletion.md) for
Server detachment, remaining host workloads, Builder limitations and uncertain
submissions without retries.

Tags add typed creation, rename and explicit assignment preview/apply tools.
Omission preserves assignments, replace with `[]` clears them, and add preserves
unrelated IDs. `find_resources_by_tag` discovers visible managed resources by an
exact renamed tag in every profile. Tag deletion reuses the shared destructive
gate and identifies visible cross-resource effects; Core 2.3.2 can leave Swarm
assignments dangling. Core tag-owner/admin and resource Write permissions remain
separate. See [tags and assignments](docs/tags.md) for exact fields, preserved
metadata, read-back verification and concurrency/visibility limits.

Use an enabled non-admin Core identity with `Read` on the resources being
inspected. Deploy, start, stop and restart require Core `Execute` on the selected
Stack or Deployment; granting the local operate capability is insufficient. Core's
`Write` level includes execution upstream, but does not override the independent
local grants. Admin or create permissions are unnecessary for operating an
existing definition. Diagnostic reads still need their relevant Core permissions
(for example, `Logs` when required by the requested log API).

Core can acknowledge an execution update before its background task checks
execution permissions. An acknowledgement is submission evidence; a subsequent
failed update may report denial. Inspect that update's bounded sanitized stages,
and keep submission, final execution outcome and workload health separate.
There is no credential fallback. Execute tools must advertise mutation and
non-idempotency: a new explicit call can create a new update or repeat effects.
Retry observation by update ID after a lost reply; do not automatically repeat
execution. See [diagnostic history](docs/diagnostics.md#incident-history).

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
| `list_repos` | Page through visible Repos with configured branch and observed cached revision |
| `get_repo` | Inspect a Repo definition and disclose saved hooks before clone/pull |
| `list_builders` / `get_builder` | Inspect supported Server Builders and safe unsupported-type summaries |
| `list_builds` / `get_build` | Inspect Build source/image selectors and disclose execution effects |
| `list_procedures` / `get_procedure` | Inspect ordered stages, parallel/disabled steps and trusted execution authority |

Each tool advertises a JSON input schema, output schema, and read-only,
idempotent, non-destructive annotations through MCP `tools/list`. Inventory and
diagnostic tools issue only Core reads. Optional deployment tools carry mutation
and non-idempotency annotations. Discovery pagination is handled by rmcp and is
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
| Repo list item | `server_id` (string/null), `repo`, `provider`, `configured_branch`, `observed_cloned_revision` (string/null) |
| Repo detail | Repo list fields plus redacted `definition` and `revision_evidence`; see [Repo semantics](docs/repositories.md) |

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

## Workload operations

With `KOMODO_ALLOW_OPERATE=true`, deploy one existing Stack or Deployment, or
start, stop and restart its existing Server-backed containers. Stack calls
require explicit whole-stack or exact service scope; Swarm supports whole deploy
only. Each call submits once and returns an update receipt. Starting/restarting
does not apply edited configuration or pull changed images. Read the
[supported targets, dependency behavior and recovery walkthrough](docs/operations.md).

Repo definitions and explicit clone/pull use the same preview, receipt and
recovery foundations. Inspect saved hook presence with `get_repo` before running,
and acknowledge those flags in the operation input. Clone can run on-clone and
on-pull; pull can run on-pull. Definition creation defaults webhooks off, enabling
them or changing execution-bearing Repo fields requires both local grants, and
complete authoritative read-back verifies saves independently of execution. See [repositories](docs/repositories.md).

## Fleet health

Use `fleet_health` to choose a workload for investigation by stable ID. The
summary combines server state and CPU/memory/disk utilization with Stack
service and Deployment observations. Healthy, unhealthy, unavailable, and
unknown observations stay distinct; a failed source retains other results.

These are `tools/call` parameters:

```json
{"name":"fleet_health","arguments":{"page":0,"page_size":30}}
```

Follow `next_page` with the same size. Resource-specific gaps identify checks
that failed or could not start. See [bounds and observation semantics](docs/diagnostics.md)
and the [shared incident fixture](tests/fixtures/core-2.3.2/INCIDENT.md).

`get_container_logs`, `get_deployment_logs`, and `get_stack_logs` return bounded,
redacted log tails. Stack logs accept exact service filters; pass one service
for Swarm stacks. See [log limits, truncation, and redaction](docs/diagnostics.md#workload-logs).

`list_alerts` and `list_updates` read one bounded page of recent records, optionally
filtered by resource kind and stable ID. `get_update` exposes bounded, redacted
failed-stage evidence without raw configuration or unrestricted logs. See
[history paging and outcomes](docs/diagnostics.md#incident-history).

`wait_for_update` observes a saved Core update ID with a 1–30-second total budget,
at most ten lookups and a fixed one-second polling pause. It is read-only and
works from a new session after timeout, cancellation or MCP restart. Incomplete
observation retains the ID/latest evidence and never means remote operation
failure or cancellation. See [wait and resume](docs/operations.md#wait-and-resume-by-update-id).

If a submission reply was lost and no update ID is known, `reconcile_operation`
searches bounded recent target history using the original scope/action and
available time window. It returns possible candidates with **unconfirmed**
attribution, including when only one matches. It never resubmits execution; see
[interrupted-submission recovery](docs/operations.md#recover-an-interrupted-submission-without-repeating-execution).

Follow the [operator walkthrough](docs/operations-walkthrough.md) for inspection,
an explicit operation, update wait/resume and a final independent health check.
It includes deterministic compiled-stdio tests and an opt-in disposable
Core/Periphery 2.3.2 smoke runner with scoped non-admin identities and verified cleanup.

`stack_diagnose` combines inventory, service state, history, alerts and a bounded
log excerpt. It returns `healthy`, `unhealthy`, or `inconclusive`, with supporting
IDs and explicit source gaps. See [diagnosis bounds and evidence](docs/diagnostics.md#stack-diagnosis). The
[incident benchmark](benchmarks/README.md#incident-triage-10)
compares the full workflow against a pinned reference while retaining inventory.

## Client and error foundation

`CoreClient` in `src/client.rs` wraps one SDK instance and one configured reqwest
transport in an `Arc`; cloned handles retain the connection pool. Its generic
`read` method accepts official typed read requests and applies the same deadline
and safe error conversion. Log and incident-history reads enforce a body limit
before typed decoding
using the SDK's wire contract and the same authenticated connection pool;
other reads use the SDK transport. Typed executions use this same pool with local
capability enforcement, a body cap and no transport retries. Operation target reads
also use the bounded adapter. `detect_version` validates and retains the observed
SemVer response. The executable shares this client and version through
`KomodoServer`; every tool uses official typed Core requests.

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
| `capability_disabled` | A direct operation API call lacks the local operate grant; no Core request |
| `unsupported_target` | Template, unassigned workload or unsupported scope; no execution |

Tools return `error.into_tool_result()` for argument, preflight and read failures.
This produces rmcp's `isError: true` result with safe text and structured error
metadata. Submission uncertainty and later execution/observation outcomes stay in
the operation receipt. Dispatch/protocol errors remain SDK protocol errors. Do not report raw
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
It also verifies release metadata and initializes the actual release executable
to check its MCP application version. Release notes live in [CHANGELOG.md](CHANGELOG.md);
maintainers use [the release skill](.agents/skills/komodo-mcp-release/SKILL.md)
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

## License

Licensed under **GPL-3.0-or-later**. See [LICENSE](LICENSE) for the declaration and
full license text. The source package includes this license. Include `LICENSE`
alongside any redistributed executable; this project does not yet build binary
distribution archives.
