# Trusted Procedures

Procedures run ordered stages sequentially and enabled executions in each stage
in parallel. Disabled entries remain in the definition. `list_procedures` uses
bounded positive pages; `get_procedure` reports safe operation kinds, reference
IDs, enabled flags, counts and execution effects. It displays at most 16 stages
and 32 executions total (at most 16 per stage), marking incomplete evidence.
Arguments and webhook secrets are omitted.

## Authority and grants

Read-only remains the default. Definition tools require
`KOMODO_ALLOW_CONFIGURATION_EDIT=true`. Run/cancel requires the independent
`KOMODO_ALLOW_OPERATE=true`.

Core checks Procedure Read/Write/create policy. Saving supplied steps checks the
editor's Execute permission on references, **including disabled entries**;
Batch and maintenance types require Core admin. MCP privately binds readable
reference state without claiming Read proves Execute/admin. No credential
fallback occurs.

Running a saved Procedure requires Core Execute on the Procedure. **Children
run as Core's built-in administrator Procedure user.** The runner need not have
the editor's permissions on each child. Trusted saved steps can have destructive,
nested, dynamic or cloud effects; direct Build tools' Server-only scope does not
sandbox Procedures. This milestone's tests provision no AWS resources.

All nonempty existing-definition updates require both grants, including an alert
flag or disabling webhooks: Core refreshes scheduling after saves and automatic
policy can race. The shared transport gates every actual update, including an
empty SDK update. A deliberate empty-patch no-op sends no write. Creation
explicitly disables schedules/webhooks.

## Typed preview and apply

Creation uses `preview_procedure_create` / `apply_procedure_create` with `name`
and `config`. Supply `stages`; Core's general name normalization preserves case
and spaces. Creation forces `schedule:""`, `schedule_enabled:false` and
`webhook_enabled:false`; enabling webhooks during creation is rejected.

Updates use `preview_procedure_update` / `apply_procedure_update`, one exact
`target` and a typed `patch`. Allowed fields are `stages`, `webhook_enabled`
and `failure_alert`. Omission preserves; `stages:[]` deliberately clears.
Null, unknown and scheduling fields are rejected. Schedule tools are separate.

A supplied stage list is a complete ordered replacement. Every stage supplies
`name`, `enabled`, and `executions`. Each entry supplies `enabled` and a typed
`execution`. Parallel groups and disabled entries remain explicit:

```json
{"name":"preview_procedure_update","arguments":{"target":{"name":"my-procedure"},"patch":{"stages":[{"name":"prepare","enabled":true,"executions":[{"enabled":true,"execution":{"kind":"sleep","duration_ms":1000}}]},{"name":"parallel","enabled":true,"executions":[{"enabled":true,"execution":{"kind":"run_build","resource_id":"000000000000000000000011"}},{"enabled":false,"execution":{"kind":"run_action","resource_id":"000000000000000000000040","args":{"mode":"example"}}}]}]}}}
```

Use actual visible IDs, inspect effects/changes, then repeat the input plus
`preview_id` in apply. Stage changes are redacted; display projections are not
round-trip editable source.

| Typed `kind` | Fields and scope |
| --- | --- |
| `none` | No additional fields. |
| `sleep` | `duration_ms`, 0–86400000. |
| `run_build`, `cancel_build`, `clone_repo`, `pull_repo`, `run_procedure`, `cancel_procedure` | Stable `resource_id`. Procedure cancellation supplies no historical Core update guard. |
| `deploy_deployment`, `start_deployment`, `restart_deployment`, `stop_deployment` | Stable `resource_id`; saved/default lifecycle settings. |
| `deploy_stack`, `start_stack`, `restart_stack`, `stop_stack` | Stable `resource_id`; whole saved Stack, no service override. |
| `run_action` | `resource_id`, explicit bounded JSON-object `args`. `{}` uses saved defaults and deliberately clears step overrides; values stay private. |
| `batch_run_build`, `batch_run_action`, `batch_run_procedure` | Admin-only `pattern`, at most 1024 bytes. Dynamic expansion under Core's built-in identity; no tag filter is supplied. |
| `global_auto_update` | Admin-only `skip_auto_update` boolean; broad Core-defined effects. |
| `clear_repo_cache` | Admin-only, no additional fields; broad cache effects. |

Inputs fit in 16384 serialized JSON bytes, at most 16 stages, 32 executions per
stage, 64 total executions and 32 distinct static references. Names are 1–128
bytes without controls. Action arguments fit in 8192 bytes with bounded nesting
and collections. Unsupported saved step types remain inspectable by kind and
are preserved during unrelated edits; they are never converted or dropped.
For a supplied `run_action` step, `args` is required so omission cannot silently clear private step arguments; use `{}` deliberately. There is no generic SDK execution/config passthrough.

Shared ten-minute single-use previews bind input, private full definition and
bounded readable static references. Apply rechecks, consumes the preview, submits
at most one partial write and verifies echo plus independent complete saved-state
read-back. Omitted steps, arguments, secrets and schedule are preserved. Hidden,
dynamic, nested and capped reference evidence remains explicit; Core validates
permissions authoritatively. No CAS, automatic retry or rollback is promised.

## Run and observe

Inspect `get_procedure.effects`, then copy its acknowledgement:

```json
{"name":"run_procedure","arguments":{"target":{"name":"my-procedure"},"expected_effects":{"core_admin_authority":true,"broader_effects_possible":true}}}
```

The tool resolves one exact saved Procedure, rejects templates, and submits its
stable ID once without writing configuration. Core rereads steps asynchronously,
so configuration/references can change after inspection.

The shared receipt separates submission, terminal outcome and workload health.
Save `update_id`; resume read-only with `get_update` / `wait_for_update`. After
a lost/corrupt reply, `reconcile_operation` uses target kind `procedure`, scope
`whole_procedure`, operation `run` or `cancel`. Every history candidate remains
unconfirmed, even a single same-operator/time match. Expired waits, local
cancellation and process restart never replay execution.

`observed_procedure_result` decodes bounded reports from a unique parent execution
log, including Core's pinned HTML formatter. Raw Execution Debug and Procedure
error chains can contain private arguments and are omitted. Safe stage names,
operation kinds and narrow reported child IDs remain available.

Core prints **Finished execution even for failed children**; per-execution
`terminal_success` stays unknown. It waits for all parallel children but reports
only the first collected error. `child_results_complete` is false. Inspect
reported child IDs separately with `get_update`; these parent-error URL reports
have unverified association and do not establish caller ownership, complete child
coverage or deployment health. Missing, malformed, ambiguous and truncated
evidence stays explicit; current saved steps are not a historical-run snapshot.

## Explicit cancellation

```json
{"name":"cancel_procedure","arguments":{"target":{"id":"000000000000000000000030"},"expected_update_id":"000000000000000000000100"}}
```

Supply the original RunProcedure ID. MCP validates its target/operation, checks
bounded nonterminal history and Core running state, then rechecks before one
cancellation. Finished, replacement, ambiguous, truncated and inactive runs send
nothing; later completed busy/failed requests are not the active run.

Core 2.3.2 cancels by **Procedure resource** and ignores optional `update_id`.
The SDK sends that field as null; the locally checked historical ID is never
sent as an atomic guard. A replacement can race after final reads and receive
cancellation instead. The result exposes this limit and replacement observations.

Core checks cancellation before stages: current children finish first, with no
child cancellation or rollback. During the last stage the original may still
succeed. The cancellation receipt acknowledges the token; `original_update`
and `original_terminal_confirmed` describe the run separately. A complete cancel
update does not prove original termination. Lost replies remain unknown.
Cancelling a local wait or closing MCP sends no remote cancellation.

`cargo test --locked --test procedures` covers these contracts through compiled
production stdio. Deterministic fixtures do not implement Core authorization;
integrated live acceptance remains mandatory in [#93](https://github.com/enthouan/komodo-mcp/issues/93).

Pinned sources: [types](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/entities/procedure.rs),
[definition validation/hooks](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/resource/procedure.rs),
[run/cancel](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/execute/procedure.rs),
[parallel execution/child reports](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/helpers/procedure.rs),
[execution identity](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/entities/user.rs)
and [log formatting](https://github.com/moghtech/komodo/blob/v2.3.2/lib/formatting/src/lib.rs).
