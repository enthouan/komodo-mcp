# Docker Hub release mirror

The manual release publisher builds the GitHub Release and promotes the tested image to GHCR.
The Docker Hub workflow starts from its independently verified index digest. A mirror failure
can be recovered on its own. The first production copy is for v1.0.0.

| Tag or pin | Meaning |
| --- | --- |
| `ghcr.io/enthouan/komodo-mcp:v1.0.0` | Primary release image verified in #51 |
| `docker.io/enthouan/komodo-mcp:1.0.0` | Immutable copy of the same index |
| `docker.io/enthouan/komodo-mcp@sha256:…` | The verified index, suitable for Compose |
| `docker.io/enthouan/komodo-mcp:latest` | Most recent eligible published stable version |

## Enable the writer

1. Sign in to Docker Hub as the owner of `enthouan`. Create or confirm the public `komodo-mcp` repository and check the namespace and repository name in its settings. Confirm the public [repository metadata](https://hub.docker.com/v2/namespaces/enthouan/repositories/komodo-mcp) independently while signed out. An anonymous 404 requires owner inspection. To enforce the version-pin policy in Hub, select **Specific tags are immutable** and match version tags with `^[0-9]+[.][0-9]+[.][0-9]+([-_].*)?$`. This leaves `latest` mutable. [Docker Hub tag settings](https://docs.docker.com/docker-hub/repos/manage/hub-images/immutable-tags/).
2. Create a dedicated mirror token with repository Read and Write. Exclude Delete and unrelated access. Use repository-scoped access where available. For a personal namespace, a dedicated collaborator granted access only to this repository limits the reach of its personal access token. Enter the token privately in the `DOCKERHUB_TOKEN` secret of the `dockerhub` GitHub environment. Set the environment variable `DOCKERHUB_USERNAME` to the collaborator login when using one. It defaults to the namespace owner, `enthouan`; a personal token inherits its account's repository access within its selected permissions. [Docker token settings](https://docs.docker.com/security/access-tokens/personal-access-tokens/).
3. In the upstream repository's [environment settings](https://github.com/enthouan/komodo-mcp/settings/environments), check that `dockerhub` restricts deployments to protected branches, requires the maintainer's review and disables admin bypass. The dispatch accepts only `main`. Reserve changes to the version and latest tags for this serialized workflow. Registry tag writes lack a cross-client compare-and-swap.

## Preview or recover

Run the publisher's receipt-based verification first. Record its release tag, full commit,
verified GHCR index digest and two platform digests on the release issue. Set `version` and
`revision` from that record. Keep the original publication receipt for every recovery.

```sh
python3 scripts/release.py mirror --version "$version" --commit "$revision" \
  --receipt target/release-publication.json
```

Inspect the preview log. It identifies the annotated tag object on main, release, current
Hub digests, platform and referrer digests and proposed writes. It fetches the published
SBOM and signed provenance and checks them against the GHCR graph. A reader streams the
configurations and layers and verifies their sizes and digests with anonymous registry access.
The [pinned registry client](https://github.com/regclient/regclient/releases/tag/v0.11.7)
discovers additional OCI referrers. GitHub access for release metadata and proof verification
uses the workflow's read-only token.

After release authorization, request the same preview followed by the protected writer
by adding `--authorized` to the same command. Review the preview in that run before approving its deployment.
The writer rechecks the original annotated tag, release eligibility and source evidence,
then recursively copies by digest with `--referrers`. It verifies the immutable tag and
the same signed proof against Hub's anonymously read graph. It advances `latest` only
for the current highest eligible stable release, after reading back the immutable pin.
Both registries carry the original build's bytes.

| Observed result | Recovery |
| --- | --- |
| Version and latest already match policy | Record the anonymous verification result. A complete matching retry performs zero registry writes. |
| Version absent or matching version has confirmed missing content | Preview the original inputs and rerun the protected copy. |
| Lost response | Inspect read-back of the immutable tag, graph and latest. A retry starts with the observed state. |
| Conflicting version, unknown latest owner, digest mismatch or inconclusive read | Investigate the reported identities and credentials. Preserve the existing tags. |
| Older or prerelease retry | Verify its exact tag and preserve the current latest. |

## Public acceptance

After successful read-back, record the preview and writer URLs and attempts, release commit,
source and destination index, both platform manifest digests, referrer and provenance results
on #153. Repeat once with the same verified inputs to confirm a zero-write result.

From a disposable installation, set `digest` to the verified Hub index and export
the scoped Core credentials and independent HTTP bearer from the
[Compose guide](../container.md). Use the Compose file from the published release.
The temporary Docker configuration represents a signed-out visitor.

```sh
export DOCKER_CONFIG="$(mktemp -d)"
docker pull docker.io/enthouan/komodo-mcp:1.0.0
export KOMODO_MCP_IMAGE="docker.io/enthouan/komodo-mcp@${digest}"
docker pull "$KOMODO_MCP_IMAGE"
docker image inspect "$KOMODO_MCP_IMAGE" --format '{{.Os}}/{{.Architecture}}'
docker compose config --quiet
docker compose up -d --wait komodo-mcp
docker compose ps komodo-mcp
```

Connect the guide's HTTP client, initialize MCP with its bearer, check the reported version,
list tools and perform a bounded `list_stacks` against disposable Core. Record the platform
that ran, guide URL, client, expected inventory and cleanup on #52 and link that record to
#153. Shut down with `docker compose down` and discard the temporary configuration.

Publish the [overview draft](docker-hub-overview.md) after filling its digest from read-back.
Inspect the public overview, exact tag, latest tag, version-matched source and legal links
signed out. Activate the verified Hub links on the website and check them from the live guide.
