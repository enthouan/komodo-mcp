# Disposable production-stdio smoke fixture

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
All are enabled non-admin users; API keys expire in
one hour. Provisioning credentials never enter an MCP child. The test workload
uses BusyBox **1.37.0** and `depends_on.restart: true`; only deploy has saved hooks.

All project containers, volumes and networks carry the invocation owner label.
Cleanup verifies those labels before deleting this project and verifies absence
afterwards. See the [walkthrough](../../../docs/operations-walkthrough.md) for
the full command, evidence, observation bounds and failure cleanup behavior.
