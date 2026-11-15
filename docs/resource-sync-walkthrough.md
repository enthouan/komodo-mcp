# Export, review and apply ResourceSync desired state

This walkthrough uses the compiled production MCP server over stdio and Core
2.3.2. Use an owned test sync and disabled test resources first. The
[definition and execution reference](resource-syncs.md) describes exact schemas,
bounds and private authorization checks; the examples below are `tools/call`
payloads. Copy returned IDs and bindings rather than the illustrative values.

## Establish scope and permissions

Start MCP with the usual Core URL/API credentials. Read-only inspection works
without local grants. Definition editing requires
`KOMODO_ALLOW_CONFIGURATION_EDIT=true`; refresh requires
`KOMODO_ALLOW_OPERATE=true`; applying desired state requires **both**. An
execution-bearing definition change also requires both, even with webhooks off.
Core independently checks visibility, Write/Attach and Execute. Creation requires
a designated Core administrator. Never replace denied caller credentials with
provisioning credentials.

Execute on a sync delegates Core's privileged sync behavior. It is not a sandbox
limited to resources the caller could edit directly. Confirm the complete source,
scope, deletion settings and automatic effects before granting it. Keep new
webhooks off unless deliberately enabling them with both grants.

## Export an explicit selection

```json
{"name":"export_resources_toml","arguments":{"targets":[{"kind":"server","target":{"name":"owned-server"}}],"include_variables":false}}
```

Inspect `selection_complete`, missing definitions, `omitted_fields`, warnings and
`authoritative_apply_input`. The returned TOML is an inspection projection:
secrets, all variable values, sensitive content and unsupported fields are
omitted. Variables require a separate opt-in to **all** variables; resource tags
do not scope them. Explicit group selection is separate too. Do not save this
sanitized projection over an authoritative source: omitted desired-state fields
can reset existing configuration to Core defaults. Export makes zero mutations.

## Define the complete trusted source

Here a new disabled Server is declared inline. The explicit empty tag list means
the resource-tag filter is unrestricted; this example keeps both deletion flags
off and excludes variables/groups.

```json
{"name":"preview_resource_sync_create","arguments":{"name":"owned-sync","config":{"source":{"kind":"inline","contents":"[[server]]\nname='owned-server'\n[server.config]\nenabled=false\nauto_prune=false\n"},"managed":false,"delete":false,"match_tags":[],"include_resources":true,"include_variables":false,"include_user_groups":false,"webhook_enabled":false}}}
```

Inspect the preview, then call `apply_resource_sync_create` with the **identical**
`name`/`config` plus its `preview_id`. Require authoritative saved-configuration
verification. Saving makes at most one definition write and zero RunSync calls;
Core can still refresh its pending cache and affect pending alerts.

To change one setting, preview/apply an update using the same two-step contract:

```json
{"name":"preview_resource_sync_update","arguments":{"target":{"name":"owned-sync"},"patch":{"pending_alert":false}}}
```

Omitting `source` preserves the saved source, credentials and other settings.
By contrast, later removing `auto_prune=false` **inside the inline TOML** asks
Core to use its default (`true`) when applying that resource. Definition patches
and desired-state TOML have different omission rules.

Choose exactly one source variant when creating or deliberately switching:

| Source | Example `source` |
| --- | --- |
| Inline | `{"kind":"inline","contents":"complete trusted TOML"}` |
| Direct Git | `{"kind":"git","repo":"owner/repo","branch":"main","provider":"github.com","https":true,"resource_paths":["resources.toml"]}` |
| Linked Repo | `{"kind":"linked_repo","target":{"name":"owned-repo"},"resource_paths":["resources.toml"]}` |
| Core-host file | `{"kind":"host","resource_paths":["resources.toml"]}` |

Core-host paths are relative to Core's sync directory and path-compatible sync
name, not the MCP machine. Paths must be explicit on creation; `[]` selects no
files. Switching sources clears conflicting active selectors. The product does
not commit/push Git, call CommitSync/WriteSyncFileContents or rewrite host files.
Prepare external files separately using your normal trusted source workflow.

## Separate cached inspection, refresh and review

```json
{"name":"get_resource_sync_pending","arguments":{"name":"owned-sync"}}
{"name":"refresh_resource_sync_pending","arguments":{"name":"owned-sync"}}
{"name":"review_resource_sync_pending","arguments":{"target":{"name":"owned-sync"},"selection":{"kind":"resources","resource_type":"server","targets":[{"name":"owned-server"}]}}}
```

Cached inspection is read-only. Standalone refresh makes one permission-checked
cache mutation and zero RunSync calls. It can fetch Git, read host files and
change alerts. An unchanged old cached hash is not current source evidence.

Review itself makes zero mutations. Read the selected creates, updates,
deletions, deployment intent, variable/group sections, exclusions, coverage gaps
and source observations. A missing `review_reference` means execution is not
authorized. Errors, incomplete evidence, truncation, unsupported dependencies
and unfaithful filtered deployment coverage withhold the reference. Every new
dependency edge must fit Core's creation order, including shared dependencies.

Existing named selections bind stable IDs; new creates bind exact typed names.
Other explicit selections are `{"kind":"resource_type","resource_type":"server"}`
and `{"kind":"whole_sync"}`. Filtered runs exclude variables/groups. Whole-sync
includes them only when enabled. Managed **or** delete enables deletion; empty
tags remove the resource restriction. Resource tags do not isolate global
variables/groups. A source containing deployment intent currently requires
whole-sync, included resources and unrestricted resource tags for authorization.
Exercise deletion/global-scope changes only where every affected object is owned.

## Apply once and inspect independent evidence

Copy the exact returned `review_reference` and full `apply_binding` into
`apply_resource_sync`, with optional `wait_timeout_seconds` (1–10, default 3).
The reference is private to this MCP process, expires after ten minutes and is
consumed when the attempt starts, including a stale, denied or unknown attempt.
Restarting MCP invalidates unused references. Tampered bindings are rejected.

Apply checks full private definitions, cached content and referenced resources,
makes at most one separately authorized preflight refresh, and rereads/audits
the fresh state. Changed inline/host content, Git revisions, definitions or
secret-only dependencies require a **new review** and submit zero RunSync.
Successful preflight permits at most one RunSync, with no automatic retry.
Record `preflight.refresh_requests_submitted` separately from
`run_sync_requests_submitted` and the receipt's submission window/update ID.
Core's additional internal refreshes are outside these explicit request counts.

Core rereads sources after the final comparison and accepts no expected revision.
Another writer can race that boundary. There is no source freeze, atomic
compare-and-swap, exactly-once guarantee, rollback or remote RunSync cancellation.

A known update ID can be observed from a new read-only MCP process:

```json
{"name":"wait_for_resource_sync_update","arguments":{"binding":{"sync_id":"COPY_SYNC_ID","selection":{"kind":"whole_sync"}},"update_id":"COPY_UPDATE_ID","timeout_seconds":10}}
```

Use the original **complete binding**; the abbreviated whole-sync form above
applies only if that was the reviewed selection. A local wait deadline stops
observation, not Core execution. Resume the known ID without resubmitting.
Check parent terminal outcome and sanitized batch failure counts. Partial failure
can leave successful changes. Core 2.3.2 can report a successful parent despite
a failed child deployment. History does not prove the selector or caller identity.

Inspect changed resources using their independent get tools, inspect deployment
or Stack state and use `fleet_health` separately. Pending cache, parent success
and observed definition differences do not prove exact desired-state equality,
causation or healthy workloads. Raw sync logs, source/error snippets and values
remain withheld, including through generic history.

## Recover an unknown acknowledgement

If no update ID was acknowledged, preserve `submission_unknown`, the original
binding and submission window. From a read-only process call:

```json
{"name":"reconcile_operation","arguments":{"target":{"kind":"resource_sync","id":"COPY_SYNC_ID"},"requested_scope":{"kind":"resource_sync","selection":{"kind":"whole_sync"}},"operation":"run","submission_window":{"started_ts":0,"ended_ts":0}}}
```

Copy the real selection and returned window rather than the illustrative zeros.
Candidate history remains **unconfirmed**, even for a single result: Core history
cannot verify RunSync selectors or invocation ownership. Reconciliation never
chooses a candidate or replays execution. Investigate independently before any
new explicit authorization. See [operation recovery](operations.md).

## Reproduce verification

Standard CI remains deterministic and does not contact a live Core:

```sh
cargo test --locked --test resource_sync_workflow --test sync_apply \
  --test sync_plans --test resource_syncs --test export
python3 -m unittest discover -s scripts -v
```

`resource_sync_workflow` composes the existing wire fixtures behind one
authenticated endpoint. It runs the actual binary through all four sources,
export/define/refresh/review/apply/inspect, omission resets, denial, source drift,
lost acknowledgement and read-only restart recovery. Component suites additionally
cover expiry, tampering/reuse, wait deadlines, concurrent callers, secret-only
changes, missing fields, pending errors and bounds. Wire fixtures do not implement
Core authorization, source fetching or Docker execution.

Opt into the complete live acceptance from a clean committed checkout with an
unused project and a new evidence path:

```sh
python3 scripts/disposable_resource_sync_smoke.py \
  --docker-context desktop-linux \
  --project komodo-mcp-smoke-resource-sync-example \
  --evidence target/resource-sync-example.json
```

This extends the **entire** [automation smoke](automation-walkthrough.md), then
runs the original Core/configuration/workload/recovery smoke too. It builds the
actual native executable and records commit/hash, MCP version, pinned
Core/Periphery 2.3.2, source revisions/digests, scopes, update IDs, permission
results, explicit refresh/RunSync counts and independent health. It exercises
all four actual sources, patch preservation versus desired defaults, tagged
managed deletion, variable/group whole/filtered scope, stale sources/references,
partial failure, failed child deployment, unhealthy workload, lost replies,
concurrent explicit callers and restart reconciliation. Expiry and selected
malformed-wire/timing races remain deterministic coverage, not live claims.

The runner owns the isolated Core database/users, private Git/host sources,
registry, nested Docker daemon and workloads. Only Core uses a loopback port;
no host Docker socket is mounted. The provisioner seeds/changes fixture sources;
MCP uses separate scoped identities and a designated test administrator without
fallback. Git refs and host digests are compared around product operations.
Per-call counts reject mutation from reads and repeated refresh/RunSync.
Before teardown, owned automatic triggers are disabled and read back. Docker
ownership is checked; containers, networks, volumes, identities in the disposable
database and private fixture material are removed. Cleanup failure fails the run
and preserves its private directory and actionable sanitized evidence.

Release readiness requires running this full smoke again from the **actual
final merged main commit**. Earlier focused/candidate proofs cannot substitute
for that report. A non-passing report or cleanup failure is not live acceptance.
