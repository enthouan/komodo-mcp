# Komodo MCP

Komodo MCP connects an MCP client to your Komodo Core instance. Use its read-only
inventory tools to list stacks visible to a dedicated Core account. For a containerized
client connection, follow the [authenticated Compose guide](https://komodo-mcp.com/docs/container/).

This is the maintainer's mirror of the multi-platform GHCR release for Linux amd64
and arm64. Exact version tags are immutable. The latest tag tracks the newest eligible
stable release. Pin the index digest when deploying.

```sh
docker pull docker.io/enthouan/komodo-mcp:1.0.0
docker pull docker.io/enthouan/komodo-mcp@VERIFIED_INDEX_DIGEST
```

Set `KOMODO_MCP_IMAGE` to the digest-pinned reference in Compose. The image is licensed
under GPL-3.0-or-later. License and third-party notices are included under
`/usr/share/doc/komodo-mcp/`. The release includes checksums, SBOMs, provenance
and buildable Corresponding Source. [License](https://github.com/enthouan/komodo-mcp/releases/download/v1.0.0/LICENSE) · [Third-party notices](https://github.com/enthouan/komodo-mcp/releases/download/v1.0.0/THIRD-PARTY-NOTICES) · [Corresponding Source](https://github.com/enthouan/komodo-mcp/releases/download/v1.0.0/komodo-mcp-v1.0.0-source.tar.gz) · [Release and verification records](https://github.com/enthouan/komodo-mcp/releases/tag/v1.0.0).
