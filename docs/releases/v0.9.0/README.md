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

## Packaged candidate evidence

The native archives, OCI image and Corresponding Source share the frozen source commit above. Cargo and MCP reported `0.8.0` during these milestone runs. The candidate record links the native and OCI workflow attempts and identifies each archive, executable and platform manifest. [Candidate record](candidate-record.json)

| Study | Observed scope and result | Evidence |
| --- | --- | --- |
| Distribution fixtures | Extracted archives initialized over stdio and returned the expected three matching Stacks on four native targets. Authenticated Compose inventory passed on both native Linux OCI platforms. | [Native distribution](https://github.com/enthouan/komodo-mcp/actions/runs/45539030427) · [Container distribution](https://github.com/enthouan/komodo-mcp/actions/runs/45539030406) |
| Original three-run disposable regression | Core and Periphery 2.3.2, Linux x86-64. Both transports verified stopped-container diagnostics, scoped denials, configuration changes and one accepted Deployment after interruption. The offline source rebuild exercised the rebuilt executable and matched the packaged SHA-256. | [Regression results and cleanup](disposable-regression.md) · [Sanitized measurements](disposable-regression.json) · [Third hosted run](https://github.com/enthouan/komodo-mcp/actions/runs/45544954963) |
| First filesystem-isolated source rebuild | The source job failed. Its scratch root omitted the public CA bundle. A control with the same executable initialized and called the fixture after that bundle was mounted read-only. Both live jobs passed. | [Hosted run 45551072725](https://github.com/enthouan/komodo-mcp/actions/runs/45551072725) · [Failure and control observations](disposable-regression.md) |
| First native sandbox preflight | Ubuntu denied the unprivileged UID mapping. The stdio and source jobs stopped before acquiring the candidate. The separate HTTP job passed its live scenarios and cleanup. | [Hosted run 45553507524](https://github.com/enthouan/komodo-mcp/actions/runs/45553507524) |
| Final packaged regression | Both live transports passed all five scenarios against Core and Periphery 2.3.2 and verified all eleven cleanup inventories were empty. The offline source job independently reproduced the native executable and read three fixture Stacks through MCP. | [Final hosted run](https://github.com/enthouan/komodo-mcp/actions/runs/45555808579) · [Artifact identities](#final-regression-artifacts) |
| Benchmark | Native Intel macOS stdio. The candidate and two pinned references returned the expected 31-Stack fixture inventory, both incident fixture answers and seven live Stacks. Measurements include per-sample latency, calls, bytes and the directly observed RSS boundary. | [Findings and reproduction](../../../benchmarks/results/README.md) · [Numeric export](benchmark-samples.json) |

The [compatibility matrix](../../v0-9-0-compatibility.md) records the exact client and protocol at each validation level. The [release tracker](https://github.com/enthouan/komodo-mcp/issues/145) records the milestone and release decision.

## Final regression artifacts

Run 45555808579, attempt 1, used harness commit `ce85cb763039da6396305d232936affb378328c8` and the shared candidate at `e8432d39d6c12c5278d26f17959f041884ab5a4c`. The source result recorded `isolated_source_build` as true. The verifier independently matched the extracted executable to the packaged Linux x86-64 SHA-256 `248d87fc69c51f011846c52b5c3e413d8fb32db857dd058b25ed72dc5d3b640e`. The table records the GitHub Actions artifact digests for the three sanitized receipts. [Workflow run](https://github.com/enthouan/komodo-mcp/actions/runs/45555808579) · [Candidate record](candidate-record.json)

| Case | Actions artifact ID | Artifact SHA-256 |
| --- | --- | --- |
| Native stdio | `14995237718` | `6e2aaf67afc516977dbbcab09fbc7ecabc2de4fec79200d5f5b649dda6dfbd21` |
| Authenticated Streamable HTTP | `14995717729` | `137ca7169ab41c21e653b82beca01f325c77cfa77db8a40226446cafb568099e` |
| Offline source rebuild | `14994964431` | `f025c3e4aa4e9161916daf9f9c8e1f03604175698211f166adb339562ad8cefc` |
