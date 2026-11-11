# Provisional inventory baseline

[`macos-x86_64.json.gz`](macos-x86_64.json.gz) is the complete, validated
`komodo-inventory-benchmark/v1` archive, including all raw samples, 3,918 HTTP
events, expected inventory, equivalence digests, per-batch distributions, RSS
observations and artifact identities. Run ID:
`603d77fc-4439-4814-ac43-bafb402600df`.

The run took place on **November 10, 2026 (Pacific)**, 04:17–04:18 UTC on
November 11. Both build source and runner were clean at
`805f95ec6d78797226814de1ce1688d53685113e`; the later reporting commit adds this
archive and interpretation without relabeling the measured revision. Preparation
and all 13 correctness tests were reproduced in an isolated checkout. Both the
checkout and reference paths contained spaces.

Host: macOS 15.8.3, x86_64, Macmini8,1, Intel Core i5-8500B @ 3.00 GHz, 6 logical
CPUs, 64 GiB memory. The kernel reported `kern.hv_vmm_present=0`; processes ran
natively on macOS. Interactive/background load, power policy, CPU frequency and
filesystem caches were uncontrolled. Rust/cargo **1.99.1**, Node **24.18.0**,
npm **11.16.0**, Python **3.13.14**. Reference: MP-Tool 1.5.0 at
`489f7e04b16019480f84e1d2fff8b872b880e335`, lockfile SHA-256
`946c81c79078bb55984e14cdda86552a34e55678a90a272bf0ac53f81e00b6ad`.

Controls: **125 stacks**, page size **25**, **zero fixture delay**, `warn` logging,
**5 warmups + 30 warm samples + 10 fresh-process startup samples per batch**,
**3 alternating batches**. Each implementation therefore has 90 timed complete
reads and 30 startup samples, plus 3 dedicated RSS sessions. Every complete read
matched the independently defined fixture oracle, with no missing/duplicate IDs
or unknown states. Both needed 5 MCP calls per complete inventory.

| Metric | Rust release executable | Pinned TypeScript reference |
| --- | ---: | ---: |
| Fresh-process startup p50 / p95, ms | 17.419 / 20.163 | 608.796 / 947.517 |
| Complete warm inventory p50 / p95, ms | 11.544 / 14.376 | 28.754 / 36.298 |
| Idle RSS median, MiB | 5.43 | 98.47 |
| Highest sampled peak RSS, MiB | 9.19 | 175.14 |
| MCP response bytes per inventory, median | 55,554 | 30,767 |
| Observed Core requests per inventory | 10 | 5 |
| Observed Core response body bytes per inventory | 169,232 | 470,065 |
| Observed startup Core requests per process | 1 | 2 |

The Rust executable was **12,737,480 bytes** (Cargo release defaults, explicit
`x86_64-apple-darwin` target, no recorded flag/profile overrides), SHA-256
`f9e854f8f6c9318fee6cff93f92aca3572da678c85155eb89ef849fe343f4f7b`.
No JavaScript entry-file size is treated as an equivalent native executable size.
Both processes produced zero stderr bytes at the declared log level in this run.

Direct authenticated, paginated Core inventory p50/p95 was **5.963/8.250 ms**;
direct `GetVersion` was **0.415/1.145 ms**. The direct inventory uses the same
page size and verifies the same 125 records, with an extra empty read at this
exact boundary. Fixture handler service time across all phases had p50/p95
**0.438/1.686 ms**. Configured delay was explicitly zero; serialization,
scheduling, loopback and instrumented fixture work still take time. These
baselines accompany MCP timings; subtraction would not precisely measure
internal server overhead.

Rust had lower startup/read latency and RSS in this synthetic run, while returning
larger MCP responses and issuing more Core reads. Its full-page continuation
probes explain the request count. The reference makes fewer requests but
reloads the entire inventory for each cursor page, explaining its higher Core
body-byte total. Rust's richer summaries and JSON text differ from the
reference's compact summaries and rendered text. The reference also starts its
full tool registry and supporting framework. This is equivalence of the
inventory projection, not equivalence of every feature or payload.

Per-batch Rust warm p95 was **14.38, 14.81, 13.79 ms**; reference warm p95 was
**38.31, 32.46, 36.10 ms**. The pooled percentile does not describe every batch
equally. RSS peaks are sampled maxima with a 20 ms sleep
between observations, and can miss brief peaks. There are no performance
thresholds or general claims about deployed Core or other hosts; initial targets
and conclusions remain **provisional**.

The earlier successful capture is retained in full as
[`macos-x86_64-initial.json.gz`](macos-x86_64-initial.json.gz), run
`be78a681-5321-48d1-85c1-1bafd45a874a`, clean revision `1d31e76`. Its pooled warm
p50/p95 was **20.451/33.809 ms** (Rust) and **50.531/144.824 ms** (reference), with
the same native binary hash, fixture and workload controls. The current capture
repeats measurement after fixing preparation's handling of toolchain proxy
symlinks. These distinct runs are not pooled; their substantial variation on a
shared host reinforces the provisional interpretation. Both archives preserve
every sample.

## Reproduce or inspect

Follow [preparation instructions](../README.md#reproduce), then run:

```sh
python3 benchmarks/bench.py run --fixture-size 125 --page-size 25 \
  --warmup 5 --samples 30 --cold-samples 10 --repetitions 3 \
  --fixture-delay-ms 0 --output target/benchmark/baseline.json.gz \
  --host-note "Describe the current host, virtualization, power policy and load"
python3 benchmarks/bench.py validate target/benchmark/baseline.json.gz
```

The archived run used the resolved Python executable with driver PATH restricted
to `/usr/bin:/bin:/usr/sbin:/sbin`; neither Cargo nor npm was available on that
PATH. Prepared Rust/Node executables were launched by absolute path. The archive
records exact executable paths, argument arrays, preparation commands, versions,
hashes and the actual host note. No compilation or package installation occurred
during measurement.

Regenerate the published table's source summaries or inspect the raw JSON:

```sh
python3 benchmarks/bench.py validate benchmarks/results/macos-x86_64.json.gz
python3 benchmarks/bench.py summarize benchmarks/results/macos-x86_64.json.gz
gzip -dc benchmarks/results/macos-x86_64.json.gz
```
