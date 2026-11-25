# Inventory and incident benchmarks

This sequential, stdio benchmark compares the Rust release executable with
[MP-Tool/komodo-mcp-server 1.5.0](https://github.com/MP-Tool/komodo-mcp-server/tree/489f7e04b16019480f84e1d2fff8b872b880e335),
commit `489f7e04b16019480f84e1d2fff8b872b880e335`. It measures a complete visible
stack inventory against a loopback Core wire fixture. Neither server is patched.
Initial measurements and any performance targets are **provisional** until
repeated runs on controlled hosts establish variance. There are no latency/RSS
pass/fail thresholds.

The original `run` inventory workload and `komodo-inventory-benchmark/v1` report
remain available. The additive `incident` workload measures broken-stack triage,
including concurrent callers and fleet growth, using the same prepared artifacts,
stdio client, HTTP instrumentation, reference pin and RSS sampler boundaries.

## Incident triage (#10)

Prepare once with the command below under **Reproduce**, then run:

```sh
python3 benchmarks/bench.py incident --fixture-size 7 --page-size 3 \
  --concurrency 2 --warmup 2 --samples 10 --repetitions 2 \
  --output target/benchmark/incident.json \
  --host-note "Local desktop; shared workload; power policy uncontrolled"
python3 benchmarks/bench.py validate target/benchmark/incident.json
python3 benchmarks/bench.py summarize target/benchmark/incident.json
```

The seed is shared with the Rust reader and stdio integration tests:
[`incident.json`](../tests/fixtures/core-2.3.2/incident.json) defines the stable
IDs and expected evidence, and
[`incident-wire.json`](../tests/fixtures/core-2.3.2/incident-wire.json) contains
Core 2.3.2 wire responses. `alpha-web` has unhealthy service `web` after failed
update `100000000000000000000001`, stage `Deploy Stack`; its log contains
`web | ERROR: database migration missing: relation orders does not exist`.
The seed also includes healthy workloads, a server returning HTTP 503, and an
unknown server. The synthetic credential sentinel is labeled `TOKEN=` and is
excluded from recorded projected evidence. Generated resources are healthy and
use deterministic distinct IDs; the seeded incident remains unchanged.

Each Rust caller exhausts **all `fleet_health` pages**, selects the unhealthy
stack, calls `stack_diagnose`, follows the returned update ID with `get_update`,
and independently requests `get_stack_logs` filtered to the affected service
with `tail: 40` and `max_bytes: 4096`. It verifies the complete stack count and
digest of every discovered stack's ID/name/state,
stable stack ID/name, service, failed update ID/stage and error log excerpt.

The unmodified pinned reference has no fleet health, stack diagnosis, alerts or
stack service-state tools. Its comparable workflow exhausts `komodo_stack_list`,
selects the unhealthy stack, reads `komodo_stack_info` to find its server/project,
lists that server's containers, and inspects each target container to identify
the unhealthy Compose service from Docker health and labels. It then follows
`komodo_update_list` → `komodo_update_info` and retrieves that container's bounded
tail with `komodo_container_logs`. Info/inspect/log calls use `inline_full: true`
so all evidence travels through the measured stdio responses. Both workflows
must produce the same independently defined projection. Rust's additional server
metrics, alerts and diagnosis are **outside the common equivalence claim**;
reference full configuration/update payloads still count toward its actual wire
bytes. These are equivalent seeded service/stage/log findings, with different
tool capabilities and work performed.

`--concurrency` (1..16, default 1) runs independently progressing logical
workflows through one persistent initialized MCP process. Each measured group
records wall time, individual caller times, summed **actual matched MCP response
frame bytes/calls**, all observed Core calls and response-body bytes, operation
counts, and peak observed simultaneous Core handlers. Core events are attributed
to the **whole concurrent group**, because neither server propagates a caller ID
to Core. No per-caller Core count is fabricated. Every caller must pass the
oracle; one failed workflow cancels/drains its siblings and fails the run.

Latency and RSS use separate process sessions. RSS runs the same concurrent
workflow, recording idle RSS, raw timestamped observations and the sampled maximum
from spawn through workload completion before shutdown. Summaries retain group
and caller warm p50/p95, per-batch group distributions, response bytes, MCP/Core
counts and RSS. Equality comparison and report writing occur outside the measured
group/caller intervals. There are no cold-start or direct-Core incident samples;
those remain part of the retained inventory workload.

`--fixture-size` accepts 2..1000 stacks, with that count plus two Servers;
`--page-size` accepts 1..100. Larger fleets exercise additional discovery pages
and bounded per-resource health reads while preserving the seed. For example:

```sh
for size in 7 31 125; do
  for callers in 1 4; do
    python3 benchmarks/bench.py incident --fixture-size "$size" --page-size 25 \
      --concurrency "$callers" --warmup 2 --samples 10 --repetitions 2 \
      --output "target/benchmark/incident-${size}-${callers}.json.gz" \
      --host-note "Describe the same hardware, power policy and load for every run"
  done
done
```

Reports use `komodo-incident-benchmark/v1` and retain artifact/runtime/source
hashes, host/control definitions, fixture/oracle hashes and full expected
projection, raw group/caller samples, all HTTP events (including startup,
shutdown, memory and background health), raw RSS and recomputed summaries.
`validate` rejects incomplete comparisons, wrong oracle findings, missing callers,
incorrect aggregate byte/call counts, unexpected fixture failures, mutations,
invalid RSS and inconsistent summaries. The seeded server's 503 reads are expected
and retained. Real prepared-artifact tests run both stdio adapters with two
concurrent callers and 7/31-stack fleets. CI retains the inventory smoke and adds
7/31-stack incident comparisons. CI validates each report and publishes the
aggregate measurements in the workflow run summary.

These are synthetic loopback wire fixtures with simulated Read permissions;
they do not run Core's database, authorization engine, Docker or an actual
rollout. Fleet-growth and concurrent smoke measurements establish repeatability
and correctness. Small shared-host samples do not establish a stable performance
ranking or production scaling limits.

## Reproduce

Prerequisites: Linux or macOS, Python 3.11+ (standard library only), Git, the
repository's pinned Rust 1.99.1 toolchain, Node.js >=22 and npm. Use Node
**24.18.0**, npm **11.16.0**, and Python **3.13.14** to reproduce the published
baseline. The CI smoke runs use the Node version in `.nvmrc`. macOS uses
`/bin/ps`. Linux needs readable `/proc`.
The runner rejects unsupported platforms or unavailable RSS rather than writing
zeroes. Preparation needs network access; measurement needs only loopback.

From a checkout, prepare both implementations **before** measurement:

```sh
python3 benchmarks/bench.py prepare --reference-dir "/tmp/komodo reference 1.5.0"
```

`prepare` clones the exact commit into that isolated location, verifies the
`1.5.0` tag and clean source, runs `npm ci --no-audit --no-fund` with the committed
lockfile, then `npm run build` (`tsc` and asset copy). An existing reference must
already be clean and at the pin; it is never reset or patched automatically.
It builds Rust with `cargo build --locked --release --bin komodo-mcp`, plus
explicit `--target <rustc host>` and `--target-dir <checkout>/target` arguments.
The executable is `target/<host>/release/komodo-mcp`; Cargo release defaults
apply unless the recorded build environment overrides them. This separate
target path prevents inherited output/target settings from selecting a stale
executable. `--node`, `--npm`, `--cargo`, and `--rustc` accept installed executable
paths, including paths containing spaces. The measured Node path is resolved
through `process.execPath`.
The Rust compiler is resolved through `rustc --print sysroot` so toolchain-manager
shims cannot lose the selected version when Cargo compiles dependencies elsewhere.

Preparation writes `target/benchmark/artifacts.json`, including commands,
source/build identities, toolchain versions, actual executable size/hash,
reference lockfile hash, built-tree hash and installed dependency-tree hash.
Keep the reference, Node executable, release executable and manifest together.
Preflight verifies them before starting any timers. Missing or changed artifacts
fail with preparation instructions; there is no Rust-only fallback.

One command runs the complete comparison using those prepared artifacts:

```sh
python3 benchmarks/bench.py run --host-note "Describe hardware/VM/container, power policy and other host load"
```

The default workload has **125 stacks, page size 25, 5 warmups, 30 warm samples,
10 fresh-process startup samples, and 3 batches per implementation**. Orders are
Rust/reference, reference/Rust, Rust/reference. Direct Core baselines run after
both implementations in each batch. The result is
`target/benchmark/result.json`. The process prints a summary only after both
implementations and direct Core pass equivalence and result validation.

Example with a different fixture, explicit zero delay and smaller sample counts:

```sh
python3 benchmarks/bench.py run --fixture-size 51 --page-size 25 \
  --warmup 2 --samples 10 --cold-samples 3 --repetitions 2 \
  --fixture-delay-ms 0 --output target/benchmark/small.json \
  --host-note "Local desktop; shared workload; power policy uncontrolled"
```

`--fixture-delay-ms` adds the same delay to **every** version/read HTTP response
for both servers and the direct baseline, including background health requests.
Zero injects no sleep. `--timeout` (default 20 seconds) bounds initialization,
individual MCP requests and each complete logical MCP read. Direct HTTP requests
have a 20-second deadline. Pagination is bounded by fixture size/page size plus
one terminal page. EOF shutdown gets 0.5 seconds, then SIGTERM gets 3 seconds,
then SIGKILL gets 3 seconds; readers also have a 3-second drain deadline. Failed
exchanges terminate/reap the process group. SIGINT/SIGTERM use the same cleanup.
There are no successful-sample retries or outlier removal. A measurement failure
writes `status: failed`, partial samples, HTTP events and an error; it exits
nonzero. Preflight failures print an actionable diagnostic before a run exists.

Run correctness checks and regenerate a summary without rerunning measurements:

```sh
python3 -m unittest discover -s benchmarks -v
python3 -m py_compile benchmarks/*.py
python3 benchmarks/bench.py validate target/benchmark/result.json
python3 benchmarks/bench.py summarize target/benchmark/result.json
```

Tests with prepared artifacts exercise the **actual two servers** for empty,
exact-boundary and partial multi-page inventories. Without preparation only the
real-server integration case is skipped; CI always prepares first. Unit tests
also inject mismatches, pagination/tool errors, malformed stdout, startup and
request timeouts, cancellation, large stderr, accounting phase changes, missing
reference artifacts and incomplete/invalid result data.

For a later Rust release, check out that revision (or apply this benchmark
directory to a release checkout), run `prepare` there with a separate artifact
manifest, then `run --artifacts <manifest> --output <new-result>`. Keep the
reference pin, host, controls and environment the same. Preparation intentionally
rebuilds the source in its checkout; it does not attribute arbitrary prebuilt
binaries to the current revision. Do not overwrite old evidence. If the runner,
fixture, workload, dependencies or executable changes, collect new measurements.

## Workload and equivalence

Rust calls `list_stacks` with `{page: 0, page_size: 25}`, follows `next_page`, and
requires consistent `has_more`/page metadata. The reference calls
`komodo_stack_list` with `{page_size: 25}`, then passes each returned opaque
`page.next_cursor` unchanged. Both exhaust all pages, reject duplicate IDs and
check exact membership/count, IDs, names and normalized states against the
fixture oracle. Unknown/missing states, malformed results, tool errors, repeated
cursors, nonfull continuation pages and nontermination fail the comparison.

The common projection is `{id, name, state}`. All Core 2.3.2 Stack states map
identically from both outputs: `deploying → deploying`, `running → running`,
`paused → paused`, `stopped → stopped`, `created → created`,
`restarting → restarting`, `dead → dead`, `removing → removing`,
`unhealthy → unhealthy`, `down → down`, `unknown → unknown`. `unknown` is an
explicit enum value; other unrecognized values are rejected. The generator
cycles through all eleven states. Sorting by ID happens only for oracle
comparison/digest, after timing; records are never dropped to force agreement.

`core.py` expands the existing synthetic
[`core-2.3.2/stacks.json`](../tests/fixtures/core-2.3.2/stacks.json) template with
deterministic IDs, names and states. The independently defined expected
inventory is archived in full, with a digest repeated in each verified sample.
The fixture signals readiness, binds `127.0.0.1:0`, checks both synthetic
credential headers, and simulates a non-admin `Read` grant on every generated
stack and its associated server. It supports authenticated envelope `POST /read`
(Rust/direct), parameter-body `POST /read/<operation>` (reference), and public
`GET /version`. Name sorting, names/terms/tags/templates filtering and page/limit
semantics (including `limit: 0`) are supported. Non-default specific filters and
non-Name sorts are rejected because this workload does not exercise them.
Unexpected reads and all mutation/execution requests are rejected.

These are **synthetic wire fixtures and simulated permissions**. They do not run
Core's database, authorization engine, Docker, networking or deployment logic,
and establish neither deployed Core performance nor authorization behavior.
The ordered Rust `Step` mock and its assertions remain unchanged. The installation
workflow in #4 / PR #56 supplies a Rust mock/client for its demonstration. It was
not merged when this harness was implemented; this driver uses the existing wire
fixture foundation with Python phase instrumentation and the pinned reference's
additional HTTP route format. The benchmark's separate CI workflow avoids a
merge-order dependency on the installation demonstration.
The additive incident workload above extends this foundation for #10. A broader
reference/version matrix remains separate work.

## Measurement boundaries

| Metric | Boundary and units |
| --- | --- |
| Fresh-process startup | `perf_counter_ns` immediately before spawning the actual executable until a parsed and validated successful `initialize` response; milliseconds. Includes runtime/application startup and startup Core requests. Sends `notifications/initialized` immediately afterward, outside the timer. New process for every sample; filesystem caches are uncontrolled. Negotiates `2025-03-26` with both servers. |
| Warm logical read | Persistent initialized process, configured untimed complete warmups, then complete sequential inventory reads including all MCP page calls, JSON response parsing, shape checks and common projection; milliseconds. Oracle sorting/comparison, phase markers, reporting and file writes are outside the timer. Normal server HTTP connection pooling stays active. |
| MCP response size | Actual matched stdout response-frame lengths, UTF-8 bytes including JSON-RPC envelope and terminating LF, summed across all pages. Preserves structured and text output. Notifications and stderr have separate byte counts in process diagnostics. Request ID length can vary the size slightly during a session. |
| Core requests | Observed at the HTTP handler boundary with implementation, batch, phase, sample, operation, route, params, status, body bytes and service time. Counts are derived from these events. Version calls during nonstartup phases are labeled background health and retained, including in phase totals. |
| Direct Core | Actual authenticated, Name-ascending `ListStacks` with the same page size to the first short page (an extra empty read on an exact boundary). One `HTTPConnection` per batch, reused across warmups/samples; first warmup establishes TCP (first timed sample if warmups=0). Includes parsing/projection and verifies the same oracle. Also records direct authenticated `GetVersion` timings, using a fresh baseline connection per batch. Bytes here are HTTP bodies, excluding headers. |
| Fixture latency | Handler entry through response write/flush, milliseconds; configured delay included, TCP accept/client receive excluded. Server sockets use TCP_NODELAY to avoid fixture-induced Nagle/delayed-ACK stalls. Every raw request retains service time; aggregate p50/p95 is also reported. |
| Idle RSS | Dedicated fresh server process after initialize and `--idle-ms` stabilization (default 200 ms), before warmup/read workload. Server PID only, bytes; excludes driver/mock. |
| Peak sampled RSS | Same dedicated process, sampled through startup, idle, warmups and the configured measured-count complete reads, ending before shutdown. Default `--rss-interval-ms 20` is sleep between observations, not a guaranteed scheduling period; actual timestamps are retained. Linux reads `/proc/PID/status` VmRSS; macOS `ps -o rss=` returns 1024-byte units, converted to bytes. A sampled maximum can miss short peaks; Linux VmRSS itself is approximate. No cumulative child-resource counter is used. Direct executable launches add no wrapper children. |
| Rust executable size | Actual file length in bytes plus SHA-256, explicit target, release profile, compiler and selected build environment. No JavaScript entry-file size is presented as an equivalent native executable size. |
| p50/p95 | Nearest rank: sort all retained samples and select `ceil(p * n) - 1`. Pooled summaries and per-batch warm summaries are included. Small smoke runs establish correctness, not a meaningful latency distribution. |

The RSS sampler runs in a **separate session** to avoid injecting `ps`/sampling
work into latency results. All those sessions' Core requests are retained under
`memory_*` phases. The startup, warm-startup, warmup, measured warm, shutdown,
memory and direct phases remain distinguishable. Phase is captured on HTTP
handler entry; phase changes wait for in-flight handlers. Runs use a fresh empty
temporary HOME/cwd/config file and an environment allowlist, excluding inherited
Komodo credentials, proxies, Node injection options and unrelated configuration.
Both log at `warn`; stderr is drained concurrently, counted, bounded and redacted.

The reference has its full tool registry, dynamic-resource infrastructure,
scrubbing and 30-second health monitor enabled. Rust makes an authenticated
startup version request. The reference pings public `/version`
and then authenticated `GetVersion` at startup. The original Rust
baseline had six inventory tools; current runs initialize the current registry.
Rust probes full Core pages to
find continuation; the reference fetches **all stacks again for every MCP page**
and paginates locally. Rust returns richer stack summaries and JSON text;
the reference returns compact summaries and rendered text. Equivalent projected
inventory does not imply identical functionality, Core work or wire payloads.
Direct timings explain fixture cost alongside end-to-end times; subtracting
them does not precisely isolate internal server overhead.

## Local reports and published findings

Keep raw reports and preparation manifests in the ignored `target/benchmark/`
directory. They include local executable paths, command lines and host details.
Commit only a short [findings summary](results/README.md) with the workload,
measured revision, aggregate results and limitations. Run
`python3 benchmarks/check_reports.py` after staging a benchmark change to check
what will enter Git.
A reporting commit can be later than the measured implementation commit: `repository`
records the runner checkout; `artifacts.rust.repository` records the source
at build time; dirty flags/status and source/file hashes identify uncommitted
changes accurately. Never relabel a measurement with a later, unmeasured revision.

`komodo-inventory-benchmark/v1` is a JSON object with these stable sections:

| Section | Contents |
| --- | --- |
| Identity | `run_id`, UTC `timestamp`/`finished_at`, `status`, `repository`, `runner` version/file hashes, `invocation` argument array. |
| `artifacts` | Preparation command/working-directory arrays, Rust source revision/dirty state/lock/binary hash and bytes/toolchain/profile/target, reference repository/release/revision/lock/build/dependency hashes and resolved Node/runtime version. |
| `host` | OS, architecture, CPU, logical CPU count, total host memory, Python version, explicit virtualization/load note; Linux container/cgroup observations. |
| `fixture`, `workload`, `controls`, `definitions` | Fixture seed/wire/oracle identities, resource count, simulated permission model, exact expected records, tool arguments, protocol, logging/environment policy, delay/scope, warmup/sample/batch/order controls and metric units/boundaries. |
| `implementations` | Every cold/warmup/warm raw sample, persistent initialization evidence, dedicated RSS observations/timestamps and process diagnostics. Complete reads include MCP calls, wire bytes, observed Core counts/operations and oracle evidence. |
| `direct_core`, `direct_version`, `core_requests` | Raw direct timings/coverage/bytes plus every instrumented fixture request, including background/untimed activity. |
| `summary`, `errors` | Recomputed nearest-rank statistics, per-batch warm distributions, or explicit failure diagnostics. Successful archives require both implementations and all expected samples. |

`validate` checks required metadata, complete sample identities, oracle digests,
observed accounting, finite metrics, RSS observations and recomputed summaries.
`summarize` regenerates the text table from raw samples after validation. CI
builds/prepares first, tests both adapters, runs 7 stacks/page size 3, 1 warmup,
2 warm samples, 1 startup sample and 2 alternating batches, validates required
evidence and writes the validated summaries to the GitHub Actions run.
The smoke check requires both implementations to complete the workload correctly.

Archive files may use `.json.gz`: `run --output <file>.json.gz`, `validate` and
`summarize` handle gzip transparently using the same JSON format. `gzip -dc`
exposes the complete raw JSON for other tools; compression/report writes happen
after measurement.

Sources for the boundaries and adapters:

- [MCP lifecycle](https://modelcontextprotocol.io/specification/2025-03-26/basic/lifecycle)
  and [stdio transport](https://modelcontextprotocol.io/specification/2025-03-26/basic/transports).
- [Python subprocess lifecycle](https://docs.python.org/3/library/asyncio-subprocess.html),
  [Linux VmRSS](https://man7.org/linux/man-pages/man5/proc_pid_status.5.html),
  and macOS's installed `man ps` (`rss` is in 1024-byte units).
- [Pinned stack handler](https://github.com/MP-Tool/komodo-mcp-server/blob/489f7e04b16019480f84e1d2fff8b872b880e335/src/tools/stack.ts),
  [pagination](https://github.com/MP-Tool/komodo-mcp-server/blob/489f7e04b16019480f84e1d2fff8b872b880e335/src/utils/pagination.ts),
  [startup client](https://github.com/MP-Tool/komodo-mcp-server/blob/489f7e04b16019480f84e1d2fff8b872b880e335/src/client.ts)
  and [dependency lock](https://github.com/MP-Tool/komodo-mcp-server/blob/489f7e04b16019480f84e1d2fff8b872b880e335/package-lock.json).

## Shared packaged candidate

Use the same verified candidate record and extracted binary as disposable regression. The benchmark checkout must contain the packaged source commit in Git history. The timed server is the extracted binary. The separate Rust comparator is [Kalaghni/komodo-mcp 0.1.0](https://github.com/Kalaghni/komodo-mcp/tree/2efbe2232d98ac0c7eec90135fd683b5b39c1cca). Its checked-in lock, optimized thin-LTO release profile, read-only stdio transport and Core 2.3.2 inventory and history APIs make both workloads reproducible. The TypeScript comparator retains its original [1.5.0 pin](https://github.com/MP-Tool/komodo-mcp-server/tree/489f7e04b16019480f84e1d2fff8b872b880e335). Keep preparations in a private work directory on a separate benchmark host.

Set `CANDIDATE_RECORD`, `EXTRACTED_BINARY` and `NATIVE_TARGET` from the shared bundle. Set `HOST_NOTE` to the machine and power conditions and `DOCKER_CONTEXT` to a local Unix-socket Docker context. Run from the checkout root with the selected Cargo, Rust, Node and npm on `PATH`.

```sh
set -eu
umask 077
mkdir -p target/benchmark
study=$(mktemp -d "$PWD/target/benchmark/packaged-XXXXXX")
manifest="$study/artifacts.json"
python3 benchmarks/bench.py prepare --artifacts "$manifest" \
  --candidate-record "$CANDIDATE_RECORD" --candidate-binary "$EXTRACTED_BINARY" \
  --candidate-target "$NATIVE_TARGET" --reference-dir target/benchmark/typescript-ref \
  --rust-reference-dir target/benchmark/rust-ref
BENCHMARK_ARTIFACTS="$manifest" python3 -m unittest discover -s benchmarks -v
python3 benchmarks/bench.py run --artifacts "$manifest" --fixture-size 31 --page-size 7 \
  --warmup 2 --samples 10 --cold-samples 5 --repetitions 2 --host-note "$HOST_NOTE" \
  --output "$study/inventory.json.gz"
for size in 7 31; do
  python3 benchmarks/bench.py incident --artifacts "$manifest" --fixture-size "$size" \
    --page-size 3 --concurrency 2 --warmup 2 --samples 10 --repetitions 2 \
    --host-note "$HOST_NOTE" --output "$study/incident-$size.json.gz"
done
project="komodo-mcp-smoke-bench-$(python3 -c 'import uuid; print(uuid.uuid4().hex[:12])')"
python3 benchmarks/disposable_inventory.py --artifacts "$manifest" --candidate-record "$CANDIDATE_RECORD" \
  --candidate-binary "$EXTRACTED_BINARY" --candidate-target "$NATIVE_TARGET" --fixture-size 7 \
  --docker-context "$DOCKER_CONTEXT" --project "$project" --evidence "$study/live-fixture.json" \
  --output "$study/live.json.gz"
for report in "$study"/*.json.gz; do python3 benchmarks/bench.py validate "$report"; done
python3 benchmarks/export_samples.py --output "$study/public-samples.json" \
  "$study/inventory.json.gz" "$study/incident-7.json.gz" "$study/incident-31.json.gz" "$study/live.json.gz"
python3 benchmarks/check_reports.py
```

Set the candidate target to the native archive for the actual host. Preparation verifies that the candidate, Rust comparator and Node runtime match the host architecture. Linux permits a native GNU or musl comparator with the static musl candidate. Preparation also checks the shared receipt, Core/OCI relationship, native executable hash, candidate Cargo manifest and repository lock at that commit. Each run hashes the participating executables and validates the reference builds before starting timers. The candidate uses the distribution workflow's frozen release with stripped symbols. The Rust comparator uses its locked release build on the host toolchain. TypeScript runs the built JavaScript directly on the selected Node executable. The public export gives separate bytes for each standalone executable, the TypeScript build tree, installed dependencies and Node runtime. The last two tree sizes count regular files and exclude symbolic links. Throughput and memory on another platform can differ.

The comparator's general read tool accepts explicit Core request parameters. For inventory the driver pages `ListStacks` because its dedicated `komodo_list` omits page and limit. For incident triage it pages the same full list, resolves the unhealthy service from `ListAllStackServices`, checks `ListUpdates` and `GetUpdate` for that Stack, and reads `GetStackLog` for the affected service. The native comparator returns serialized JSON inside MCP text. The driver counts the complete JSON-RPC frame and checks the independent oracle after parsing. Truncated JSON, incomplete discovery, duplicate IDs and mismatched stage or log evidence invalidate the run. Candidate fleet-wide probes and the TypeScript Docker inspection sequence do additional work. Timing order rotates across the three implementations.

The live disposable runner provisions seven owned Stacks and grants its reader Read on each. It verifies the provisioner's ID, name and state oracle before and after measurement and checks teardown. Each of two batches measures ten warm inventory reads following two warmups at concurrency one and page size three. It measures MCP frame bytes and calls for the MCP participants and HTTP body bytes and calls for direct Core reads. Core traffic from the live MCP participants is uninstrumented. Live incident results require independent verification of every seeded diagnostic signal. Full reports and preparation manifests stay under `target/benchmark/`.
