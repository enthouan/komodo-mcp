# Changelog

Release sections are the reviewed source for GitHub Release notes. Application
versions follow SemVer; publication dates and commit identities belong in the
GitHub Release and tracking evidence.

New changes go under `Unreleased` until the next release is prepared.

## Unreleased

- Add explicit single-target Stack/Deployment deploy tools, named Compose service
  scope, stable-ID submissions, bounded sanitized update receipts and immediate
  observation. Reject unsupported filters without expanding scope; retain unknown
  submission outcomes and known update IDs without automatic retry.
  ([#12](https://github.com/enthouan/komodo-mcp/issues/12))

- Add independent operate and configuration-edit capability grants, defaulting
  to read-only, with one policy for discovery, lookup and direct dispatch.
  Preserve read-only recovery history and Core's permission authority, including
  denial recorded after execution acknowledgement.
  ([#11](https://github.com/enthouan/komodo-mcp/issues/11))

## v0.2.0

Read-only fleet incident diagnosis with bounded, linked supporting evidence.

- Configure weekly Cargo/Actions dependency updates and document the required
  checks, solo-maintainer main protection, and release compatibility.
  ([#64](https://github.com/enthouan/komodo-mcp/issues/64))
- Add paged fleet health with CPU, memory, disk, and service observations,
  four concurrent reads per call, deadlines, and resource-specific evidence gaps.
  ([#6](https://github.com/enthouan/komodo-mcp/issues/6))
- Clarify the dedicated changelog release cut and exact release commit/PR naming.
- Add bounded container, deployment, and service-filtered stack log readers with
  Core input caps, joint line/UTF-8 byte limits, truncation metadata, and credential
  redaction before tail selection, including camelCase labels and multiline values.
  ([#8](https://github.com/enthouan/komodo-mcp/issues/8))
- Add bounded alert/update history and update lookup, explicit queued/running/final
  outcomes, and sanitized failed-stage evidence with capped upstream bodies.
  ([#7](https://github.com/enthouan/komodo-mcp/issues/7))
- Add one-call stack diagnosis with current health outcomes, affected services,
  linked evidence and source gaps, plus bounded reads, wait and serialized output.
  ([#9](https://github.com/enthouan/komodo-mcp/issues/9))
- Verify fleet-to-stack diagnosis and independent evidence through production
  stdio, and extend the retained inventory benchmark with concurrent incident
  workflows and fleet growth against the pinned reference.
  ([#10](https://github.com/enthouan/komodo-mcp/issues/10))

### Evidence and distribution limits

Verification uses synthetic **Core 2.3.2 wire fixtures** and simulated Read
permissions. No deployed Core, Docker or authorization engine was exercised.
The application version `0.2.0`, MCP protocol, SDK version and fixture Core
version are independent identities.

The [incident measurements and raw reports](https://github.com/enthouan/komodo-mcp/blob/49389893b80c39744213c21970b56b1c94cfe554/benchmarks/results/incident-final-README.md)
cover 7/31/125 stacks with one/four callers and retain the inventory workload.
Results are provisional shared-host observations. Both implementations return
the same common incident evidence, while Rust also reads broader fleet metrics,
services, alerts and diagnosis; these different workloads establish no general
performance ranking. Archives preserve their actual pre-release source,
executable and application-version identities. This version/changelog cut does
not claim new performance measurements.

Release artifacts are an annotated source tag and GitHub Release notes. Build
the stdio executable from source using the
[installation walkthrough](https://github.com/enthouan/komodo-mcp/blob/main/README.md#fresh-checkout-demo-synthetic-core).
No container images or native binary packages are distributed in this release.

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
