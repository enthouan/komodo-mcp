# Diagnostic tools

These tools are read-only, idempotent, and non-destructive. They use the shared
Core client and safe application errors; resource IDs come from visible inventory.

| Tool | Inputs and bounds |
| --- | --- |
| `fleet_health` | `page` and `page_size` per Server, Stack, and Deployment family; default 30, maximum 100. Follow `next_page` with the same size until exhausted. |
| `get_container_logs` | Stable `server_id`, exact `container` name or ID. |
| `get_deployment_logs` | Stable Deployment `id`. |
| `get_stack_logs` | Stable Stack `id`, optional `services` (at most 16 distinct exact names). Compose supports empty for all services; Core 2.3.2 Swarm requires exactly one service. |

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
JSON without decoding escapes into physical lines. It sanitizes each complete
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
configured-credential matcher is compiled once per configuration and shared by
the client and diagnostic writer, avoiding repeated long-prefix comparisons.
It cannot recognize every unlabeled secret or recover a label already omitted
by Core's upstream tail. Only request and share logs appropriate for the caller.

Tests cover service forwarding, failures, oversized UTF-8 lines, and empty output.
Production-stdio cases exercise all three tools and check configured credentials,
Authorization schemes, nested/escaped JSON, camelCase labels, and multiline values.
The log-only shared
wire fixture extends the incident contract; full diagnosis and end-to-end
benchmark acceptance remain separate work.
