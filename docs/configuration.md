# Typed configuration edits

Set `KOMODO_ALLOW_CONFIGURATION_EDIT=true` in the MCP child environment. The
default is false. This trusted grant is independent of
`KOMODO_ALLOW_OPERATE`; Core remains the permission authority. Preview needs
Read and saving an existing definition needs Write on the Stack. Use the same
credentials throughout; no permission or credential fallback occurs.

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

The minimal Stack surface currently supports:

| Field | Omitted | Explicit clear | Preview |
| --- | --- | --- | --- |
| `auto_pull` | Preserve | Unsupported; use true/false | Before/after boolean |
| `ignore_services` | Preserve | `[]` | Before/after ignored service sets |
| `environment` | Preserve | `""` | Nonempty values redacted, including secret references |

Unknown fields and JSON null are rejected. Patches are limited to 16384 JSON
bytes; ignore lists to 32 nonempty names of at most 128 bytes. Environment is
the complete replacement string for that one field. Preserve any references
you intend to keep when replacing it. A display marker such as `[redacted]` or
`[hidden]` is rejected, never written as configuration. An omitted environment
leaves its real saved secret references/placeholders untouched. The one-field
request uses the pinned SDK's partial type and sends no unrelated defaults or
nulls. The pinned environment deserializer appends a final newline to a nonempty
string when needed; comparison and submission use that canonical value. Stack
creation and source/target changes are a separate extension.

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
read-back of resource identity/observed runtime information. No deploy, file-write
or lifecycle endpoint is called by this editor. Core itself refreshes Stack and
target caches during UpdateStack. Saving does not establish deployment or health;
use explicit operation and diagnosis tools separately. These initial fields do
not trigger UpdateStack's source-change auto-update check.

Implementation sources: pinned [partial request](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/api/write/stack.rs),
[Core write handler](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/write/stack.rs),
[resource update](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/resource/mod.rs),
and [Stack hooks](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/resource/stack.rs).

Deterministic verification launches the production binary over stdio:

```sh
cargo test --locked --test configuration --test capabilities
```

It measures one-field requests, zero-write previews/no-ops, raw-secret baseline
changes, altered input, supported clears, four capability profiles, upstream
denial, malformed/oversized/lost responses and process restart. The opt-in
[disposable-Core runner](operations-walkthrough.md#opt-in-disposable-coreperiphery-smoke)
adds real Core/Periphery 2.3.2 Server-backed inline Compose evidence and the
same-identity restart/edit permission boundary. Swarm and other sources are not
claimed as live coverage for this initial surface.
