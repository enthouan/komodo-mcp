# Variables and secret metadata

`list_variables` and `get_variable` inspect global Komodo variables.
`list_core_secret_names` separately lists configured Core secret key names.
None of these tools submits a mutation or resolves a secret reference.

```json
{"name":"list_variables","arguments":{"page":1,"limit":20}}
{"name":"get_variable","arguments":{"name":"APP_MODE"}}
{"name":"list_core_secret_names","arguments":{}}
```

Names are exact and case-sensitive. Variable selectors use 1–500 ASCII bytes
matching `[A-Za-z_][A-Za-z0-9_]*`. Lists omit all values. Individual inspection
and change previews include an ordinary value only when it fits in 1024 bytes,
contains no control characters or URL, does not contain a configured MCP
credential, and has none of the case-insensitive markers `password`, `secret`,
`token`, `api_key`, `private_key`, `authorization` or `-----begin`. Otherwise
the whole value is omitted before any clipping. An empty ordinary value is
valid. Literal references remain references; they are never interpolated here.
This conservative display policy is not a general secret scanner or a guarantee
that Core's ordinary classification is appropriate.

Secret entries expose names, bounded descriptions and explicit `is_secret`
status. Their value field, masked text and value length are omitted, even when
the credential is an administrator and Core returns the real value. A secret
description containing its returned secret value is also omitted. Raw mutation
acknowledgements and authentication configuration are never displayed.

## Review and apply

Both tools use a closed typed `change` union. No arbitrary field patch,
classification switch, secret creation, value rotation or secret deletion is
supported.

```json
{"name":"preview_variable_change","arguments":{"change":{"kind":"create_ordinary","name":"APP_MODE","value":"blue","description":"Future workload mode"}}}
{"name":"apply_variable_change","arguments":{"change":{"kind":"create_ordinary","name":"APP_MODE","value":"blue","description":"Future workload mode"},"preview_id":"<returned token>"}}
```

Other explicit changes use the same pair:

```json
{"change":{"kind":"set_ordinary_value","name":"APP_MODE","value":"green"}}
{"change":{"kind":"set_description","name":"API_CREDENTIAL","description":"Metadata only; value preserved"}}
{"change":{"kind":"delete_ordinary","name":"APP_MODE"}}
```

Preview and apply require local `KOMODO_ALLOW_CONFIGURATION_EDIT=true`.
Ordinary creation, value updates and deletion additionally require
`KOMODO_ALLOW_OPERATE=true` at apply, including at the shared typed transport.
Changing a variable can affect future automatic workload execution or
notification delivery without a separate execute call. Preview submits no write;
save submits no Deploy, TestAlerter, SendAlert or other execution. Description
edits require configuration-edit alone and may target either classification.

Every write requires an enabled Core administrator. The process uses the same
credentials for a bounded authenticated `GET /user`, retains only identity and
authority flags, and explicitly checks enabled/admin status. Core remains the
write permission authority; a denial never selects another credential or API.

Values are limited to 8192 input bytes, descriptions to 4096, without NUL,
within the shared 16384-byte JSON input budget. Omitted create description means
empty; an explicit empty value or description clears that field. No display
marker may be used as an original value.

Private reviews share the existing 16-entry, ten-minute, process-local store.
They bind the exact typed change, complete raw entry including secret-only and
unknown-field changes, and current caller identity/authority. Apply consumes the
token once, revalidates immediately before submission in the shared transport,
preserves unrelated fields, and independently reads back the full entry or
deletion absence. An unchanged proposal performs zero writes. Tokens do not
survive restart and cannot be reused for another operation or change.

Preview makes at most two Core requests. Apply makes at most six (two for a
no-op), including the write and read-back; each request has the existing
15-second deadline. Read tools make one request. Private entry/caller baselines
fit in 65536 JSON bytes and share the bounded preview capacity.

Core uses a unique name without an immutable variable ID, version or atomic
compare-and-swap. Final-read races and identical same-name replacements cannot
be excluded. A successful acknowledgement and matching read-back are separate
fields. A matching state after a lost/malformed reply remains
`submission_unknown`; it cannot attribute the write to this caller. Read-back
failure or drift remains `verification_failed`. Never automatically repeat an
uncertain write: inspect the exact name, decide explicitly, and make a new
preview. These write APIs do not return a Core execution update ID; none is
fabricated. Changing or deleting a variable does not erase existing upstream
history, database records in backups, or logs.

## Bounds, configured secrets and history

Pinned Core's variable list is unpaginated. Each response is capped at 2 MiB,
512 entries are scanned, and local pages return at most 50 entries with bounded
descriptions. A truncated list is explicit. Exact lookup, mutation baseline and
absence checks require a complete bounded response; incomplete enumeration or
an upstream error never establishes absence. Local paging does not bound Core's
internal database enumeration. Raw `is_secret` and `value` fields are required:
SDK defaults cannot manufacture an ordinary classification or verified save.
An omitted empty description is normalized according to pinned serialization.

Configured Core secrets are distinct from `is_secret` variables. The names tool
uses only authenticated `ListSecrets {target: None}`, with the same page/scan/body
bounds. Unsafe key names are omitted. Server/Builder expansion is refused in
the shared read transport as well as the tool schema. No configured value,
host configuration read or config-file write is exposed.

Pinned Core 2.3.2 records value-bearing history for `CreateVariable`,
`UpdateVariableValue` and `DeleteVariable`, including external administrator
secret writes. Shared `get_update`/`wait_for_update` projections replace their
stage/error text and omit revision observations. Commands, stdout and other raw
payloads remain omitted. This is operation-based and works after restart without
remembering secret values. Description/classification updates have no history
operation variant in this Core release. Scoped TOML export already omits every
variable value, including ordinary values; it is an incomplete inspection view.

The compiled-stdio tests use deterministic pinned-wire fixtures. Actual Core
administrator rules, isolated identities and integrated live acceptance remain
part of #112.
