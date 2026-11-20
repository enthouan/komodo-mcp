# Benchmark findings

The Rust server and the pinned MP-Tool 1.5.0 reference both returned the expected
results against a synthetic Komodo Core fixture. These runs used a shared
x86-64 macOS machine. Timings varied between runs.

| Workload | Rust median | Reference median |
| --- | ---: | ---: |
| Fresh startup, 125-stack fixture | 17.4 ms | 608.8 ms |
| Complete warm inventory, 125 stacks | 11.5 ms | 28.8 ms |
| Incident triage, 7 stacks, one caller | 16.5 ms | 27.4 ms |
| Incident triage, 125 stacks, one caller | 125.1 ms | 54.1 ms |
| Incident triage, 125 stacks, four concurrent callers | 468.6 ms | 103.2 ms |

The inventory run at `805f95e` used three batches with 30 warm reads and 10
fresh starts per implementation per batch. An earlier run of the same workload
had warm medians of 20.5 ms for Rust and 50.5 ms for the reference.

The incident run at `3bacb1b` timed each group of callers completing the same
seeded diagnosis, with ten measured groups per configuration. Rust also collected
fleet-wide health, service and alert data. At 125 stacks with one caller, it made
406 Core requests compared with 12 for the reference. That extra work matters
when interpreting the timings on larger fleets.

To repeat the measurements with a different build or host, follow the
[benchmark instructions](../README.md#reproduce). Run
`python3 benchmarks/bench.py summarize` with the path to a local report to
validate it and print the aggregate results.
