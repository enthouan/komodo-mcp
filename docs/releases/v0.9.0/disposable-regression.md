# Packaged candidate regression

Both packaged transports passed against Komodo Core 2.3.2 and Periphery 2.3.2 on Linux amd64. The offline Corresponding Source build reproduced the packaged executable byte for byte. The candidate reports `0.8.0` at `e8432d39d6c12c5278d26f17959f041884ab5a4c`. [Candidate record](candidate-record.json) · [Machine-readable results](disposable-regression.json) · [Hosted run](https://github.com/enthouan/komodo-mcp/actions/runs/45544954963)

| Case | What ran | Result |
| --- | --- | --- |
| Native stdio | Verified `x86_64-unknown-linux-musl` executable in a disposable Core project | Initialization, discovery and live calls passed. |
| Authenticated Streamable HTTP | Matching OCI image at its verified linux/amd64 manifest, in a second disposable project | Missing and incorrect tokens were rejected. Authenticated initialization, discovery and live calls passed. |
| Offline source rebuild | Matching source archive, fresh Cargo home, Rust 1.99.1, musl and loopback-only networking | Rebuilt archive passed its MCP inventory probe. A second stdio call to `list_stacks` returned the three expected fixture rows and reached Core's fixture once. |

The live jobs used `komodo-mcp-disposable-candidate` version 1, Python 3.12.16, MCP protocol 2025-11-25 and nested Docker 27.5.1. The native, container and rebuilt executables share SHA-256 `248d87fc69c51f011846c52b5c3e413d8fb32db857dd058b25ed72dc5d3b640e`. The shared record gives both distribution run IDs and attempts, Actions artifact IDs and digests, archive hashes, and the OCI index and platform manifest digests. [Native distribution](https://github.com/enthouan/komodo-mcp/actions/runs/45539030427) · [Container distribution](https://github.com/enthouan/komodo-mcp/actions/runs/45539030406)

## Live checks

- Both transports listed one running container by default and returned two one-item pages when stopped containers were included. Fresh inspection of the exact stopped ID matched Docker's exited state, exit code 17, unhealthy status and zero restarts. A caller with Server Read and Attach could list the container. Core denied inspection without Inspect, and a direct call to a hidden tool was rejected.
- Stack configuration preview submitted zero writes. Apply submitted one write, and Core read-back confirmed the change to `auto_pull` and equality of every other configuration field.
- A scoped ResourceSync reader discovered the selected tools with both local capability gates enabled and reviewed the owned plan. Core denied the apply preflight's single refresh for lack of Execute. Both transports reported zero `RunSync` submissions. Authoritative history had zero additional updates and the target Server configuration was unchanged. A separate edit after review returned `stale_preview` and added zero `RunSync` records.
- After an accepted deployment, the fixture killed the observing MCP process. A fresh read-only client resumed observation using the known update ID. Both runs found exactly one new Core deployment update by the scoped operator and independently observed the container running in Docker.
- The fixture verified a recognizable private value in the seeded health output and checked MCP frames, protocol errors, process diagnostics and retained scenario evidence for disclosure.

The final inspections found zero remaining Stack, Deployment, ResourceSync and Server records, scoped service users, nested fixture and deployment containers, or outer MCP containers. Separate Compose inspections found zero project containers, networks and volumes. [Per-transport measurements and evidence artifact digests](disposable-regression.json)

## Earlier runs and scope

The first hosted run completed all five live scenarios on both transports and passed the source rebuild. Its fixture client reached its 10-second deadline while Core was deleting the running Deployment, which had a 10-second termination grace period. Each pre-Compose inspection still found two Servers, four scoped service users and two nested fixture containers. All three Compose inventories were empty after teardown. [First hosted run](https://github.com/enthouan/komodo-mcp/actions/runs/45542374298)

The next run passed all three jobs after the fixture allowed 45 seconds for that single deletion. It verified all eleven inventories were empty on both transports. The recorded run repeated those checks after adding contract ancestry and blob verification and scanning stdio output through EOF. All three runs used the same candidate bytes. Comparing the recorded rebuild digest with the native manifest confirmed byte-for-byte equality. Subsequent runs isolate the packaged build script from the native and OCI downloads, then require checksum equality before the MCP probe and again before exporting a passing result. [Cleanup repeat](https://github.com/enthouan/komodo-mcp/actions/runs/45543736702) · [Recorded run](https://github.com/enthouan/komodo-mcp/actions/runs/45544954963)

Live coverage is limited to Core and Periphery 2.3.2 on Linux amd64 with this scripted MCP client. The source rebuild used a synthetic Core fixture. Other architectures, third-party clients, Core versions and production deployments were outside these runs. [Reproduction commands](../../../tests/fixtures/disposable-core/README.md#packaged-v09-candidate-regression)
