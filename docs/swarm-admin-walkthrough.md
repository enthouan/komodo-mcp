# Swarm and administration walkthrough

Use a dedicated Core identity for the intended resource and keep provisioning
credentials out of the MCP environment. The supported upstream contract is
Core/Periphery 2.3.2. Start with read-only inspection, inspect the evidence source
and coverage, and use an exact ID from that observation for subsequent work.

## Independent capabilities and upstream permissions

All four flags default to false. Supply only the grants needed by that MCP
process; restarting with a different profile does not transfer private previews.

| Local environment grant | Enables | Core authority still required |
| --- | --- | --- |
| No mutation flags | Inventory, Docker/Swarm reads, health/logs, history, recovery, export and administration metadata | Read; Inspect/Logs where the endpoint requires them; admin for privileged directories |
| `KOMODO_ALLOW_CONFIGURATION_EDIT=true` | Definition/change previews and configuration saves | Write or administrator as documented per family |
| `KOMODO_ALLOW_OPERATE=true` | Explicit workload execution and saved Alerter tests | Execute on that exact resource |
| `KOMODO_ALLOW_PRUNE=true` **and operate** | Preview and acknowledged Docker category prune | Execute on the exact Server |
| `KOMODO_ALLOW_TERMINAL=true` **and operate** | One bounded command in a fresh owned terminal | Read + Terminal; fresh container checks also require Inspect |

Access-changing applies, ordinary-variable value/create/delete operations,
enabled Alerter creation and existing Alerter updates additionally need both
configuration-edit and operate. Secret description edits need configuration-edit.
Ordinary configuration/operate profiles enable neither pruning nor terminals.
Local grants cannot confer upstream rights or sandbox trusted automation. A denial
never justifies switching credentials, targets or APIs within that operation.
See [access](access.md), [variables](variables.md), [Alerters](alerters.md),
[pruning](pruning.md) and [terminals](terminal.md) for exact requirements.

## Diagnose, save, execute, observe and check health

1. `list_swarms`, `list_swarm_nodes`, `list_swarm_services` and
   `list_swarm_tasks` expose bounded Core caches. Use `diagnose_swarm_service`
   for one exact service, then fresh task/node details and exact-service logs.
   Pending or historical tasks are not healthy current replicas. Empty cached
   results do not prove an empty cluster. [Swarm inspection](swarms.md) explains
   fresh versus cached evidence and incomplete coverage.
2. Select one existing Komodo-managed Swarm Deployment. Use
   `preview_deployment_scale` / `apply_deployment_scale` for the same target and
   requested count, including deliberate zero. Check the saved configuration's
   independent read-back. Saving alone submits no deployment.
3. Use `review_deployment_scale_deploy` / `deploy_scaled_deployment` explicitly.
   Pinned Periphery ignores the saved replicas field; the editor also manages
   only its `--replicas=N` argument and preserves every other argument. Core
   recreates the service, so expect interruption and a new service ID. Stack
   replicas belong in Compose source; unmanaged services are unsupported.
4. Retain the Core update ID and observe it through bounded `wait_for_update`.
   Then use `observe_deployment_scale` to check current replicas separately.
   Neither a successful save nor update establishes convergence or health.
5. Check application health through an independently authorized endpoint. For a
   scale to zero, confirm the current service's desired count and current task
   observations; historical task counts cannot establish success.

[Scaling](scaling.md) documents save/deploy separation, partial outcomes and the
remaining configuration/reference races. No tool provides automatic rollback.

## Cleanup decisions, variables and administration

Inspect one Server's images, networks, volumes and safe image history before
considering cleanup. Inventory is supporting evidence, never a frozen prune dry
run. [Docker inspection](docker.md) omits raw environment, labels, options and
build commands. A prune preview names exactly one Server and category, explains
its actual effects, and requires a new explicit destructive acknowledgement.
Image prune removes all unused images; volume prune includes unused named
volumes. System prune includes volume effects and build cache. Docker 27.5.1
system prune removes unused anonymous volumes but preserves unused named volumes;
category volume prune can remove those named volumes. Running containers and
in-use objects must be checked independently. Core's Server `auto_prune` policy
is separate from MCP's opt-in; disable it on causal disposable test fixtures.

Manage ordinary variables through typed preview/apply changes. Secret entries
allow metadata and description edits only; their values are always omitted,
including administrator, history and export responses after restart. Configured
Core secret key names are a separate names-only read. A safe display is not a
private apply baseline. Treat changes as possible effects on future automated
workloads and notification routing even when they submit no execution.

Use separate ordinary-admin, designated super-admin and affected test identities
for access work. Inspect direct records separately from the affected caller's
Core effective permission. Group and everyone/type-wide inheritance can survive
removal of a direct grant. Preserve unrelated memberships and specifics. Self
lockout and protected super-admin changes are refused; another concurrent admin
can still race the final checks. User deletion does not erase API keys or scoped
permission rows. Credential revocation and leftover cleanup require independent
provisioning/administration verification; no transactional cleanup is promised.

## Notification and terminal evidence

Create an Alerter disabled, inspect and save routing separately, then deliberately
call `test_alerter` on one saved target. Saving never sends a test. A Core receipt,
provider outcome and complete HTTP receiver receipt are distinct facts. A disabled
test fails without sending. TestAlerter bypasses routing filters but honors
maintenance; a maintenance-suppressed successful update can produce no provider
attempt. Keep endpoint URLs, tokens and interpolation private. Use an owned local
receiver for acceptance, never a real recipient or broadcast SendAlert.

Terminal execution accepts exact Server, full container ID on a Server, named
service of a Server-backed Stack, or Server-backed Deployment. Swarm task shells
are unsupported. Each explicit command allocates a new namespace and has separate
CreateTerminal, command and DeleteTerminal counts. It never attaches to a default,
foreign or old shell. Input, output, concurrency and observation are bounded.
Reported exit status is upstream evidence; EOF alone is unknown. Timeout or
client disconnect stops observation and attempts owned cleanup; neither proves
descendant termination. A hard-killed MCP cannot run local cleanup: the owned
shell's remote lifetime wrapper bounds its normal lifetime, and a fresh process
must preserve the old namespace. Core/Periphery may log commands. Known-secret
redaction cannot guarantee arbitrary command output is secret-free.

## Uncertain submissions and restart recovery

A preview expires, is single-use, and binds private raw state and applicable
caller/references. Secret-only changes invalidate it. Make a new explicit review
when stale; never reconstruct a baseline from sanitized display data.

Preserve a known update ID and restart with a read-only profile to observe it.
For a lost acknowledgement, `reconcile_operation` returns unattributed history
candidates rather than replaying. Inspect exact saved state independently after
an uncertain configuration write. An observed match does not attribute that
write to this client. No CAS, frozen downstream interpolation, cross-client
exactly-once, rollback or reliable cancellation is claimed.

## Reproduce disposable acceptance

Requires Python 3.11+, the pinned Rust toolchain and a local Unix-socket Docker
context with Compose and enough resources for two privileged nested Docker
engines, Core, Periphery, Mongo, a local HTTP receiver, registry and Git source.
Nested engines use owned named volumes and never mount the outer Docker socket
or publish engine ports. Only Core has an outer loopback port. The fixture creates
no product cluster/node-administration or credential-issuance tools.

From a clean committed candidate, use a new evidence file and unused project:

```sh
python3 scripts/disposable_swarm_admin_smoke.py \
  --expected-commit FULL_40_CHARACTER_SHA \
  --docker-context desktop-linux \
  --project komodo-mcp-smoke-v07-unique \
  --evidence /tmp/komodo-v07-unique.json
```

The default `all` suite includes the complete retained Core regression,
automation and four-source ResourceSync workflows. `--suite features` is an
explicit development shortcut and cannot produce final acceptance. Standard CI
runs deterministic compiled-stdio and script tests without Docker or accounts.

After merge, fetch main, select its actual squash commit in a clean checkout and
repeat the default command with `--require-main`. The script builds the production
executable, checks source/executable/MCP identity, provisions separate identities,
records request/update/resource IDs and independent health/HTTP receipts, and
labels injected lost acknowledgements separately from actual live effects.
All 16 capability profiles run against production stdio. Raw malformed streams,
expiry and some concurrency races remain deterministic fixture coverage.

Cleanup runs even after a failed check. It disables owned automatic effects,
removes test keys/users and terminals, destroys only the owner-checked Compose
project and its volumes, removes the private fixture directory, then inventories
that exact project in separate Docker processes. If cleanup cannot be verified,
the run fails and retains a private recovery directory in evidence; inspect it
without printing credentials. Do not replay an interrupted ledger or broadly
prune the host. Outer image caches are shared Docker state: retain preexisting
images; remove only newly pulled unchanged references after checking concurrent
use. Final acceptance additionally requires independently reviewing the cleanup
and image-cache evidence, not merely a zero CLI exit status.
