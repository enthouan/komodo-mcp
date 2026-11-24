# Self-hosted beta installation

Use these instructions with the matching native archive or Compose checkout
and image. The project [releases](https://github.com/enthouan/komodo-mcp/releases)
list the artifacts available for each published tag. Pick an enabled Core
account scoped to the resources this instance should see.

## Native stdio

Choose the archive for the machine that launches the MCP client. Both macOS
archives require macOS 15 or later. See [native binaries and matching source](distribution.md)
for platform requirements and the source rebuild instructions.

| System | CPU | Rust target in the archive name |
| --- | --- | --- |
| macOS | Apple silicon | `aarch64-apple-darwin` |
| macOS | Intel | `x86_64-apple-darwin` |
| Linux | arm64 | `aarch64-unknown-linux-musl` |
| Linux | amd64 | `x86_64-unknown-linux-musl` |

Place the archive and its release's `SHA256SUMS` in a new directory. Run these
commands there, replacing the sample name with the exact local filename.

```sh
set -eu
archive=komodo-mcp-vX.Y.Z-aarch64-apple-darwin.tar.gz
awk -v name="$archive" '$2 == name { print; n++ } END { if (n != 1) exit 1 }' SHA256SUMS > "$archive.sha256"
if command -v sha256sum >/dev/null; then
  sha256sum -c "$archive.sha256"
else
  shasum -a 256 -c "$archive.sha256"
fi
mkdir extracted
tar -xzf "$archive" -C extracted
printf '%s\n' "$PWD/extracted/${archive%.tar.gz}/komodo-mcp"
```

Configure the MCP client's **stdio** server command with the absolute path
printed by the last command. Each archive includes the license and third-party
notices. The same release provides its matching Corresponding Source bundle.
Set the environment of that launch using these entries.

```text
KOMODO_ADDRESS=https://komodo.example.test
KOMODO_API_KEY=<from the scoped Core account>
KOMODO_API_SECRET=<from the scoped Core account>
KOMODO_ENABLED_TOOLS=list_stacks,get_stack,stack_diagnose
```

Supply the key and secret through the client's private configuration. Leave
`KOMODO_TRANSPORT` unset for stdio. Launch it from the client and follow
[discovery and the first read](operator-quickstart.md#discover-and-diagnose-with-three-tools).
Use the same client configuration with a previously verified executable to roll
back. Reload the server in the client whenever you change the path or settings.

## Docker Compose

See [Docker Compose installation](container.md) for private CAs, local candidate images
and the container health check. Use the `compose.yaml` from the matching source revision in an operator-owned
directory. Keep its `.env` private with file mode `0600`. These are the entries
for a three-tool server. Replace the values in angle brackets in your secret
store or private file.

```text
KOMODO_ADDRESS=https://komodo.example.test
KOMODO_API_KEY=<from the scoped Core account>
KOMODO_API_SECRET=<from the scoped Core account>
KOMODO_HTTP_TOKENS=<random inbound token for this client>
KOMODO_ENABLED_TOOLS=list_stacks,get_stack,stack_diagnose
```

Use a Core address reachable **from the container**. Give each client its own
cryptographically random token (at least 32 random bytes) and retain the
matching value in its secret store. For a verified digest pin, set
`KOMODO_MCP_IMAGE` to the exact image reference from the release evidence,
including its `@sha256:` digest. The Compose file selects the matching image
tag by default. Use `KOMODO_HTTP_PORT` to choose a different host port.

```sh
docker compose --env-file .env config --quiet
docker compose --env-file .env up -d
docker compose ps komodo-mcp
```

Wait for the service to report healthy. Connect a Streamable HTTP MCP client to
`http://127.0.0.1:3000/mcp` (or your chosen host port) and configure the header
`Authorization: Bearer <your-inbound-token>` on every request. The Compose
listener is published on host loopback. For access across a network, configure
a trusted HTTPS reverse proxy. Give browser-based clients an exact allowed
Origin through `KOMODO_HTTP_ALLOWED_ORIGINS`. Keep the incoming bearer token
and the Core key pair in their respective client and server secret stores.

Verify [discovery and the first read](operator-quickstart.md#discover-and-diagnose-with-three-tools).
For a separately authorized deployment, use the [staging configuration](operator-quickstart.md#staging-deployment).
To stop this Compose service and remove its containers and network, run
`docker compose --env-file .env down` in the same directory.
