# Repositories: definitions, clone and pull

`list_repos` and `get_repo` are read-only in every local profile. They use the
same bounded pagination and exact ID/name resolution as inventory tools.
Source/provider/branch displays are limited to 256 characters; malformed or
credential-bearing locators are hidden. Credential account names, hook source,
environment, descriptions and webhook secrets are never returned. Lists use
positive limits of 1–100 and at most one lookahead; all Repo reads use the shared
2 MiB body cap and 15-second complete-request deadline.

`configured_branch` and `definition.source.configured_commit` are desired
selectors. `observed_cloned_revision` is Core's current Periphery/cache
observation, which can lag or belong to another operator's run. An update's
`observed_revision` is evidence from that update. These are separate claims.

## Create or patch a definition

Enable `KOMODO_ALLOW_CONFIGURATION_EDIT=true` for these trusted tools:

- `preview_repo_create` / `apply_repo_create`
- `preview_repo_update` / `apply_repo_update`

Core requires an enabled identity, Read for inspection, Write for editing an
existing Repo, and Read with Attach on a supplied Server. Repo creation requires
Core's non-admin creation policy to permit the caller, or an explicitly authorized
admin identity. Neither local grant changes those permissions. Credentials are
never replaced with provisioning/admin credentials after denial.

Creation requires a name and explicit `repo`, `branch` and `target`. Names use
Core's path-compatible normalization; the preview shows the normalized name.
ObjectId-shaped, empty and overlong names are rejected. For example:

```json
{"name":"preview_repo_create","arguments":{"name":"example-repo","config":{"repo":"example/workload","branch":"main","target":{"kind":"server","target":{"name":"example-server"}},"git_provider":"github.com","git_https":true,"git_account":"existing-account-reference"}}}
```

Inspect the redacted changes, target, effects and `preview_id`; submit the
**exact input** with that ID to `apply_repo_create`. Creation explicitly sets
`webhook_enabled:false` unless supplied, rather than inheriting Core's enabled
default. It submits one CreateRepo and no execution request.

Updates use the existing shared preview/apply contract:

```json
{"name":"preview_repo_update","arguments":{"target":{"id":"00000000000000000000000a"},"patch":{"branch":"release/v05"}}}
```

Then call `apply_repo_update` with the exact target/patch and the returned
`preview_id`. A one-field edit sends only that Core field. All omitted fields,
including credential references, webhook secrets, hooks, environment, existing
Builder associations and other configuration, are preserved.

| Typed field | Supported value / explicit clear |
| --- | --- |
| `repo`, `branch` | Nonempty bounded safe selectors, without URL credentials or shell syntax |
| `git_provider` | Host/domain with optional port; no scheme, path, credentials, query or fragment |
| `git_https` | Explicit boolean |
| `git_account` | Existing Core/Periphery credential-account reference; `""` clears |
| `commit` | Hexadecimal pinned commit; `""` clears the pin |
| `target` | `{"kind":"server","target":{"id":"…"}}` or exact name; `{"kind":"unassigned"}` clears |
| `path` | Trusted destination on the Server; `""` restores the name-derived path |
| `on_clone`, `on_pull` | Complete `{path,command,shell_mode}` objects; an empty command disables the hook |
| `environment` | Complete secret-bearing replacement string; `""` clears |
| `webhook_enabled` | Boolean; supplying `true` requires both local grants at apply |

Omission preserves; JSON null, unknown fields and display redaction markers are
rejected. Patches are bounded to 16384 serialized JSON bytes. Commands/environment
use the pinned Core newline normalization before comparison and submission.
Hook commands and custom destination paths are trusted executable configuration,
including host access under Periphery's authority.

Previews write nothing. They share the existing 16-entry, ten-minute,
process-local, single-use store. The private authoritative configuration and
supplied Server definition/pinned trust are revalidated before apply. Changing
hidden secrets or a selected reference makes a preview stale. An unchanged
proposal is a zero-write no-op. Credential-token availability in Core/Periphery
is not inspected or claimed as validated; omitted references are preserved.

A valid write echo is followed by a separate bounded authoritative GetRepo that
verifies the complete expected saved configuration, including preserved fields.
`outcome:saved` acknowledges the write; `saved_configuration_verified:true`
additionally requires that read-back. A denied/changed/unavailable read-back
retains the acknowledgement with verification false and `observation_error`.
Lost/invalid write acknowledgements return `submission_unknown` and consume the
preview. No write is retried automatically. Another writer can change the
definition after final reads or after read-back: Core provides no general
compare-and-swap or invocation-idempotency mechanism.

The inspected Core 2.3.2 definition-write hooks refresh caches without invoking
clone/pull or running configured hooks. The SDK warns about old-Server cleanup;
the inspected update path does not perform it, so old files can remain. Inspect
both Servers after moving a Repo. Enabling webhooks permits future Core-defined
clone/pull/Repo-build executions with the saved trusted configuration. It needs
both independent local grants in tool handlers and the shared transport, including
creation; it does not submit an execution itself. This surface neither changes
the saved Builder association nor exposes Repo-build execution.

## Inspect effects before one clone or pull

Use `get_repo` first and inspect `definition.on_clone_configured`,
`on_pull_configured`, environment presence, webhook policy and target. Enabling
`KOMODO_ALLOW_OPERATE=true` exposes `clone_repo` and `pull_repo`; configuration
editing is independent. Core Execute on the selected Repo remains required;
inspection/history also need their normal Read access.

Supply both inspected hook flags explicitly:

```json
{"name":"clone_repo","arguments":{"target":{"id":"00000000000000000000000a"},"expected_hooks":{"on_clone":true,"on_pull":true}}}
```

Clone can run **on-clone followed by on-pull**. Pull can run **on-pull only**.
Core's clone deletes the existing configured Repo directory before cloning;
uncommitted files there can be lost. Clone/pull modify the working tree and
Periphery can write the configured environment file before hooks, including
overwriting an existing file. Inspect custom destination paths in Core before
operating a trusted Repo. The receipt reports these configured effects separately
from evidence that they completed.
Hooks/environment execute trusted saved code on the attached Server; selecting
one Repo is not host/process isolation. A hook-presence mismatch requires fresh
inspection and submits nothing. Core reads saved configuration asynchronously,
so source, target or hook changes after this inspection still race.

Each call resolves one stable Repo ID and submits at most one CloneRepo/PullRepo.
It never rewrites definitions or submits an additional Git-file editor, commit,
push or Repo-build request. Templates and unassigned targets are rejected before
submission.
The shared receipt separates acknowledgement, terminal update outcome and
observation failure, retains known update IDs and includes bounded sanitized
failed-stage evidence. Successful clone/pull does not establish workload health.

Resume an acknowledged ID with `get_update` or `wait_for_update` from any
read-only session. The same bounded wait, local cancellation and restart behavior
as [workload operations](operations.md#wait-and-resume-by-update-id) applies.
Cancelling observation does not cancel remote Repo work.

If no ID survived, use `reconcile_operation` with the original stable target,
`requested_scope:{"kind":"whole_repo"}` and operation `clone` or `pull`:

```json
{"name":"reconcile_operation","arguments":{"target":{"kind":"repo","id":"00000000000000000000000a"},"requested_scope":{"kind":"whole_repo"},"operation":"pull"}}
```

Retain original submission times when known and use the optional UTC millisecond
window. Reconciliation remains bounded and read-only. One history candidate,
matching actor or matching time does not prove attribution; original submission
stays unknown and every candidate stays unconfirmed. No timeout, restart, lost
reply or incomplete observation triggers execution replay.

Observed revisions use a validated hexadecimal `Update.commit_hash` when Core
returns it. Core 2.3.2's clone handler drops that structured hash; its specific
successful `Latest Commit` stage can provide the observed short hash instead.
The projection decodes only that bounded, unambiguous pinned stage, including
its ANSI formatting, and never returns unrestricted stdout or commit messages.
Unparseable/missing revision evidence stays absent. URL-bearing history tokens
are redacted because Git failures can echo embedded credentials.

## Validation and sources

`cargo test --locked --test repositories` launches the compiled stdio server
with pinned-wire fixtures. It covers all capability profiles/direct calls,
one-field preservation, zero-write previews/no-ops, clears, stale private
baselines/references, authoritative read-back, hook disclosure/failure, denials,
revision evidence, lost acknowledgements, bounded waits and read-only recovery
across restarted/concurrent independent clients. These deterministic fixtures do
not execute Core's authorization engine or Git hooks. The final live integrated
automation acceptance remains [#93](https://github.com/enthouan/komodo-mcp/issues/93).

Pinned sources: [Repo requests](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/api/write/repo.rs),
[resource permissions/hooks](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/resource/repo.rs),
[Core clone/pull](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/execute/repo.rs),
and [Git revision log](https://github.com/moghtech/komodo/blob/v2.3.2/lib/git/src/lib.rs).
