# Shared incident fixture (#10)

`incident.json` is the scenario contract for fleet health (#6), history (#7),
workload logs (#8), diagnosis (#9), stdio integration and benchmarks (#10).
Define new wire responses using the pinned `komodo_client` 2.3.2 types and these
IDs; do not change the existing inventory or quickstart fixture's behavior.

The broken Compose stack is `alpha-web` (`000000000000000000000002`) on
`fixture-server` (`000000000000000000000001`). Service `web` is unhealthy after
update `100000000000000000000001` (`DeployStack`, completed, `success: false`).
Its failed stage is `Deploy Stack`, with stderr `web failed its healthcheck
after rollout`. Critical StackStateChange alert `200000000000000000000001`
targets that same stack. Its bounded service log contains exactly:

```text
web | ERROR: database migration missing: relation orders does not exist
```

Service `db` remains healthy. `beta-other` is a healthy stack on `other-server`;
the separate unavailable and unknown servers distinguish failed reads from
unobserved health. Healthy and inconclusive variants retain the IDs while
changing observations or source availability. All data and timestamps are
synthetic. Credential/config/other-data markers in the contract are adversarial
sentinels and must not appear in projected output. API credentials are separately
supplied by the test harness and must also be redacted.

Larger fleets add deterministic resources without changing this seeded failure.
The benchmark and stdio client must follow the returned stack/update/alert IDs
and service filter to independently verify the same evidence. Fixtures simulate
Read access and reject mutation routes; they do not run Core, Docker, its
authorization engine, or an actual rollout.
