# Benchmark findings

## v0.9.0 packaged candidate study

The packaged candidate returned the same complete inventory and seeded diagnosis as both references. It initialized faster and used less memory than TypeScript. Both references completed warm inventory and incident triage faster than the candidate on this host. The three servers also agreed with direct HTTP reads of a real seven-stack Core inventory. The benchmark defines no numeric latency or RSS acceptance threshold. [Per-sample measurements, summaries and artifact identities](../../docs/releases/v0.9.0/benchmark-samples.json)

### Artifacts and controls

- **Candidate** — `x86_64-apple-darwin` package from `e8432d39d6c12c5278d26f17959f041884ab5a4c`. Cargo and MCP both reported `0.8.0`. The executable SHA-256 was `6c57369b9c456fab4c95e92e05fd220472e19fc92de298d2da3c1219f213f319`. Rust 1.99.1 built the frozen release with distribution features and stripped symbols. The executable was 32,497,640 bytes, and the complete compressed native archive was 11,400,012 bytes. [Shared candidate record](../../docs/releases/v0.9.0/candidate-record.json) · [Native workflow](https://github.com/enthouan/komodo-mcp/actions/runs/45539030427) · [Matching OCI workflow](https://github.com/enthouan/komodo-mcp/actions/runs/45539030406)
- **Rust reference** — Kalaghni 0.1.0, `2efbe2232d98ac0c7eec90135fd683b5b39c1cca`. Cargo and rustc 1.99.1 built the locked release for the same host with thin LTO, one codegen unit and stripped symbols. Its executable was 5,673,600 bytes. [Pinned Rust source](https://github.com/Kalaghni/komodo-mcp/tree/2efbe2232d98ac0c7eec90135fd683b5b39c1cca)
- **TypeScript reference** — MP-Tool 1.5.0, `489f7e04b16019480f84e1d2fff8b872b880e335`. npm 11.16.0 installed the lock and compiled the distribution before measurement. The built tree was 1,708,728 bytes, installed dependencies 142,646,635 bytes and the Node 24.18.0 executable 123,320,000 bytes. Tree sizes sum regular files and exclude symlinks. [Pinned TypeScript source](https://github.com/MP-Tool/komodo-mcp-server/tree/489f7e04b16019480f84e1d2fff8b872b880e335)

An Intel Core i5-8500B at 3.00 GHz with six logical CPUs and 64 GiB RAM ran macOS 15.8.3. One-minute load averages at the four run starts ranged from 3.05 to 3.91. Workstation scheduling and power were uncontrolled. The Python 3.13.14 driver `komodo-inventory-benchmark/1` negotiated MCP `2025-03-26` over stdio. Direct Core used authenticated HTTP. The harness rechecked executable and build hashes before timing. All 24 prepared tests passed, including empty and exact-boundary inventory pages and concurrent triage against both fleet sizes.

Each workload ran two batches with two warmups and ten measured reads or caller groups per participant per batch. MCP position rotated between batches. The inventory workload also measured five fresh starts per participant per batch. Tables give pooled nearest-rank p50 / p95 in milliseconds and median call and response-byte counts. RSS used a separate process per batch, a 200 ms idle stabilization and 20 ms sampling intervals. Both RSS percentiles use just two observations, and samples may miss short peaks.

### Synthetic inventory

One caller read 31 stacks with page size seven. The Core 2.3.2 wire fixture modeled non-admin Read on every stack and associated server and rejected writes. All five MCP pages agreed with the independent ID, name and state oracle. There was no added fixture delay. HTTP handler service time across all 1,084 inventory-run events was 0.26 / 0.96 ms.

| Participant | Startup ms | Warm inventory ms | MCP calls | Core calls | MCP bytes | Core body bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Candidate | 17.38 / 51.59 | 178.16 / 206.04 | 5 | 9 | 14,734 | 41,380 |
| Rust reference | 15.62 / 18.31 | 4.28 / 6.02 | 5 | 5 | 27,074 | 23,323 |
| TypeScript | 583.05 / 680.54 | 20.69 / 24.95 | 5 | 5 | 8,614 | 116,595 |
| Direct Core | — | 2.84 / 3.11 | — | 5 | — | 23,323 |

Startup spans process spawn through the matching initialize reply. The candidate made one upstream request, TypeScript made two, and the Rust reference made zero. Direct `GetVersion` took 0.39 / 0.70 ms. During warm inventory the candidate probed the following Core page to report `has_more`. TypeScript fetched all 31 stacks on each cursor call. The Rust reference requested one Core page per MCP call and returned full JSON text. These differences account for the distinct call counts and payload sizes. [Inventory protocol and accounting](../README.md#workload-and-equivalence)

### Synthetic incident triage

Two callers shared one initialized MCP process. Each had to identify the affected stack and service, failed update ID and stage, and matching log excerpt from the same visible fleet. Page size was three. The candidate also collected fleet health, service and alert data. TypeScript inspected containers, and the independent Rust reference used typed Core reads for stack discovery, service status, history and logs. All three produced the expected evidence in every measured caller. Calls and bytes below are totals per two-caller group.

| Stacks | Participant | Group ms | Caller ms | MCP calls | Core calls | MCP bytes | Core body bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 7 | Candidate | 316.87 / 337.04 | 306.97 / 336.84 | 14 | 92 | 41,540 | 42,440 |
| 7 | Rust reference | 6.84 / 9.27 | 6.60 / 9.14 | 14 | 14 | 16,950 | 13,788 |
| 7 | TypeScript | 38.66 / 51.56 | 37.39 / 51.24 | 20 | 20 | 17,880 | 36,860 |
| 31 | Candidate | 729.14 / 828.02 | 719.12 / 820.20 | 30 | 284 | 116,210 | 144,216 |
| 31 | Rust reference | 17.68 / 23.50 | 17.41 / 23.24 | 30 | 30 | 60,100 | 50,332 |
| 31 | TypeScript | 80.00 / 99.70 | 78.30 / 99.41 | 36 | 36 | 33,922 | 523,228 |

Growing the fleet from seven to 31 stacks increased candidate median group latency from 317 to 729 ms and its upstream calls from 92 to 284. TypeScript transferred the most upstream bytes at 31 stacks. The Rust reference returned the smallest group times at both sizes. Direct HTTP has no equivalent full diagnostic workflow in this harness.

### Memory

Each cell gives idle p50 / p95, then peak p50 / p95 in MiB for the directly launched server process. The driver, synthetic Core and disposable services are outside these boundaries.

| Participant | Inventory, 31 stacks | Incident, 7 stacks | Incident, 31 stacks |
| --- | ---: | ---: | ---: |
| Candidate | 5.45 / 5.51 · 87.86 / 90.30 | 5.38 / 5.43 · 155.95 / 156.84 | 5.40 / 5.45 · 156.89 / 159.20 |
| Rust reference | 3.73 / 3.75 · 5.19 / 5.20 | 3.74 / 3.79 · 5.26 / 5.27 | 3.72 / 3.84 · 5.30 / 5.30 |
| TypeScript | 98.51 / 98.82 · 146.84 / 146.87 | 98.64 / 98.78 · 169.96 / 170.42 | 98.37 / 98.38 · 174.95 / 178.30 |

### Live Core inventory

Docker Desktop ran an owned Core 2.3.2 and Periphery 2.3.2. An independent provisioner verified the seven expected stack IDs, names and `down` states before and after measurement. The non-admin benchmark reader had Read on each selected stack. The three MCP servers and direct Core each returned the complete inventory at page size three with one caller. Each participant produced 20 measured reads. Teardown verified zero project containers, networks and volumes.

| Participant | Warm inventory ms | Calls | Response bytes | Measured boundary |
| --- | ---: | ---: | ---: | ---: |
| Candidate | 440.85 / 471.35 | 3 | 3,750 | MCP frames |
| Rust reference | 200.98 / 224.27 | 3 | 5,521 | MCP frames |
| TypeScript | 210.86 / 221.60 | 3 | 2,251 | MCP frames |
| Direct Core | 202.86 / 216.82 | 3 | 4,586 | HTTP bodies |

MCP-to-Core traffic, live startup and live RSS were uninstrumented. The direct baseline timed real `/read` requests, parsing and projection. Live incident triage was untested for all participants because this fixture contained no seeded rollout failure. Authenticated Streamable HTTP, other platforms, production fleets and larger real inventories fall outside these timing results. [Disposable regression and compatibility matrix](../../docs/v0-9-0-compatibility.md)

### Reproduce

Obtain the selected native artifact while it is retained, or use a preserved copy matching the receipt. For the measured host, start in the benchmark checkout with Cargo and rustc 1.99.1, Node 24.18.0, npm 11.16.0 and Python 3.13.14 on `PATH`.

```sh
set -eu
mkdir -p target/benchmark
bundle=$(mktemp -d "$PWD/target/benchmark/native-XXXXXX")
export CANDIDATE_RECORD="$PWD/docs/releases/v0.9.0/candidate-record.json"
revision=$(jq -r '.distribution.revision' "$CANDIDATE_RECORD")
export NATIVE_TARGET=x86_64-apple-darwin
name=komodo-mcp-v0.8.0-x86_64-apple-darwin
gh run download 45539030427 --repo enthouan/komodo-mcp \
  --name "komodo-mcp-distribution-$revision" --dir "$bundle"
archive_sha=$(jq -er --arg target "$NATIVE_TARGET" '.distribution.files[] | select(.target == $target) | .sha256' "$CANDIDATE_RECORD")
printf '%s  %s\n' "$archive_sha" "$bundle/$name.tar.gz" | shasum -a 256 -c -
tar -xzf "$bundle/$name.tar.gz" -C "$bundle"
export EXTRACTED_BINARY="$bundle/$name/komodo-mcp"
export DOCKER_CONTEXT=desktop-linux
export HOST_NOTE='Record actual hardware, load and power policy'
```

Run the two-batch inventory, incident, disposable Core, validation and export commands in the [packaged benchmark instructions](../README.md#shared-packaged-candidate). The public sample file includes a `summary` per participant and distribution, including nearest-rank p50/p95. Full reports and manifests remain in `target/benchmark/`.

## Historical synthetic measurements

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
