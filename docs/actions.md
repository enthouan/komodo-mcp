# Trusted Actions

`list_actions` returns bounded pages of stable IDs, cached state and last
completed-run start time (successful or failed). `get_action` describes saved
settings and execution effects.
Source, default argument values and webhook secrets stay private. An Action can
have multiple simultaneous runs.

Core 2.3.2 executes Action TypeScript on the Core host using Deno `--allow-all`
and a temporary API key for its built-in **administrator Action user**. Scripts
can access the host, filesystem, network and Core resources beyond the initiating
caller's permissions. They can call cloud APIs. Treat source and argument edits
as trusted executable configuration. The direct Build tools' Server-only scope
does not constrain arbitrary saved automation.

## Definitions and permissions

Use `preview_action_create` / `apply_action_create` or
`preview_action_update` / `apply_action_update`. These share the bounded,
ten-minute, single-use preview store and full private baseline used by the other
[configuration tools](configuration.md). Preview writes nothing. Apply consumes
the token, checks the fresh baseline, sends at most one partial write, verifies
the response and independently reads back the full saved configuration.

| Field | Meaning |
| --- | --- |
| `source` | Trusted TypeScript, at most 8192 UTF-8 bytes; `""` deliberately clears source |
| `default_arguments` | Bounded JSON object saved as JSON defaults; `{}` deliberately clears saved defaults |
| `reload_deno_deps` | Deno dependency reload setting |
| `run_at_startup` | Enables running saved code on a future Core startup |
| `webhook_enabled` | Enables incoming webhook execution |
| `failure_alert` | Core failure alert policy |

The pinned SDK normalizes supplied nonempty source/default text by trimming
trailing whitespace and adding one newline; previews and read-back use that same
normalization. Empty text stays empty.

Omission preserves source, the original KeyValue/Toml/Yaml/Json argument format
and text, secrets, schedule and unrelated settings. Source and argument changes
are redacted in previews. Unknown fields and nulls are rejected. Schedule fields
are managed separately, with no generic configuration passthrough.

Both local grants remain independent. Read-only exposes list/get/history.
Configuration-edit exposes previews and inert creation; Core requires an
**administrator** to create an Action. Creation requires explicit source and
forces startup, webhooks and schedule disabled with an empty schedule, avoiding
Core's implicit default script and automatic-execution defaults. Core's saved
schedule/cache hooks run after creation/update; no explicit `RunAction` is sent.

Every nonempty existing-definition update requires configuration-edit **and**
operate before Core I/O, independently of the currently observed automatic
policy. A truly empty patch can return a write-free no-op with edit alone.
Core still requires Write on the Action; no privileged fallback is used. Core
has no compare-and-swap: another editor or automatic policy can race the last
read. An uncertain save is never retried.

Example preview:

```json
{"target":{"name":"trusted-action"},"patch":{"reload_deno_deps":false}}
```

Apply the identical target/patch with the returned `preview_id`.

## Run and observe

Enable operate and obtain Core Read/Execute on the selected Action. Running
needs no configuration-edit grant and never writes the saved source/defaults.
Inspect `get_action.effects`, then copy its acknowledgement:

```json
{
  "target":{"name":"trusted-action"},
  "args":{"region":"owned-test","items":["extra"]},
  "expected_effects":{
    "core_admin_authority":true,
    "host_access":true,
    "broader_effects_possible":true
  }
}
```

`run_action` resolves one stable ID, rechecks the selected definition and sends
one `RunAction` with optional argument overrides. Core asynchronously reads the
saved source; concurrent configuration changes remain possible. MCP neither
substitutes source nor persists merged arguments.

Core's pinned `mogh_config` 1.1.0 merge recursively combines objects, appends
arrays and replaces scalars. For example, defaults `{"o":{"a":1},"a":[1]}`
and overrides `{"o":{"b":2},"a":[2]}` produce
`{"o":{"a":1,"b":2},"a":[1,2]}`. Overriding an existing object or array with
a different type, including null, fails in Core. Empty per-run objects/arrays
preserve defaults; they do not clear them. Omitted/null `args` uses only defaults.

Argument objects are limited to 8192 JSON bytes, nesting depth 8, 64 entries
per collection and 128-byte keys without control characters; definition patches
are limited to 16384 bytes. Core parses and merges saved defaults authoritatively.
A merge failure is an execution failure, without rewriting the definition.

The shared receipt retains known update IDs, acceptance status and observation
errors. `get_update`, `wait_for_update` and `reconcile_operation` work read-only
across restarts. Recovery uses kind `action`, scope `whole_action`, operation
`run` or `cancel`. Lost/invalid acknowledgements remain unknown; even one
matching history candidate does not establish caller attribution. Deadlines,
local wait cancellation and restart never replay work or request remote cancel.

Arbitrary Action stdout/stderr may contain unlabelled argument values, source or
temporary administrator credentials. MCP omits that text, including in generic
history after a restart. Bounded stage timestamps/success, structured Core update
outcome and `observed_action_result` retain command-stage success, output presence
and the exact Core process-group cancellation marker when reported. Missing or
duplicate `Execute Action` stages provide no command result; scripts can print
the cancellation marker themselves. Core update status is distinct from submission
acknowledgement, child-operation results and workload health.

## Explicit cancellation

```json
{"target":{"name":"trusted-action"},"expected_update_id":"000000000000000000000100"}
```

`cancel_action` requires operate/Core Execute and an explicit original update
ID. Both the handler and shared execution transport verify its exact selected
Action and `RunAction` operation before submitting. Core does not validate this
association itself. MCP always sends the exact ID; it never uses Core's fallback
to the first running Action update. Other overlapping runs remain independent.
For update-targeted cancellation, active evidence describes this original ID,
not an exhaustive list of all running Actions.

An already-complete original sends nothing. Token removal/completion can race
the final read: Core may acknowledge the cancellation submission but its own
update can fail to find an active token. Inspect that cancellation receipt and
the original update separately. A successful cancellation update acknowledges a
token signal, not the original's terminal state. Core kills the Deno process
group without rolling back effects or cancelling child Core operations already
submitted by the script. A concurrent replacement run is never selected by a
resource-only fallback. Preserve both known IDs if subsequent observation fails.

Pinned implementation references: [Action entity and defaults](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/entities/action.rs),
[Core execution and cancellation](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/execute/action.rs),
[Core startup Actions](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/startup.rs),
and [mogh_config 1.1.0 source](https://docs.rs/crate/mogh_config/1.1.0/source/src/merge.rs).
