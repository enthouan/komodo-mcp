# Diagnostic tools

All eight diagnostic tools are read-only, idempotent, and non-destructive. They
use the shared Core client and safe application errors. Resource IDs come from
visible inventory; no reader submits an update or retries an unlimited read.

| Tool | Inputs and bounds |
| --- | --- |
| `fleet_health` | `page` and `page_size` per Server, Stack, and Deployment family; default 30, maximum 100. Follow `next_page` with the same size until exhausted. |
| `list_alerts`, `list_updates` | Optional `{target:{kind,id},page}`; fixed Core pages of at most 100 records, newest first. Pass `next_page` unchanged. No arbitrary Mongo query or `page_size` is accepted. |
| `get_update` | Stable update `id`; at most 20 stages, failures first with original indices. Stage names: 128 UTF-8 bytes; redacted stderr: 1024 bytes each. |
| `get_stack_logs` | Stable Stack `id`, up to 16 exact `services`; an omitted/empty list includes all services. |
| `get_deployment_logs` | Stable Deployment `id`. |
| `get_container_logs` | Stable `server_id` and exact `container` name or ID. |
| `stack_diagnose` | Stable Stack `id`; at most seven Core reads, four concurrent, in two 2-second waves. |

For example, inspect alerts for one stack with these `tools/call` parameters:

```json
{"name":"list_alerts","arguments":{"target":{"kind":"stack","id":"000000000000000000000002"},"page":0}}
```

The history target kind uses snake_case, such as `server`, `stack`, or
`deployment`. Omit the target to read recent records across visible resources.
Queued and running updates are distinct from completed outcomes. Completed
updates derive `succeeded` or `failed` from Core's `success` field. Raw current
and previous TOML, `other_data`, command, and unrestricted stdout/stderr never
enter history projections. Failed stages are prioritized even when they occur
after the first 20 entries.

Fleet health uses at most four simultaneous Core reads per call, a 2-second
source deadline, and a 10-second total budget. It returns healthy, unhealthy,
unavailable, and unknown observations, with server CPU/memory/disk utilization
when supplied. A failed family or server retains other successful observations.
Resource-specific gaps identify unavailable checks; a check that never started
before the budget expired remains unknown. Templates are not probed. Stack
service output is capped at 100 entries and labels omitted services. Docker
healthchecks can override a running container lifecycle state. Unknown health
is never a confirmed failure.

All log tools accept `tail` (default 100, maximum 500), `max_bytes` (default
16384, maximum 65536), and `timestamps` (default false). Positive line and byte
limits apply **jointly across stdout and stderr**. The tools retain the tail in
stdout-then-stderr order; Core supplies no ordering between streams. Configured
API credentials, common sensitive assignments, complete Authorization values,
and Bearer tokens are redacted before UTF-8 clipping. Literal and nested
JSON assignments retain their escaped delimiters. This cannot identify every unlabeled application secret.
Results include returned line/byte counts and flags for local line/byte clipping
and possible upstream history beyond a full tail. Core supplies no complete-log
counts. A failed log command produces a safe error without a fallback request.
JSON escaping and MCP framing add bytes beyond the log-content limit.

Diagnosis retains at most 20 service observations, five recent history records
per source, five failed-update stages, and one 40-line/4096-byte log tail filtered
to at most four affected services. It returns `healthy`, `unhealthy`, or
`inconclusive`, affected services, resource/update/alert IDs, and source gaps.
Independent observations are not an atomic snapshot: correlated history and logs
are evidence rather than proof of causality. Old failed rollouts do not override
currently healthy workload observations. Missing service coverage prevents a
healthy conclusion. An update that changes target or outcome during inspection
is discarded with a gap while independent evidence survives.

The diagnosis's serialized structured-report JSON is capped at 65536 bytes,
including JSON escaping, by reducing optional evidence and labeling truncation.
MCP text content duplicates that report; transport framing adds bytes. The
two-wave wait budget excludes local serialization and scheduler overhead.

The [shared incident fixture](../tests/fixtures/core-2.3.2/INCIDENT.md) connects a
failed rollout, unhealthy `web` service, critical alert, and database-migration
log excerpt. Tests follow these IDs through the actual stdio executable. The
[benchmark harness](../benchmarks/README.md) retains inventory comparisons and
adds concurrent incident workflows and fleet growth. All such evidence uses
synthetic Core 2.3.2 wire data and simulated Read access. It does not exercise a
deployed Core, Docker rollout, or Core's authorization engine.
