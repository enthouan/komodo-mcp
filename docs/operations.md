# Workload operations

Set `KOMODO_ALLOW_OPERATE=true` in the MCP server's process environment and
restart it. The default remains read-only. `KOMODO_ALLOW_CONFIGURATION_EDIT`
is independent and exposes no production configuration-edit tools yet. Use an
enabled non-admin Core identity with Read visibility and Execute permission on
the selected Stack or Deployment. Core remains authoritative; local grants never
change credentials or supply missing upstream permissions.

## Supported deploy targets

| Resource / target | Whole workload | One named service |
| --- | --- | --- |
| Stack on Server (Compose) | Supported | Supported, exact Compose service |
| Stack on Swarm | Supported | Rejected before execution: Core 2.3.2 ignores this filter |
| Deployment on Server (container) | Supported | Not a Deployment input |
| Deployment on Swarm (service) | Supported | Not a Deployment input |
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
must occur exactly once in Core's raw **latest saved-definition service inventory**;
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
`DeployStack`, and a Deployment uses `Deploy`.

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
and [Docker Compose up](https://docs.docker.com/reference/cli/docker/compose/up/).
