# ResourceSync definitions and pending plans

`list_resource_syncs` and `get_resource_sync` inspect visible definitions in every
local capability profile. Lists use positive `page_size` (1–100, default 30),
name-ascending order and `next_page` with the same page size. Get accepts exactly
one stable 24-hex `id` or exact case-sensitive `name`, including ID-shaped names.
Names are bounded to 128 bytes. Core retains its Read/visibility decision.

```json
{"name":"list_resource_syncs","arguments":{"page_size":30}}
{"name":"get_resource_sync","arguments":{"name":"owned-sync"}}
```

Inspection returns bounded source/settings and cached state, without refreshing
pending changes. Inline TOML, credentials, webhook secrets, raw pending TOML,
remote contents, source errors and commit messages are withheld. Presence flags
identify configured secrets/content; truncated path/tag lists are explicit.
Reads submit zero Core mutation requests. These definition tools do not supply a
reviewed desired-state plan or authorize RunSync.

## Preview and save

Enable `KOMODO_ALLOW_CONFIGURATION_EDIT=true` for the four typed tools:
`preview_resource_sync_create`, `apply_resource_sync_create`,
`preview_resource_sync_update`, and `apply_resource_sync_update`.
Previews make zero writes and return the shared `preview_id`. Apply requires the
**exact same name/config or target/patch** plus that ID.

```json
{"name":"preview_resource_sync_create","arguments":{"name":"owned-sync","config":{"source":{"kind":"inline","contents":"[[server]]\nname = 'owned-server'\n[server.config]\nenabled = false\n"},"managed":false,"delete":false,"match_tags":["owned"],"include_resources":true,"include_variables":false,"include_user_groups":false}}}
```

Copy the exact `name` and `config`, add the returned `preview_id`, and call
`apply_resource_sync_create` deliberately. Core requires an **administrator** to
create a ResourceSync. Existing definitions require Write; a supplied linked
Repo requires Read/Attach. Core denials are preserved without credential fallback.
Names use Core's trimming, preserving internal spaces; creation rejects control
characters, path separators, `.`/`..`, ObjectIds and excess length.

All fields are optional in an update patch. Omission preserves the saved value,
including secret references and inactive source-specific settings. `null`, unknown
fields and display-redaction markers are rejected. Settings are `managed`,
`delete`, `include_resources`, `match_tags`, `include_variables`,
`include_user_groups`, `pending_alert`, `webhook_enabled`, and `webhook_secret`.
An empty `webhook_secret` restores Core's default secret; an empty tag list
deliberately removes the resource-tag restriction.

```json
{"name":"preview_resource_sync_update","arguments":{"target":{"name":"owned-sync"},"patch":{"pending_alert":false}}}
```

The private preview binds the full definition and relevant active linked Repo
configuration, including secrets, even when `source` is omitted. Apply re-resolves
those identities and compares authoritative baselines before saving. Secret-only
or referenced Repo changes reject the preview. Unchanged normalized edits are
zero-write no-ops. Previews share a 16-entry process-local store, expire after
ten minutes, and are consumed once before apply reads. Invalid/tampered/consumed
references cannot authorize a save; restarting MCP invalidates them.

## Four explicit sources

| Source | Typed fields and preservation |
| --- | --- |
| `inline` | Required `contents`: complete trusted desired-state TOML, including explicit empty content. |
| `git` | Required `repo` such as `owner/repo`; optional `branch`, `provider`, `https`, account reference `account`, pinned hex `commit`, and `resource_paths`. Omitted options preserve saved values; `account:""`/`commit:""` clear those fields. URLs containing credentials are rejected. |
| `linked_repo` | Required exact Repo `target` (`id` or `name`); optional `resource_paths`. The stable Repo ID and its private configuration are bound/rechecked. |
| `host` | Required `resource_paths`, on the **Core host**. No MCP-host file is read. |

Git and linked-Repo creation require explicit `resource_paths`. In updates,
omitted paths preserve the saved list. Paths contain at most 32 entries of at most
512 bytes, must be relative and cannot traverse `..`. `.` can explicitly select
the source root directory. **`[]` reads no files**; it does not select a default.
Core still controls filesystem access, directories and symlinks.

Core's precedence is **host → linked Repo → direct Git → inline**. Supplying
`source` deliberately clears conflicting `files_on_host`, `linked_repo`, `repo`
and `file_contents` selectors. Unrelated options and omitted credential references
remain intact. Core-host paths are relative to
`sync_directory / to_path_compatible_name(sync.name)`; for `Owned Sync`, this is
the `Owned_Sync` subdirectory. Git paths are relative to the fetched repository.
Changing a pointer does not change authoritative source files or Git refs.

Inline validation uses actual TOML parsing and Core 2.3.2 resource types, including
its triple-double-quoted backslash adaptation and trailing-newline normalization.
Unknown top-level sections, syntax errors and incompatible resource value types
have separate sanitized corrections. Error snippets, unknown keys and values are
withheld. This validates the document shape, not its complete future execution,
references, permissions or health. Core's own refresh can still report errors.
Inputs/serialized partial writes fit within 16 KiB JSON. Shared reads/writes have
a complete-request deadline of 15 seconds and a streamed 2 MiB body cap. Display
paths/tags are limited to 32 entries, respectively 256/128 characters per entry;
lists return bounded summaries instead of full documents.

## Desired state, deletion and authority

An omitted **definition-patch field preserves** that field. An omitted field
**inside desired-state TOML can reset an existing resource to its default** when
Core later applies the sync. Do not treat an inspection-only sanitized export as
a lossless source file. Review the complete desired state and its pending effects
before separately authorizing execution.

`effective_delete` is **`managed || delete`**. Managed mode enables deletion even
with `delete:false`. An empty resource-tag filter is unrestricted; it can expand
creation/update/deletion scope. Variables and user groups have their own inclusion
flags and are not isolated by resource tags. These settings are previewed before
the save; saving is separate from applying their desired state.

New definitions explicitly default `webhook_enabled:false`, overriding the SDK's
true default. Enabling webhooks requires both local grants. Every supplied
execution-bearing update also requires `KOMODO_ALLOW_OPERATE=true`, independently
of observed webhook state, including unchanged supplied fields: active or
concurrently enabled triggers could execute the changed configuration. Empty
patches, `pending_alert` changes and explicit webhook disabling need edit alone.
Discovery and direct dispatch enforce edit; the apply handler and shared typed
transport enforce effect-based operate before Core I/O.

## Save effects and uncertainty

MCP submits at most one typed CreateResourceSync or partial UpdateResourceSync.
**It submits no RunSync, CommitSync, WriteSyncFileContents or deployment.**
Core's create/update hooks run pending-cache refresh through its built-in sync
administrator. This can fetch/clone Git into **Core caches**, read Core-host files,
compute pending changes and create/resolve pending alerts. A successful save does
not prove source parsing or desired-state application succeeded. A later RunSync
checks Execute on the sync and applies resources as Core's built-in administrator
sync identity, with broader delegated authority than the initiating caller's own
direct resource permissions. Refresh and scoped review are described below;
reviewed application is the separate #31 workflow.

The saved echo and an independent bounded full raw configuration read-back are
checked separately. `saved_configuration_verified:false` and `observation_error`
retain uncertainty without resubmitting a write. A failed acknowledgement can
mean the save occurred. Core has no conditional configuration update; a race
after the final read remains. Never reconstruct a proposal from redacted output.

External **Git/host write-back remains deferred**. Managed settings are inspectable
and editable, but no v0.6 definition tool exposes commits, pushes, host-file
rewrites or a generic Core passthrough. Full compiled-stdio and actual-final-main
disposable Core/Periphery workflow evidence is tracked by
[#103](https://github.com/enthouan/komodo-mcp/issues/103).

Run `cargo test --locked --test resource_syncs --test capabilities --test stdio_smoke`
for source switching, one-field/secret preservation, stale linked references,
capability and upstream denials, admin-only creation, input/pagination/diagnostic
bounds, lost/invalid save replies, full read-back and measured zero execution.

Pinned source: [source/settings](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/entities/sync.rs),
[create/update hooks and Attach](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/resource/sync.rs),
[source precedence/host paths](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/sync/remote.rs),
[TOML adaptation](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/sync/mod.rs),
[pending refresh](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/write/sync.rs).

## Pending plans and explicit refresh

`get_resource_sync_pending` accepts the same exact `id` or `name` selector. It
uses only bounded Core reads and returns `observation:"cached"`, creates,
updates, deletions, variable/group changes, deployment intent/dependencies and
separate `pending_error`, `pending_deploy_error` and source-file-error counts.
Raw TOML, source contents, variable values, group memberships/permissions, commit
messages and error snippets stay private. Missing fields yield incomplete
evidence; errors never become an empty clean plan. Each section displays at most
32 rows with its reported total and truncation flag.

```json
{"name":"get_resource_sync_pending","arguments":{"name":"owned-sync"}}
{"name":"refresh_resource_sync_pending","arguments":{"name":"owned-sync"}}
```

Refresh requires `KOMODO_ALLOW_OPERATE=true` before any Core I/O and retains
Core's Execute decision. It makes **one RefreshResourceSyncPending attempt and
zero RunSync requests**. Core can read source files, clone/fetch Git into its
cache, store pending state and create/resolve/send alerts. This is a mutation,
separate from desired-state application. An invalid/lost reply returns
`submission_unknown`; an upstream denial returns `denied`, without fallback or
retry. Inspect cached state after uncertainty. A refresh-response observation is
not a frozen plan, and Core can skip an unconfigured source.

`review_resource_sync_pending` is available with read access in every local
profile. Its required `selection` is one of these explicit forms:

```json
{"target":{"name":"owned-sync"},"selection":{"kind":"whole_sync"}}
{"target":{"name":"owned-sync"},"selection":{"kind":"resource_type","resource_type":"server"}}
{"target":{"name":"owned-sync"},"selection":{"kind":"resources","resource_type":"server","targets":[{"id":"000000000000000000000001"},{"name":"new-server"}]}}
```

Named selections contain 1–16 exact targets within one type. Existing resources
bind stable IDs; new resources bind their exact typed pending-create names.
Missing, ambiguous, duplicate and invalid selectors fail without dropping
entries or falling back to whole-sync. New ObjectId-shaped names and names Core
would normalize cannot authorize a create. Variables/groups enter review scope
only for a whole-sync selection and their enabled inclusion flags; resource tags
do not isolate those changes. Filtered output explicitly counts excluded rows.

Core 2.3.2's cached view omits template-only resource changes and `is_secret`-only
variable changes. Review audits current resource template metadata and variable
secret flags through bounded reads and adds `origin:"metadata_audit"` rows.
`cached_status` and `reviewed_status` remain distinct. Denied or incomplete
metadata observations withhold the reference. Core computes deployment effects
differently for filtered runs, including when resources are disabled. A source
requesting any Stack/Deployment `deploy=true` therefore permits a review reference
only for whole-sync, `include_resources:true` and an empty resource-tag filter.
Other combinations remain inspectable with an explicit coverage gap.
This conservative deployment-intent check covers every source row before tag
filtering, including rows absent from Core's cached deployment list.

With a nonempty tag filter, ID-shaped tags in source resources also withhold the
reference. Core has an initial literal source filter and a later ID-to-name tag
predicate; review does not assume those stages are interchangeable or infer tag
identity from a clean cache. Use source tag names for review authorization.

A reference is issued only for inspected Core **2.3.2**, complete evidence and
an exact displayable selection. It shares the 16-entry process-local preview
store, expires after ten minutes and is single-use, bound to the apply operation,
sync and canonical selection. Restarting MCP invalidates it. Minting a reference
is non-idempotent but makes zero Core mutations. Private baselines retain full
definitions, source contents, pending TOML, secret-only changes and relevant
linked Repo data; sanitized output is never reconstructed as authorization.

Review bounds source documents to 32 files/entries and 128 KiB total, cache
sections to 256 entries, public JSON to 256 KiB, private baselines to 2 MiB and
the metadata audit to 30 seconds, in addition to each request's 15-second/2 MiB
transport limits. Duplicate/unsupported source identities, missing fields,
truncation, pending errors or excess coverage produce explicit gaps and no
reference. Source errors do not expose their contents.

Git revisions describe Core's observed cache only. Inline and host sources have
no Git revision, even if an old hash remains in Core. No observation claims the
external source is currently unchanged. A later authorized apply must refresh
and recheck definition, references, source and plan, counting that separate cache
mutation, and reject detected drift before at most one RunSync. Core rereads the
source and has no expected-revision parameter: a final-read/execution race
remains. Sync success and workload health are separate observations.

Run `cargo test --locked --test sync_plans --test resource_syncs --test capabilities`
for deterministic production-stdio evidence. The same fixture sync supports
cached observer access and separately permission-checked refresh; #31 adds apply
and #103 owns the actual-final-main live end-to-end evidence.

Pinned semantics: [cached resource view](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/sync/view.rs),
[execution and selectors](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/execute/sync.rs),
[resource execution](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/sync/execute.rs),
[variable execution](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/sync/variables.rs),
[deployment planning](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/sync/deploy.rs).
