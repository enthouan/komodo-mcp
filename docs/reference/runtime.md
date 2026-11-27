# Server configuration and permissions

## Authenticated Streamable HTTP

For Docker installation, private CA configuration and container health checks,
see [Run Komodo MCP with Docker Compose](../container.md).

A source build can run as a persistent MCP server at `http://127.0.0.1:3000/mcp`.
Set `KOMODO_ADDRESS`, `KOMODO_API_KEY` and `KOMODO_API_SECRET` in the server environment. Generate an inbound
token in a private shell and keep it in the server and client secret stores.

```sh
export KOMODO_TRANSPORT=http
export KOMODO_HTTP_TOKENS="$(openssl rand -hex 32)"
./target/release/komodo-mcp
```

In a Streamable HTTP client, set the URL to `http://127.0.0.1:3000/mcp` and
send `Authorization: Bearer <your-inbound-token>` on every MCP request,
including stream resumption and session deletion. Initialize, run `tools/list`,
then call `list_stacks` with `{}`. A session ID identifies a session and every
request still requires its own bearer credential. Stdio remains the default.

`KOMODO_HTTP_TOKENS` accepts up to 32 distinct comma-separated tokens.
Give each client its own randomly generated token. All clients of one server
use that server's configured **Core service account** and local grants. Run
separate instances with separate Core credentials for callers who need different
resource permissions. Restart the instance after rotating a token.

For clients that negotiate a session ID, previews and terminal cleanup belong
to that session. For stateless MCP requests, they belong to the authenticated
token. Sharing a token also shares its stateless previews. Closing a session
invalidates its previews and signals its active terminal workers. The process
divides 32 initialized sessions, 32 GET streams, and 32 other in-flight requests
among the configured tokens. A client can close a session while its GET stream
is open. The process also limits previews to 16 and active terminal calls to four.
It divides each budget across tokens. All sessions for one token use that
token’s share. When more tokens are configured than there are slots, each
token can use one slot while capacity is available.

The default bind address is loopback. Set `KOMODO_HTTP_BIND` to an IP and port
when a different listener is required. Terminate HTTPS at a trusted reverse
proxy before sending bearer credentials across a network. Have the proxy forward
the request to the listener with its loopback Host (or its explicit bind IP)
and preserve the client's Origin and Authorization headers. Native clients can
omit `Origin`. To permit browser clients, set `KOMODO_HTTP_ALLOWED_ORIGINS`
to their exact comma-separated HTTP or HTTPS origins, including any non-default
port. With the setting unset, requests carrying an Origin are rejected.

After a disconnected mutation, use `wait_for_update` with a known Core update
ID or use `reconcile_operation` and check workload state before submitting
another operation. An MCP session ID cannot establish a Core operation's outcome.

## Select individual tools

Both transports read the same tool selection when the server starts. For a
stack triage instance, set the following variable and restart the server.

```sh
export KOMODO_ENABLED_TOOLS=list_stacks,get_stack,stack_diagnose
```

Initialize your MCP client and run `tools/list`. In this configuration the
response's `tools[].name` contains exactly `list_stacks`, `get_stack`, and
`stack_diagnose`. Call `list_stacks` with `{}` to choose a visible stack.
Your Core identity still needs Read permission on the resources it inspects.

To keep your usual tools and remove only image history, leave
`KOMODO_ENABLED_TOOLS` unset and set the disabled list.

```sh
export KOMODO_DISABLED_TOOLS=get_image_history
```

Use comma-separated, exact built-in names. Spaces around a name are allowed.
Unknown names and blank entries, including an explicitly empty variable, fail
startup. Leaving both variables unset preserves the default selection. The
enabled list narrows the tools allowed by local grants, then the disabled list
removes matching entries. The result can be empty. These settings apply to
every caller of the instance. Discovery and direct calls use the same selection.

## Local capabilities and Core permissions

The process reads independent optional grants at startup:

| Variable | Default | Enables |
| --- | --- | --- |
| `KOMODO_ALLOW_OPERATE` | `false` | Single-workload operations and explicit Repo clone/pull |
| `KOMODO_ALLOW_CONFIGURATION_EDIT` | `false` | Typed configuration preview/apply tools |
| `KOMODO_ALLOW_PRUNE` | `false` | Docker category preview/apply, also requiring operate and Core Execute |
| `KOMODO_ALLOW_TERMINAL` | `false` | One command in a fresh private terminal, also requiring operate and Core Read + Terminal |

Each value must be exactly `true` or `false`. An unset variable means `false`;
an empty, misspelled, whitespace-padded, or non-Unicode value fails startup with
a safe configuration error before contacting Core. Restart the MCP process after
changing a grant. For example, set `KOMODO_ALLOW_OPERATE=true` in the MCP client's
server environment to opt into operations while configuration editing stays off.
Enabling configuration editing alone never enables operations. These grants never
change the caller's Core credentials or Core permissions.
The pruning opt-in controls explicit MCP tools. Core's independent daily Server
`auto_prune` policy remains configured upstream; the pruning preview shows its
saved state without changing it.
The [terminal tool](../terminal.md) adds one bounded command in a fresh owned
session on an exact Server or supported container target, with separate cleanup
and completion uncertainty reporting.

Discovery, tool lookup and direct dispatch use the same restricted router.
Disabled, unknown and unclassified tools are rejected before argument handling,
target reads or execution; constructing a call manually cannot bypass discovery.
With both selection settings unset, every grant combination includes the
sixty-six read tools, including `get_update` and `list_updates` for recovery. Core still
controls which resources and history that identity can read. Configuration editing
adds `preview_stack_create`/`apply_stack_create` and
`preview_stack_update`/`apply_stack_update`. Core `Write` on an existing Stack,
creation permission for new Stacks, and Read/Attach on new Server/Swarm/Repo
references remain required. See the [configuration workflow](../configuration.md)
for inline/Git/host sources, partial fields, secret handling, stale previews,
Core auto-deploy effects, no-op and uncertain submissions.
Applying a supplied Stack source requires both local grants because Core can
auto-deploy; configuration-only access still permits creation and ordinary
settings/attachment edits.

The [variable workflow](../variables.md) adds bounded ordinary/secret metadata
and configured Core secret key names. Its typed preview/apply supports ordinary
creation, value updates and deletion with both grants, plus description edits
with configuration-edit alone. Every write also requires an enabled Core
administrator. Secret values and authentication payloads remain omitted;
variable history is sanitized even after restart.

The [access workflow](../access.md) adds safe user/group inspection, direct
permission records and Core's effective permission result for the current
credential. Typed private previews cover service users, base/admin permissions,
groups, additive membership and explicit scoped/type-wide grants. Every apply
requires both local grants and enabled Core admin authority; admin-status edits
require super-admin. Direct records do not establish another user's effective
access, and deletion reports visible leftovers without claiming credential
erasure or transactional cleanup.

The [Alerter workflow](../alerters.md) adds safe configuration inspection,
private create/update previews and explicit single-target test delivery. New
Alerters default disabled. Existing-definition applies require both grants;
`test_alerter` requires operate and Core Execute. Saving never sends a test.
Provider endpoints and recipient credentials stay private, and Core success
remains separate from independently observed receipt.

Deployment definitions add `preview_deployment_create`/`apply_deployment_create`
and `preview_deployment_update`/`apply_deployment_update` through the same contract.
Image/Build updates also require both local grants; Core can inspect registries
and auto-deploy under its policy. See [Deployment definitions](../deployments.md)
for typed fields, Server/Swarm targets, Build versions, preservation and the
separate saved/runtime observations.
Server definitions add `preview_server_create`/`apply_server_create` and
`preview_server_update`/`apply_server_update`. Connection-field applies and
enabled creation require both local grants; ordinary alert/monitoring edits and
disabled creation need configuration access. Initial pinned trust is explicit,
existing keys are preserved, and connectivity is observed separately from a
verified save. See [Server records](../servers.md) for fields, Core permissions,
background-policy effects and the absence of provisioning/key-rotation fallback.

ResourceSync definitions add bounded `list_resource_syncs`/`get_resource_sync`
and typed create/update preview/apply tools for inline TOML, direct Git, linked
Repos and Core-host files. Core preserves administrator-only creation and
Write/Attach permissions. New webhooks default off; execution-bearing updates
require both local grants. Saving can refresh pending/source caches and alerts,
but submits no RunSync or external source write-back. Inspect managed deletion,
empty tag filters and desired-state omission/reset behavior in
[ResourceSync definitions](../resource-syncs.md).

`get_resource_sync_pending` reads cached changes without mutations.
`refresh_resource_sync_pending` requires operate and Core Execute, and makes one
explicit source/cache/alert refresh. `review_resource_sync_pending` audits an
explicit whole-sync, resource-type or named selection and returns a private
single-use reference only when coverage is complete. See
[pending plans](../resource-syncs.md#pending-plans-and-explicit-refresh) for
freshness, metadata and deployment limits. `apply_resource_sync` requires both
local grants and the exact single-use review, performs one counted preflight
refresh, rejects detected drift, and submits at most one RunSync.
`wait_for_resource_sync_update` resumes a retained update ID through read-only
identity checks. Parent success, independent resource observations and workload
health remain separate; see [reviewed application](../resource-syncs.md#apply-a-reviewed-selection).

Follow the [Swarm and administration walkthrough](../swarm-admin-walkthrough.md)
for the complete operator workflow and opt-in manager/worker acceptance, including
all retained Core, automation and ResourceSync regressions.

Follow the [ResourceSync operator walkthrough](../resource-sync-walkthrough.md)
for export through independent inspection, source/reset/deletion semantics,
recovery and the opt-in full disposable Core/automation integration smoke.

Builders and Builds add `list_builders`/`get_builder`, `list_builds`/`get_build`
and typed create/update preview/apply tools. Direct writes support explicit
existing-Server Builders only and preserve Server fallback order. Core retains
admin-only Builder creation and attachment permissions. Build saves can refresh
Git/host Dockerfile caches; nonempty Build edits and existing Builder Server-list
edits require both grants. These definition tools submit zero build executions.
See [Builders and Build definitions](../builds.md) for credential evidence,
registry/redeployment effects, supported sources and read-back limits.

`run_build` requires operate and Core Execute, with prior acknowledgement of the
effects shown by `get_build`. It submits the saved Build once, rechecking its
actual Server Builder before execution. `cancel_build` is an explicit resource-level
request with a local original-update check; Core cannot enforce that update guard.
Bounded shared receipts, history and read-only recovery keep submission, observed
image results, cancellation and deployment health separate. See [Build operations](../builds.md#run-one-saved-build).

Procedures add bounded list/get, typed create/update preview/apply and explicit
saved run/cancel tools. Preserve sequential stage order, parallel groups and
disabled entries; existing nonempty updates require both grants. Saved children
run under Core's built-in administrator Procedure identity, with explicit effect
acknowledgement. Cancellation is resource-level and waits for the current stage;
parent/child reports, acknowledgement, terminal state and health stay separate.
See [trusted Procedures](../procedures.md) for permissions, limits and recovery.

Actions add private typed source/default-argument previews, inert administrator-only
creation and explicit saved run/cancel. Scripts execute on the Core host with Deno
`--allow-all` and built-in administrator API access. Per-run arguments use Core
recursive/array-appending merges without saved-definition writes. Cancellation
validates the exact Action/run update ID; arbitrary secret-bearing output is
omitted while structured Core outcomes remain available. See [trusted Actions](../actions.md).

[Schedule tools](../schedules.md) list nonempty scheduled owners (including
disabled/templates) and privately preview/apply five schedule fields while
preserving the complete parent definition. Actual updates require both grants.
Core owns Cron/English parsing, timezone defaults and next-run calculation; saved
configuration and cached execution evidence remain separate. Disabling does not
cancel already dispatched/running work. MCP runs no scheduler.

Deliberate deletion adds `preview_resource_delete` for trusted configuration
access and `apply_resource_delete` only when both local grants are enabled.
Select one exact Stack/Deployment/Server kind and stable ID with `action=delete`.
Preview identifies visible workloads/references and incomplete coverage; apply
revalidates before one typed write. Core can tear down live workloads inside that
write. Definition removal, fresh runtime observations, cleanup errors and visible
reference changes are reported separately. See [deletion](../deletion.md) for
Server detachment, remaining host workloads, Builder limitations and uncertain
submissions without retries.

Tags add typed creation, rename and explicit assignment preview/apply tools.
Omission preserves assignments, replace with `[]` clears them, and add preserves
unrelated IDs. `find_resources_by_tag` discovers visible managed resources by an
exact renamed tag in every profile. Tag deletion reuses the shared destructive
gate and identifies visible cross-resource effects; Core 2.3.2 can leave Swarm
assignments dangling. Core tag-owner/admin and resource Write permissions remain
separate. See [tags and assignments](../tags.md) for exact fields, preserved
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
execution. See [diagnostic history](../diagnostics.md#incident-history).

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
