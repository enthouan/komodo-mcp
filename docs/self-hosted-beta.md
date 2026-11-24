# Self-hosted beta installation

These instructions use the [v0.8.0 release](https://github.com/enthouan/komodo-mcp/releases/tag/v0.8.0). Pick an enabled Core
account scoped to the resources this instance should see.

## Native stdio

Choose the archive for the machine that launches the MCP client. Both macOS
archives require macOS 15 or later. See [native binaries and matching source](distribution.md)
for platform requirements and the source rebuild instructions.

| System | CPU | v0.8.0 archive |
| --- | --- | --- |
| macOS | Apple silicon | [komodo-mcp-v0.8.0-aarch64-apple-darwin.tar.gz](https://github.com/enthouan/komodo-mcp/releases/download/v0.8.0/komodo-mcp-v0.8.0-aarch64-apple-darwin.tar.gz) |
| macOS | Intel | [komodo-mcp-v0.8.0-x86_64-apple-darwin.tar.gz](https://github.com/enthouan/komodo-mcp/releases/download/v0.8.0/komodo-mcp-v0.8.0-x86_64-apple-darwin.tar.gz) |
| Linux | arm64 | [komodo-mcp-v0.8.0-aarch64-unknown-linux-musl.tar.gz](https://github.com/enthouan/komodo-mcp/releases/download/v0.8.0/komodo-mcp-v0.8.0-aarch64-unknown-linux-musl.tar.gz) |
| Linux | amd64 | [komodo-mcp-v0.8.0-x86_64-unknown-linux-musl.tar.gz](https://github.com/enthouan/komodo-mcp/releases/download/v0.8.0/komodo-mcp-v0.8.0-x86_64-unknown-linux-musl.tar.gz) |

Download the archive and [SHA256SUMS](https://github.com/enthouan/komodo-mcp/releases/download/v0.8.0/SHA256SUMS) into a new directory on the machine that will run the MCP server. The following commands select the matching archive, verify it and print the executable path.

```sh
(
set -eu
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Darwin-x86_64) target=x86_64-apple-darwin ;;
  Linux-aarch64|Linux-arm64) target=aarch64-unknown-linux-musl ;;
  Linux-x86_64) target=x86_64-unknown-linux-musl ;;
  *) echo "Choose a supported platform from the table." >&2; exit 1 ;;
esac
archive="komodo-mcp-v0.8.0-$target.tar.gz"
awk -v name="$archive" '$2 == name { print; n++ } END { if (n != 1) exit 1 }' SHA256SUMS > "$archive.sha256"
if command -v sha256sum >/dev/null; then
  sha256sum -c "$archive.sha256"
else
  shasum -a 256 -c "$archive.sha256"
fi
mkdir extracted
tar -xzf "$archive" -C extracted
executable="$PWD/extracted/${archive%.tar.gz}/komodo-mcp"
test -f "$executable"
test -x "$executable"
printf '%s\n' "$executable"
)
```

Configure the MCP client's **stdio** server command with the absolute path
printed by the last command. Each archive includes the license and third-party
notices. The [matching Corresponding Source](https://github.com/enthouan/komodo-mcp/releases/download/v0.8.0/komodo-mcp-v0.8.0-source.tar.gz), [license](https://github.com/enthouan/komodo-mcp/releases/download/v0.8.0/LICENSE), and [third-party notices](https://github.com/enthouan/komodo-mcp/releases/download/v0.8.0/THIRD-PARTY-NOTICES) accompany the native downloads and container image.
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

Download the [versioned Compose file](https://raw.githubusercontent.com/enthouan/komodo-mcp/v0.8.0/compose.yaml) into a new, operator-owned directory. It uses the [published container image](https://github.com/users/enthouan/packages/container/package/komodo-mcp).

```sh
curl --fail --show-error --location --output compose.yaml https://raw.githubusercontent.com/enthouan/komodo-mcp/v0.8.0/compose.yaml
(umask 077 && touch .env) && chmod 600 .env
```

Enter the following settings in the private `.env` file. Replace the
angle-bracketed values with credentials from your secret store. Use a shell
with no exported `KOMODO_*` settings from another environment. Compose gives
exported values precedence over entries in `.env`.

See [Docker Compose installation](container.md) for private CAs and the container health check.

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
tag `ghcr.io/enthouan/komodo-mcp:v0.8.0` by default. Use `KOMODO_HTTP_PORT` to choose a different host port.

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
For a separately authorized deployment, create a second Compose project in
another directory, with its own staging Core credentials and HTTP token. Give
the staging project an unused `COMPOSE_PROJECT_NAME` in its private `.env` and
leave that variable unset in the calling shell. Set `KOMODO_HTTP_PORT` to a free host port and configure a separate MCP client
entry for that port. Use the [staging configuration](operator-quickstart.md#staging-deployment)
in that project and keep the first project restricted to its three tools.
To stop this Compose service and remove its containers and network, run
`docker compose --env-file .env down` in the same directory.
