# Diagnostic tools

`fleet_health` is read-only, idempotent, and non-destructive. It uses the shared
Core client and safe application errors; resource IDs come from visible inventory.

| Tool | Inputs and bounds |
| --- | --- |
| `fleet_health` | `page` and `page_size` per Server, Stack, and Deployment family; default 30, maximum 100. Follow `next_page` with the same size until exhausted. |

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
