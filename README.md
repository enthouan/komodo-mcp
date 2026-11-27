# Komodo MCP

Komodo MCP is a self-hosted Model Context Protocol server that connects your MCP client to a Komodo Core instance you can access. Ask “Which of my Stacks failed, and what did its last update report?” The server uses your scoped Core account to list Stacks and inspect the selected Stack’s status, updates and logs. It starts read-only.

**[Get started with native stdio or Docker Compose](https://komodo-mcp.com/getting-started/)**. Both paths take you from a verified package to a successful `list_stacks` call. Use native stdio when your client launches a local process. Use Compose for a persistent HTTP endpoint with a separate client bearer.

## Operator guides

- [Tool catalog](https://komodo-mcp.com/reference/tools/) for tool inputs, outputs, permissions and effects.
- [First read and Stack diagnosis](docs/operator-quickstart.md) for an example using your own Stack IDs.
- [Stable support policy](docs/stable-support.md) for tested Core versions, platforms, clients and transports.
- [Upgrade and rollback](docs/upgrade-v1.md) for switching packages while preserving your configuration.
- [Troubleshooting](https://komodo-mcp.com/guides/troubleshooting/) for connection, authentication and permission errors.
- [Security and data flow](https://komodo-mcp.com/guides/security/), [support](https://komodo-mcp.com/reference/reporting-issues/) and [private vulnerability reports](SECURITY.md).

## Development

See the [source build and fixture demo](docs/reference/building.md), [runtime configuration](docs/reference/runtime.md), [tool and error reference](docs/reference/tools.md), [benchmark results](benchmarks/results/README.md), [development checks](docs/reference/development.md) and [repository maintenance](docs/maintenance.md).

## License

Komodo MCP is licensed under [GPL-3.0-or-later](LICENSE). Binary releases include the license, third-party notices and version-matched Corresponding Source. The [installation guide](https://komodo-mcp.com/getting-started/) links to the published files.

The [Starlight patch notice](patches/starlight-LICENSE.txt) covers the website search component patch. The generated client icons carry their Font Awesome Free attribution in `client-icons.css`.
