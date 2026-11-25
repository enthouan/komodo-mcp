# Disposable production-stdio smoke fixture

The automation extension, `scripts/disposable_automation_smoke.py`, adds real
Repo/Server Builder/Build/registry/Deployment and Procedure/Action/schedule
acceptance while retaining the original checks below. See the
[automation walkthrough](../../../docs/automation-walkthrough.md) for separate
admin test authority, private HTTPS Git, runtime evidence and policy disabling
before verified teardown. The coverage exclusions below describe the base runner.

Used only by `python3 scripts/disposable_core_smoke.py` with an explicit local
Docker context and unused `komodo-mcp-smoke-...` project. Standard CI runs the
synthetic Rust scenarios and runner safety tests; it does not start Docker/Core.

`compose.json` is secret-free. The runner substitutes random database/admin/JWT
credentials and an invocation-specific ownership label into a private temporary
file. Core and Periphery are pinned to **2.3.2**, matching `komodo_client`; their
runtime versions must match before provisioning test workloads. Mongo **7.0.16**
and Docker **27.5.1-dind** provide disposable persistence/runtime only. The nested
daemon is privileged, with private named volumes and no host Docker socket/binds.
The only published endpoint is Core on an ephemeral loopback port.

The provisioner creates one Server-backed Stack/Deployment, an Execute operator,
a Read-only identity and a Write editor for the configuration permission proof.
The isolated Core enables non-admin creation for Stack creation acceptance; only
the editor receives Read/Attach on the original and a disabled alternate Server.
Both Servers are provisioned separately. The editor creates an inline Stack,
patches one field, and moves the running original Stack to the alternate Server
and back while observing old-target containers. No live Git/host/Swarm claim is
made.
The configuration-only editor can save ordinary settings but its direct source
update is denied locally with zero Core reads/writes, preserving the independent
operate boundary against Core auto-deploy.
The editor also creates a direct-image Deployment and patches only its image,
preserving authoritative ports, environment and target. Combined grants permit
image updates; configuration-only direct apply is denied before Core reads/writes.
The runner compares saved desired image with the original running container and
checks Deployment Server reassignment/restoration without changing its ID, image
or running state. Build references and Swarm targets remain fixture-only coverage.
Only the Write editor also receives Server creation permission. It creates a
disabled Server through production MCP, changes one alert setting while comparing
all other saved fields/trust, and verifies no-op and local connection-apply denial.
With combined grants it creates an enabled Server with independently obtained
initial trust against an unreachable endpoint inside the owned Core container:
the definition is saved while connectivity remains NotOk. The Execute identity
has no Server creation permission and proves sanitized Core denial without a
fallback. Server sessions issue zero MCP executions and preserve all existing
workload IDs/images/states. New Server records disable pruning and key rotation.
Before resource deletion, the editor creates and renames owned tags, changes
Stack assignments with additive preservation, zero-write omission/no-op and
explicit clearing, and discovers the resource by its new tag name through a
separate read-only process. Exact request field names and unrelated authoritative
metadata/configuration are checked. Configuration-only tag deletion is denied
locally; the Read identity with both local grants receives Core's tag-owner
denial. The owner deliberately deletes both new tags through the shared policy,
restoring initial assignments and retaining workload IDs/states/start times.
Swarm's omitted tag cleanup and other resource-family assignments are covered
by deterministic wire fixtures and pinned source, without a live matrix claim.

Deletion uses a fourth enabled non-admin identity with scoped Write and Server
Inspect access. The provisioner creates dedicated running Stack/Deployment
fixtures with short termination settings; production MCP previews and deletes
them once. Fresh Core/Periphery inspections and independent nested Docker reads
check exact container-ID absence separately from definition removal. The Read
identity, even with both local grants, receives Core's Write denial. All three
incomplete local grant profiles hide and deny direct deletion before Core I/O.
Finally the owned original Server is deleted: Stack/Deployment/Repo and disabled
Alerter references are checked, the current Builder vector is observed remaining,
and original workload IDs/images/states remain unchanged. This final scenario
runs after earlier operation/configuration proofs. Swarm deletion, cleanup errors
and skipped cleanup retain deterministic fixture coverage. Provisioner access is
used only for setup and independent observations, never as an MCP fallback.
All are enabled non-admin users; API keys expire in
one hour. Provisioning credentials never enter an MCP child. The test workload
uses BusyBox **1.37.0** and `depends_on.restart: true`; only deploy has saved hooks.

All project containers, volumes and networks carry the invocation owner label.
Cleanup verifies those labels before deleting this project and verifies absence
afterwards. See the [walkthrough](../../../docs/operations-walkthrough.md) for
the full command, evidence, observation bounds and failure cleanup behavior.

## Packaged v0.9 candidate regression

Run the two transport checks in distinct, unused disposable projects. Each needs
the verified JSON candidate record, the local Docker context, Python 3.11 or
later, and a checkout containing the package and its contract baseline history.
The runner verifies the frozen contract SHA-256 at both Git revisions.
The stdio run requires an unprivileged Linux account, Bubblewrap at
`/usr/bin/bwrap` with working user namespaces, a system CA bundle at
`/etc/ssl/certs/ca-certificates.crt`, and the matching static Linux executable
extracted from its native archive. Each stdio launch, including the rebuilt
MCP probe, sees the read-only executable and public CA bundle, a private PID
namespace and temporary space. It reaches Core on the caller's loopback network.
The HTTP run takes the downloaded OCI archive and
the Docker engine platform.

```sh
python3 scripts/disposable_candidate_regression.py \
  --candidate-record "$CANDIDATE_RECORD" --transport stdio --binary "$CANDIDATE_BINARY" \
  --docker-context "$DOCKER_CONTEXT" --project komodo-mcp-smoke-v09-stdio-01 \
  --evidence target/regression/stdio-01.json

python3 scripts/disposable_candidate_regression.py \
  --candidate-record "$CANDIDATE_RECORD" --transport http --oci-archive "$CANDIDATE_OCI_TAR" \
  --platform linux/arm64 --docker-context "$DOCKER_CONTEXT" --project komodo-mcp-smoke-v09-http-01 \
  --evidence target/regression/http-01.json
```

Use `linux/amd64` when the Docker engine runs on x86-64. Both runs verify the
package bytes, match MCP initialization to the embedded version and record the
candidate-record checksum. The HTTP run checks the selected image manifest and
configuration, hashes the executable copied from the container, and verifies
that initialization requires a bearer token. Keep measurements in a separate
environment while a run provisions the nested Docker daemon.

The runner observes actual Core and Periphery versions. It lists a running
container, pages through all cached states and inspects one stopped container
with a seeded exit code and private health-check output. Separate scoped Core
users exercise Read + Inspect, Read without Inspect, and execution. An editor
previews and applies a Stack configuration change. A ResourceSync reader with
the local operate and configuration-edit gates enabled discovers the selected
tools, reads the owned plan and attempts to apply it. Core rejects the preflight
`RefreshResourceSyncPending` write for lack of Execute. The runner checks that
the attempt submitted zero `RunSync` executions, that Core's `RunSync` history
and managed Server configuration are unchanged, and records any other updates
on the Sync separately. A second check changes the inline source after the
editor's review and verifies the stale-plan rejection against Core history
and the target configuration. After a scoped deployment is acknowledged, it
kills the MCP process, reconnects with a read-only identity, and checks the
original update ID, final Docker state and number of deployment updates in Core.

The evidence file records each completed scenario and the remaining owned
resources after cleanup. A failed cleanup retains the fixture directory for
local recovery. Keep this failure record private until any local paths have
been removed for publication. A run exercises the pinned Core/Periphery 2.3.2
fixture and the identified packaged candidate on the selected platform.


The **Packaged candidate regression** Actions workflow runs the same checks in
separate Ubuntu 24.04 jobs for Linux x86-64 stdio and authenticated HTTP. A
third job extracts the candidate's Corresponding Source in a loopback-only
network namespace. The source-provided build runs as an unprivileged user
in a network-disabled container with a read-only root. It receives the source
archive, its extracted tree, the pinned Rust and system toolchains, fresh
scratch space and an empty output directory. A trusted entry point checks
the mounts and verifies that the other downloaded packages, checkout and
Docker socket are inaccessible. The shipped build uses a fresh Cargo home
and runs its MCP inventory probe. After the container exits, the outer job
verifies the native executable digest against the distribution manifest,
extracts the rebuilt native archive, calls `list_stacks` over
stdio and checks the three expected fixture rows and observed Core reads.
Each job re-downloads both selected Actions
artifacts and checks their complete identities against the tracked record.
Each successful acquisition produces an allowlisted JSON evidence artifact,
including partial results following an exercise failure. Live evidence records
workload cleanup and Compose teardown separately. Remaining resource counts
are zero for an observed empty inventory and null when the listing failed or
did not run. Overall cleanup requires verified removal across all eight
workload inventories and the project containers, networks, and volumes.
Successful transport results include the negotiated MCP protocol, client and
server versions, actual platform, and versions reported by Core and Periphery.
They record the checks for fixture secrets in MCP frames, protocol errors,
process diagnostics, and retained scenario evidence. The HTTP checks cover
response status text, headers and bodies, including rejected bearer probes.
The fixture scans each candidate container's final logs and retained health
diagnostics after the process exits. JSON response and notification contents
are checked after decoding on both transports.
The same-repository PR introducing the workflow runs these jobs when opened
ready for review, marked ready, or updated while ready. After that PR merges,
use **Run workflow** with the repository-relative candidate record path.
