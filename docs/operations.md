# Workload operations

Set `KOMODO_ALLOW_OPERATE=true` in the MCP server's process environment and
restart it. The default remains read-only. `KOMODO_ALLOW_CONFIGURATION_EDIT`
is independent and exposes no production configuration-edit tools yet. Use an
enabled non-admin Core identity with Read visibility and Execute permission on
the selected Stack or Deployment. Core remains authoritative; local grants never
change credentials or supply missing upstream permissions.

## Supported targets

| Resource / target | Deploy | Start / stop / restart |
| --- | --- | --- |
| Stack on Server (Compose) | Whole stack or exact saved service | Whole stack or exact deployed service |
| Stack on Swarm | Whole stack only; service filter rejected | Rejected before execution |
| Deployment on Server (container) | Whole container | Whole existing container |
| Deployment on Swarm (service) | Whole service | Rejected before execution |
| Template or unassigned resource | Rejected | Rejected |

This matrix follows the pinned Core **2.3.2** implementation. An operation uses
the existing saved definition. It accepts no configuration, image, termination,
batch-pattern or command overrides. A resource's runtime state alone does not
authorize a substitute action or establish that execution will succeed.

## Select the exact workload and scope

Discover and inspect with `list_stacks` / `get_stack` or `list_deployments` /
`get_deployment` first. Prefer the returned stable ID. ID/name resolution follows
the [inventory contract](../README.md#inventory-tools): exactly one selector,
case-sensitive literal names, bounded ambiguity handling and stable-ID execution.
The name is not a wildcard/pattern. Missing, invisible, ambiguous or inconsistent
targets do not execute.

These examples are `tools/call` parameters. Use IDs actually visible to the
configured identity. Stack calls require an explicit scope:

```json
{"name":"deploy_stack","arguments":{"target":{"id":"000000000000000000000002"},"scope":{"kind":"whole_stack"}}}
```

```json
{"name":"deploy_stack","arguments":{"target":{"id":"000000000000000000000002"},"scope":{"kind":"service","service":"web"}}}
```

```json
{"name":"deploy_deployment","arguments":{"id":"000000000000000000000003"}}
```

A Stack service is one exact 1–128-byte identifier using ASCII letters, digits,
underscores, dots and hyphens, without a leading hyphen. Blank values, whitespace,
wildcards, arrays, shell syntax and unknown fields are rejected. The identifier
must occur exactly once in Core's raw **latest saved-definition service inventory**
for deploy, or **deployed service inventory** for start/stop/restart;
an absent, ambiguous or unavailable inventory requires inspection/cache refresh
in Core. Never copy a clipped or redacted display name as an execution selector.
The server submits the exact validated raw service and resolved stable resource
ID. Failed service resolution never becomes a whole-stack request. Core's cached
service inventory, saved files and remote sources can change after inspection;
this is not an atomic configuration snapshot.

## Saved definitions, dependencies and hooks

Deploy applies the saved definition and configured source/image policy. Core's
Server Deployment path pulls the selected image/build version and recreates the
container. Stack deployment uses saved Compose sources, `auto_pull`, build policy,
extra arguments and command wrappers; `auto_pull=false` does not promise a fresh
image. Image tags/build selections may still resolve to an unchanged image.
Restarting existing containers does not apply edited configuration or replace
their image with a changed tag; use deploy for that change.

For Compose, the selected service is passed to the saved deployment workflow.
Compose can start dependencies; saved flags can change that behavior. Core runs
saved pre-deploy hooks before the Compose work and post-deploy hooks after a
successful deploy, including service-filtered deploys. Those hooks/wrappers can
affect other services or resources. Saved destroy-before-deploy settings and
project-name changes also retain their configured behavior. The MCP server adds
no hidden `--no-deps`, pull, hook-skip or termination overrides.

The receipt reports **requested scope** separately from observed configured
effects: pre/post hook presence, Stack auto-pull policy, extra arguments/wrapper
presence and possible broader effects. It never exposes hook commands or claims
that service targeting isolates dependencies/hooks, or that configured hooks
actually ran. Use update stages and subsequent health observations for evidence.

## Start, stop and restart existing containers

Use `start_stack`, `stop_stack`, `restart_stack` with the same explicit Stack
input, or `start_deployment`, `stop_deployment`, `restart_deployment` with the
same Deployment selector. For example:

```json
{"name":"stop_stack","arguments":{"target":{"id":"000000000000000000000002"},"scope":{"kind":"service","service":"web"}}}
```

```json
{"name":"start_stack","arguments":{"target":{"id":"000000000000000000000002"},"scope":{"kind":"service","service":"web"}}}
```

```json
{"name":"restart_deployment","arguments":{"id":"000000000000000000000003"}}
```

Core 2.3.2 uses the deployed Compose project and existing containers for these
actions. It does not write the saved Compose file, pull images, run deployment
hooks or apply deployment extra arguments/wrappers. Editing saved sources does
not create a new service that lifecycle can start; deploy it first. A stale cache
can still race with remote changes and Core may reject the selected service.

Dependency behavior depends on the action and the Periphery's installed Compose
version. Core submits the requested service list without a `--no-deps` override:

| Action | Existing Compose project behavior |
| --- | --- |
| Start | Starts existing selected containers and may start their existing dependencies, in dependency order. Does not create missing containers. |
| Stop | Stops the selected existing services without removing containers; whole-stack stop uses reverse dependency order. Does not expand a named selection to its dependencies. |
| Restart | Restarts existing containers; dependency relations configured with `depends_on.restart` can restart additional dependents. Does not apply edited configuration or a newly pulled image. |

The receipt's `compose_dependencies` identifies the action-specific behavior;
its hook/pull/extra-argument fields describe settings used by the requested action,
so deployment-only settings are false/null for lifecycle. This is intended behavior,
not evidence that dependencies changed; inspect update stages and runtime health.

Runtime state is not a reason to change the requested action. The MCP server sends
the exact start, stop or restart even if inventory reports running, stopped,
paused or unavailable. Core/Docker can fail or do nothing in those states. No
unpause, deploy, retry queue or other substitute follows a failure. Deployment
stop retains Core's saved termination signal/timeout because no override is sent.
A new explicit call creates a new operation, including after busy/conflict.

## Submission receipt and immediate observation

Each explicit call makes **at most one** typed Core execution through the shared
authenticated transport. Redirects and transport retries are disabled. Target
reads, acknowledgement bodies and update reads have a 2 MiB cap before typed
decoding, including chunked responses. Each Core operation has the existing
15-second complete-request deadline (5-second connection timeout).

The receipt includes the resolved resource kind/ID and bounded sanitized display
name, requested scope, operation, Core operation variant, local submission time
window, update ID when known, latest bounded update and separate submission /
observation errors. Structured JSON is capped at 65536 bytes after sanitization
and escaping; stage evidence is marked truncated if reduced. Text duplicates the
same sanitized projection. Raw configuration, command, unrestricted stdout and
credentials are omitted.

| `submission` | Meaning and next action |
| --- | --- |
| `accepted` | A valid acknowledgement identifies this target/operation/update. Inspect the ID and latest observation; acceptance is not successful rollout. |
| `rejected` | A submission-level client-error response rejected the request. Inspect the safe error and correct the stated access/input/conflict problem. |
| `unknown` | Transport loss, timeout, unexpected server/proxy error, or invalid/oversized acknowledgement leaves acceptance unestablished. Inspect history before deciding whether to issue another operation. |

Core 2.3.2 creates an update before background execution finishes, and permission
or busy-target denial can appear as a subsequently failed update. A named Stack
service uses Core's `DeployStackService` update variant; whole-stack deploy uses
`DeployStack`, and a Deployment uses `Deploy`. Lifecycle uses `StartStack`,
`StopStack`, `RestartStack` (with a `Service` suffix for named selections) or
`StartDeployment`, `StopDeployment`, `RestartDeployment`.

After acknowledgement, the tool makes **one** update lookup with a 2-second
observation budget. It does not poll or wait for rollout. On failed/slow lookup,
the receipt keeps the acknowledged ID and latest acknowledgement plus an explicit
observation error. A failed Core update retains its actual bounded sanitized stage
evidence, whether denial, busy/conflict, or rollout failure. Execution success is
separate from workload health. Two independent callers each receive their own
update/outcome; the server neither queues nor resubmits a busy request.

Typical ID/ordinary-name calls use two target reads, one execution and one update
lookup. An ObjectId-shaped name can use four bounded list pages plus one detail
read, execution and lookup: at most seven Core requests and 92 seconds of request
budgets, excluding local work/scheduling. Startup's GetVersion probe is separate.

## Wait and resume by update ID

Save `update_id` before starting a wait. `wait_for_update` is available in a
read-only session; Core still requires access to the update's target/history.
It has no dependency on the submitting process, local memory or an operate grant:

```json
{"name":"wait_for_update","arguments":{"id":"100000000000000000000001","timeout_seconds":10}}
```

The total local observation budget is 1–30 seconds (default 10), including Core
reads and sleeps. There are at most **10** lookups, with a pause of at least one
second after each unfinished response. Requests retain the shared 2 MiB body cap
and request deadline; the total wait deadline can stop an in-flight read sooner.
The request cap can end a wait before the requested time budget. Result JSON is
capped at 65536 bytes after redaction/escaping; latest evidence is marked truncated
if stages are reduced, preserving the ID and prioritized failure evidence.

| `end` | Meaning |
| --- | --- |
| `complete` | Core reported Complete. `latest_update.outcome` uses Core's final `success` field to distinguish succeeded/failed. |
| `deadline` | The local time budget expired; remote execution may still be queued/running or already complete. |
| `poll_limit` | The ten-lookup cap was reached; observation is incomplete. |
| `observation_failed` | A read failed, such as missing/inaccessible history, permission loss or unavailable Core; inspect `observation_error`. |
| `cancelled` | Local observation was cancelled; no remote cancellation was requested. A cancelled MCP request may have no response. |

Every incomplete result retains `update_id` and the last valid `latest_update`
when available. A later read failure never overwrites that evidence or turns a
queued/running update into an execution failure. If the first read fails or the
budget expires before any observation, `latest_update` is null. There is no silent
fallback, background poller or execution resubmission.

MCP `notifications/cancelled` stops local polling, including an in-flight lookup.
Process loss also ends that observer. Save the ID in the calling client: the SDK
can discard responses to cancelled requests. Cancellation of a wait does not
cancel the Core operation. After timeout, cancellation or process/session restart,
call this tool again with the same ID, or use `get_update` for one lookup. A fresh
read-only session is sufficient. Retrying observation reads history; issuing a
new deploy/start/stop/restart call starts a new operation.

Core execution success is separate from workload health. After a successful
wait, use `stack_diagnose` / `fleet_health` and bounded logs. The production stdio
fixture includes a completed successful update followed by an **unhealthy** stack
diagnosis, so a successful command is never treated as a health guarantee.

## Inspect results and recover without repeating execution

Save the receipt and its `update_id`. In this or a new read-only MCP session:

```json
{"name":"get_update","arguments":{"id":"100000000000000000000001"}}
```

If no ID was received, retain the stable workload ID, requested service/scope,
operation and submission time window when available. Inspect bounded resource
history with `list_updates`; a matching recent update is evidence, not proof that
it belongs to this call. Another operator may have issued an identical action.
History may be delayed or inaccessible. Never automatically resubmit an uncertain
operation or infer that a lost reply means it was not executed. There is no
upstream idempotency-key guarantee or exactly-once promise across independent
client retries. A new explicit operation is a new submission and can repeat effects.

Retrying read-only observation does not execute again. Once Core reports a final
outcome, inspect `stack_diagnose` / `fleet_health` and bounded logs independently:

```json
{"name":"stack_diagnose","arguments":{"id":"000000000000000000000002"}}
```

The current deterministic HTTP/stdio fixtures validate request selection,
submission/observation behavior and saved-effect evidence. They do not execute
Core's permission engine, Docker dependencies or hooks. The focused disposable
Core/Periphery smoke and final integrated operator walkthrough remain #74.

Sources: pinned [Core acknowledgement](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/execute/mod.rs),
[update initialization](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/helpers/update.rs),
[Stack requests](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/api/execute/stack.rs),
[Deployment requests](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/api/execute/deployment.rs),
[Periphery Compose execution](https://github.com/moghtech/komodo/blob/v2.3.2/bin/periphery/src/api/compose.rs),
[Core Stack lifecycle](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/stack/execute.rs),
[Core Deployment lifecycle](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/execute/deployment.rs),
[Docker Compose up](https://docs.docker.com/reference/cli/docker/compose/up/),
[start](https://docs.docker.com/reference/cli/docker/compose/start/),
[stop](https://docs.docker.com/reference/cli/docker/compose/stop/), and
[restart](https://docs.docker.com/reference/cli/docker/compose/restart/).
