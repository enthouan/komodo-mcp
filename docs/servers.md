# Typed Server records

Set `KOMODO_ALLOW_CONFIGURATION_EDIT=true` in the MCP child environment. The
same enabled Core identity needs Read to preview an existing Server and Write
to save it. Creation requires admin, or `create_server_permissions` with Core's
non-admin creation policy enabled. A Server record points to an existing
Periphery agent; creating it does not install or provision that agent.

Applying any supplied `address`, `enabled` or `insecure_tls` also requires
`KOMODO_ALLOW_OPERATE=true`, even for unchanged connection input. Creating an
enabled record requires both grants. Core reconnects/refreshes and existing
background pruning/key policies can follow a changed target; policies can race.
Configuration-only access supports disabled creation and ordinary alert,
monitoring, region and ignore-mount edits. Operate-only access does not expose
or dispatch these tools.

## Create, inspect and patch

Use `list_servers` to find a visible Server’s stable ID and `get_server` to inspect it.

Stage a disabled record with explicit connection and TLS choices:

```json
{
  "name": "staged-server",
  "config": {
    "address": "wss://periphery.example:8120",
    "enabled": false,
    "insecure_tls": false,
    "region": "west"
  }
}
```

Pass that object to `preview_server_create`, inspect the name, redacted changes
and effects, then pass the exact object plus its `preview_id` to
`apply_server_create`. Names preserve spaces, case and dots; Core trims surrounding
whitespace and replaces newlines with underscores. Names are bounded and cannot
be ObjectId strings. Core enforces global uniqueness; bounded visible name search
cannot rule out hidden names or concurrent creators. Use the returned stable ID
with `get_server` and subsequent edits.

Pass this object to `preview_server_update`, then the same target/patch plus its
token to `apply_server_update`:

```json
{
  "target": { "id": "<Server ID>" },
  "patch": { "send_unreachable_alerts": false }
}
```

Only that typed partial field is submitted. Omission preserves connection,
legacy passkey, trust, background policies and every other setting. An unchanged
proposal revalidates the baseline and submits zero writes. The
[shared preview/apply contract](configuration.md) supplies one bounded sixteen-entry,
ten-minute, process-local, single-use token store. Operation/resource-kind binding
prevents Stack or Deployment tokens from authorizing a Server write.

| Field | Supported value and clear behavior |
| --- | --- |
| `address` | At most 2048 bytes; ws/wss/http/https with a host, without credentials, query, fragment, whitespace, backslash or port zero. `""` selects Periphery-to-Core. Core converts HTTP schemes to WebSocket schemes. |
| `enabled` | Boolean, explicitly required for creation. Disabling disconnects Core's cache and marks observations unknown; it does not stop containers. |
| `insecure_tls` | Boolean, explicitly required for creation. `true` skips certificate validation for outbound TLS; it does not replace public-key trust. |
| `region` | At most 256 bytes without control characters; `""` clears. |
| `stats_monitoring` | Boolean; does not provision monitoring software. |
| `send_unreachable_alerts`, `send_cpu_alerts`, `send_mem_alerts`, `send_disk_alerts`, `send_version_mismatch_alerts` | Boolean alert toggles; false explicitly updates. |
| `ignore_mounts` | At most 32 nonempty strings of at most 256 bytes, without control characters; `[]` clears. |

Null, unknown fields and display redaction markers are rejected. Input/partial
JSON is capped at 16 KiB. Display bounds never truncate private baselines or
submitted fields. Existing thresholds, maintenance windows, links and automatic
policies are preserved and are outside this patch surface.

## Initial trust and preserved keys

An enabled save requires an explicitly pinned Periphery public key. Supply
`initial_public_key` alongside `name`/`config` only when creating a Server, using
the actual public key obtained independently from the intended agent. Bounded
nonblank input and Core's echoed key are checked privately; this does not
establish that agent's authenticity or reachability. Disabled creation may omit
the key and establish trust separately in Core later.

Existing key replacement and rotation are separate Core APIs. These tools never
invoke `UpdateServerPublicKey`, `RotateServerKeys`, onboarding, network creation
or provisioning. They reject key/passkey fields in update patches. Accepted
trust participates in the private stale baseline; an attempted key is only an
observation. A mismatch remains a connection failure requiring separate
deliberate trust management. No fallback accepts it.

Inspected Core 2.3.2 skips outbound public-key validation without an explicit key.
Creation also explicitly sets `auto_prune=false` and `auto_rotate_keys=false`
instead of inheriting Core's true defaults. Those additional fields appear in
the preview and create request. Updates preserve existing policies; independent
Core jobs can still act according to them.

Addresses and initial keys are always redacted in changes. Accepted/attempted
keys, legacy passkeys and raw connection errors are never returned. Summaries
show presence and direction. Configured API credentials are removed before
clipping other metadata. This protects MCP output/stderr, not Core's own
logging or storage policy.

## Saved configuration versus connectivity

`saved_configuration_verified=true` verifies the stable identity, typed config
echo and initial/preserved key from the write response. SDK defaults cannot turn
missing echoed fields or missing initial trust into verification. Lost, invalid,
oversized, denied or mismatched replies retain `submission_unknown`, consume
the token and never trigger a retry.

`definition.details.before`/`after` show desired configuration.
`definition.details.connectivity` separately reports Core's cached state,
marked `preview_time` or `after_save`. `connection_verified` remains false even
for `Ok`; cached status does not prove a new connection to the saved address.
`NotOk` carries a fixed actionable error directing checks of address, network,
TLS and pinned trust. A failed post-save read reports its sanitized error there
while retaining the verified save.

Core's create/update hooks refresh cache. Enabled records can authenticate and
poll system/Docker status and attached Repo state; disabled records disconnect.
Saving can succeed while connection fails. MCP submits no execution, host setup,
shell, lifecycle or key-management request. Saving does not deploy, stop or restart
attached workloads.

Raw full configuration and accepted trust are revalidated before submission.
Core lacks conditional writes and invocation idempotency; another writer, key
rotation or policy can race after final reads. Preview does not freeze remote
identities, network endpoints, certificates or cache state.

## Verification and pinned sources

Production stdio fixtures cover four profiles, direct dispatch, exact typed
payloads, omission/clears, preserved trust, raw private staleness, cross-resource
tokens, redaction, malformed/lost/denied replies, deadlines and post-save read
failure. Invalid, stale and unchanged proposals submit zero writes; no execution
or fallback request is allowed by the fixture.

The disposable runner exercises disabled creation/inspection, alert-only
preservation, local connection-apply denial with zero Core reads/writes, enabled
creation against an unreachable owned-container endpoint using independently
supplied initial trust, separate saved/NotOk evidence, Core creation-permission
denial and unchanged workload IDs/images/states. It verifies cleanup. Successful
outbound reconnection and actual inbound/changed-key handshakes have source
coverage, without a live matrix claim; fixtures exercise their saved settings,
cached states and accepted-versus-attempted trust boundaries.

Pinned references: [Server writes](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/write/server.rs),
[permissions/hooks](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/resource/server.rs),
[key validation](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/connection/mod.rs),
[cache refresh](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/monitor/mod.rs),
and [scheduled pruning](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/helpers/prune.rs).
