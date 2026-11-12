# Core 2.3.2 fixtures

These synthetic fixtures describe an enabled, non-admin service user with a
`Read` grant on server `000000000000000000000001`. The separate
`restricted-server` has no grant. The mock checks both API credential headers,
the SDK's `POST /read` envelope, and the exact read sequence. It rejects write,
execute, and unexpected requests. It does not execute Core's authorization code.

Source: [moghtech/komodo v2.3.2](https://github.com/moghtech/komodo/tree/v2.3.2),
commit `cc1c5aa5dabb738e843c47f5e87fc040648c2bb7`:

- [SDK request transport](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/request.rs):
  `x-api-key`, `x-api-secret`, and `{ "type": ..., "params": ... }`.
- [GetVersion resolver](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/read/mod.rs):
  the running Core package version, returned as `{ "version": "2.3.2" }`.
- [ListServers and GetServer resolvers](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/read/server.rs),
  [server response types](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/entities/server.rs),
  and [resource list types](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/entities/resource.rs):
  list responses are arrays; no visible resources means `[]`.
- [User and permission helpers](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/permission.rs):
  list queries filter by `Read`; a restricted `GetServer` produces the exact
  message in `permission.json`. This helper returns `anyhow::Error`, converted
  through `?` to **HTTP 500**, rather than 403.
- [Core authentication](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/auth/mod.rs)
  and [credential validation](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/auth/middleware.rs):
  invalid credentials or a disabled user produce **HTTP 401**.
- The [tagged Core lockfile](https://github.com/moghtech/komodo/blob/v2.3.2/Cargo.lock)
  selects `mogh_error 1.0.4` and `mogh_auth_server 1.8.0`.
  [Published mogh_error 1.0.4](https://docs.rs/crate/mogh_error/1.0.4/source/src/axum.rs)
  converts an unannotated error to HTTP 500 and serializes its chain as
  `{ "error": "...", "trace": [] }`.

Tests also append synthetic credential markers to error messages and `trace`
entries. Those bodies are adversarial reporting tests, not literal Core output.
The successful server is disconnected and contains no real infrastructure data.
No running Core was used: this evidence verifies the official client and wrapper
against the tagged wire contract, not a deployed Core or its permission engine.

## Inventory extensions (issue #3)

`stacks.json` / `deployments.json` add computed summaries; `server-detail.json`,
`stack-detail.json`, and `deployment-detail.json` add full resource responses.
`reader-permissions.json` gives the same synthetic non-admin user only `Read`
grants for fixture Servers, Stacks, Deployments (IDs 1–8 in each family), and the
Swarm association. The original server grant remains for foundation tests.
Generated variants model pagination, duplicate names, inaccessible targets,
missing association names, Swarm precedence, identity races, and state changes.
These are controlled wire fixtures; the mock does not run Core's permission
engine or assert that normally prohibited duplicate names can be created.

Additional tagged sources:

- [ListStacks/GetStack](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/read/stack.rs),
  [Stack wire types and project-name precedence](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/entities/stack.rs).
- [ListDeployments/GetDeployment](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/read/deployment.rs),
  [Deployment wire types and target precedence](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/entities/deployment.rs).
- [Computed Server summaries](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/resource/server.rs),
  [Stack summaries](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/resource/stack.rs),
  [Deployment summaries](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/resource/deployment.rs).
- [ResourceQuery filters and ObjectId interpretation](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/entities/resource.rs):
  `names` splits ObjectIds from exact names; `terms` escapes regex metacharacters
  and searches names case-insensitively. Tools verify case-sensitive equality.
- [Resource helpers](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/resource/mod.rs):
  Name sorting streams from Mongo; pagination skips `page * limit` visible items;
  limit zero is unlimited. `get` reports `Did not find any {Type} matching {id}`
  through anyhow/mogh_error as HTTP 500. Only the supported Get operations with
  that complete signature and a hexadecimal ObjectId are classified `not_found`.
  Unrelated 500/404 responses remain upstream failures.

Distinct synthetic markers populate actual SDK fields: Server passkey/public
keys, Stack git account/environment/Compose/stored contents/remote error files,
and Deployment registry account/environment/command/stored digest. A typed
fixture test verifies they deserialize into those fields. Error traces have a
separate marker. Exact output-key assertions and marker scans cover structured
content, text content, ambiguity/error metadata, and verbose subprocess stderr.
Selected display fields are separately tested with the configured API key and
secret to verify redaction.

The bounded name-search stress fixtures generate 100 near matches per page to
exercise the 400-summary bound; they are not deployed Core captures. No live
Core, production infrastructure, or private credentials were used.

## Reusable quickstart mock (issue #4)

`examples/support/mock_core.rs` reuses the base summary/detail/version/error
fixtures above, extending them according to `demo.json`. It embeds all files at
compile time. Three Servers (`fixture-server`, `other-server`, `empty-server`)
and six Stacks exercise multiple matching pages, another Server, Swarm precedence,
and an unassigned Stack. `demo-permissions.json` explicitly grants the existing
reader Read access to these resources, the original Deployment, and the Swarm.
The assembled wire shapes and grants are validated when the mock starts.

Unlike the ordered `tests/support/MockCore`, this mock dispatches by official
request type without consuming a queue. Repeated independent clients work in
any order. It models the production tools' common filters, Name ordering, and
pagination, rejects unsupported query features and non-read requests, and records
safe request evidence. Unknown/missing details use the tagged Core's HTTP 500
missing-resource shape; `restricted-server` uses `permission.json`. Rejected
credential headers return the tagged HTTP 401 authentication fixture. No real
Core or authorization engine is run. The separate sequence-based harness remains
useful for exact-request, adversarial response, and race tests.

## Shared incident extensions (issues #6–#10)

[INCIDENT.md](INCIDENT.md) defines the seeded failure; `incident.json` is its
scenario contract and `incident-wire.json` supplies typed SDK wire responses.
Rust reader/diagnosis/stdio tests and the Python benchmark reuse those IDs.
Adversarial variants exercise failed sources, incomplete coverage, changing
update outcomes, oversized UTF-8 content, and redaction. These remain synthetic
wire fixtures with simulated permissions.
