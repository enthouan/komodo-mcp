# Changelog

Release sections are the reviewed source for GitHub Release notes. Application
versions follow SemVer; publication dates and commit identities belong in the
GitHub Release and tracking evidence.

New changes go under `Unreleased` until the next release is prepared.

## Unreleased

## v0.4.0

Manage resource configuration with typed previews, deliberate applies and
separate runtime observations.

- Add typed tag creation/rename and explicit resource assignment preview/apply,
  preserving omitted assignments and unrelated metadata, with additive set
  behavior, deliberate empty-set clearing and read-only discovery by renamed
  tags. Reuse guarded deletion for exact tags and report visible assignment
  effects, partial cleanup and Core's omitted Swarm cleanup separately. Verify
  authoritative read-back against silent resolution drops and retain uncertain
  responses without retries. ([#20](https://github.com/enthouan/komodo-mcp/issues/20))

- Add deliberate single-resource Stack/Deployment/Server deletion through the
  shared private preview store, requiring both local grants for apply. Revalidate
  deployed identities, runtime and visible references; separate definition
  removal, fresh known-container observations, cleanup errors and detachment.
  Disclose hidden effects, Server workloads and legacy Builder cleanup limits;
  retain uncertain submissions without retries or exposing archived definitions.
  ([#21](https://github.com/enthouan/komodo-mcp/issues/21))

- Add typed Server creation and partial connection/alerting updates through the
  shared preview/apply contract. Preserve omitted settings and accepted trust,
  require explicit pinned keys for enabled saves, disable automatic pruning/key
  rotation on creation, and gate connection applies with both local grants.
  Report verified configuration separately from cached connectivity and sanitized
  connection failures, without provisioning or key-management fallback.
  ([#19](https://github.com/enthouan/komodo-mcp/issues/19))

- Add typed Deployment creation and partial updates using the shared private
  preview/apply contract, with direct images, versioned Build references and
  existing Server/Swarm targets. Preserve omitted ports, environment references
  and targets; redact privileged/secret fields; require both local grants for
  image updates; distinguish desired configuration from cached/runtime evidence
  and disclose Core registry, auto-deploy and old-target effects. Bind tokens
  to operation/resource kind and redact configured credentials before bounded
  display clipping. ([#18](https://github.com/enthouan/komodo-mcp/issues/18))

- Extend the shared typed Stack preview/apply flow with creation, inline/Git/
  linked-Repo/host source declarations and existing Server/Swarm attachments.
  Preserve omitted source fields, bind raw referenced definitions, separate
  desired configuration from runtime observations, and disclose Core auto-deploy
  and old-target workload effects. Source updates require both operate and
  configuration-edit grants, regardless of the observed auto-update policy.
  Definition saves submit no execution or file
  mutation requests. ([#17](https://github.com/enthouan/komodo-mcp/issues/17))

- Add trusted typed Stack configuration preview/apply with private authoritative
  baselines, explicit omission/clear/no-op behavior, redacted field changes,
  single-use previews and bounded single-submission uncertainty. Preserve
  independent operate/edit grants and verify the same-identity permission
  boundary with the disposable Core/Periphery runner.
  ([#16](https://github.com/enthouan/komodo-mcp/issues/16))

### Permissions and Core effects

Configuration tools opt in through `KOMODO_ALLOW_CONFIGURATION_EDIT=true`,
independently of `KOMODO_ALLOW_OPERATE`. Applying a supplied Stack source,
Deployment image or Server connection setting also requires operate access,
including unchanged inputs. Enabled Server creation and resource/tag deletion
require both grants. Core remains the upstream permission authority.

Definition saves issue no separate execution request, but Core can auto-deploy
after source/image updates or reconnect/poll Periphery after connection edits.
Resource deletion can tear down workloads, detach references or leave runtime
cleanup incomplete. Core 2.3.2 can leave Builder `server_ids` after Server deletion
and omits Swarm assignments from tag-deletion cleanup. Final-read races, hidden
dependencies and uncertain replies remain explicit; no mutation is retried
automatically and record absence never proves global cleanup.

### Evidence and distribution limits

Verification uses production stdio MCP processes, synthetic Core 2.3.2 wire
fixtures and fresh owned disposable Core/Periphery 2.3.2 acceptance recorded in
the [release tracker](https://github.com/enthouan/komodo-mcp/issues/85). Live scope
covers inline Stacks and direct-image Deployments on Server targets, Server
record settings and saved-but-unreachable observations, Stack tag assignments,
running-workload deletion and Server detachment. Git/linked-Repo/host sources,
Build references and Swarm behavior retain deterministic fixture/source
coverage. Successful outbound/inbound/changed-key handshakes are not part of
the live matrix. The pinned `komodo_client =2.3.2` and `rmcp =3.9.0` are unchanged;
archived benchmark measurements retain their original identities.

Release artifacts remain an annotated source tag and GitHub Release notes.
Build the stdio executable from source using the
[installation walkthrough](https://github.com/enthouan/komodo-mcp/blob/main/README.md#fresh-checkout-demo-synthetic-core).

## v0.3.1

Dependency and CI maintenance for workload operations and recovery.

- Update the official MCP SDK from rmcp 3.8.0 to 3.9.0 with the existing stdio
  feature selection, verified through MCP lifecycle, tool-schema and subprocess
  checks.
  ([#67](https://github.com/enthouan/komodo-mcp/pull/67))
- Pin the Rust setup action by commit and explicitly retain compiler 1.99.1 in
  CI and benchmarks, so action updates do not select an unavailable compiler.
  ([#66](https://github.com/enthouan/komodo-mcp/pull/66))

### Evidence and distribution limits

Verification uses production stdio MCP processes with synthetic **Core 2.3.2
wire fixtures** and the pinned `komodo_client =2.3.2`. The existing disposable
Core/Periphery 2.3.2 [live smoke evidence](https://github.com/enthouan/komodo-mcp/issues/74#issuecomment-6844295403)
remains pinned to the v0.3.0 implementation commit `4f8d3f8`. This dependency cut
adds no new live compatibility or performance claim; archived benchmark reports
retain their original source and executable identities.

Release artifacts are an annotated source tag and GitHub Release notes. Build
the stdio executable from source using the
[installation walkthrough](https://github.com/enthouan/komodo-mcp/blob/main/README.md#fresh-checkout-demo-synthetic-core).

## v0.3.0

Operate existing workloads with explicit scope, single-submission receipts and
recoverable observation.

- Verify the complete production stdio inspection/operation/receipt/wait/health
  workflow, independent callers and interrupted recovery with deterministic CI
  scenarios. Add a reproducible operator walkthrough and opt-in disposable
  Core/Periphery 2.3.2 smoke with scoped non-admin identities, dependency/hook
  checks, bounded observers and verified ownership-scoped cleanup.
  ([#74](https://github.com/enthouan/komodo-mcp/issues/74))

- Add read-only interrupted-submission reconciliation with bounded resource
  history/service evidence, explicit unknown acceptance and unconfirmed candidate
  attribution. Verify accepted disconnects, timeouts, malformed/oversized replies,
  process loss and independent callers across deploy/lifecycle/service operations
  without automatically executing again.
  ([#15](https://github.com/enthouan/komodo-mcp/issues/15))

- Add read-only bounded waiting and cross-session resume by Core update ID,
  with cancellation, total/request/polling limits, retained latest evidence and
  explicit incomplete observation. Keep Core execution outcome separate from
  workload health and never resubmit execution while waiting or resuming.
  ([#14](https://github.com/enthouan/komodo-mcp/issues/14))

- Add start, stop and restart for existing Server-backed Stacks and Deployments,
  including exact deployed Compose service scope. Reuse single-submission receipts,
  preserve Core failures and busy outcomes, reject Swarm lifecycle actions, and
  describe action-specific dependencies without substituting unpause or redeploy.
  ([#13](https://github.com/enthouan/komodo-mcp/issues/13))

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

### Operations and compatibility

Operations are opt-in through `KOMODO_ALLOW_OPERATE=true`; default discovery and
dispatch remain read-only. Local grants do not replace Core permissions. An
enabled identity needs the selected workload's Execute permission and the Read,
Inspect and Logs access used for observation. Configuration editing remains
future scope, with an independent local grant.

Server-backed Stacks support deploy/start/stop/restart for the whole Stack or one
exact Compose service; Server-backed Deployments support all four actions for
the whole workload. Swarm supports whole-Stack/Deployment deploy only. Swarm
service filters and lifecycle operations are rejected before execution. Invalid
or unresolved selection never widens to a whole workload.

Deploy applies saved configuration and image policy. Server Deployment deploy
pulls the selected image/build and recreates its container; Stack deploy follows
saved auto-pull, build, Compose flags and wrappers. Lifecycle uses existing
containers and does not apply changed configuration/images or run deploy hooks.
Service deploy can start dependencies and runs saved pre/post hooks; start can
start existing dependencies, restart can propagate to configured dependents,
and stopping a selected service can affect dependent applications. Requested
service scope is not an isolation guarantee.

Each explicit call submits at most one typed Core execution. Acknowledgement
establishes acceptance; later permission, busy or rollout failure stays attached
to that update. Successful execution and workload health are observed separately.
Lost replies and untrustworthy acknowledgements retain unknown acceptance.
Known update IDs resume through bounded read-only observation from a new session;
local wait timeout/cancellation does not cancel the remote operation. Missing-ID
history searches return unconfirmed candidates, including when only one matches.
Clock differences, delayed visibility and permissions can hide an accepted
request. Recovery never automatically resubmits; independent explicit calls can
repeat effects, with no upstream idempotency or exactly-once guarantee.

See the [operator walkthrough](https://github.com/enthouan/komodo-mcp/blob/4f8d3f8860bb0adffb368753e8b0db7a87f5a373/docs/operations-walkthrough.md)
for configuration, supported targets, observation bounds and recovery steps.

### Evidence and distribution limits

The supported API/fixture baseline is **Core 2.3.2**. The integrated implementation
was verified through compiled production stdio and a disposable **Core/Periphery
2.3.2** Server-backed smoke using scoped non-admin identities, separate
provisioning credentials and verified cleanup. Live evidence used Compose
**5.4.0** and isolated Docker **27.5.1**; Swarm contracts are fixture-tested, with
no live multi-version/Swarm certification claim. The [implementation acceptance
and live evidence](https://github.com/enthouan/komodo-mcp/issues/74#issuecomment-6844295403)
retain the exact tested commit, executable identity, measured recovery traffic
and final independent health observations. Application, MCP protocol and
Core/SDK versions are independent identities.

Archived benchmark reports retain their actual pre-release source, executable
and application-version identities. This version/changelog cut does not claim
new performance measurements or a general performance ranking.

Release artifacts remain an annotated source tag and GitHub Release notes.
Build the stdio executable from source using the
[installation walkthrough](https://github.com/enthouan/komodo-mcp/blob/main/README.md#fresh-checkout-demo-synthetic-core).
No container images or native binary packages are distributed in this release.

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
