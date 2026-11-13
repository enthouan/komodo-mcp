# Tags and exact resource assignments

Tag creation, rename and assignment use the shared typed preview/apply contract
with `KOMODO_ALLOW_CONFIGURATION_EDIT=true`. Tag deletion uses the completed
[destructive policy](deletion.md): preview needs configuration access, and apply
requires **both** independent local grants. Read-only `find_resources_by_tag`
remains available in every profile. No tag tool executes a workload.

Core is the permission authority. Creation follows its enabled-caller/non-admin
creation policy. Rename and deletion require the tag's **owner or an admin**.
Assignment requires **Write on the resource**; tag ownership is unnecessary.
Deletion can remove assignments from resources the tag owner cannot read or
edit. Tags themselves are globally readable to authenticated callers in pinned
Core 2.3.2. No credential or permission fallback occurs.

Create and rename one exact tag:

```json
{"name":"preview_tag_create","arguments":{"name":"production"}}
{"name":"apply_tag_create","arguments":{"name":"production","preview_id":"<create token>"}}
{"name":"preview_tag_rename","arguments":{"tag":{"id":"000000000000000000007002"},"name":"live"}}
{"name":"apply_tag_rename","arguments":{"tag":{"id":"000000000000000000007002"},"name":"live","preview_id":"<rename token>"}}
```

Use the actual ID returned by creation. Names are exact and case-sensitive;
case and whitespace are preserved. New names must contain a non-whitespace
character, fit in 128 bytes, contain no control characters and not resemble a
24-hexadecimal ObjectId. Creation uses Core's default Slate color. Rename preserves
the stable ID, owner, color and assignment IDs. Unchanged rename is a zero-write
no-op. The typed surface does not edit colors or transfer ownership.

Resource assignment supports the existing managed Server, Stack and Deployment
definitions by exact kind/stable ID. Preview shows `before_tags` and intended
`after_tags`; apply separately reports `observed_tags` and `change_verified`.

| Input | Behavior |
| --- | --- |
| Omit `tags` | Preserve the current set; apply submits zero writes. |
| `mode:"replace"`, `tags:[...]` | Replace with exactly the resolved tag-ID set; this is the default mode. |
| `mode:"replace"`, `tags:[]` | Deliberately clear all assignments. |
| `mode:"add"`, `tags:[...]` | Union resolved IDs with the existing set; preserve unrelated assignments. |
| `mode:"add"`, `tags:[]` or already assigned tags | Explicit zero-write no-op. |
| `tags:null` | Rejected. Omission and clearing are distinct. |

```json
{"name":"preview_resource_tags","arguments":{"target":{"kind":"stack","id":"000000000000000000000002"},"mode":"add","tags":[{"name":"live"}]}}
{"name":"apply_resource_tags","arguments":{"target":{"kind":"stack","id":"000000000000000000000002"},"mode":"add","tags":[{"name":"live"}],"preview_id":"<assignment token>"}}
```

Each tag selector contains exactly one stable ID or exact name. At most 32 tags
may be requested and 64 unique IDs retained in a resulting set. Sets are sorted
and deduplicated; unrelated tag IDs remain when adding. The request sends only
`target` and `tags` through typed `UpdateResourceMeta`. Description, template and
configuration are not sent or changed. Private full-resource metadata/configuration
and resolved tag definitions are revalidated before submission and checked again
at read-back.

Core's metadata endpoint can implicitly create unresolved tag names and silently
drop failed resolutions. MCP resolves names first and submits only stable IDs.
An unresolved existing assignment can remain in an omitted no-op or be explicitly
cleared; adding refuses to risk dropping it. A tag can still disappear after the
final read, or a concurrent resource edit can race with exact-set replacement.
Read-back reports dropped IDs or changed unrelated metadata separately from an
acknowledgement, without retrying.

Find a visible resource by the renamed tag in a default read-only process:

```json
{"name":"find_resources_by_tag","arguments":{"tag":{"name":"live"},"kind":"stack","page":0,"page_size":20}}
```

The tool resolves the exact name to its stable ID and uses positive Core pages
of 1–100 records, including a bounded lookahead. Returned records must carry
that ID. Continue with the returned page while keeping page size fixed. Hidden
records and concurrent changes remain possible; this is not a snapshot.

Delete deliberately through a new preview:

```json
{"name":"preview_tag_delete","arguments":{"tag":{"id":"000000000000000000007002"},"action":"delete"}}
{"name":"apply_tag_delete","arguments":{"tag":{"id":"000000000000000000007002"},"action":"delete","preview_id":"<delete token>"}}
```

Deletion reuses the shared 16-entry, ten-minute, single-use store and both-grant
gate. It binds the exact tag and bounded visible assignment effects before one
typed `DeleteTag`. Preview makes zero writes; discovery/direct dispatch and the
shared transport deny apply without both grants. All mutation families share
the same capacity and reject cross-operation token replay.

Core first removes the ID across Server, Stack, Deployment, Build, Repo,
Procedure, Action, ResourceSync, Builder and Alerter collections, then removes
the tag record. Cross-collection cleanup can partially happen before an error
while the tag remains. **Core 2.3.2 omits Swarm cleanup**, so Swarm assignments can
remain dangling. The preview scans all eleven families, including Swarms, using
two positive pages of 20 visible records per family. It displays at most 32
effects and privately binds additional retained effects. Hidden records,
additional pages and permission gaps never prove no assignments.

Results separate acknowledgement, exact tag-record observation and each visible
assignment after-state. `assignment_cleanup_fully_verified` stays false; missing
tag records do not prove global cleanup. No lifecycle, repair or fallback write
is submitted. Tag changes return no invocation update ID. Core provides neither
conditional writes nor an invocation idempotency guarantee.

Raw tag ownership, resource configuration and secret-bearing values remain in
private baselines. Only bounded allowlisted projections leave MCP, with configured
credentials redacted before clipping. Preflight has a 15-second/256-KiB private
snapshot budget and post-attempt reads have ten seconds. Lost, invalid, oversized
or mismatched replies consume the token and keep `submission_unknown`, including
when later reads observe the desired state. Never automatically repeat a mutation.

The disposable runner exercises real tag lifecycle, Stack assignment/add/no-op/
clear, renamed read-only discovery, non-owner denial, guarded deletion, restored
initial assignments and unchanged running containers. Server/Deployment assignment,
all eleven deletion families, Swarm omission, partial cleanup and resolution-drop
races also have deterministic wire-fixture coverage. See the
[live walkthrough](operations-walkthrough.md) for owned isolation and cleanup.
