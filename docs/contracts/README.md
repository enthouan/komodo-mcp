# v1 MCP tool contract

The [v1 snapshot](v1.json) records what clients discover and receive from the
running executable. The required GitHub `check` job runs the comparison as part
of `cargo test --locked --all-targets`. To run it locally:

```sh
cargo test --locked --test contract_v1 running_server_matches_v1 -- --exact
```

The harness launches the production binary on stdio and on an ephemeral
loopback HTTP listener. Both clients initialize with MCP `2025-11-25`. HTTP
uses a synthetic bearer token. Each process receives only its fixture settings.
Read responses, denied operations and races come from the Core 2.3.2 wire
fixture. Real Core permissions and packaged client interoperability require
separate live evidence.

- `application.discovery.tools` records every advertised tool and annotation.
  The `inputSchema` and `outputSchema` entries reference `discovery.schemaPool.schemas`.
  To expand a schema, restore its local `$defs` from `schemaDefinitions` and
  `definitions` in the same pool. The harness independently expands the
  [generated catalog](../tool-catalog.json) and compares every wire schema.
  The terminal tool has a documented result type and omits a wire output schema.
- `application.profiles` contains the discovered tool names for every
  combination of operate, configuration edit, prune and terminal grants.
  `application.calls.perToolVisibility` records selection overlap and a valid
  empty selection. The harness also verifies direct dispatch of hidden tools.
- `application.calls.examples` records the actual arguments and full JSON-RPC
  result or error of each fixture call. Examples include the running-only
  container inventory, a paged list followed by fresh inspection of the returned
  stopped-container identity, the read-only caller's inspect denial, a
  replaced or missing identity, invalid arguments, a clipped Docker scan and a
  log tail clipped by both line and byte limits.
- `streamableHttp` records HTTP admission and session ownership behavior.
  The clients compare only application behavior across transports. The harness
  checks that session IDs are distinct and nonempty and excludes the generated
  values. It validates the MCP version field against Cargo before substituting
  its version placeholder. Fixture identifiers and timestamps are deterministic.
  Array order, field presence, limits, permission results and error codes remain
  visible in the snapshot.

## Review a change

1. Reproduce the old and proposed behaviors on the focused call or tool.
   Explain the operator impact in the PR, including compatibility, permission
   changes, and any migration from the previous input or response.
2. Add or update a focused test on both transports for the changed behavior.
   Regenerate the offline catalog if registered metadata changed and inspect
   its diff.
3. Regenerate the snapshot explicitly and review the exact JSON delta. A change
   that affects only one transport fails before a replacement snapshot can be
   written.

   ```sh
   cargo run --locked --example tool_catalog -- --write
   cargo test --locked --test contract_v1 regenerate_v1 -- --ignored --exact
   git diff -- docs/tool-catalog.json docs/contracts/v1.json
   cargo test --locked --test contract_v1 running_server_matches_v1 -- --exact
   ```

4. Update the operator guide, compatibility claims and `Unreleased` notes as
   appropriate. Review the snapshot alongside the implementation and its tests.
   Require the protected PR checks and record the merged SHA and the green
   main CI run in the tracking issue. To identify the exact frozen file in a
   checkout, run `git rev-parse HEAD:docs/contracts/v1.json`.

## Reproduce a rejected change

For a fast diagnostic check, run

```sh
cargo test --locked --test contract_v1 one_changed_annotation_identifies_the_public_field -- --exact
```

For a full drift demonstration, start from a checkout containing the reviewed
baseline. Change the `idempotent_hint` annotation on `list_containers` from
`true` to `false` in `src/server.rs`. Regenerate the website catalog, then run
the focused comparison against the reviewed snapshot.

```sh
cargo run --locked --example tool_catalog -- --write
cargo test --locked --test contract_v1 running_server_matches_v1 -- --exact
git diff --no-index docs/contracts/v1.json target/contract-v1.actual.json
```

The comparison exits nonzero and identifies the JSON pointer
`/application/discovery/tools/list_containers/annotations/idempotentHint`.
The diff shows the original and observed booleans. For hosted CI evidence,
commit the temporary annotation and catalog together, link the failed `check`
run, then restore those two changes to obtain a green comparison.

## Hosted drift demonstration

The [required CI check](https://github.com/enthouan/komodo-mcp/actions/runs/45535493657/job/138940951094) rejected a real change to
`list_containers` at [`0eea9e97302b`](https://github.com/enthouan/komodo-mcp/commit/0eea9e97302b418e6154c2d32c610f803b918769).
It compared the updated advertisement and generated catalog with the frozen file.

```text
v1 contract drift: /application/discovery/tools/list_containers/annotations/idempotentHint: true -> false
```

The [contract PR](https://github.com/enthouan/komodo-mcp/pull/147) restores
the original annotation. Container discovery and inspection were already present
in [the reviewed server](https://github.com/enthouan/komodo-mcp/commit/d823a02fefc241033e652917b448e84d5a83397f).
