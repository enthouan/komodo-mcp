# Diagnostic tools

These tools are read-only, idempotent, and non-destructive. They use the shared
Core client and safe application errors; resource IDs come from visible inventory.

| Tool | Inputs and bounds |
| --- | --- |
| `fleet_health` | `page` and `page_size` per Server, Stack, and Deployment family; default 30, maximum 100. Follow `next_page` with the same size until exhausted. |
| `list_alerts`, `list_updates` | Optional `{target:{kind,id},page}`; fixed Core pages of at most 100 records. Follow `next_page` unchanged with the same target. |
| `reconcile_operation` | Original stable Stack/Deployment target, scope/action and optional submission window; at most three history pages, five service-detail lookups/five candidates, 2-second reads/10-second total. Candidates always have unconfirmed attribution. [Recovery](operations.md#recover-an-interrupted-submission-without-repeating-execution). |
| `wait_for_update` | Known update `id`, optional `timeout_seconds` (1–30; default 10); at most ten read lookups, at least one second between unfinished responses. [Wait/resume semantics](operations.md#wait-and-resume-by-update-id). |
| `get_update` | Stable update `id`; at most 20 stages, failures first with original indices. Stage names: 128 UTF-8 bytes; sanitized stderr: 1024 bytes each. |
| `get_container_logs` | Stable `server_id`, exact `container` name or ID. |
| `get_deployment_logs` | Stable Deployment `id`. |
| `get_stack_logs` | Stable Stack `id`, optional `services` (at most 16 distinct exact names). Compose supports empty for all services; Core 2.3.2 Swarm requires exactly one service. |
| `stack_diagnose` | Stable Stack `id`; at most seven Core reads, four concurrent, in two 2-second waves, with 2 MiB source-body caps. |

For example, use these `tools/call` parameters:

```json
{"name":"fleet_health","arguments":{"page":0,"page_size":30}}
```

Fleet health uses at most four simultaneous Core reads per call, a 2-second
source deadline, and a 10-second total wait budget. It returns healthy, unhealthy,
unavailable, and unknown observations, with server CPU/memory/disk utilization
when supplied. A failed family or server retains other successful observations.
Resource-specific gaps identify unavailable checks; a check that never started
before the budget expired remains unknown. Templates are not probed.

Stack service output is capped at 100 entries and labels omitted services.
Complete distinct service coverage is required before calling a stack healthy;
missing services, repeated replicas, and names changed by redaction or clipping
cannot establish that coverage. Docker healthchecks can override a running
container lifecycle state. Unknown health
is never a confirmed failure. Utilization includes Core's cache refresh timestamp;
independent observations are not an atomic snapshot or a live host probe.

The reusable service-state reader and [shared incident contract](../tests/fixtures/core-2.3.2/INCIDENT.md)
support later diagnosis and integration work. Evidence uses synthetic Core 2.3.2
wire data and simulated Read access; it does not run a deployed Core, Docker
rollout, or Core's authorization engine.

## Incident history

For example, read alerts for a stack with these `tools/call` parameters:

```json
{"name":"list_alerts","arguments":{"target":{"kind":"stack","id":"000000000000000000000002"},"page":0}}
```

Target kinds use snake_case, such as `server`, `stack`, and `deployment`. Omit
the target to inspect recent records across visible resources. Core returns
newest records first. Continue using `next_page` and the same target until null;
a full final page may require one additional empty page. Arbitrary Mongo queries
and caller-selected page sizes are not accepted. Pages must be below 4294967295.

Summaries expose stable IDs, target, timestamps, operation/alert variant, and
outcome. Queued and running updates remain distinct; completed outcomes derive
from Core's `success` field. Update lookup prioritizes failed stages even when
they occur after the first 20. Stage names and stderr use the shared log
sanitizer before UTF-8 clipping, with explicit truncation flags. Raw current and
previous TOML, `other_data`, command, operator, and unrestricted stdout/stderr
are omitted.

All three history reads enforce a 2 MiB JSON-body cap before typed decoding,
including chunked responses, and retain the shared whole-operation deadline.
Oversized input returns `response_too_large` without a fallback read. A stage
field exceeding the sanitizer's 1 MiB input cap is hidden and marked truncated.
Synthetic history records reuse the shared incident IDs; they do not run Core's
authorization engine or establish real deployment behavior.

## Workload logs

Each log call sends one positive, bounded Core read. `tail` defaults to 100 and
accepts 1–500; `max_bytes` defaults to 16384 and accepts 1–65536. `timestamps`
defaults to false. Container/service names accept 1–128 ASCII letters, digits,
underscores, dots, and hyphens, without a leading hyphen. For example:

```json
{"name":"get_stack_logs","arguments":{"id":"000000000000000000000002","services":["web"],"tail":100,"max_bytes":16384}}
```

Line and UTF-8 byte limits apply jointly to returned stdout and stderr after
redaction. Core returns separate streams without a shared order, so local tail
selection uses stdout then stderr; it does not claim timestamp ordering. Byte
clipping retains the latest suffix at valid UTF-8 boundaries. JSON escaping and
MCP text/structured duplication add transport bytes beyond this content budget.
The result includes the target, requested bounds, timestamps, returned line/byte
counts, and separate line/byte truncation flags. `upstream_may_have_more` signals
that a full received tail might omit earlier context; Core supplies no complete
log count. Empty output succeeds. An upstream error or failed log command returns
a safe unavailable-source error; it never triggers an uncapped fallback read.
Core's log command and stage fields are excluded.

Input is bounded separately from the requested output: log JSON bodies are
limited to **2 MiB before typed deserialization**, including chunked responses
without Content-Length. Decoded stdout and stderr share a **1 MiB input limit**
before redaction. Exceeding either returns `response_too_large` with guidance to
reduce the tail or service filter; the reader drops the body without a fallback.
It never cuts an input stream before sanitizing it. The operation deadline still
includes headers and every body chunk.

The sanitizer removes configured Core credentials, common labeled application
credentials (including compound snake/hyphen/camelCase keys such as
`AWS_SECRET_ACCESS_KEY` and `secretKey`), Authorization
values, and standalone Bearer tokens. It handles quoted and repeatedly encoded
JSON, including Unicode-escaped credential keys, without decoding escapes into
physical lines. It sanitizes each complete
received stream before selecting the local tail or clipping bytes. Ambiguous
quotes and YAML block secret values conservatively hide the remaining received
stream while preserving CR/LF, so later benign context may also be hidden.
Unquoted labeled credentials hide the remainder of their physical line because
spaces and punctuation may belong to a password or connection string.
Plain sensitive keys with multiline values, indented continuations, or YAML
comments/tags/anchors hide the remaining stream. Genuinely empty Authorization
headers retain following unindented or blank lines. Structured credential values
retain neighbors only when their ending is unambiguous in a complete JSON context.
Configured credentials are matched at repeated JSON encoding depths that fit
the input budget, without decoding escapes into physical output lines.
Comment-only lines do not end a sensitive value. Compose prefixes and Docker
timestamps are accounted for when checking continuation indentation. The
continuation scans share an aggregate byte budget proportional to the received
stream; exhaustion conservatively hides the remaining stream. The
configured-credential matcher is compiled once per configuration and shared by
the client and diagnostic writer, avoiding repeated long-prefix comparisons.
It cannot recognize every unlabeled secret or recover a label already omitted
by Core's upstream tail. Only request and share logs appropriate for the caller.

## Stack diagnosis

```json
{"name":"stack_diagnose","arguments":{"id":"000000000000000000000002"}}
```

The report combines the existing inventory, service-state, alert/update, and
log readers. It identifies affected services and retains resource, update and
alert IDs so clients can inspect supporting evidence independently. Every source
failure becomes a gap while successful evidence remains available. Current
outcomes are `healthy`, `unhealthy`, or `inconclusive`; historical rollout failures
do not override confirmed current health. Complete service-name coverage is
required for health, including names preserved through redaction and clipping.

Each call makes at most seven Core reads and awaits at most four concurrently.
The first wave includes both inventory reads inside one 2-second deadline; the
second reads at most one failed update and one log tail. The two-wave wait budget
is 4 seconds, excluding local projection/serialization and scheduler overhead.
Diagnosis enforces a 2 MiB response-body cap before typed decoding for every
source, including chunked responses. Existing standalone inventory callers keep
their transport behavior. Oversized sources become gaps rather than discarding
other evidence. There are no retry or fallback reads.

Evidence retains at most 20 services, five recent records per history source,
five update stages, and one 40-line/4096-byte log tail. Unhealthy replicas are
deduplicated before selecting up to four affected services. Compose logs use
those filters; Swarm selects one known service, or reports a gap when no name is
available. Further service observations remain in the report up to its limit.

Serialized structured-report JSON is capped at 65536 bytes **after** final
credential redaction and JSON escaping. Optional evidence is reduced and marked
truncated as needed. MCP text content duplicates the same report; framing adds
bytes. Independent observations are not an atomic snapshot, and correlated
history/logs are evidence rather than proof of causality. The shared incident
fixtures simulate Read access without running Core, Docker or its authorization
engine. The full stdio workflow starts at fleet health and independently follows
the diagnosis's update, alert and service-log evidence. The
[benchmark harness](../benchmarks/README.md) retains inventory and compares
concurrent incident workflows as fleet size grows, with source/executable
identities and provisional measurements recorded separately from validation.
