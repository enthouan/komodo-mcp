# Users, groups and access

The access tools inspect bounded user/group metadata and apply one explicit
administration change through Core 2.3.2. They never create human accounts,
passwords or API keys, rotate credentials, or configure authentication providers.
Authentication payloads, password hashes, tokens, service descriptions and raw
write replies are omitted. User names and group names use bounded, sanitized
display projections; use original input for a description edit.

## Inspect direct and effective access

```json
{"name":"list_users","arguments":{"kind":"service","page":1,"limit":20}}
{"name":"get_user","arguments":{"id":"000000000000000000000081"}}
{"name":"list_user_groups","arguments":{"page":1,"limit":20}}
{"name":"get_user_group","arguments":{"name":"Deployment operators"}}
{"name":"list_direct_permissions","arguments":{"subject":{"kind":"selected","subject":{"kind":"user","target":{"id":"000000000000000000000081"}}}}}
{"name":"list_direct_permissions","arguments":{"subject":{"kind":"current_caller"}}}
{"name":"get_effective_permission","arguments":{"resource_kind":"server","target":{"id":"000000000000000000000001"}}}
```

User listing/details and selected-user/group permission inspection require an
enabled Core administrator. User `kind` is `all`, `human` or `service`. Group
reads retain Core's visibility rules: a non-admin can see explicit memberships,
while an implicitly inherited everyone group can be absent from the list.
Current-caller direct inspection uses Core `ListPermissions`; this excludes even
the user's own type-wide grants. Selected-subject inspection also includes that
subject's direct type-wide grants. Neither result calculates another user's
effective access.

Only `get_effective_permission` uses Core's authoritative `GetPermission` for
the current credential. This includes resource base permissions, direct grants,
user/group type-wide grants, everyone inheritance and specifics such as
`inspect`, `logs`, `attach`, `terminal` and `processes`. Removing a direct grant
or group membership can leave other inherited access. Check again using the
actual affected identity; an administrator's result says nothing about another
user's effective access.

Names and readable IDs are positively resolved to an existing resource. A
stable-ID query that Core reports as `none` returns no resource metadata and
sets `target_identity_verified=false`; it does not independently establish
existence. Core's admin shortcut reporting `write` cannot establish existence
by itself. Failed resolution or denial never selects another resource or
credential. All observations can race subsequent changes.

## Preview one change, then apply it

`preview_access_change` requires local
`KOMODO_ALLOW_CONFIGURATION_EDIT=true`. Every `apply_access_change` requires
**both** that grant and `KOMODO_ALLOW_OPERATE=true`, including at the shared
typed transport. Both tools use the same enabled Core administrator credential.
Admin-status changes additionally require Core super-admin; ordinary admins
cannot modify or delete admin users. Disabled users cannot receive direct
permission changes or admin-status changes. Protected super-admin targets,
self-delete, self-disable and self-admin revocation are refused. These checks
cannot provide atomic protection against another concurrent administrator.

```json
{"name":"preview_access_change","arguments":{"change":{"kind":"set_group_member","group":{"name":"Deployment operators"},"user":{"id":"000000000000000000000081"},"present":true}}}
{"name":"apply_access_change","arguments":{"change":{"kind":"set_group_member","group":{"name":"Deployment operators"},"user":{"id":"000000000000000000000081"},"present":true},"preview_id":"<returned token>"}}
```

The closed `change` variants are:

| Kind | Explicit fields and effect |
| --- | --- |
| `create_service_user` | Exact `username`, optional `description` (default empty); creates an enabled non-admin service user with no create permissions or keys. |
| `set_service_user_description` | `target`, `description`; empty clears; preserves authentication and permissions. |
| `set_user_base` | `target`, optional `enabled`, `create_servers`, `create_builds`; omission preserves each field. |
| `set_user_admin` | `target`, `admin`; super-admin only, without a super-admin promotion API. |
| `delete_user` | `target`; deliberate deletion, with separate leftover inspection below. |
| `create_group` / `rename_group` | Exact `name`, plus `target` for rename; names are not normalized. New groups have no members/grants and `everyone=false`. |
| `delete_group` | `target`; Core can remove the group before permission cleanup fails. |
| `set_group_member` | Exact `group`, exact `user`, `present`; adds/removes only that member and preserves all others. |
| `set_group_everyone` | `target`, `everyone`; explicitly changes broad implicit membership. |
| `change_permission` | Explicit `subject`, `scope`, `patch`; preserves unrelated records and specifics. |

Each selector requires exactly one stable `id` or exact case-sensitive `name`.
A name shaped like a Mongo ID is still treated as a name. Missing, ambiguous or
incompletely enumerated targets fail closed. Names are limited to 128 bytes
without controls; descriptions to 4096 bytes without NUL. Explicit null fields
are rejected. The total typed change must fit the shared 16384-byte input limit.

For example, add one specific flag without dropping existing flags or changing
the direct level:

```json
{"change":{"kind":"change_permission","subject":{"kind":"group","target":{"name":"Deployment operators"}},"scope":{"kind":"resource","resource_kind":"server","target":{"id":"000000000000000000000001"}},"patch":{"add_specific":["terminal"]}}}
```

`patch.level` is `none`, `read`, `execute` or `write`. Omission preserves it.
`add_specific` and `remove_specific` are disjoint, unique lists of the five
supported flags. Omitting them preserves every existing flag. Setting level
`none` alone does **not** clear specifics or inherited access. Explicitly remove
the intended flags, then verify effective access using the affected identity.
An unsupported flag in the changed grant prevents replacement; unknown flags
in unrelated records remain private and untouched. Read projections omit
unknown values and report incompleteness.

The only type-wide scope is explicit
`{"kind":"resource_type","resource_kind":"server"}`. It covers all current
and future resources of that type. Everyone-group changes, type-wide grants,
admin changes and deletion are marked `broad_scope` in the preview. A missing
resource target never implies a type-wide or global change.

## Freshness, outcomes and cleanup

Previews share the process-local 16-entry, ten-minute, single-use store. They
bind the typed input, current caller identity/authority, full raw user/group
state, relevant membership/direct grants and the complete selected resource.
Membership and everyone changes bind the group's separate scoped permission
records as well as its type-wide grants before exposing that inherited access.
This detects private authentication, secret/reference and unknown-field drift
without displaying those values or a baseline hash. Apply consumes its token,
revalidates again at the final shared transport, and submits at most one typed
write. A no-op submits none. Restart invalidates old previews.

The raw private baseline is capped at 256 KiB. Lists scan at most 512 entries
from a bounded 2 MiB response, with local pages of at most 50 and explicit
truncation. Exact mutation/absence proofs require complete enumeration. Member
output is limited to 32 IDs; writes require complete, valid membership. These
local bounds do not limit Core's internal database enumeration. Each request
uses the existing deadline; access tools do not poll or automatically retry.

Core has no compare-and-swap. Another administrator can change state after the
final read. `acknowledgement_valid` and `saved_state_matches` are separate
facts: a valid reply does not prove successful independent verification, and
matching state after a lost/malformed reply remains `submission_unknown`.
`before`/`after` show the proposed metadata/grant; use the outcome, verification
and error fields to assess what was actually established. No execution update
ID, atomic transaction, rollback or cross-client exactly-once guarantee is
invented. Inspect current state and make a new explicit decision before another
write.

Core `DeleteUser` removes the user and attempts group-reference cleanup, but
leaves scoped permission and API-key records. `DeleteUserGroup` can remove the
group before permission cleanup fails. The result separately inspects visible
group references and remaining direct permission rows, labels incomplete or
denied observations, and never submits hidden cleanup writes. Credential
cleanup and immediate cached-access revocation are **not verified**. A missing
user/group record does not prove complete access cleanup. Existing upstream
logs, history, database backups and credential records are not erased.

Compiled-stdio tests use deterministic pinned-wire fixtures for role rules,
group-only/everyone access, revocation, preservation, stale previews, uncertain
replies and partial cleanup. Actual Core permission-engine acceptance uses
separate provisioning credentials and isolated ordinary-admin/super-admin and
affected-user identities in #112; fixture behavior is not that live proof.
