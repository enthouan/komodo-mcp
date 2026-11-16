# One command through a private terminal

`execute_terminal` requires **both** `KOMODO_ALLOW_TERMINAL=true` and
`KOMODO_ALLOW_OPERATE=true` at process startup. Terminal access defaults off and
is independent of configuration editing and Docker pruning. Discovery, lookup,
handler and the closed terminal transport enforce this boundary. Generic
configuration writes cannot create/delete terminals.

Core must grant the same enabled caller **Read + Terminal** on the original
resource. Execute or Write alone does not grant the Terminal specific permission.
Container targets additionally require **Inspect** on that original resource
to establish a fresh complete Docker identity. Stack/Deployment paths use their
own inspection API, never a Server inspection fallback after denial. The saved
Server definition must also be readable to bind its private connection baseline.
No administrator or provisioning credential fallback is used.

## Exact targets

Each resource selector accepts exactly one stable 24-hex `id` or exact `name`.

| `target.kind` | Additional fields | Execution context |
| --- | --- | --- |
| `server` | `target` resource selector | Periphery runtime context on that Server; a containerized agent is not the physical host namespace |
| `container` | `server` selector and `container: {"id":"<64-hex Docker ID>"}` or `{"name":"exact-name"}` | One freshly inspected running container, pinned to its complete ID |
| `stack` | `target` selector and exact `service` | One unique deployed service container of a Server-backed Stack |
| `deployment` | `target` selector | The freshly inspected current container of a Server-backed Deployment |

Stacks require a unique cached service, matching complete fresh container ID,
name and Compose project/service labels. Deployments require the actual deployed
container name. Unknown, duplicate, stale or unreadable identities fail closed.
Missing or malformed container label metadata is incomplete evidence, rather
than proof that a container is outside Swarm. Pinned Core's complete empty label
map remains supported. Swarm-backed parents and containers with Docker Swarm task/service/node labels
are unsupported on every path. Docker ID prefixes, arbitrary target variants,
fleet execution and fallback targets are unsupported.

For example, send one tool call after resolving the intended disposable target:

```json
{"name":"execute_terminal","arguments":{"target":{"kind":"container","server":{"id":"000000000000000000000001"},"container":{"name":"owned-check"}},"command":"printf 'hello\\n'","observation_seconds":10,"max_output_bytes":4096}}
```

Commands are one nonempty physical line, at most 4096 UTF-8 bytes, without control
characters or physical CR/LF. The fully escaped command **plus Periphery's
status envelope and newline** must also fit within 4095 bytes for its canonical
Linux PTY. Literal apostrophes expand during quoting, so even a shorter original
line can be rejected before any Core I/O. Shell metacharacters have shell meaning. The tool
wraps the line in a separately quoted `sh -c` to keep trailing comments and `exit`
from consuming Core's appended status marker. It sends **one** authenticated
`POST /terminal/execute` with an explicit owned name and `init: null`. It never
replays a command, create or delete after an uncertain reply. No Core update ID
exists for this endpoint.

## Ownership and lifetime

Every call allocates an unpredictable private name and a separate private
initializer nonce using the operating system's `/dev/urandom`; unavailable
entropy fails closed. Its in-memory ownership proof binds the MCP process/call,
authenticated caller, exact target, full private saved parent/Server definitions,
effective permission record, initializer, name and creation timestamp. No
ownership journal is adopted on restart. A new call always gets a fresh shell;
there is no persistent interactive state or cross-call environment.

Before creating, the tool checks absence in the exact namespace using a bounded
name-filtered list. A Stack check covers its whole terminal namespace because
Core's matching ignores the service in that namespace. Creation uses **Never**
recreate and container **exec** mode. The tool checks the acknowledgement and an
independent list, including the full private initializer identity. Foreign,
default, replaced or unverified shells are preserved. There is no attach mode,
user-provided terminal name, initializer, recreation setting or batch deletion.

Complete saved routing, caller and effective authority are re-read before
creation, command and cleanup. Fresh container identity is rechecked before
creation/command. Cleanup does not require the container still to be running,
but it refuses changed saved routing or authority rather than deleting through
a different current parent Server. Raw private baselines are capped at 256 KiB;
evidence HTTP bodies retain the shared 2 MiB cap. Lists request only the private
name, page zero and a two-entry limit; incomplete/collision proof fails closed.
These bounds do not bound Core's own enumeration work.

The fixed initializer uses `timeout -k 1 90 sh ...` and a private line reader.
Both `timeout` and `sh` must exist in the selected runtime/container. A missing
utility fails creation before the application command, with **no unbounded-shell
fallback**. The 90-second limit starts when the shell starts; it does not bound
arbitrary Core queue delay. An escaped child or a command interfering with the
timeout/process group can outlive that shell. There is no process-tree rollback
or guaranteed descendant termination.

Up to **four active calls** are allowed per MCP process; additional calls are
rejected without queuing. Resolution/final guards, lifecycle HTTP requests,
confirmation and cleanup have explicit budgets. `observation_seconds` is 1–30
(default 15), including the final command guard and HTTP stream. Cleanup has a
separate ten-second total budget. Cancellation or a dropped RPC signals a
supervised worker; stdin shutdown cancels workers and drains cleanup for up to
25 seconds before the runtime exits. A hard process kill cannot run local
cleanup; the remote lifetime wrapper remains the normal-shell limit.

Core/Periphery **do not provide atomic reservation, expected container ID or
terminal identity CAS**. Random names, a private initializer nonce, Never
recreate and repeated exact identity checks protect ordinary separate callers;
they cannot exclude a concurrently privileged upstream replacer between check
and request. Stack/Deployment Core paths ultimately resolve current names, so
there is also a final container-name/reassignment race. Saved/inspect checks
reduce it without claiming atomic targeting.

## Reading the result

`attempts.create`, `attempts.command` and `attempts.delete` count HTTP submissions
separately. A final guard rejection is zero command submissions. An attempted
command with a lost response remains one, and is never repeated automatically.

`observation.end` describes the local observation. Only one exact final
`__KOMODO_EXIT_CODE:<0–255>` line yields `reported_exit_code`. Nonzero is a valid
reported result. **This marker is spoofable by arbitrary command output**; it is
upstream-reported evidence, not independent proof of completion, service health,
or successful effects. EOF without a marker does not imply success. Deadline,
cancellation, transport failure, malformed/duplicate/nonfinal markers and invalid
UTF-8 leave remote completion unknown; no exit status is invented.

The complete stream is capped at **65,536 raw bytes** before UTF-8 decoding.
Split network frames and UTF-8 code points are assembled before parsing. Invalid,
incomplete, interrupted, oversized or malformed streams omit **all** partial
output. Complete output has terminal controls removed, known API credentials
redacted before and after normalization, and common secret assignments redacted
before display clipping. `max_output_bytes` is 1–16,384 (default 4096), clipped
at a UTF-8 boundary; `output_truncated` reports clipping.

`cleanup.state: absence_observed` records a bounded independent read of the owned
session's absence, including after an unacknowledged delete. It does not prove
descendant termination. `unconfirmed` preserves the original command result and
reports why ownership/removal could not be established. After an uncertain
creation, an early empty list is only absence **at observation**: the create can
finish later, so `late_creation_possible` remains true. Do not repeat the
command to repair uncertainty. Inspect the exact target with the same identity
and follow the remote lifetime limit; a new MCP process never adopts old names.

Only validated caller/target IDs, the generated name, duration, observation end,
cleanup state and separate attempt counts enter MCP audit events. Raw command
input, output, private initializer and upstream error bodies are omitted, even
when `RUST_LOG=trace`. **Core/Periphery can record commands themselves. Arbitrary
unrecognized output cannot be guaranteed secret-free.** Terminal opt-in controls
direct MCP exposure; it is not a sandbox for trusted Actions, Procedures or other
automation with its own authority.

Pinned semantics: [terminal contracts](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/api/terminal.rs),
[lifecycle requests](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/api/write/terminal.rs),
[Core routing](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/terminal.rs),
and [Periphery PTY execution](https://github.com/moghtech/komodo/blob/v2.3.2/bin/periphery/src/api/terminal.rs).

Deterministic compiled stdio checks run with `cargo test --locked --test terminal`.
They inject pinned-wire faults rather than executing a shell. Real owned
Core/Periphery/PTY observations and final integrated cleanup are recorded
separately in milestone acceptance.
