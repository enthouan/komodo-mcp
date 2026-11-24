# Scoped resource TOML export

`export_resources_toml` needs no local capability grant. Operators can include or exclude it through tool selection. It uses
the caller's existing credentials and only Core read requests. It performs no
file write, cache refresh, RunSync or deployment. Core retains its authoritative
Read/visibility decisions; an upstream denial remains a denial.

Select each resource by its explicit kind and exactly one stable ID or exact
name. The `targets` array is required and has at most 16 entries. Supported
kinds are `server`, `swarm`, `stack`, `deployment`, `build`, `repo`, `builder`,
`alerter`, `procedure`, `action` and `resource_sync`.

```json
{"name":"export_resources_toml","arguments":{"targets":[{"kind":"stack","target":{"name":"example-stack"}},{"kind":"deployment","target":{"id":"000000000000000000000003"}}]}}
```

Replace example selectors with actual visible resources. IDs are 24 hexadecimal
characters. Names are exact, case-sensitive, nonblank, at most 128 bytes, and
contain no control characters. Resolution reuses bounded inventory searches,
including explicit names that look like ObjectIds. Missing, ambiguous, duplicate,
invalid and unsupported selections fail; they never become an unfiltered export.
Each ordinary target takes one targeted list and one authoritative Get, then the
single scoped export request. ObjectId-shaped names can take up to four search
pages and one Get per target. These observations are not an atomic snapshot.

User groups require a separate explicit array of at most 16 selectors:

```json
{"name":"export_resources_toml","arguments":{"targets":[],"user_groups":[{"name":"example-group"}],"include_variables":true}}
```

Core's visible group list resolves selectors to stable IDs; hidden or missing
groups fail. `include_variables` defaults to false. **True selects all variables
returned by Core**, with no per-variable selection or independent completeness
attestation. Every variable value is omitted, including non-secret variables.
An empty `targets` array requires explicit groups or `include_variables:true`.
Missing selection never silently exports everything. There is no
`ExportAllResourcesToToml` fallback or user-supplied `existing` TOML parameter.

## Sanitization and reuse limits

The returned `toml` parses as a complete TOML document. A real parser and
serializer remove comments and project a positive allowlist. Resource names,
bounded safe tags/dependency references, selected resource-reference fields and
specific boolean settings can remain. Arbitrary descriptions, environment,
Compose/TOML/Dockerfile content, scripts, commands, arguments, paths, URLs,
credential references/values, group membership/permission subtrees and unknown
fields are omitted. Builder projection retains its known variant and only safe
Server references; AWS/URL parameters are omitted. This is deliberately more
conservative than removing keys named `password` or `secret`: embedded scripts
and documents can contain secrets under any key. The same policy applies to
administrator export replies, which can contain raw secret variables.

`omitted_fields` counts omitted fields/subtrees, rather than echoing potentially
sensitive field names or values. Configured API credentials are removed from
both MCP representations and trace diagnostics. If final credential redaction
would alter the serialized TOML syntax, the tool rejects the response instead
of returning a corrupted document. Parse errors never include source snippets.

`targets` and `user_groups` report `returned_by_core` for every explicit
selection. `selection_complete` concerns those definitions only; it never
claims complete configuration or a lossless round trip. Core 2.3.2 omits inline
and unconfigured ResourceSync definitions from its export implementation. Other
missing selected definitions are also disclosed. Unexpected/unselected or
duplicate returned definitions fail closed rather than expanding the selection.
References can point to resources outside the selected set.

**`authoritative_apply_input` is always false.** The document and warnings label
it as an incomplete inspection export. Restore and review the complete intended
configuration before constructing desired state. An omitted typed definition
patch field preserves that setting, whereas an omitted desired-state TOML field
can reset existing configuration to its default when Core applies a sync.
Importing this sanitized document unchanged could therefore clear settings or
values. Export does not supply a reviewed apply reference or authorize execution.

## Bounds and verification

All resolution/export reads use the shared 15-second complete-request deadline
and 2 MiB response-body cap, including streamed/chunked bodies. Source TOML is
limited to 128 KiB and 128 total resource/group/variable entries; sanitized TOML
to 64 KiB; the final structured JSON projection to 256 KiB. Safe reference lists
contain at most 32 entries of at most 128 bytes. Excess body, source, entries or
output fails without returning any partial or byte-truncated TOML. Narrow the
selection or exclude the all-variable scope when a budget is exceeded.

```sh
cargo test --locked --test export --test stdio_smoke --test capabilities
```

These tests launch the production executable, exercise all eleven resource
kinds and four local profiles, and measure zero mutation requests. They cover
exact/ambiguous/missing and ObjectId-shaped-name resolution, group/all-variable
scope, privileged secret-bearing replies, quoted/multiline/nested TOML,
upstream denials, Core omissions, malformed replies and input/body/source/output
bounds. Fixtures verify the MCP contract; live Core authorization and integrated
ResourceSync evidence belong to [#103](https://github.com/enthouan/komodo-mcp/issues/103).

Pinned sources: [scoped request](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/api/read/toml.rs),
[Core export and omission/variable behavior](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/read/toml.rs),
and [Core TOML conversion](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/sync/toml.rs).
