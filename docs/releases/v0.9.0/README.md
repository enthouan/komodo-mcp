# v0.9.0 compatibility and coverage

| Matrix | Evidence |
| --- | --- |
| [Compatibility](../../v0-9-0-compatibility.md) | Core, Periphery, clients, transports, platforms and validation levels |
| [Upstream API coverage](../../v0-9-0-api-coverage.md) | API families at Komodo v2.3.2 |
| [Official CLI coverage](../../v0-9-0-cli-coverage.md) | Command names, aliases, flags, scopes, grants and executable subcommands |

Container tools reached main at [`d823a02fefc241033e652917b448e84d5a83397f`](https://github.com/enthouan/komodo-mcp/commit/d823a02fefc241033e652917b448e84d5a83397f).
The reviewed [running-server contract](https://github.com/enthouan/komodo-mcp/blob/e8432d39d6c12c5278d26f17959f041884ab5a4c/docs/contracts/v1.json)
and [tool catalog](https://github.com/enthouan/komodo-mcp/blob/e8432d39d6c12c5278d26f17959f041884ab5a4c/docs/tool-catalog.json) are frozen
at [`e8432d39d6c12c5278d26f17959f041884ab5a4c`](https://github.com/enthouan/komodo-mcp/commit/e8432d39d6c12c5278d26f17959f041884ab5a4c).
The contract file's SHA-256 is `62c5f0e6050ffed72c21c32c3e9f017c98a49c83b18bacbe7507692fe33f79f0`.
The [required PR check](https://github.com/enthouan/komodo-mcp/actions/runs/45536255347) passed on this tree.
[Main CI](https://github.com/enthouan/komodo-mcp/actions/runs/45539030458) exercises the protected merge commit.

The API and CLI matrices use [Komodo v2.3.2 at
`cc1c5aa5dabb738e843c47f5e87fc040648c2bb7`](https://github.com/moghtech/komodo/tree/cc1c5aa5dabb738e843c47f5e87fc040648c2bb7).

## Candidate work

The [disposable regression](https://github.com/enthouan/komodo-mcp/issues/48)
targets Core and Periphery 2.3.2 with native Linux x86-64 stdio, authenticated
HTTP on linux/amd64 and a clean offline source rebuild. The
[benchmark comparison](https://github.com/enthouan/komodo-mcp/issues/49)
targets the companion Intel macOS package for the inventory and incident
workloads. Both workstreams will use the same source revision and package
receipts. The [release tracker](https://github.com/enthouan/komodo-mcp/issues/145)
collects the verified evidence.
