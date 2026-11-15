# Inspect existing Swarms

Swarm tools are read-only in every local capability profile. Use the same
enabled Core identity throughout; local grants never confer Core permissions.
These tools provision no clusters and perform no node, service, secret or config
writes. Live manager/worker acceptance is tracked separately in #112.

| Tool | Core requirement | Evidence |
| --- | --- | --- |
| `list_swarms` | Read on visible Swarms | Stored identity, cached health/error presence |
| `get_swarm` | Read + Inspect | Cached Docker cluster ID and rotation flag |
| `list_swarm_nodes`, `list_swarm_services`, `list_swarm_tasks` | Read | Core's cached Docker lists |
| `get_swarm_node`, `get_swarm_service`, `get_swarm_task` | Read + Inspect | Fresh exact-object inspection through a configured manager |
| `diagnose_swarm_service` | Read + Inspect | Cached discovery with bounded fresh object details and source gaps |
| `get_swarm_service_logs` | Read + Logs | Fresh exact-service log tail; no Inspect grant needed |

List Swarms with `list_swarms({"page_size":30})`, following `next_page` with the
same page size. Inspect one Swarm with `get_swarm({"name":"test-swarm"})` or its
stable 24-character Komodo resource ID. An absent or inaccessible Swarm never
selects another cluster. Templates and records without managers are unsupported
for Docker observations.

For objects, pass a Swarm `target` and one explicit `object` selector:

```json
{"name":"list_swarm_services","arguments":{"target":{"name":"test-swarm"},"page_size":30}}
{"name":"get_swarm_service","arguments":{"target":{"id":"000000000000000000000050"},"object":{"name":"test-web"}}}
{"name":"diagnose_swarm_service","arguments":{"target":{"id":"000000000000000000000050"},"object":{"id":"ccccccccccccccccccccccccc"}}}
```

IDs above are synthetic. Use IDs from the actual list. Docker Swarm IDs are full
25-character lowercase alphanumeric identifiers; prefixes and wildcard filters
are rejected. Names are exact and case-sensitive; node names select hostnames.
Multiple matching names require an explicit ID. Details check the freshly
returned ID against the cached selection. Missing, invalid, ambiguous, stale or
incomplete selection never falls back to every service or another target.

`list_swarm_tasks` accepts an optional exact `service` selector. Omit it
deliberately for the Swarm's full bounded task inventory. With a service selector,
failed resolution returns an error instead of a broader list. Explicit `null`
selectors are invalid; only omission selects the full task inventory.

## Cache, placement and health

Core 2.3.2 reads node/service/task lists from its Swarm cache. Periphery's poll
can suppress individual Docker failures to empty vectors, and Core returns empty
vectors when its list cache is missing. Empty therefore describes the received
cache, without proving an empty or healthy cluster. No cache generation timestamp
or atomic relationship across requests is available. Swarm health and error
presence help identify unreachable infrastructure; arbitrary error text is omitted.
A transport failure or explicit permission denial remains an error, not an empty
success. Unsupported endpoint responses preserve their HTTP status.

Object details freshly inspect one selected ID. Docker may omit running-task
counts from service inspection even when the cached list has them; `null` remains
unknown. Desired replicas, including zero, are separate from observed counts.
A task whose desired state is `running` is classified `current`; desired
`shutdown`/`remove` is `historical`, and other/missing desired states remain
`unknown`. Historical tasks in the running state never count as current replicas.
Unclassified tasks have their own count and withhold the cache under-replication
comparison; a missing task state never becomes a pending-task observation.

Diagnosis compares desired replicas with **cached** current-running tasks,
identifies pending tasks, and inspects up to eight tasks and four nodes. Failed or
pending tasks take priority over running history. Placement includes node role,
availability and state, task/service/node IDs, slot and exit code where supplied.
For unassigned pending tasks, candidate nodes are shown without claiming they
satisfy the service's placement constraints. Conservative failure categories
include unavailable images, nonzero exits, insufficient resources and unsatisfied
constraints. Raw Docker error strings are withheld because they can contain
credentials, commands or resolved configuration.

`gaps` retains denied or failed follow-up sources. Coverage flags distinguish
local clipping, incomplete fresh task/node coverage and cached display clipping.
Even complete local coverage is not proof that Core's cache is complete or fresh.
Use independent workload health after execution; a successful inspection does not
prove deployment completion, convergence or application health.

## Bounds and sensitive fields

Each upstream body is capped at 2 MiB before decoding. Object lists scan at most
512 records and return positive local pages of 1–100 items; they expose observed
count and scan clipping. Object resolution refuses a locally incomplete list.
These limits do not bound Core's or Docker's internal enumeration. Separate calls
can observe intervening changes; pagination is not a snapshot token.

With an ordinary name or stable Komodo ID, an object list takes at most three
Core reads; a detail or service-filtered task list takes four. An explicit Swarm
name shaped like a Mongo ObjectId can need up to four search pages, adding three
reads to those bounds. Diagnosis takes at most 21 reads, sequential follow-ups
and 30 seconds total. It displays at most 64 cached tasks and 64 cached
nodes, with explicit display and fresh coverage flags. Each observation output is
capped at 96 KiB, and selected names are bounded to 128 UTF-8 bytes after configured
credential redaction. Oversized evidence fails safely without submitting mutations.

Projections omit join tokens, CA/TLS keys, registry credentials, environment,
secret/config contents and references, labels, driver settings, commands and raw
status/error text. No raw Docker object leaves the tool. Reads require no
operation or configuration-edit capability.

Logs reuse the existing [redaction and output contract](diagnostics.md#workload-logs).
Pass `service` explicitly with the Swarm target. After unique cached resolution,
Core receives only the validated full service ID. This needs no Inspect permission
and cannot follow a replacement service with the same name. `details` is always
off and `no_resolve` on. Request `tail` 1–500 (default 100) and `max_bytes` 1–65536
(default 16384); redaction happens before local line/byte clipping. Known-secret
and labeled-value redaction cannot guarantee arbitrary application output is
secret-free. Failed log commands and unsafe upstream errors are omitted.

## Verification and pinned sources

`cargo test --locked --test swarms` exercises the compiled production executable
with synthetic Core 2.3.2 wire responses: all capability profiles, Read/Inspect/
Logs denial, exact selection, stale identities, pending/failed placement,
historical tasks, sensitive metadata, clipped caches, oversized bodies, partial
follow-up evidence, bounded logs and zero `/write` or `/execute` submissions.
These fixtures do not run Core's permission engine or a Docker Swarm. Real
manager/worker coverage and final integrated executable evidence belong to #112.

Behavior was checked against pinned [Core resolvers](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/read/swarm.rs),
[Periphery poll/inspection](https://github.com/moghtech/komodo/blob/v2.3.2/bin/periphery/src/api/swarm/mod.rs),
[Docker task conversion](https://github.com/moghtech/komodo/blob/v2.3.2/bin/periphery/src/docker/task.rs)
and [service logs](https://github.com/moghtech/komodo/blob/v2.3.2/bin/periphery/src/api/swarm/service.rs).
