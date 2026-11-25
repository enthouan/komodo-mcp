# Run Komodo MCP with Docker Compose

Use the [v0.9.0 Compose file](https://raw.githubusercontent.com/enthouan/komodo-mcp/v0.9.0/compose.yaml)
on Linux amd64 or arm64. Set `KOMODO_MCP_IMAGE` in your Compose environment
to pin the verified multi-platform image.

```text
KOMODO_MCP_IMAGE=ghcr.io/enthouan/komodo-mcp:v0.9.0@sha256:0dac5c0d80dd463c8d4eed87a8538606f2059988eb90862f8404177016ab3b21
```

To rehearse an unpublished candidate, load the verified
`candidate.oci.tar` from a Container distribution run and set
`KOMODO_MCP_IMAGE=komodo-mcp-candidate:git-<full revision>`. The candidate
reports the Cargo version at that revision.

Supply these settings in the process environment or a private env file:

| Setting | Value |
| --- | --- |
| `KOMODO_ADDRESS` | HTTPS address of your Core, reachable from the container. |
| `KOMODO_API_KEY`, `KOMODO_API_SECRET` | A dedicated Core credential with Read on the resources you intend to inspect. |
| `KOMODO_HTTP_TOKENS` | An independently generated bearer for each MCP client, comma-separated. Generate each with `openssl rand -hex 32`. |
| `KOMODO_MCP_IMAGE` | An exact image reference, or the reviewed local candidate reference during rehearsal. |

Keep the env file readable only by its owner. The Compose service forwards
values at runtime. Anyone with Docker administrator access can inspect them.
Use `KOMODO_ENABLED_TOOLS=list_stacks,get_stack,stack_diagnose` for a three-tool
triage client. Configure `KOMODO_HTTP_ALLOWED_ORIGINS` with exact origins when
using a browser client. See [HTTP transport](../README.md#authenticated-streamable-http) for authentication and
client configuration.

With the settings exported, run the following in the directory containing
`compose.yaml`. For a private env file, add `--env-file /path/to/file`
immediately after `docker compose` on each command.

```sh
docker compose config --quiet
docker compose up -d --wait komodo-mcp
docker compose ps komodo-mcp
```

The default endpoint is `http://127.0.0.1:3000/mcp`. Set `KOMODO_HTTP_PORT`
to change the host port. The container listens on port 3000 inside its Docker
network. The built-in `--healthcheck` probes the local listener with a
two-second deadline and expects its Bearer challenge. Initialize your MCP
client with its bearer, check the server version, list the tools and call
`list_stacks` to verify Core access. Shut the service down with
`docker compose down`.

The container runs as UID and GID 65532, with a read-only root filesystem and
all Linux capabilities dropped by Compose. For a private Core CA, mount a
combined PEM bundle containing the trusted roots at
`/etc/ssl/certs/ca-certificates.crt` read-only. The Core URL hostname must match
its TLS certificate.

## Verify an image

The Container distribution workflow waits for the successful standalone Native
distribution at the same revision. It verifies the immutable GitHub artifact
ID and SHA-256 and uses its exact two Linux executables. On a manual run from
main, select the exact native run ID and attempt that the release will use. Both native architectures exercise the same
OCI archive through Compose. The `komodo-mcp-oci-<revision>` Actions artifact
contains the archive and `oci-receipt.json`. The receipt records the repository
revision, Cargo version, workflow run and attempt, versioned image reference,
index digest, platform manifest and config digests, executable SHA-256, and
the originating native workflow run, attempt, artifact ID and artifact digest.
The outer archive checksum verifies the file downloaded from Actions. The
index digest identifies the multi-platform image in GHCR.

Every image contains its matching `LICENSE`, `THIRD-PARTY-NOTICES`, CA attribution
and `SOURCE.txt` under `/usr/share/doc/komodo-mcp/`. The OCI labels and
`SOURCE.txt` point to the versioned Corresponding Source archive on GitHub
Releases. The executable and these files can be copied out with `docker cp` from
a container created with `docker create`.

The release helper verifies the public source bundle before promotion. It
uploads shared blobs and platform manifests by digest, checks the version tag
again, publishes the index and reads back its digest and platform identities.
Use one authorized publisher per version. GHCR's standard manifest PUT has no
atomic create-if-absent guarantee across independent publishers. A private
package or a registry access failure requires a successful anonymous
verification before the release is complete.
