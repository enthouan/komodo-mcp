# Workload operations

[Repo clone/pull](repositories.md#inspect-effects-before-one-clone-or-pull) extends
the same receipt, update, wait and read-only reconciliation contracts. Inspect
saved hooks first, then acknowledge their presence in the explicit operation
input. Repo recovery uses target kind `repo`, scope `whole_repo` and operation
`clone` or `pull`; a history match never establishes caller attribution.

[Build run/cancel](builds.md#run-one-saved-build) uses the same receipt/history/wait/
recovery contracts with target kind `build`, scope `whole_build`, and operation
`run` or `cancel`. Inspect `get_build` first. Image output reports are bounded
observations; Build success does not prove a dependent deployment or its health.

Set `KOMODO_ALLOW_OPERATE=true` in the MCP server's process environment and
restart it. The default remains read-only. `KOMODO_ALLOW_CONFIGURATION_EDIT`
is independent and exposes the typed configuration tools. Use an
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
The request cap can end a wait before the requested time budget. Structured result JSON is
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

## Recover an interrupted submission without repeating execution

Before calling a mutation, retain the **resolved stable target ID**, exact original
service/scope, action and a local UTC start time in the calling client. Save the
receipt, including its submission window and update ID, when one arrives. If the
MCP process dies before a receipt, the client still has the original context;
record the observed end/loss time when available. Recovery does not depend on
an in-memory MCP session or a local queue.

If an update ID is known, resume it with `get_update` / `wait_for_update` from a
new read-only session. An acknowledged update that later fails due to permissions,
busy/conflict or rollout is still an **accepted submission**. Failure is not a
reason to treat the original request as unsent or to automatically repeat it.

```json
{"name":"get_update","arguments":{"id":"100000000000000000000001"}}
```

When acceptance is unknown and no update ID was received, call the read-only
`reconcile_operation` with the original context. Use actual retained values;
these are illustrative `tools/call` parameters:

```json
{"name":"reconcile_operation","arguments":{"target":{"kind":"stack","id":"000000000000000000000002"},"requested_scope":{"kind":"service","service":"web"},"operation":"deploy","submission_window":{"started_ts":1794500000000,"ended_ts":1794500015000}}}
```

Deployment scope is `{"kind":"whole_deployment"}` with target kind `deployment`;
whole-Stack scope is `{"kind":"whole_stack"}`. Actions are `deploy`, `start`,
`stop`, `restart`. Resource names, wildcards, scope changes and arbitrary history
queries are not accepted. The stable target and exact service must come from the
original input/receipt, not a clipped or redacted display name. Optional window
bounds use UTC Unix milliseconds; omit unknown bounds or the whole window.
Core's clock and history visibility may differ from the client, so time filtering
can miss an accepted execution. A missing receipt never authorizes execution again.

Reconciliation reads at most **three fixed 100-record resource-history pages**
(newest first), **five update details** for service evidence and returns at most
**five possible candidates**. Each read has a 2-second budget inside a 10-second
total budget; cancellation stops this local observer. Bodies retain the shared
2 MiB cap. Structured output is capped at 65536 bytes after sanitization/escaping.
Truncation, partial/error evidence and request counts are explicit. The search
deduplicates stable update IDs because page contents can change during reads.
It never waits for visibility in the background, queues work or submits execution.

Candidates match the target, Core operation variant and available time bounds.
For a named service, Core 2.3.2's specific `Service/s` requested-service stage can
corroborate the scope. This is not a claim that dependencies/hooks were isolated.
No raw stdout, commands, operator identity, saved configuration or `other_data`
is returned. Missing/ambiguous service evidence, denied detail access or the
lookup cap is labeled `scope_evidence: unavailable`; a proven different requested
service is excluded. Busy/denied failed updates remain visible candidates.

**`original_submission` stays `unknown` and `attribution` stays `unconfirmed`.**
No candidate is selected automatically, including when only one exists. Another
operator or independent client using the same credentials can submit the same
action on the same workload/service within the same window. Core exposes no
unique invocation/idempotency key that proves which client call produced a
history record. Inspect a candidate's ID with read tools only; doing so observes
that candidate, not proof that it was the original submission.

| Evidence | Next step |
| --- | --- |
| Zero candidates | Retain uncertainty. Visibility, permissions, clock differences or the page/time budget can hide an accepted request. Retry bounded observation later; do not infer that execution is safe to repeat. |
| One candidate | Inspect its Core update and external context. A single match, even for the same Core actor, does not prove attribution. |
| Multiple candidates | Keep every candidate unconfirmed. Do not guess which belongs to the interrupted call. |
| Inaccessible/partial history | Keep the context and returned evidence gap; restore Read access or inspect Core with an authorized identity. No credential fallback occurs. |
| Known acknowledged ID | Follow that exact update with `get_update` / `wait_for_update`, including a failed busy/denied outcome. |

Retrying observation never executes again. If an operator decides after reviewing
evidence that a new operation is needed, a new explicit mutation call is a new
submission and can repeat effects. There is no exactly-once guarantee across
independent client retries, automatic history-based attribution, or implicit
resubmission in transport, handlers or reconciliation.

After observing a final Core outcome, inspect workload health independently:

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
