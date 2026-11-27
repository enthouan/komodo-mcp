# Tool behavior and error reference

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
| `list_actions` / `get_action` | Inspect safe Action settings and delegated host/administrator execution authority |
| `list_schedules` / `get_schedule` | Inspect saved Action/Procedure schedules and separate Core cache/parse/next-run evidence |
| `export_resources_toml` | Export explicit resources/groups and optional all-variable metadata as sanitized, incomplete inspection TOML |
| `list_alerters` / `get_alerter` | Inspect enabled status, endpoint type, filters and maintenance metadata without endpoint secrets |
| `list_variables` | Page through variable metadata without returning any values |
| `get_variable` | Inspect one exact name; return only ordinary values passing the safe projection |
| `list_core_secret_names` | List configured Core key names without values or Server/Builder expansion |
| `get_resource_sync_pending` | Inspect cached desired-state changes, deployment intent and distinct errors without mutations |
| `review_resource_sync_pending` | Audit an explicit selection and retain a bounded private review reference without Core mutations |
| `wait_for_resource_sync_update` | Resume an explicitly known RunSync update ID with sync/operation checks on every read |

Each tool advertises a JSON input schema, output schema, and effect annotations
through MCP `tools/list`. Read tools are non-destructive; scoped review is
non-idempotent because it retains a new process-local reference. Inventory and
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
| Repo detail | Repo list fields plus redacted `definition` and `revision_evidence`; see [Repo semantics](../repositories.md) |

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
[supported targets, dependency behavior and recovery walkthrough](../operations.md).

Repo definitions and explicit clone/pull use the same preview, receipt and
recovery foundations. Inspect saved hook presence with `get_repo` before running,
and acknowledge those flags in the operation input. Clone can run on-clone and
on-pull; pull can run on-pull. Definition creation defaults webhooks off, enabling
them or changing execution-bearing Repo fields requires both local grants, and
complete authoritative read-back verifies saves independently of execution. See [repositories](../repositories.md).

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
that failed or could not start. See [bounds and observation semantics](../diagnostics.md)
and the [shared incident fixture](../../tests/fixtures/core-2.3.2/INCIDENT.md).

`get_container_logs`, `get_deployment_logs`, and `get_stack_logs` return bounded,
redacted log tails. Stack logs accept exact service filters; pass one service
for Swarm stacks. See [log limits, truncation, and redaction](../diagnostics.md#workload-logs).

`list_alerts` and `list_updates` read one bounded page of recent records, optionally
filtered by resource kind and stable ID. `get_update` exposes bounded, redacted
failed-stage evidence without raw configuration or unrestricted logs. See
[history paging and outcomes](../diagnostics.md#incident-history).

`wait_for_update` observes a saved Core update ID with a 1–30-second total budget,
at most ten lookups and a fixed one-second polling pause. It is read-only and
works from a new session after timeout, cancellation or MCP restart. Incomplete
observation retains the ID/latest evidence and never means remote operation
failure or cancellation. See [wait and resume](../operations.md#wait-and-resume-by-update-id).

If a submission reply was lost and no update ID is known, `reconcile_operation`
searches bounded recent target history using the original scope/action and
available time window. It returns possible candidates with **unconfirmed**
attribution, including when only one matches. It never resubmits execution; see
[interrupted-submission recovery](../operations.md#recover-an-interrupted-submission-without-repeating-execution).

Follow the [operator walkthrough](../operations-walkthrough.md) for inspection,
an explicit operation, update wait/resume and a final independent health check.
It includes deterministic compiled-stdio tests and an opt-in disposable
Core/Periphery 2.3.2 smoke runner with scoped non-admin identities and verified cleanup.

`stack_diagnose` combines inventory, service state, history, alerts and a bounded
log excerpt. It returns `healthy`, `unhealthy`, or `inconclusive`, with supporting
IDs and explicit source gaps. See [diagnosis bounds and evidence](../diagnostics.md#stack-diagnosis). The
[incident benchmark](../../benchmarks/README.md#incident-triage-10)
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
