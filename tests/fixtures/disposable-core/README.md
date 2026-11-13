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
All are enabled non-admin users; API keys expire in
one hour. Provisioning credentials never enter an MCP child. The test workload
uses BusyBox **1.37.0** and `depends_on.restart: true`; only deploy has saved hooks.

All project containers, volumes and networks carry the invocation owner label.
Cleanup verifies those labels before deleting this project and verifies absence
afterwards. See the [walkthrough](../../../docs/operations-walkthrough.md) for
the full command, evidence, observation bounds and failure cleanup behavior.
