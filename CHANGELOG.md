# Changelog

New changes go under `Unreleased`. Release sections provide the GitHub Release
notes: short, categorized bullets with links to the relevant pull requests.

## Unreleased

### Added

- Configure named Alerters and explicitly test one saved endpoint with safe delivery reporting. ([#118](https://github.com/enthouan/komodo-mcp/pull/118))
- Manage ordinary variables and edit secret metadata without returning secret values. ([#117](https://github.com/enthouan/komodo-mcp/pull/117))
- Preview, save, deploy and observe managed Swarm Deployment replicas, including zero. ([#116](https://github.com/enthouan/komodo-mcp/pull/116))
- Inspect Docker images, networks and volumes, including safe image history. ([#115](https://github.com/enthouan/komodo-mcp/pull/115))
- Inspect Swarm nodes, services and tasks, diagnose placement failures, and read exact-service logs. ([#114](https://github.com/enthouan/komodo-mcp/pull/114))

## v0.6.0

### Added

- Export selected resources and groups as sanitized, inspection-only TOML. ([#105](https://github.com/enthouan/komodo-mcp/pull/105))
- Create and edit ResourceSync definitions using inline, Git, linked Repo or Core-host sources. ([#106](https://github.com/enthouan/komodo-mcp/pull/106))
- Inspect, refresh and review scoped ResourceSync pending changes before applying them. ([#107](https://github.com/enthouan/komodo-mcp/pull/107))
- Apply reviewed ResourceSync changes and recover interrupted observations without replaying execution. ([#108](https://github.com/enthouan/komodo-mcp/pull/108))
- Add a complete ResourceSync walkthrough and integration checks across all four sources. ([#109](https://github.com/enthouan/komodo-mcp/pull/109))

## v0.5.0

### Added

- Inspect and configure Repos, with explicit clone and pull operations. ([#95](https://github.com/enthouan/komodo-mcp/pull/95))
- Configure Server-backed Builders and Builds, then run and cancel saved Builds. ([#96](https://github.com/enthouan/komodo-mcp/pull/96), [#97](https://github.com/enthouan/komodo-mcp/pull/97))
- Manage, run and cancel trusted Procedures and Actions. ([#98](https://github.com/enthouan/komodo-mcp/pull/98), [#99](https://github.com/enthouan/komodo-mcp/pull/99))
- Inspect and update Action and Procedure schedules, including Cron, English expressions and timezones. ([#100](https://github.com/enthouan/komodo-mcp/pull/100))
- Add an automation walkthrough and integration checks from Repo cloning through deployment health. ([#101](https://github.com/enthouan/komodo-mcp/pull/101))

## v0.4.0

### Added

- Preview and apply Stack configuration changes, including creation, sources and targets. ([#86](https://github.com/enthouan/komodo-mcp/pull/86), [#87](https://github.com/enthouan/komodo-mcp/pull/87))
- Create and edit Deployments with direct images or Build references. ([#88](https://github.com/enthouan/komodo-mcp/pull/88))
- Create and edit Server connection and alert settings. ([#89](https://github.com/enthouan/komodo-mcp/pull/89))
- Delete individual Stacks, Deployments and Servers with explicit previews and cleanup reporting. ([#90](https://github.com/enthouan/komodo-mcp/pull/90))
- Create, rename, assign and delete resource tags. ([#91](https://github.com/enthouan/komodo-mcp/pull/91))

## v0.3.1

### Changed

- Update the MCP SDK from rmcp 3.8.0 to 3.9.0. ([#67](https://github.com/enthouan/komodo-mcp/pull/67))
- Pin the Rust setup action while retaining Rust 1.99.1 in CI and benchmarks. ([#66](https://github.com/enthouan/komodo-mcp/pull/66))

## v0.3.0

### Added

- Add opt-in workload operations and an independent configuration-edit capability grant, with read-only access by default. ([#76](https://github.com/enthouan/komodo-mcp/pull/76))
- Deploy individual Stacks, Deployments or named Compose services. ([#77](https://github.com/enthouan/komodo-mcp/pull/77))
- Start, stop and restart Server-backed workloads and Compose services. ([#78](https://github.com/enthouan/komodo-mcp/pull/78))
- Wait for updates, resume observation across sessions and investigate interrupted submissions without replaying them. ([#79](https://github.com/enthouan/komodo-mcp/pull/79), [#80](https://github.com/enthouan/komodo-mcp/pull/80))
- Add an operations walkthrough and integration checks for execution, recovery and workload health. ([#81](https://github.com/enthouan/komodo-mcp/pull/81))

## v0.2.0

### Added

- Inspect fleet health, including CPU, memory, disk and service observations. ([#68](https://github.com/enthouan/komodo-mcp/pull/68))
- Read bounded, redacted workload logs and alert/update history. ([#69](https://github.com/enthouan/komodo-mcp/pull/69), [#70](https://github.com/enthouan/komodo-mcp/pull/70))
- Diagnose a Stack with linked health, log and incident evidence. ([#71](https://github.com/enthouan/komodo-mcp/pull/71))
- Add incident-triage integration checks and concurrent-fleet benchmarks. ([#72](https://github.com/enthouan/komodo-mcp/pull/72))

### Changed

- Configure weekly dependency updates and document main-branch protection and release conventions. ([#61](https://github.com/enthouan/komodo-mcp/pull/61), [#65](https://github.com/enthouan/komodo-mcp/pull/65))

## v0.1.0

### Added

- Introduce the Rust stdio MCP server and official Komodo client integration. ([#53](https://github.com/enthouan/komodo-mcp/pull/53), [#54](https://github.com/enthouan/komodo-mcp/pull/54))
- List and inspect Servers, Stacks and Deployments with six read-only tools. ([#55](https://github.com/enthouan/komodo-mcp/pull/55))
- Add installation, mock Core and MCP Inspector examples. ([#56](https://github.com/enthouan/komodo-mcp/pull/56))
- Add reproducible startup and inventory benchmarks against the TypeScript reference. ([#57](https://github.com/enthouan/komodo-mcp/pull/57))
- Add release tooling with annotated tags, recovery checks and executable version verification. ([#59](https://github.com/enthouan/komodo-mcp/pull/59))
