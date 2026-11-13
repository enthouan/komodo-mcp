# Typed configuration edits

Set `KOMODO_ALLOW_CONFIGURATION_EDIT=true` in the MCP child environment. The
default is false. This trusted grant is independent of
`KOMODO_ALLOW_OPERATE`; Core remains the permission authority. Preview needs
Read and saving an existing definition needs Write on the Stack. A new attachment
or linked Repo also needs Read plus the specific Attach permission on that
resource. Creation requires Core's creation policy to allow the caller (an enabled
non-admin user needs non-admin creation enabled). Local configuration access alone
grants none of these permissions. Use the same credentials throughout; no
permission or credential fallback occurs.

Resolve the Stack with `get_stack`, then preview with the real stable ID:

```json
{"name":"preview_stack_update","arguments":{"target":{"id":"000000000000000000000002"},"patch":{"auto_pull":false}}}
```

Inspect the resolved resource, redacted `changes`, effects and `preview_id`.
Preview issues **zero writes**. Apply the exact same target and patch, adding
that preview ID:

```json
{"name":"apply_stack_update","arguments":{"target":{"id":"000000000000000000000002"},"patch":{"auto_pull":false},"preview_id":"<returned preview_id>"}}
```

Stack settings support:

| Field | Omitted | Explicit clear | Preview |
| --- | --- | --- | --- |
| `auto_pull` | Preserve | Unsupported; use true/false | Before/after boolean |
| `ignore_services` | Preserve | `[]` | Before/after ignored service sets |
| `environment` | Preserve | `""` | Nonempty values redacted, including secret references |
| `project_name` | Preserve | `""` uses the Stack name | Docker-normalized project name, at most 128 bytes |
| `source` | Preserve every source field | Unsupported | Declared source and its effects, described below |
| `attachment` | Preserve target | Unsupported | Existing resolved Server or Swarm |

Unknown fields and JSON null are rejected. Patches are limited to 16384 JSON
bytes; ignore lists to 32 nonempty names of at most 128 bytes. Environment is
the complete replacement string for that one field. Preserve any references
you intend to keep when replacing it. A display marker such as `[redacted]` or
`[hidden]` is rejected, never written as configuration. An omitted environment
leaves its real saved secret references/placeholders untouched. The one-field
request uses the pinned SDK's partial type and sends no unrelated defaults or
nulls. The pinned environment deserializer appends a final newline to a nonempty
string when needed; comparison and submission use that canonical value. Inline
Compose uses the same pinned newline normalization.

## Create and inspect a Stack

Use `preview_stack_create`, inspect the normalized name/source/target/effects, then
submit the exact input plus its `preview_id` to `apply_stack_create`:

```json
{"name":"preview_stack_create","arguments":{"name":"example-stack","config":{"attachment":{"kind":"server","target":{"name":"example-server"}},"source":{"kind":"inline","contents":"services:\n  app:\n    image: busybox:1.37.0\n"},"auto_pull":false}}}
```

Creation requires an explicit source and attachment. Names follow Core's Docker
normalization (trim, lowercase, replace unsupported characters with underscores,
strip leading nonalphanumerics); the preview shows the resulting name. Empty,
overlong and ObjectId-shaped normalized names are rejected. The preview's resource
ID is null until Core returns the new stable ID. Inspect that ID with `get_stack`,
then use `preview_stack_update`/`apply_stack_update` for a one-setting patch. Saving
`{"auto_pull":true}` sends just that field and preserves the existing Compose
source, credentials, environment references and target.

Creation checks bounded visible name absence and rechecks it at apply. Hidden
resources and concurrent creators remain possible; Core enforces global
uniqueness and creation authorization. A failed/lost create response stays
uncertain and must not be retried automatically.

## Declare sources and targets

Both creation's `config` and update's `patch` accept these source shapes:

| `source.kind` | Required fields | Optional fields and effects |
| --- | --- | --- |
| `inline` | `contents` (nonempty Compose) | `run_directory`, `file_paths`; saves inline content and clears conflicting host/Git/linked-Repo selectors |
| `git` | `repo` (`namespace/repo`) | `provider` (host with optional port), `branch`, `https`, `account`, `commit`, `run_directory`, `file_paths`; changes the saved source pointer and clears conflicting host/linked/inline selectors |
| `linked_repo` | `target` (existing Repo ID or exact name) | `run_directory`, `file_paths`; resolves and saves the Repo ID, clears conflicting host/direct-Git/inline selectors; Git settings come from the referenced Repo |
| `host` | `run_directory`, `file_paths` | Selects existing host files; preserves inactive inline/Git fields |

The `attachment` is `{"kind":"server","target":{"id":"<stable ID>"}}`
or `{"kind":"swarm","target":{"name":"<exact name>"}}`. Selectors require
exactly one ID or name and use the existing bounded positive-match resolution;
template or ambiguous matches are rejected. Switching target kinds clears the
opposite target ID. These tools use pre-existing targets; they create no host or
Swarm.

Omitted source options preserve existing fields. `run_directory:""` selects the
source root; `file_paths:[]` selects Core's default `compose.yaml`. Git `account:""`
clears the credential-account reference and `commit:""` clears a pinned commit.
An omitted account preserves the real reference even across source changes.
Empty repo/provider/branch/inline content and all JSON nulls are unsupported.
Repo/branch inputs use a bounded ASCII subset (letters, numbers, `.`, `_`, `-`,
`/`, no empty/dot traversal segments; branch cannot start with `-`). Commit is
an empty string or at most 64 hex digits. Direct Git URLs, embedded credentials,
shell syntax and provider paths are rejected. Directories are at most 1024 bytes;
there are at most 32 nonempty paths of 512 bytes each, without NUL/newlines.
The complete typed patch remains limited to 16384 JSON bytes.

`definition.details` separates before/after desired source/target from the
runtime state observed at preview time. It identifies source precedence as host,
linked Repo, direct Git, then inline. Inline/environment/account values stay
redacted; no Compose text or credential reference is returned. Locators containing
URL credential syntax are hidden and source display is bounded. Referenced raw
Server/Swarm/Repo definitions are retained privately and revalidated at apply;
their secret-only changes invalidate the preview too. Mutable Git branches and
external host-file contents are **not** frozen by a definition preview; pin a
commit where an immutable Git input is needed.

These operations write definitions only. Core may clone/fetch Git into its own
cache and read referenced files while refreshing Compose information. Selecting
a host path does not rewrite it, and changing a Git pointer does not commit to
the repository. Core's separate `WriteStackFileContents` endpoint can write a
host file or commit Git content with Stack Write permission; this MCP surface
does not expose it. Compose/source/attachment changes can define privileged
workloads and always require trusted configuration access.

## Runtime effects

The MCP editor submits no deployment, lifecycle, host-file-write or Git-commit
request. Core 2.3.2 refreshes Stack/target caches on definition writes. UpdateStack
also launches `CheckStackForUpdate` when `repo`, `linked_repo`, `file_contents` or
`files_on_host` is submitted. If the saved `auto_update` policy is enabled, **Core
itself can deploy**, including when the MCP operate grant is disabled. Preview
and result disclose that policy and possibility; they do not promise that saving
is free of runtime effects. Policy changes after the final read can race.

The pinned SDK warns that changing the attached Server can clean up the old
Stack. The inspected **Core 2.3.2 UpdateStack implementation does not perform
old-target teardown**: its hooks refresh the new target. Old workloads can remain;
inspect both targets and deliberately operate them separately. Changing a Compose
project name can likewise leave an existing project running. Do not infer cleanup
or deployment from a saved definition.

Dependency debug/trace payload logs are filtered even with `RUST_LOG=trace`, so
new configuration secrets cannot leak through SDK request diagnostics. Application
diagnostics remain on stderr with API-credential redaction; stdout carries MCP.

The server privately retains the **authoritative configuration** and resource
identity, including fields hidden from display. A secret-only baseline change
invalidates the preview. The target selector and typed patch must match the
preview exactly; changing either fails before a target read. Apply consumes the
preview before any I/O, reads the target again, and rejects changed baselines
with `stale_preview` before submission. No-op preview/apply returns `no_op` with
zero writes. A process holds at most 16 pending previews; each expires after ten
minutes, can be used only once, and is unavailable after process restart. Prepare
a new preview after a stale, expired or mismatched one. Display of an existing
ignore list is capped at 32 names and 128 characters per name, with
`before_truncated` when clipped. Private baseline comparisons retain every value.

**Concurrency limit:** Core 2.3.2 UpdateStack has no conditional-write parameter.
Another writer can change configuration between our final read and submission.
This is no atomic compare-and-swap, invocation idempotency or exactly-once
guarantee. The partial body limits which fields our submission can overwrite.
Independent explicit calls can still repeat effects.

Apply submits at most one authenticated `/write` request with the existing
15-second deadline and 2 MiB response-body cap. Redirects, automatic mutation
retries and credential fallback are disabled. A lost, invalid, oversized or
failed response yields `submission_unknown`, an application-owned sanitized
`observation_error`, and `saved_configuration_verified: false`. Core may already
have saved configuration before an error. The preview stays consumed, including
after local cancellation or process loss. Inspect configuration before deciding
whether a new explicit edit is needed; never automatically repeat the write.

`saved` and `saved_configuration_verified` mean the typed Core response reports
the requested saved fields on the exact target. Use `get_stack` for independent
read-back of resource identity/observed runtime information. A successful save does
not establish deployment or health; use explicit operation and diagnosis tools
separately. `observed_runtime_state` is the preview-time observation, not a claim
that the saved desired source is running.

Implementation sources: pinned [partial request](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/api/write/stack.rs),
[Core write handler](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/write/stack.rs),
[resource update](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/resource/mod.rs),
and [Stack hooks](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/resource/stack.rs).
File effects are in the same write handler and
[source reading](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/stack/remote.rs).

Deterministic verification launches the production binary over stdio:

```sh
cargo test --locked --test configuration --test capabilities
```

It measures one-field requests, zero-write previews/no-ops, raw-secret baseline
changes, altered input, supported clears, four capability profiles, upstream
denial, malformed/oversized/lost responses and process restart. The opt-in
[disposable-Core runner](operations-walkthrough.md#opt-in-disposable-coreperiphery-smoke)
adds real Core/Periphery 2.3.2 Server-backed inline Compose creation, inspection,
one-setting source preservation, zero execution submissions and same-identity
restart/edit permission evidence. It also reassigns an owned running Stack to a
disabled pre-provisioned Server and back, observing unchanged old-target container
IDs. Direct Git, linked Repo, host-file sources and Swarm attachments have
deterministic production-stdio fixture coverage; they are not claimed as live
coverage.
