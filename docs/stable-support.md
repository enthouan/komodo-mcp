# Stable support policy

Choose the row that matches the machine running Komodo MCP, its connection to the MCP client, and the versions of Core and Periphery you run. The supported upstream target is **Core 2.3.2 with Periphery 2.3.2**. A different Core version may answer the startup probe. That probe checks connectivity and reports the version. Individual tools can still fail against a different API.

## Platforms, transports and clients

| Package and transport | Client and protocol | Evidence for this combination |
| --- | --- | --- |
| Linux x86-64 musl executable, stdio | `komodo-mcp-disposable-candidate` 1, MCP 2025-11-25 | Live Core and Periphery 2.3.2 on Ubuntu x86-64. Discovery, a scoped denial, container reads, configuration preview/apply and interrupted deployment recovery passed. |
| Linux amd64 image, Compose and authenticated Streamable HTTP | Same client and protocol | The same live scenarios passed. The listener also rejected missing and incorrect bearers. |
| Intel macOS 15.8.3 executable, stdio | Inventory benchmark client 1, MCP 2025-03-26 | Read seven known Stacks from live Core 2.3.2. The benchmark called tools by name. A separate rmcp fixture run checked discovery. |
| Linux x86-64 and arm64 musl on Ubuntu 24.04, Intel and Apple silicon on macOS 15, stdio | stacks-on-server 1 with rmcp 3.11.0, MCP 2025-11-25 | All four extracted archives initialized, discovered read tools and read paginated inventory from a 2.3.2 fixture. The macOS packages require macOS 15 or later. |
| Linux amd64 and arm64 images, Compose and authenticated Streamable HTTP | container-fixture 1, MCP 2025-11-25 | Both platforms initialized with a bearer, listed the selected tools and called `list_stacks` against a fixture on native Ubuntu 24.04 runners. |

The v0.9.0 compatibility study used packages built from `e8432d39` that reported `0.8.0`. The published v0.9.0 rebuilt and probed all six package targets and retained the same server source and contract. The Linux arm64 and Apple silicon rows have fixture coverage only. Windows has no native release package. Each test applies to its named client, transport and workflow. [Full matrix and run links](v0-9-0-compatibility.md) · [Published v0.9.0 evidence](https://github.com/enthouan/komodo-mcp/issues/145#issuecomment-7176096008)

MCP Inspector CLI 2.14.0 initialized, discovered and performed stdio fixture reads. Claude Code 2.1.228 parsed the stdio configuration. Codex CLI 0.151.0, Claude Desktop, VS Code and OpenCode have reviewed stdio setup recipes. The latter three have no recorded client version. These recipes have no recorded live tool call. Desktop client support over HTTP remains unverified. [Client setup evidence](v0-9-0-compatibility.md#earlier-releases-and-client-setup-evidence)

## What clients can rely on

The v1 compatibility guarantee covers existing tool names, accepted inputs, documented result fields and their meanings, capability requirements, and the common application behavior on stdio and authenticated HTTP. Compatible 1.x additions can introduce tools and optional fields. Discover the enabled tools at connection time and tolerate additional fields in results. A change that breaks an existing documented call requires a new major version and migration notes.

The frozen contract records schemas, annotations, discovery under 16 local grant combinations, direct-call restrictions and representative results, errors and bounds. CI runs the production executable over both transports and checks changes to that snapshot. Tool discovery reflects the local grants and tool selection. Core separately controls the resources and actions available to its account. All bearer tokens on an HTTP instance use that same Core identity. [Contract and change procedure](contracts/README.md) · [Machine-readable contract](contracts/v1.json)

Consult the coverage tables before depending on a particular upstream action. Each entry names the supported scope or the missing behavior, the relevant permission and the evidence. Timing measurements apply to the recorded workload and host. [Core API families](v0-9-0-api-coverage.md) · [Official km commands](v0-9-0-cli-coverage.md) · [Coverage issue #47](https://github.com/enthouan/komodo-mcp/issues/47)

## Getting help

For an installation change, follow the [v0.9.0 upgrade and rollback guide](upgrade-v1.md). For a compatibility or support issue, include the artifact identity, initialized server version, platform, Core/Periphery and client versions, transport and a sanitized failing call. Send security-sensitive findings privately. [Report a support issue](https://komodo-mcp.com/reference/reporting-issues/) · [Private vulnerability report](https://github.com/enthouan/komodo-mcp/security/advisories/new)
