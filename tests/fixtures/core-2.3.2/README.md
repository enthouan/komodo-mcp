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
