# Deliberate resource deletion

`preview_resource_delete` and `apply_resource_delete` support one exact Stack,
Deployment or Server definition. Resolve it through inventory, then supply its
kind and stable 24-hexadecimal ID. Names, wildcard/bulk selection, templates and
other resource families are unsupported.

Preview needs `KOMODO_ALLOW_CONFIGURATION_EDIT=true` and submits zero writes.
Apply requires **both** that grant and `KOMODO_ALLOW_OPERATE=true`, even when the
preview reports a stopped or unknown workload. Core can perform runtime teardown
inside the delete write. Operate-only access cannot delete; disabled apply is
absent from discovery and rejected on direct dispatch before Core reads. The
shared authenticated transport checks both grants too. Core independently needs
Write on the selected definition; no credential or permission fallback occurs.

```json
{"name":"preview_resource_delete","arguments":{"target":{"kind":"stack","id":"000000000000000000000002"},"action":"delete"}}
```

Read the exact resource, known live workloads, visible references, coverage,
effects and evidence gaps. Deliberately apply the same input with the returned
token:

```json
{"name":"apply_resource_delete","arguments":{"target":{"kind":"stack","id":"000000000000000000000002"},"action":"delete","preview_id":"<returned preview_id>"}}
```

The token uses the same process-local store as configuration previews: at most
16 total previews, ten-minute expiry and single use. It binds the action/type/ID,
full private definition, deployed cleanup identity, attachment and accepted trust,
relevant runtime observations, visible structured references and history scope.
Apply rechecks them before one typed `DeleteStack`, `DeleteDeployment` or
`DeleteServer` request. Changed state rejects before submission. Raw secrets,
container configuration and archived definitions never enter the public preview.

Core has no conditional-delete parameter or invocation idempotency key. Another
writer, reference or workload can race after the final read. An invalid, lost,
oversized or mismatched acknowledgement produces `submission_unknown`; the token
is consumed and no mutation retry occurs. Later reads can observe removal while
the submission outcome remains unknown. Never repeat deletion automatically.

| Selected definition | Core 2.3.2 effects |
| --- | --- |
| Stack on Server | May run Compose `down --remove-orphans` using the **deployed** project name. Unobserved orphans can be affected. |
| Stack on Swarm | May remove Swarm stacks with detached/asynchronous cleanup. |
| Deployment | May remove its **deployed** container or Swarm service, using configured termination settings. |
| Server | Leaves host workloads running; detaches Stack/Deployment/Repo Server references, updates onboarding links/alerts and drops cached connectivity. |
| All three | Removes permissions/definition and clears Alerter whitelist/blacklist references; archives the full definition in Core history. |

Unknown/down/not-deployed state, disabled or missing targets, and cleanup failures
can leave runtime workloads while Core removes the definition. Server deletion
targets a legacy scalar Builder `server_id`; current `server_ids` arrays can
remain dangling. Each visible reference receives its own after-state. The tool
does not repair dangling references with additional writes.

Definition and runtime evidence stay separate:

- `definition_removal.reported_by_core` means the old-resource acknowledgement
  matched the previewed definition. `observation` reports a separate direct read,
  including still present, absent or unavailable.
- `known_workloads` contains at most eight fresh inspections of validated Docker
  IDs. Post-delete absence requires the pinned Docker no-such-container response
  for a known full ID. Empty caches, a missing Server and unavailable inspection
  do not prove cleanup. New containers/orphans and Swarm services remain outside
  this evidence, so `runtime_cleanup_fully_verified` remains false.
- `cleanup_updates` contains at most three new matching history candidates,
  without guaranteed invocation attribution. Failed/skipped stages are reported
  separately, including Core's `Failed to destroy stack` log whose `success`
  flag can be true. Successful update/command flags never establish runtime
  cleanup. Missing or unreadable history remains an explicit gap.

Core removes resource permissions before recording its deletion update. An
ordinary caller can lose history access after deletion. The tool keeps the same
identity and reports that gap. Core's `Deleted Toml` archive can contain secrets;
MCP never returns its stdout or raw cleanup stderr. Core history permissions and
retention still govern that upstream archive.

Reference scans use two positive pages of 20 visible records per supported family
(Alerters, plus Stacks/Deployments/Repos/Builders for Server deletion). At most 32
references are displayed; extra visible effects remain privately bound. Hidden
records, additional pages, arbitrary Procedure/Action/ResourceSync scripts,
onboarding keys and alert details are not exhaustively enumerated. Permission
gaps or a completed visible scan never prove no dependencies. Cached runtime
sampling retains at most 128 entries; Stack services use one positive page of
100. Preflight has a 15-second overall budget and 256-KiB private snapshot bound;
post-attempt observation has ten seconds. Incomplete observations remain explicit.

The disposable runner exercises running Compose Stack and Server Deployment
deletion, Read-identity denial, Server detachment across all five visible
reference families, the Builder limitation, and independently observed surviving
host workloads. Swarm deletion and cleanup failure/skip paths have deterministic
wire-fixture coverage. See the [live walkthrough](operations-walkthrough.md) for
owned Docker isolation, evidence identities and cleanup requirements.
