# Workload operation walkthrough

This walkthrough uses the production stdio server and the supported Core 2.3.2
API. It covers existing Server-backed Stacks/Deployments and exact Compose service
scope. See the [target/action matrix](operations.md#supported-targets)
for whole-workload Swarm deploy and the rejected Swarm filters/lifecycle actions.
For managed Swarm replicas, use the separate [save, deploy and task-observation
workflow](scaling.md), including intentional zero and service recreation effects.
For deletion, use the separate [deliberate preview/apply workflow](deletion.md):
Core can perform teardown inside a definition write, and removal is observed
separately from cleanup. Both local grants and Core Write are required.
For user/group administration, follow the separate [access workflow](access.md).
Direct records and current-caller effective access are distinct; every access
apply requires both local grants and actual Core admin/super-admin authority.

## Inspect, act, follow and check health

Use an enabled API identity with **Execute** on the selected Stack/Deployment,
**Read** on its Server, and the Read/Inspect/Logs access needed for history and
diagnosis. The MCP process needs no provisioning/admin credentials. Core remains
the permission authority, including a denial recorded after acknowledgement.

1. Configure the stdio child with `KOMODO_ADDRESS`, `KOMODO_API_KEY`,
   `KOMODO_API_SECRET`, `KOMODO_ALLOW_OPERATE=true` and
   `KOMODO_ALLOW_CONFIGURATION_EDIT=false`. These are process environment values;
   `.env` files are not loaded. Restart the child after changing capabilities.
   With tool selection unset and both optional grants disabled, discovery contains sixty-four read tools,
   including exact [discovery by tag](tags.md).
   Operate adds twenty tools; configuration editing independently adds typed
   preview/apply tools. See [client configuration](../README.md#local-capabilities-and-core-permissions)
   and the [configuration workflow](configuration.md).
2. Initialize your stdio client and call `tools/list`. Resolve an exact workload
   name with `get_stack` / `get_deployment`, then save the returned stable ID.
   Inspect `stack_diagnose` or `fleet_health` before choosing an action.
3. Retain the target ID, requested scope/action and local UTC start time in your
   client **before** execution, so this context survives process loss. For a named
   service, inspect the original exact service name: deploy validates the latest
   saved service inventory; lifecycle validates the deployed inventory.
4. Submit one operation. These are `tools/call` parameters; replace the example
   stable IDs with the IDs you just resolved, and select the intended scope:

   ```json
   {"name":"deploy_stack","arguments":{"target":{"id":"000000000000000000000002"},"scope":{"kind":"service","service":"web"}}}
   ```

   Whole-Stack scope is explicitly `{"kind":"whole_stack"}`. For an existing
   Deployment, a lifecycle call is:

   ```json
   {"name":"restart_deployment","arguments":{"id":"000000000000000000000003"}}
   ```

   Deploy applies the saved definition. Stack deploy follows saved `auto_pull`,
   Compose/build flags and wrappers; Server Deployment deploy pulls the selected
   image/build and recreates the container. Restart uses existing containers and
   does not apply changed configuration/images or run deploy hooks. Start/stop
   perform those actions; they never substitute unpause or redeploy.

   Service selection is not isolation: deploy can start dependencies and runs
   saved deployment hooks; start can start existing dependencies; restart can
   restart configured dependents. Stop selects the requested service, whose
   absence can still affect dependent applications. Read the receipt's requested
   scope and configured effects separately. Invalid/unsupported selection never
   expands to the whole workload.
5. Save the receipt and its `update_id`. `accepted` establishes acknowledgement,
   not final success. Busy, permission and rollout failures can appear in that
   accepted update. Each explicit operation is a new non-idempotent submission.
6. Inspect `get_update` and wait on that exact ID:

   ```json
   {"name":"wait_for_update","arguments":{"id":"100000000000000000000001","timeout_seconds":30}}
   ```

   Each wait is bounded by time and ten reads. `poll_limit` can arrive before
   thirty seconds; `deadline` is incomplete observation. Repeat the **read-only
   wait**, with the same saved ID, in this or a new default read-only session.
   Permission loss/read failure retains the ID/latest observation. Cancelling
   local observation does not cancel Core's operation.

   If the submission reply was lost and no ID is known, retain uncertainty and
   use [bounded reconciliation](operations.md#recover-an-interrupted-submission-without-repeating-execution)
   with the saved context. Zero candidates never proves non-acceptance; one or
   several candidates never establishes attribution automatically. Another caller
   can issue an identical operation. Recovery never executes again. A new
   explicit operation can repeat effects; there is no upstream invocation key or
   exactly-once guarantee across client retries.
7. End with an independent health observation, including after a successful Core
   update:

   ```json
   {"name":"stack_diagnose","arguments":{"id":"000000000000000000000002"}}
   {"name":"fleet_health","arguments":{}}
   ```

   Inspect the Stack's service evidence, Deployment health and source gaps.
   Complete/succeeded execution and healthy workload are separate conclusions;
   failed recent updates can also remain diagnosis evidence.

Repo operators follow [the same inspection, explicit hook acknowledgement and
receipt/recovery sequence](repositories.md#inspect-effects-before-one-clone-or-pull).
The configured branch, current cached clone revision and per-update observed
revision are distinct. Clone/pull never rewrite the saved Repo definition.

## Deterministic stdio validation

From the repository root:

```sh
cargo test --locked --test operations_stdio --test incident_stdio --test quickstart --test repositories
```

Cargo builds and launches the production binary with synthetic loopback Core
fixtures and the official rmcp client. No live Core, Docker, inherited Core
credentials or network service is needed. The integrated transcript covers all
four actions on Stack/Deployment and whole/named scope, read-only defaults,
asynchronous denied/busy/rollout failures, local timeout/resume, successful
execution with unhealthy service evidence, configured hooks/dependencies,
invalid selectors/Swarm filters, and independent callers with process loss and
read-only reconciliation. Existing incident and quickstart workflows remain
part of the command. Request counts prove no execution from observation/recovery.
The standalone [inventory mock](mock-stdio.md) serves reads only.

## Opt-in disposable Core/Periphery smoke

Use Python 3.11+, the pinned Rust toolchain, Docker and Compose on a local Unix
socket context that can run a privileged nested Docker daemon. Commit the
candidate and use a clean checkout. Select the context explicitly and choose an
unused project name; the runner refuses to adopt an existing project:

```sh
python3 scripts/disposable_core_smoke.py \
  --docker-context desktop-linux \
  --project komodo-mcp-smoke-v030 \
  --evidence target/disposable-core-v030.json
```

Replace `desktop-linux` with your intended local context (for example `default`
on Linux). Choose a new evidence path on subsequent runs; existing evidence is
preserved. The runner builds the actual Cargo-reported release executable from
the clean commit and verifies its MCP identity/current package version. It uses
the checked-in [Compose fixture](../tests/fixtures/disposable-core/README.md),
Core/Periphery **2.3.2**, Mongo **7.0.16**, Docker **27.5.1-dind**, and a two-service
BusyBox **1.37.0** workload. Versions observed inside the environment are recorded.
This Server-backed smoke makes no live Swarm or multi-version claim.

Only the disposable Core gets a dynamically allocated `127.0.0.1` port. Periphery
uses the private nested daemon, without the host Docker socket or host workload
mounts. Provisioning uses a separate temporary admin; MCP children receive only
scoped enabled non-admin Execute, Read and Write identities. Write includes
upstream Execute; local operate/edit grants remain independent. Keys expire after
one hour.
API calls disable redirects/proxies, MCP stderr is drained/bounded, and commands,
RPCs, responses, waits and shutdowns have bounds. Private files use a private
temporary directory and mode 0600. No real-Core address or credential input is
accepted by this runner.

The smoke deploys the Stack/Deployment, exercises whole and named lifecycle,
checks saved deploy-hook stages and existing-container dependency effects,
observes a denied caller's accepted failed update, and uses `get_update` plus
bounded wait/resume. Lost-ACK service operations and two independent callers
remain unknown/unconfirmed after a new read-only session; the fault proxy records
acknowledged IDs only as test instrumentation. It finishes with read-only
`stack_diagnose` and `fleet_health`, independently of execution outcomes.
Both recovery sessions use a separately authenticated Read proxy that counts
execute attempts before forwarding. The evidence records those measured counters
and history-request counts; a zero is asserted only after observed recovery reads.

The sanitized JSON records the built commit, executable hash, package/MCP and
Core/Periphery/Compose/Docker versions, scoped resource/identity IDs, operation
outcomes, recovery counts, final health and cleanup. Before `compose down -v`,
the runner verifies this invocation's ownership labels on every project
container, network and volume; afterwards it requires zero remaining project
resources and removes temporary credentials/config. Cached third-party images
are retained. A failure exits nonzero and retains evidence. If cleanup fails,
the report identifies the private directory and owner label for targeted
recovery; it never prunes other projects or silently adopts their resources.

The current runner also proves the configuration boundary using the **same
non-admin Core Write identity**, which includes upstream Execute. With operate
enabled and configuration editing disabled, a production MCP Stack restart
succeeds while direct `apply_stack_update` is rejected locally with zero Core
reads/writes. A separate configuration-only process previews and applies one
`auto_pull` field, reads back all unrelated saved fields unchanged, measures a
zero-write no-op and restores the fixture policy. Its proxy captures write field
names and counts zero execution attempts. Provisioning remains a separate admin.

The configuration-only identity also creates and inspects an inline Stack,
changes one setting with authoritative source/unrelated-field preservation, and
verifies that the new definition has no containers. The fixture explicitly enables
Core's non-admin resource creation policy and grants its Write editor Read/Attach
on two pre-provisioned Servers. It moves the owned running Stack to a disabled
Server and back, verifying that Core 2.3.2 preserves the old-target container IDs
and submits no MCP execution requests. Direct Git, linked Repo, host files and
Swarm attachments have deterministic stdio fixture coverage only. See
[configuration effects](configuration.md#runtime-effects).
The editor also previews a source update and directly attempts apply without the
operate grant: the local denial performs zero Core reads/writes. Source apply
requires both grants even when the current auto-update policy is disabled.

The same editor creates and inspects a direct-image Deployment with a port mapping
and environment reference. A configuration-only image apply is denied with zero
Core reads/writes. A combined-grant session changes only the image and reads back
all unrelated saved fields unchanged, including ports/environment/target. It
separately changes the owned running Deployment's desired image and inspects its
actual nested-Docker container: ID, running image and running state stay unchanged.
The desired image is restored. Moving the Deployment to the disabled alternate
Server and back preserves that same running container. Every definition session
counts zero MCP execute submissions; no-op image apply adds zero writes. Build
references and Swarm attachments have deterministic fixture coverage only. See
[Deployment definitions](deployments.md) for Core registry/auto-deploy effects and
why Core's reported image can fall back to desired configuration.

For trusted host/admin automation, follow the [Action walkthrough](actions.md):
inspect authority, preview/apply private source/default changes, then run the saved
Action with bounded overrides. Retain its update ID; explicitly cancel that exact
Action/run if needed and observe the original separately. No replay occurs after
lost replies or restarted read-only recovery.

For recurring trusted automation, follow [schedule inspection and changes](schedules.md).
Check saved fields and the separate Core cache before and after apply. Use an
explicit timezone for reproducible policy, retain Core next-run/error evidence,
and verify the eventual run and workload health independently. Disabling a
schedule does not cancel already dispatched/running automation.

For read-only Docker image, network and volume inventory on one exact Server,
including safe image history, use the [Docker inspection walkthrough](docker.md).
Cached inventory is not an authoritative prune dry run.
