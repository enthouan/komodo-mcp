# Server Builders and Build definitions

`list_builders` / `get_builder` and `list_builds` / `get_build` are read-only in
every local capability profile. They use exact stable-ID/name resolution and
positive pages of 1–100 items, ascending name order and at most one lookahead.
All new reads use the shared 2 MiB body cap and 15-second complete-request
deadline. Names and safe selectors are bounded to 256 characters.

Inspection omits credential names/tokens, scripts, Dockerfiles, build/secret
arguments, cloud settings, private cache errors and addresses/passkeys on URL
Builders. `cached_built_revision` / `cached_latest_revision` are current Core
cache observations. They are separate from configured source, pin and version,
and do not prove a particular execution, registry push or deployment health.

## Scope and minimum permissions

Direct Builder writes and Build definitions support **existing-Server Builders**
only. AWS configuration/provisioning is deferred beyond v0.5, and URL Builders
are unsupported for these writes and direct Build execution. Unsupported types
remain inspectable with explicit support limits and no credentials. A Builder
cannot be silently converted by supplying Server configuration. A Build can be
explicitly reassociated with a selected supported Server Builder.

Core requires an enabled identity, Read for inspection, Write on an existing
definition and Read plus Attach on newly selected Servers, Builders and Repos.
Builder creation is **admin-only** in Core 2.3.2. Build creation requires admin
or Core's enabled non-admin creation policy and the caller's Build creation
permission. Local grants do not replace these checks. Denial never borrows
provisioning/admin credentials.

Set `KOMODO_ALLOW_CONFIGURATION_EDIT=true` for preview/apply discovery. Applying
a supplied Server list to an existing Builder needs `KOMODO_ALLOW_OPERATE=true`
as well: the Builder may be used by automatically triggered Builds, and policy
can change after inspection. Creating a standalone Server Builder requires
local edit access and Core admin authority; the inspected write hooks do no
execution or provisioning.

Every nonempty Build update needs **both local grants**, including unchanged
supplied fields and webhook disabling. Core's post-write cache refresh may use
the saved source, and that source/webhook policy can race. An empty update is a
zero-write no-op with edit alone. Inline-only Build creation with webhooks off
permits edit alone; Git, linked-Repo or host creation, or enabling webhooks, needs
both. Creation explicitly defaults webhooks off instead of inheriting Core's
enabled default. Handlers, discovery/direct dispatch and the shared typed
transport enforce the same local authority boundary.

## Ordered Server Builder configuration

Create a Builder using an explicit Server selector:

```json
{"name":"preview_builder_create","arguments":{"name":"example-builder","config":{"kind":"server","servers":[{"name":"primary-builder-server"},{"name":"overflow-builder-server"}]}}}
```

The preview resolves Server IDs in that exact order. Inspect its effects and
apply the **same name/config** plus the returned `preview_id` to
`apply_builder_create`. The wire configuration is always explicitly
`{"type":"Server","params":{"server_ids":["…","…"]}}`.

The pinned full SDK default is AWS and the partial default is URL. Neither is
used to choose a Builder variant here. The ordered Server list is Core's
overflow/fallback priority; neither IDs nor input selectors are sorted.

Updates use `preview_builder_update` / `apply_builder_update`:

```json
{"target":{"id":"000000000000000000000010"},"patch":{"kind":"server","servers":[{"id":"000000000000000000000002"},{"id":"000000000000000000000001"}]}}
```

Omit `servers` to preserve it. `servers:[]` deliberately clears it and leaves no
Server available for a future build. Creation requires 1–16 Servers; duplicate
resolved IDs and templates are rejected. An empty supported list can be
inspected, but a Build write needs an associated Builder with existing Servers.
Only `kind:server` and the typed Server selectors are accepted; no AWS, URL or
SDK passthrough fields are available.

## Build sources, image and trusted settings

Create a Build with an explicit Builder, source, image name and registry list:

```json
{"name":"preview_build_create","arguments":{"name":"example-build","config":{"builder":{"name":"example-builder"},"source":{"kind":"git","repo":"example/workload","branch":"main"},"image_name":"workload","image_registry":[{"domain":"registry.example.invalid:5000","account":"existing-account-reference","organization":"owned-workloads"}]}}}
```

An empty image name uses the normalized Build name. `image_registry:[]` means
local-only and explicitly clears a previous registry list. Registries retain
their supplied order: Core uses the **first registry** for attached Deployments.
Accounts are existing references, never supplied token values. Registry domains
exclude schemes, credentials, paths and queries; organizations/image names and
tags use bounded safe selectors. This tool does not provision a registry.

The source is one of:

| `kind` | Typed fields and effect |
| --- | --- |
| `git` | Required `repo` and `branch`; optional `git_provider`, `git_https`, `commit`. Git credentials use top-level `git_account`. |
| `linked_repo` | Exact Repo `target`; binds its private configuration and preserves its source/credential authority. |
| `inline` | Complete nonempty trusted `dockerfile` text. |
| `host` | Use saved `build_path` and `dockerfile_path` on the selected Builder Server. |

Supplying a source deliberately clears competing `files_on_host`, `repo`,
`linked_repo` and `dockerfile` selectors as appropriate. Omitted Git provider,
HTTPS, pin and credential fields remain unchanged, including across a source
switch. `commit:""` inside a Git source clears a pin. This is explicit source
selection rather than a generic SDK configuration passthrough.
Git/linked-Repo build and Dockerfile paths stay relative to the source cache:
absolute paths, parent traversal and shell syntax are rejected, including in
inherited configuration before an ordinary edit triggers a cache refresh.
Explicit host mode uses the selected Server and permits trusted host paths.
Trusted source and Core filesystem state, including symlinks, still govern the
cache reader under Core's built-in identity.

Updates take `target` and a typed `patch` with these optional fields:

| Fields | Omission / explicit clearing |
| --- | --- |
| `builder`, `source` | Preserve; supply a new supported exact reference/source to change. |
| `image_name`, `image_tag` | Preserve; `""` restores name fallback or clears the extra tag. |
| `image_registry` | Complete ordered replacement, at most 8; `[]` clears. Each entry supplies domain/account/organization. |
| `git_account` | Preserve existing reference; `""` clears it. |
| `version` | Explicit nonnegative `major.minor.patch`; preserve when omitted. |
| `auto_increment_version`, `include_latest_tag`, `include_version_tags`, `include_commit_tag`, `webhook_enabled`, `use_buildx`, `skip_secret_interp` | Preserve; explicit booleans change policy. |
| `pre_build` | Complete trusted `{path,command,shell_mode}`; empty command disables the hook. |
| `build_path`, `dockerfile_path` | Preserve; use `.` / `Dockerfile` to restore defaults. Host paths are trusted configuration. |
| `build_args`, `secret_args`, `labels` | Complete replacement strings; `""` clears. Build args can be exposed in the resulting image. |
| `extra_args` | Complete trusted Docker argument list, at most 32; `[]` clears. Core's blank-entry normalization is preserved. |

Unknown fields and null are rejected. Configuration payloads fit within 16384
serialized JSON bytes; Dockerfile/command/argument strings have additional
bounds. Display redaction markers cannot be copied back into configuration.
Executable text, argument values, paths and registry-account objects are redacted
in preview changes. Omitted webhook secrets, other scripts, links and unrelated
settings are preserved, including in the complete read-back comparison.

## Effects, visible references and saved-state evidence

These definition tools submit **zero Build execution requests**. Core 2.3.2
still performs work during Build creation/update: its post-write hook refreshes
the Build cache using Core's built-in build identity. A Git/linked-Repo source
can clone/pull into Core's cache and read a Dockerfile; a host source can contact
Periphery to read the host Dockerfile. This cache path does **not** carry Repo
hooks or run the pre-build command. Cache failure can be logged after the
definition was saved. A write API does not imply an absence of runtime effects.

Dockerfiles, hooks, build arguments and extra Docker options are trusted
configuration executed by a later build. Future builds may push images and Core
may redeploy linked Deployments with `redeploy_on_build`. Previews inspect at
most 16 visible linked Deployments, reporting truncation/unavailable evidence.
Hidden, new and concurrently changed dependents may exist; the preview never
claims global coverage, successful redeployment or health.

Previews bind the private full baseline, actual Server Builder and all ordered
Server configuration/trust, linked Repo and relevant visible credential evidence.
Credential checks inspect filtered database account records when Core permits
them and token-free config listings for **each** fallback Server. Database token
values remain private and are never returned/logged. If every source is readable
and the selected reference is absent, preview rejects it. Denied/unavailable
sources remain an explicit evidence gap; no elevated fallback occurs. A visible
account is not proof that its token works or that a registry push is authorized.

Apply consumes the common bounded ten-minute, process-local single-use preview,
revalidates its input, raw baseline, references and visible dependent policy, and
sends at most one typed partial write. Previews, stale applies and no-ops write
zero times. A shared transport recheck rejects unsupported Builder changes
before submitting a mutation; a known preflight rejection stays distinct from
an uncertain attempted write.

A valid write echo is followed by an independent bounded authoritative read of
the entire expected saved config. `outcome:saved` acknowledges the write;
`saved_configuration_verified:true` additionally requires this read-back. A
failed or changed read-back retains acknowledgement with verification false.
Lost/invalid write replies return `submission_unknown`. No automatic retry,
execution replay or privilege fallback follows any uncertain outcome.

Core has no general compare-and-swap: another operator can change source,
Builder, Servers or automatic execution after the last read. Read-back is an
observation, not a lock or future guarantee. Saved definition/configuration,
cached revision, build completion and deployment health remain separate claims.

## Validation and pinned sources

`cargo test --locked --test build_definitions` drives the compiled stdio server
against deterministic pinned-wire fixtures. It covers capability profiles and
direct calls, explicit Server wire variants/order, unsupported type inspection
and rejection, creation/attachment denial, secret preservation and stale private
references, one-field/no-op/clear writes, credential evidence limits, preflight
variant races, uncertain replies and independent read-back failures. The fixture
simulates permissions; it does not implement Core's authorization engine.

The integrated live Core/Periphery test and final source → build → image →
deployment/health proof belong to [#93](https://github.com/enthouan/komodo-mcp/issues/93).
Build execution/cancellation is described below ([#24](https://github.com/enthouan/komodo-mcp/issues/24)).

Pinned Core 2.3.2 sources: [Builder types/defaults](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/entities/builder.rs),
[Builder permissions and write hooks](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/resource/builder.rs),
[Build fields](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/entities/build.rs),
[Build resource hooks](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/resource/build.rs),
[cache refresh](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/write/build.rs),
[credential/config reads](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/read/provider.rs),
and [build-triggered redeployment](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/execute/build.rs).

## Run one saved Build

`run_build` is exposed with `KOMODO_ALLOW_OPERATE=true`; configuration-edit
access is independent and unnecessary. The same Core identity needs Read access
to the Build, actual Builder and its ordered Server definitions, and Core Execute
on the Build. No elevated credentials are substituted.

First call `get_build` and inspect `configured_effects`: the saved trusted
pre-build command, source/image settings, registry pushes, visible linked
Deployments and visibility gaps. Copy its `acknowledgement` object into the
explicit execution call:

```json
{"name":"run_build","arguments":{"target":{"name":"my-build"},"expected_effects":{"pre_build":true,"registry_pushes":true,"dependent_redeployments_possible":true}}}
```

Use the actual returned flags. `dependent_redeployments_possible` must always be
acknowledged: hidden, new or concurrently changed dependents can exist. Core may
run those Deployments under its built-in automatic-redeploy identity. Linked
Repo clone/pull hooks are explicitly suppressed in this Build execution path;
the Build's own pre-build command and Docker instructions remain trusted code.

The tool resolves the exact saved Build, checks an explicit nonempty Server
Builder and preserves Server order. Immediately before one stable-ID RunBuild,
the shared transport rereads the actual association, Builder type and Servers.
AWS/URL variants and templates are rejected without execution. Core chooses
cached-healthy Servers by least current usage, breaking ties by configured order;
its update does not attest the chosen Server. Another operator can change the
Builder, source, hooks or policy after preflight. Core has no CAS here; this is
not cloud isolation against concurrent configuration changes.

No definition write or per-run override is sent. Core can auto-increment the
attempted version and save `config.version` on successful completion. The shared
receipt separates `submission` from the observed update's terminal outcome and
workload health. Retain `update_id` and use `get_update` / `wait_for_update` to
resume read-only observation after deadlines or process restart. A lost/invalid
acknowledgement stays unknown; `reconcile_operation` with `kind:"build"`,
`whole_build` and `operation:"run"` returns unconfirmed history candidates,
even when only one matches. It never replays execution or assigns another
operator's concurrent run to this call.

Build update evidence uses structured commit/version fields when available.
`observed_build_result.version` is an attempted version, including on failure.
Image references/digests come only from a bounded unique Docker Build stage's
recognizable Docker/BuildKit tag or manifest-push reports. Configured tags,
commands and arbitrary output are not image evidence; missing, conflicting and
truncated evidence stays explicit. A reported manifest push is not an independent
registry verification. Build and push failures can share the Docker Build stage;
`registry_push_failure_reported` identifies a recognizable engine push error.

Core 2.3.2 starts dependent redeployments asynchronously and returns no child
Deployment update IDs/results in a Build update. Inspect the disclosed Deployment
IDs, their history and runtime health separately. Timing or a successful Build
never proves that a particular Deployment ran or became healthy.

## Explicit cancellation and its race

```json
{"name":"cancel_build","arguments":{"target":{"id":"000000000000000000000011"},"expected_update_id":"000000000000000000000100"}}
```

Supply the selected original RunBuild ID, not an example ID. Operate and Core
Execute are required. MCP validates its resource and operation, checks bounded
nonterminal RunBuild history plus Core's building state, and rechecks both before
one CancelBuild. Completed originals, replacement/different active runs,
ambiguous history, truncated active evidence and nonbuilding state submit zero
cancellations. Later completed busy/denied RunBuild updates do not substitute for
the active run. Read failures stop preflight without an execution submission.

**Core accepts only a Build ID, with no expected update ID.** Another run can
replace the original after the final reads, so cancellation can affect that run.
`expected_update_id` is a local check, not an atomic Core guard. The result reports
this race, the known original ID, active evidence and replacement observations.

`cancellation_receipt` describes submission/observation of the CancelBuild update;
`original_update` and `original_terminal_confirmed` describe the original RunBuild
separately. Core marks its cancellation update complete when queued, or during
its timeout cleanup; this does not prove the original stopped. A terminal
original can have succeeded before cancellation took effect. No rollback or
cancellation of a specific historical invocation is claimed. Lost replies stay
unknown and never repeat CancelBuild; read-only recovery uses `operation:"cancel"`.
Cancelling a local `wait_for_update` or terminating the MCP process sends **no**
remote cancellation.

`cargo test --locked --test build_operations` covers these contracts through the
compiled production stdio server: success and observed evidence, failed hooks/
build/push, upstream denial, all local grants, unsupported/raced Builders,
concurrent calls, missing/lost/corrupt acknowledgements, restart/recovery,
bounded waits, explicit cancellation, completed originals and replacement races.
The fixture is deterministic; the live environment and registry/deployment proof
remain required in #93.
