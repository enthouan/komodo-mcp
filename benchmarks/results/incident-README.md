# Historical incident and fleet-growth measurements

These archives retain the initial implementation measured at `70f7671` with
their original source, executable, run and fixture identities. Subsequent reader
and redaction changes are validated and measured separately.

These complete `komodo-incident-benchmark/v1` archives compare full stack discovery and the same affected service, failed update stage, and runtime log excerpt. Every workflow matched the independent shared incident oracle, including the complete discovered ID/name/state digest. The original inventory workload and its older baseline remain intact.

Rust also collects all visible server state/utilization, all stack service observations, and alerts. The pinned reference has no fleet-health, stack-diagnosis, or alert tools; it discovers stacks and inspects the selected stack’s containers/update/logs. Equivalent common evidence does not imply identical functionality or Core work.

On this host, Rust used fewer MCP calls and less sampled memory. The reference workflow was faster for the larger fleets and made fewer Core requests. Rust’s broader fleet reads grow with fleet size; these results do not isolate language/runtime overhead or establish a general performance ranking.

## Controls and identity

- Clean build and runner commit: `70f7671cce911306d8f17f171b79d4d85ab20cb4`. The application version was still `0.1.0`; the dedicated v0.2.0 version/changelog cut follows separately. The reporting commit does not relabel the measured source. Later review fixes for duplicate replicas and Authorization, nested JSON, and multiline credential redaction are covered by regressions and CI; these captures retain their original implementation identity.
- Release executable SHA-256: `658fbf66aa51e98feff94cf4379ab9cc0099bf50cde5548691fc9d765ff49a24`. Exact compiler, source/lock hashes, executable size, Node/dependency identities, commands, and environment are retained in every archive.
- Reference: MP-Tool 1.5.0, commit `489f7e04b16019480f84e1d2fff8b872b880e335`, unchanged and built using its committed lockfile.
- Fleets: 7, 31, and 125 stacks, with two additional Servers and one Deployment; one seeded broken stack in every fixture. Page size 25; zero injected HTTP delay.
- Callers: 1 or 4 independently progressing workflows on one persistent MCP process. Per run: 1 warmup and 5 measured groups per batch, 2 alternating batches, plus separate RSS sessions.
- p50/p95 use nearest rank. Each group distribution has only 10 samples; four-caller distributions contain 40 dependent caller samples. These are provisional captures, not stable percentile estimates or performance thresholds.
- Core/MCP counts and bytes below apply to the **whole concurrent group**. Core has no per-caller correlation ID; the driver does not invent per-caller attribution.
- Host load, CPU frequency, power policy, filesystem caches, and scheduling are uncontrolled. No concurrent task build ran during measurements. RSS is a sampled server-PID maximum from a separate process, excluding the driver/mock; observation timestamps are retained.

Host metadata: `macOS-15.8.3-x86_64-i386-64bit-Mach-O`; `x86_64`; `Intel(R) Core(TM) i5-8500B CPU @ 3.00GHz`. Recorded UTC interval: `2026-11-12T01:49:19.207694+00:00` through `2026-11-12T01:51:13.832575+00:00`.

## Recorded results

| Stacks | Callers | Implementation | Group p50/p95 ms | Caller p50/p95 ms | MCP bytes p50 | MCP calls p50 | Core calls p50 | Peak RSS MiB | Peak observed Core concurrency |
| ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 7 | 1 | rust | 18.282/23.670 | 18.139/23.524 | 19680 | 4 | 37 | 15.26 | 4 |
| 7 | 1 | reference | 33.358/38.994 | 33.215/38.840 | 8368 | 8 | 8 | 142.57 | 1 |
| 7 | 4 | rust | 49.000/54.170 | 48.211/52.757 | 78720 | 16 | 148 | 18.99 | 15 |
| 7 | 4 | reference | 55.022/72.070 | 54.255/69.501 | 33502 | 32 | 32 | 163.53 | 4 |
| 31 | 1 | rust | 42.949/53.752 | 42.797/53.586 | 54628 | 5 | 112 | 16.03 | 4 |
| 31 | 1 | reference | 39.959/46.348 | 39.784/46.173 | 14350 | 9 | 9 | 141.32 | 1 |
| 31 | 4 | rust | 147.607/190.096 | 145.741/187.768 | 218512 | 20 | 448 | 19.99 | 16 |
| 31 | 4 | reference | 78.345/170.650 | 76.790/124.831 | 57436 | 36 | 36 | 168.86 | 4 |
| 125 | 1 | rust | 148.538/192.134 | 148.231/191.880 | 191538 | 9 | 406 | 16.62 | 4 |
| 125 | 1 | reference | 65.809/69.491 | 65.664/69.276 | 37517 | 12 | 12 | 154.32 | 1 |
| 125 | 4 | rust | 557.641/616.548 | 548.685/613.579 | 766188 | 36 | 1624 | 21.14 | 16 |
| 125 | 4 | reference | 126.561/140.526 | 126.241/138.397 | 150116 | 48 | 48 | 182.07 | 4 |

Rust’s per-call four-read cap is distinct from aggregate concurrency across multiple callers. Each diagnosis itself has at most seven Core reads; the workflow independently re-reads the update and filtered logs after diagnosis. Expected unavailable-server state/stats responses remain explicit evidence gaps rather than failures of the comparison. MCP bytes include the full actual response frames, including structured and text content; Core bytes in the archives are HTTP bodies.

## Raw archives

- [incident-macos-x86_64-n7-c1.json.gz](incident-macos-x86_64-n7-c1.json.gz) — run `c6a01575-87dd-4ed5-bd91-c465dd4c68e9`.
- [incident-macos-x86_64-n7-c4.json.gz](incident-macos-x86_64-n7-c4.json.gz) — run `a95b1d7e-2433-4b1a-85b5-9331c83a0614`.
- [incident-macos-x86_64-n31-c1.json.gz](incident-macos-x86_64-n31-c1.json.gz) — run `a655268e-3fb7-4509-9baf-55d77d6482b4`.
- [incident-macos-x86_64-n31-c4.json.gz](incident-macos-x86_64-n31-c4.json.gz) — run `3bb41652-354b-4c9b-97d4-493ac13ea2e2`.
- [incident-macos-x86_64-n125-c1.json.gz](incident-macos-x86_64-n125-c1.json.gz) — run `8ad78ec1-04f8-4533-9304-2372516bfca6`.
- [incident-macos-x86_64-n125-c4.json.gz](incident-macos-x86_64-n125-c4.json.gz) — run `840caf0f-c9d2-4727-b185-483d341e6465`.

[inventory-macos-x86_64-smoke.json.gz](inventory-macos-x86_64-smoke.json.gz) verifies the retained inventory workload with 7 stacks/page size 3, 1 warmup, 2 warm samples, 1 startup sample, and 2 batches. It is a correctness smoke capture, not a replacement for the larger [original inventory baseline](README.md).

Each incident archive includes raw caller/group samples, all instrumented Core events and observed concurrency, byte/call totals, RSS observations, process diagnostics, fixture/oracle hashes, and recomputed summaries. All seven new archives passed `bench.py validate`.

## Reproduce or inspect

Prepare from the recorded commit using the [harness instructions](../README.md), then run each fleet/caller pair:

```sh
python3 benchmarks/bench.py incident --fixture-size 125 --page-size 25 \
  --concurrency 4 --warmup 1 --samples 5 --repetitions 2 \
  --output target/benchmark/incident.json.gz \
  --host-note "Describe the actual host and uncontrolled load/power policy"
python3 benchmarks/bench.py validate target/benchmark/incident.json.gz
python3 benchmarks/bench.py summarize target/benchmark/incident.json.gz
```

All fixtures use synthetic Core 2.3.2 wire data and simulated Read access. No deployed Core, Docker rollout, permission engine, or production credentials are exercised. The expected failure remains `alpha-web` / `web`, update stage `Deploy Stack`, and the database-migration error defined in [INCIDENT.md](../../tests/fixtures/core-2.3.2/INCIDENT.md).
