# Finalized incident and fleet-growth measurements

These fresh captures measure clean source `3bacb1b6aeae1f6e11fa21e46b50ec20001c7be8` and release executable SHA-256 `58ec626f3c02cc1fa6ab5282c4a0f6cfdb453382fcff2467f9aa81fb689cd45a`. The application version remains `0.1.0` pending the separately approved v0.2.0 release PR. A later reporting commit adds these unchanged archives and documentation; it does not relabel their measured source.

All callers matched the independent common evidence oracle: complete discovered stack IDs/names/states, the affected service, failed update ID/stage, and expected error excerpt. Rust additionally collects fleet-wide server state/utilization, services, alerts and diagnosis. The pinned reference inspects the selected stack’s containers, update and logs; it lacks fleet-health, diagnosis and alert tools. Equivalent seeded evidence therefore includes different functionality and Core work.

The [historical matrix](incident-README.md), its seven archives, and the original [inventory baseline](README.md) retain their original identities and bytes.

## Controls and identities

- Source: `3bacb1b6aeae1f6e11fa21e46b50ec20001c7be8`, clean for preparation and every measurement. Source hash: `bb85ac050a1eaf401ad8c2da2d621fee54641bde99a18d79c9739f025a02faf0`; Cargo lock hash: `92f8ffa68ebe715dfd1e33eefd01f5092819cb8e6d9ef8890c44a38b48992352`.
- Reference: MP-Tool 1.5.0, commit `489f7e04b16019480f84e1d2fff8b872b880e335`, unmodified with its locked dependencies. Node `v24.18.0` and npm `11.16.0`; complete build/dependency/runtime hashes remain in every report.
- Rust: `rustc 1.99.1 (58cf8f9c3 2026-10-13)`; `cargo 1.99.1 (5f94df478 2026-08-27)`; target `x86_64-apple-darwin`, release executable 14744328 bytes.
- Fleets: 7, 31 and 125 stacks, two additional Servers and one Deployment; the seeded incident is unchanged. Page size 25, zero injected HTTP delay, one or four independently progressing callers on one persistent MCP process.
- Each run uses one warmup and five measured groups per batch, two alternating batches, and separate RSS sessions. Idle stabilization: 200 ms; RSS polling interval: 20 ms, with actual observation timestamps retained.
- p50/p95 use nearest rank with no retries, filtering or outlier removal. Each group distribution has only ten samples; four-caller distributions contain 40 dependent caller observations. These are provisional shared-host measurements, with no performance pass/fail thresholds.
- MCP/Core byte and call totals apply to the whole group. MCP bytes are actual UTF-8 JSON-RPC response frames, including duplicated structured/text output and LF; Core bytes are HTTP bodies. Core has no caller correlation ID, so no per-caller Core attribution is claimed.
- Host: `macOS-15.8.3-x86_64-i386-64bit-Mach-O`, `x86_64`, `Intel(R) Core(TM) i5-8500B CPU @ 3.00GHz`, 6 logical CPUs. Background workload, power policy, caches and scheduling are uncontrolled. No task build ran concurrently with measurements.
- RSS is a sampled server-PID maximum from a separate fresh process; it excludes driver/mock memory and can miss short peaks.
- Recorded UTC interval: `2026-11-12T10:44:53.951879+00:00` through `2026-11-12T10:46:48.488948+00:00`.

## Recorded results

| Stacks | Callers | Implementation | Group p50/p95 ms | Caller p50/p95 ms | MCP bytes/calls p50 | Core body bytes/calls p50 | Peak RSS MiB | Peak Core concurrency |
| ---: | ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| 7 | 1 | rust | 16.466/35.134 | 16.344/34.963 | 19754/4 | 21206/37 | 15.18 | 4 |
| 7 | 1 | reference | 27.432/35.855 | 27.307/35.660 | 8368/8 | 7860/8 | 142.95 | 1 |
| 7 | 4 | rust | 42.435/49.142 | 42.056/48.709 | 79016/16 | 84824/148 | 19.30 | 15 |
| 7 | 4 | reference | 43.368/49.432 | 42.315/48.022 | 33502/32 | 31440/32 | 160.99 | 4 |
| 31 | 1 | rust | 36.773/43.112 | 36.584/42.976 | 54702/5 | 72066/112 | 16.30 | 4 |
| 31 | 1 | reference | 33.832/39.163 | 33.708/39.036 | 14350/9 | 49673/9 | 141.95 | 1 |
| 31 | 4 | rust | 122.713/142.814 | 121.945/141.716 | 218808/20 | 288264/448 | 21.18 | 16 |
| 31 | 4 | reference | 56.834/62.468 | 55.709/61.014 | 57436/36 | 198692/36 | 169.21 | 4 |
| 125 | 1 | rust | 125.079/135.039 | 124.820/134.803 | 191612/9 | 271269/406 | 16.77 | 4 |
| 125 | 1 | reference | 54.099/59.044 | 53.943/58.898 | 37517/12 | 477990/12 | 155.13 | 1 |
| 125 | 4 | rust | 468.616/537.644 | 468.283/534.429 | 766484/36 | 1085076/1624 | 22.53 | 16 |
| 125 | 4 | reference | 103.169/109.515 | 102.052/108.224 | 150116/48 | 1911960/48 | 180.84 | 4 |

Rust used fewer MCP calls and less sampled memory on this host. The reference completed the larger-fleet workflows faster and made fewer Core requests. Rust’s broader fleet reads grow with fleet size; these measurements do not isolate runtime overhead or establish a general performance ranking. Per-call four-read concurrency limits are distinct from aggregate concurrency across multiple callers.

## Raw reports

- [incident-final-3bacb1b-macos-x86_64-n7-c1.json.gz](incident-final-3bacb1b-macos-x86_64-n7-c1.json.gz) — run `4fc09e06-ac0e-42f6-9529-9c615748f9db`; gzip SHA-256 `08a89f9dfd64465a066b3c631e7097206d199b7eabd3047b70d7b62b02b1cbd0`.
- [incident-final-3bacb1b-macos-x86_64-n7-c4.json.gz](incident-final-3bacb1b-macos-x86_64-n7-c4.json.gz) — run `951ebfd0-b76a-4f5f-ad2e-7f01b5cad1b7`; gzip SHA-256 `ad902e7b5fed9aad3c1c48ac21d2606fe55ea13ecd68c5d41e2557e62d4a08eb`.
- [incident-final-3bacb1b-macos-x86_64-n31-c1.json.gz](incident-final-3bacb1b-macos-x86_64-n31-c1.json.gz) — run `a21c5d36-e7b6-4532-ac67-fb8d586c16cd`; gzip SHA-256 `63e36ca11c844c7c5298e3b1699f86a82143dce425eda014ec577409abcbf6ea`.
- [incident-final-3bacb1b-macos-x86_64-n31-c4.json.gz](incident-final-3bacb1b-macos-x86_64-n31-c4.json.gz) — run `6d44142b-78a8-4f2e-9872-4846b4ad3013`; gzip SHA-256 `f0c5f54faadd5e33bbe0f4b45c6476ce1354a8481a9499107ac22160957541de`.
- [incident-final-3bacb1b-macos-x86_64-n125-c1.json.gz](incident-final-3bacb1b-macos-x86_64-n125-c1.json.gz) — run `fdbb2d05-1f7b-40ac-b693-cfef1020c99f`; gzip SHA-256 `ce2b9174859bf17b5b34b688b90a4b27a14056aa841153615746883a1ffee201`.
- [incident-final-3bacb1b-macos-x86_64-n125-c4.json.gz](incident-final-3bacb1b-macos-x86_64-n125-c4.json.gz) — run `309bc5ac-f07e-43b9-9d42-76f2321d936a`; gzip SHA-256 `b66c006b6692655a04849788a593dd459b48d7f252915275e496e83587e608b1`.
- [inventory-final-3bacb1b-macos-x86_64-smoke.json.gz](inventory-final-3bacb1b-macos-x86_64-smoke.json.gz) — run `dfbfab96-eb8a-437a-8e9f-346d0b16df0e`; gzip SHA-256 `2cd497124071946bef4ba4b6d35b084bb7a4134f6e5bf7381e320e9b6751d3ef`.

The fresh inventory smoke retains 7 stacks/page size 3, one warmup, two warm samples, one fresh-process startup sample and two batches. It validates the retained workload rather than replacing the larger historical inventory baseline. Every report passed the harness validator and retains raw samples, process lifecycle, fixture/oracle hashes, observed Core events, actual byte/call accounting, RSS observations, and source/executable identities.

## Reproduce

Prepare the exact source with the [harness instructions](../README.md), then run each fleet/caller pair with these controls:

```sh
python3 benchmarks/bench.py incident --fixture-size 125 --page-size 25 \
  --concurrency 4 --warmup 1 --samples 5 --repetitions 2 \
  --fixture-delay-ms 0 --idle-ms 200 --rss-interval-ms 20 \
  --output target/benchmark/incident.json.gz \
  --host-note "Describe the actual host, power policy and background load"
python3 benchmarks/bench.py validate target/benchmark/incident.json.gz
python3 benchmarks/bench.py summarize target/benchmark/incident.json.gz
```

All evidence uses synthetic Core 2.3.2 wire fixtures and simulated Read access. Core’s database, authorization engine, Docker and real deployments are not exercised. The expected incident remains `alpha-web` / `web`, failed stage `Deploy Stack`, and the database-migration error in [INCIDENT.md](../../tests/fixtures/core-2.3.2/INCIDENT.md).
