# Follow a revision through trusted automation

Use Core/Periphery **2.3.2** and a compiled `komodo-mcp` stdio server. The
[configuration](configuration.md) and [operations](operations.md) contracts also
apply here. Definitions, execution acknowledgement, terminal execution outcome,
and workload health are separate evidence. Keep the returned stable resource
IDs and update IDs throughout the workflow.

## Authority and permissions

| Work | Local grants | Core permissions |
| --- | --- | --- |
| Inspect definitions, schedules, updates and health | Default read-only | Read on each visible resource; additional Inspect/Logs where required |
| Create inert Repo/Build/Procedure definitions | Configuration edit; operate when source/cache effects require it | Core creation policy, enabled user; Build creation permission; Read/Attach on referenced Servers and access to selected resources |
| Create a Server Builder or Action | Configuration edit, plus operate for execution-bearing configuration | Core administrator; Builder creation requires explicit existing Servers |
| Update execution-bearing definitions or schedules | Both operate and configuration edit | Owner Write and Core's reference/attachment validation; Procedure step permissions also apply |
| Clone/pull Repo or run saved Build/Procedure/Action | Operate | Execute on the selected resource; direct Build also needs readable supported Builder/Server definitions |
| Explicitly cancel an execution | Operate | Core's cancellation permission on the selected resource, plus readable original update/history |

An MCP capability never grants a Core permission. The caller's credentials are
used throughout; a denied request has no administrator fallback. Setting both
`KOMODO_ALLOW_OPERATE=true` and `KOMODO_ALLOW_CONFIGURATION_EDIT=true` opts into
execution-bearing configuration. Default sessions remain read-only.

Direct Builder writes and Build execution support explicit **Server** Builders
only. The ordered Server list preserves Core's fallback behavior. The configured
list does not attest which Server actually ran a build. AWS/URL Builders remain
inspectable but are rejected before unsupported mutation/execution; no AWS
resources are provisioned by the acceptance tests.

Hooks, Dockerfiles, Procedures and Actions are trusted code. Core executes
Procedure children with its built-in administrator identity. Actions run Deno
with `--allow-all` on the Core host and receive a temporary Core administrator
API key. Their arbitrary behavior can exceed the selected resource's authority
and the direct tools' Server-builder scope. Do not treat these tools as a sandbox.
Action output is omitted from MCP because it can contain unknown arguments or
temporary credentials. Structured outcomes remain available.

## Select and save definitions

Resolve exact resources with `get_server`, `get_repo`, `get_builder` and
`get_build`. Use bounded `list_*` continuation when discovery is needed. Copy IDs
from responses rather than reconstructing them from names.

Create or update through the appropriate `preview_*` and `apply_*` pair. Review
the returned changes/effects, then apply the same input with its `preview_id`.
Previews are private, bounded, process-local, ten-minute and single-use. A stale
baseline/reference requires a new preview. Confirm
`saved_configuration_verified == true`; a submission with unavailable read-back
is not verified saving. Core offers no general compare-and-swap: a writer can
still race after final reads. Never automatically repeat an uncertain apply.

For a Repo, supply a provider host, repository path, branch and explicit Server
target. `git_https` selects HTTPS versus HTTP in pinned Core. A static bare Git
fixture needs its actual `.git` path. Keep credentials in a credential reference,
away from repository/provider strings. Inspect `get_repo.definition` for hooks.
For a Build, select an explicit Server Builder and typed Git/linked-Repo/inline/
host source, image name and registry entries. Saving Git/host Build configuration
can refresh Core's Dockerfile cache. Build saves do not themselves submit a
RunBuild; webhooks and dependent redeployment policy can cause other effects.

## Clone, build, deploy and check health

1. Call `get_repo` and inspect trusted hooks. Pass its
   `on_clone_configured`/`on_pull_configured` values as `expected_hooks` when
   calling `clone_repo` or `pull_repo`. Clone can run both hooks; pull can run
   on-pull. Each explicit call submits at most one execution using the selected ID.
2. Retain the receipt's `update_id`, then use `get_update` or bounded
   `wait_for_update`. Require the correct resource/operation and terminal outcome.
   Record `observed_revision.revision` when Core reports it. Configured branch or
   commit selectors are separate from this observed evidence.
3. Call `get_build`. Inspect `configured_effects` and pass its `acknowledgement`
   as `expected_effects` to `run_build`. This discloses pre-build code, registry
   push potential and possible dependent redeployments. Execution uses the saved
   definition; successful Core auto-version advancement is a Core side effect.
4. Follow the Build update. Record the observed revision,
   `observed_build_result.version`, image references and digest when available.
   Image evidence is derived conservatively from a unique bounded Docker Build
   stage. Missing/truncated/ambiguous evidence stays unknown. A configured
   registry alone proves no push; independently inspect a registry/image when
   stronger proof is needed.
   Retain at least one tag policy when pushing; disabling latest, version and
   commit tags together can leave Core's build command without a push tag.
5. Use a typed Deployment image reference to the selected Build and the explicit
   observed version. `0.0.0` deliberately means latest and may change. Preview,
   apply and read back the definition, then explicitly call `deploy_deployment`.
   Retain its separate update ID. Core's asynchronous auto-redeploy does not
   expose child deployment update links in the Build update. Leave such links
   unknown; timing, matching actor or a single candidate does not prove causality.
6. After terminal deployment success, call `fleet_health` or inspect the workload
   through the appropriate incident tools. State the current health and gaps.
   Successful command completion can coexist with an exited or unhealthy
   container. The disposable test additionally checks nested Docker health,
   image digest and the source marker inside the actual container.

## Procedures, Actions and schedules

Create Procedures with explicit ordered stages; executions within one stage run
in parallel. Preserve enabled/disabled entries. `get_procedure.effects` provides
the acknowledgement for `run_procedure`. Parent completion does not establish
every child's success or health. Only explicit child update IDs can be followed;
missing links remain unknown. Failed child steps and partial results need their
own evidence.

Use `get_action.effects.acknowledgement` for `run_action.expected_effects`. Optional
per-run `args` override saved defaults without saving source/default changes.
Pinned Core recursively merges objects and appends arrays. Empty objects/arrays
do not clear saved values per run; incompatible object/array replacements can
fail. Keep argument values private. Running either resource does not silently
rewrite its saved definition.

For either owner kind, call `preview_schedule_update` then
`apply_schedule_update`, supplying `owner_kind`, exact `target` and a typed patch.
For example:

```json
{
  "owner_kind": "action",
  "target": {"id": "COPY_ACTION_ID"},
  "patch": {
    "expression": "0 0 9 * * *",
    "format": "cron",
    "timezone": "America/New_York",
    "enabled": true,
    "alert": false
  }
}
```

Cron requires seconds. `english` uses Core's English conversion; its SDK default
is English. Inspect `get_schedule.saved` and the separate `observation` from
Core's schedule cache. The effective next time is
`observation.reported.next_scheduled_run` in UTC Unix milliseconds, when present.
An empty timezone inherits Core's configured timezone or Core host local time,
never the MCP host's timezone. Invalid expressions/timezones can save successfully
while Core reports a separate parse error. Disabled owners are not parsed.

Update the expression/timezone with another preview/apply, then disable with
`{"enabled":false}` or clear with `{"expression":""}`. Source, defaults, stages
and omitted secrets remain unchanged. Disabling does not stop work already
dispatched. An in-flight execution can reschedule using its captured old policy;
Core's cache has no atomic generation link to the saved definition. Inspect both
after editing. `last_run_at` can describe a completed failed or manually invoked
run; it does not identify a scheduled trigger or imply health.

## Timeouts, cancellation and recovery

An expired or locally cancelled wait ends observation only. It sends no remote
cancellation and does not replay execution. Resume `get_update`/`wait_for_update`
with a known ID, including after restarting MCP. Observation failures preserve
the acknowledged update ID.

After a lost/invalid acknowledgement, preserve `submission: unknown`. Restart
and call read-only `reconcile_operation` with the original stable target, scope,
operation and available submission window. Candidates remain **unconfirmed**
even with one match; concurrent callers can produce indistinguishable updates.
Reconciliation never resubmits work.

Cancel only with a separate explicit `cancel_build`, `cancel_procedure` or
`cancel_action`, giving the original `expected_update_id`. Inspect
`cancellation_receipt` separately from `original_update` and
`original_terminal_confirmed`. Build/Procedure Core APIs ultimately cancel by
resource, so MCP checks bounded active history/action state but cannot eliminate
the final replacement-run race. Procedure cancellation is checked between stages;
current children can finish and the last stage may still succeed. Action
cancellation sends the verified exact run ID; concurrent Action invocations are
supported. Core token/process cancellation is not rollback and need not cancel
child Core API operations. Already-completed or detected replacement runs submit
no cancellation. Missing permission/history evidence prevents confident targeting.

## Reproduce acceptance

Normal CI needs no live Core, Docker or AWS:

```sh
cargo test --locked --all-targets
python3 -m unittest discover -s scripts -v
```

`tests/automation.rs` runs one production stdio session across Repo, Builder,
Build, Procedure, Action, schedules and explicit deployment/independent health.
The component suites add all local grant combinations, upstream denials, stale
previews, private preservation, large sanitized output, AWS/URL rejection,
failed stages/partial results, cancellation races, expired/local-cancelled waits,
lost acknowledgements, process restart and concurrent operators. These fixtures
model pinned wire behavior; they do not run Core's authorization or Docker engine.

Opt into real pinned Core/Periphery and disposable user-workload images from a
clean committed checkout. Choose an unused project and a new evidence file:

```sh
python3 scripts/disposable_automation_smoke.py \
  --docker-context desktop-linux \
  --project komodo-mcp-smoke-automation-example \
  --evidence target/automation-example.json
```

The runner builds the actual native release binary and records its commit/hash
and MCP version. It creates a private HTTPS Git source with an explicitly trusted
fixture CA, private registry, nested Docker daemon and BusyBox workload. Core and
Periphery are 2.3.2; registry is 2.8.3; source is nginx 1.27.5-alpine. Only Core is
published on loopback. No host Docker socket is mounted. Use a local Unix-socket
context; the runner refuses an existing project or evidence path.

Provisioning credentials create scoped enabled Read/Execute/Write service users
and a separate, explicitly designated non-super-admin test identity for Core's
admin-only creation APIs. The provisioner never enters an MCP process or serves
as a fallback. Evidence distinguishes these identities and live permission
results. All original disposable operation/configuration/deletion tests also run.

The JSON report records versions, built commit/hash, selected resources, source
revision, update IDs, image/version/digest, independent container contents/health,
schedule cache observations, permission checks, fault/recovery evidence and gaps.
Per-call transport counters require at most one mutation and zero mutations from
read-only observation. Lost-reply concurrency uses two separate upstream users.
It omits credentials, Action source/defaults/arguments and arbitrary Action logs.
Failures retain a non-passing report. Before teardown the runner verifies owned
identities and disables/clears automatic execution; failure of that step fails
acceptance. Cleanup still runs, verifies Docker ownership labels and requires zero
project containers/networks/volumes. Private provisioning files and fixture TLS
keys are removed only after verified teardown. Preserve a failed cleanup's private
directory for inspection and use its exact owned project for recovery.

Final acceptance must identify the actual integrated commit being evaluated.
Candidate evidence alone is insufficient if the final implementation changes.
Building/pushing these disposable user-workload images publishes no komodo-mcp
distribution. Project distribution remains annotated source tags and GitHub
Release notes; native/container distribution is tracked separately.
