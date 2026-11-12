# Changelog

Release sections are the reviewed source for GitHub Release notes. Application
versions follow SemVer; publication dates and commit identities belong in the
GitHub Release and tracking evidence.

New changes go under `Unreleased` until the next release is prepared.

## Unreleased

- Clarify the dedicated changelog release cut and exact release commit/PR naming.
- Add paged fleet health with CPU, memory, disk, and service observations,
  four concurrent reads per call, deadlines, and resource-specific evidence gaps.
  ([#6](https://github.com/enthouan/komodo-mcp/issues/6))
- Add bounded resource-filtered alerts and update history, plus update lookup
  with distinct queued/running/completed outcomes and selected failed stages.
  ([#7](https://github.com/enthouan/komodo-mcp/issues/7))
- Add bounded container, deployment, and service-filtered stack log tails with
  credential redaction, UTF-8 preservation, and explicit truncation metadata.
  ([#8](https://github.com/enthouan/komodo-mcp/issues/8))
- Add `stack_diagnose` to correlate current service health with inventory,
  history, alerts, and logs in one read-only call, retaining partial evidence
  and reporting unhealthy, healthy, or inconclusive outcomes.
  ([#9](https://github.com/enthouan/komodo-mcp/issues/9))
- Define a shared broken-rollout fixture, verify triage and independent evidence
  through production stdio, and extend the existing pinned-reference benchmark
  with concurrent incident callers and larger fleets while retaining inventory.
  ([#10](https://github.com/enthouan/komodo-mcp/issues/10))


## v0.1.0

First release of the Rust stdio MCP server for inspecting Komodo inventory.

- Implement the MCP initialization, tool discovery, request, and clean stdin-EOF
  shutdown lifecycle using the official rmcp SDK.
  ([#1](https://github.com/enthouan/komodo-mcp/issues/1),
  [#53](https://github.com/enthouan/komodo-mcp/pull/53))
- Connect through the official `komodo_client`, with validated environment
  configuration, bounded requests, credential redaction, and safe error results.
  ([#2](https://github.com/enthouan/komodo-mcp/issues/2),
  [#54](https://github.com/enthouan/komodo-mcp/pull/54))
- Expose six read-only Server, Stack, and Deployment list/detail tools, with
  bounded pagination, exact ID/name lookup, compact allowlisted projections, and
  explicit incomplete or ambiguous lookup results.
  ([#3](https://github.com/enthouan/komodo-mcp/issues/3),
  [#55](https://github.com/enthouan/komodo-mcp/pull/55))
- Provide a synthetic mock Core installation walkthrough, an official rmcp
  stdio client example that computes stacks on a server across every page, and
  MCP Inspector configuration examples.
  ([#4](https://github.com/enthouan/komodo-mcp/issues/4),
  [#56](https://github.com/enthouan/komodo-mcp/pull/56))
- Add reproducible fresh-process and complete-inventory benchmarks against a
  pinned TypeScript reference, with correctness oracles, raw measurements, and
  recorded workload and executable identities.
  ([#5](https://github.com/enthouan/komodo-mcp/issues/5),
  [#57](https://github.com/enthouan/komodo-mcp/pull/57))
- Establish a reusable release skill, annotated-tag/GitHub-Release publisher,
  recovery checks, and release-executable MCP version verification.
  ([#58](https://github.com/enthouan/komodo-mcp/issues/58))

### Evidence and distribution limits

Verification uses synthetic **Core 2.3.2 wire fixtures**, including simulated
permissions. No deployed Core was exercised; compatibility and authorization
behavior beyond those fixtures remain unverified. The application version
`0.1.0`, MCP protocol version, dependency version, and detected Core version are
independent identities.

Benchmark findings are **provisional and workload-specific**. The archived run
observed lower Rust startup/read latency and memory use, alongside larger MCP
responses and more Core reads. It does not establish general performance on
deployed Core or other hosts. See the
[baseline evidence and caveats](https://github.com/enthouan/komodo-mcp/blob/main/benchmarks/results/README.md).
No new performance baseline is claimed for this release commit.

This release publishes an annotated source tag and GitHub Release notes. Build
the stdio executable from source using the
[installation walkthrough](https://github.com/enthouan/komodo-mcp/blob/main/README.md#fresh-checkout-demo-synthetic-core).
It does not distribute container images or native binary packages. Later work
extends the same release process with
[GHCR/Compose (#41)](https://github.com/enthouan/komodo-mcp/issues/41),
[native binaries and Corresponding Source (#42)](https://github.com/enthouan/komodo-mcp/issues/42),
and [stable artifact verification, SBOMs, and provenance (#51)](https://github.com/enthouan/komodo-mcp/issues/51).
