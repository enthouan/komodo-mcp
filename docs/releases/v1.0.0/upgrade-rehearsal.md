# Published v0.9.0 upgrade and rollback rehearsal

On November 26, 2026, both an installed native stdio server and a Compose server running authenticated Streamable HTTP moved from the public v0.9.0 package to the packaged main candidate and back. All eight MCP sessions reported `0.9.0`. The candidate predates the v1.0.0 version cut. Public v1.0.0 installs, the four operator workflows, and rollback from the published stable packages remain open on [#52](https://github.com/enthouan/komodo-mcp/issues/52).

## Packages exercised

The starting installation came from the public release downloads and an anonymous pull of the GHCR digest with a fresh Docker configuration. The candidate came from the successful native and container distribution jobs on the same main commit. Each Actions ZIP matched the artifact digest, and the container receipt identified the selected native artifact. [Published baseline receipt](https://github.com/enthouan/komodo-mcp/issues/145#issuecomment-7176096008) · [Native candidate run](https://github.com/enthouan/komodo-mcp/actions/runs/45716513518) · [Container candidate run](https://github.com/enthouan/komodo-mcp/actions/runs/45716513615)

| Identity | Published v0.9.0 | Pre-cut candidate |
| --- | --- | --- |
| Source commit | `ebf42362d508767b6c3fcf1740172cf82194f257` | `465c776dc8cb437398a22c938645d555e65fe560` |
| Archive SHA-256, `x86_64-unknown-linux-musl` | `e6fad7c8e2ce8140eb83543c20d515ad6d4c5595ce59ba07989d697d3b35cc76` | `25b2991f26fcf6238f1474ea374e83f929e0ea19eddef5c875550e7a8689002c` |
| Installed executable SHA-256 | `a89347f000371e7fa5a33e4b68e832b098c635a78a71ce9d895786356da00aa2` | Same bytes |
| OCI index digest (SHA-256) | `0dac5c0d80dd463c8d4eed87a8538606f2059988eb90862f8404177016ab3b21` | `0a914a9f6c69eccc032d6eba6328d5da2de5acca43e58b8892646ecb951df480` |
| Linux amd64 manifest digest (SHA-256) | `892b33e8c46ff5f32247b1f86dba5a630a4919d09888d356a8e7c95c4a577c56` | `b4a7ddd3158f9cbaaf0377f53bc9bf89a5e834b6c4b42e96bafbebaa0be054a9` |

## Setup and changes

Each transport had its own disposable Core 2.3.2 and database. Each MCP server used a non-admin service account with Read permission on one Server and the `rehearsal-staging` Stack. A second Stack existed outside its grants. The provisioner checked the service identity and its one-Stack inventory directly against Core. The server kept `KOMODO_ENABLED_TOOLS=get_stack,list_stacks,stack_diagnose` and the default disabled local grants. The HTTP client used a separate random inbound bearer. This inventory rehearsal used saved Stack definitions on a disabled Server with no Periphery.

**Native stdio.** The published archive matched its exact `SHA256SUMS` entry. An unprivileged Linux amd64 host received the two archives in separate directories and ran `sha256sum "$archive"`, `tar -xzf "$archive" -C extracted`, and `sha256sum "$executable"` for each. The client saved its absolute command and private environment file. It launched the published executable, closed the process, switched its command to the candidate, launched a second candidate session, then restored the saved command and environment and relaunched the public executable. Each launch checked the selected path's SHA-256. The first setup attempt stopped before extraction because Docker's copy API could not address the host's tmpfs. Streaming the archive into that directory and verifying it on the host completed the installation.

**Compose HTTP.** The installation used the tagged v0.9.0 Compose file and pinned the public index with `KOMODO_MCP_IMAGE`. The private environment file held the scoped Core settings and the independent bearer. A second file held only the image pin. The fixture added a project-owned bridge to reach Core and selected a free loopback port. After saving the Compose file and both environment files, the operator changed only the image pin to the locally imported `komodo-mcp-candidate:git-465c776dc8cb437398a22c938645d555e65fe560`, recreated the service twice, then restored the three saved files and recreated it again. [Published Compose file](https://raw.githubusercontent.com/enthouan/komodo-mcp/v0.9.0/compose.yaml)

The Compose invocation included the disposable project name, both files, the tagged Compose definition and its fixture network override. These sanitized commands show the checks from the private installation directory.

```sh
compose() {
  docker --context desktop-linux compose -p "$project" --env-file runtime.env --env-file pin.env \
    -f compose.yaml -f operator.json "$@"
}
compose config --quiet
compose up -d --force-recreate --wait --wait-timeout 80 --pull never
cid=$(compose ps -q komodo-mcp)
docker --context desktop-linux inspect --format '{{.Config.Image}} {{.Image}} {{.ImageManifestDescriptor.Digest}} {{.State.Health.Status}}' "$cid"
```

## Observed results

| Fresh MCP session on each transport | Native command | Compose running image | Discovery and bounded read |
| --- | --- | --- | --- |
| Baseline | Published path | Public index, healthy | Passed |
| Upgrade | Candidate path | Candidate index, healthy | Passed |
| Restart | Candidate path | New container at candidate index, healthy | Passed |
| Rollback | Restored published path | New container at public index, healthy | Passed |

Each session initialized with `komodo-mcp-disposable-candidate` 1 (Python 3.13.14, MCP `2025-11-25`). Sorted `tools/list` returned exactly `get_stack`, `list_stacks`, and `stack_diagnose`. The excluded read `list_deployments` returned JSON-RPC `-32602`. A `tools/call` for `list_stacks` with `{"page":0,"page_size":1}` returned the expected Stack name and the same ID as the direct scoped Core read. All four HTTP sessions rejected missing and incorrect bearers with HTTP 401 before accepting the configured token. The candidate and rollback preserved the original tool selection and private environment.

Docker Desktop Engine 29.9.3 served Linux amd64 on Intel macOS 15.8.3 with Compose 5.7.1. The successful runs and the stopped native setup attempt each finished with zero project containers, networks and volumes. The Compose service project was also removed. The private database and temporary credential files were discarded.
