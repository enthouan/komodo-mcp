# Verify a stable release

Starting with v1.0.0, the GitHub Release **Assets** section carries two CycloneDX inventories and two signed SLSA provenance bundles. Each pair binds the inventory to named SHA-256 subjects, the release commit, and the native or container distribution workflow on `main`. The stable installation links become active after the maintainer verifies the published release.

| Distribution | Bill of materials | Signed provenance | Subjects |
| --- | --- | --- | --- |
| Native and source | `native.sbom.cdx.json` | `native.provenance.sigstore.json` | Four native archives, Corresponding Source, `SHA256SUMS`, `LICENSE`, `THIRD-PARTY-NOTICES`, and the native SBOM |
| Container | `oci.sbom.cdx.json` | `oci.provenance.sigstore.json` | GHCR index, its `linux/amd64` and `linux/arm64` manifests, and the OCI SBOM |

The native archive inventory covers the crates compiled for the executable, including build dependencies, and the pinned Rust and applicable musl runtime sources. The Corresponding Source inventory includes the full vendored lockfile, toolchain materials, and CA inputs. Each container platform uses its native executable and the pinned CA bundle. The OCI index also contains BuildKit provenance for each platform. The two detached Sigstore bundles carry the distribution workflow signatures.

## Verify a native download

Use Git, curl, a recent GitHub CLI with `gh attestation verify --source-digest`, and an empty working directory. Choose the native target from [Native binaries and matching source](distribution.md). After the stable release is published, run the following in a POSIX shell. The peeled annotated tag supplies the exact commit.

```sh
set -eu
repo=enthouan/komodo-mcp
version=1.0.0
tag="v$version"
base="https://github.com/$repo/releases/download/$tag"
revision=$(git ls-remote "https://github.com/$repo.git" "refs/tags/$tag^{}" | awk '{print $1}')
test "${#revision}" = 40
target=x86_64-unknown-linux-musl
archive="komodo-mcp-$tag-$target.tar.gz"
for asset in "$archive" SHA256SUMS native.sbom.cdx.json native.provenance.sigstore.json; do
  curl --fail --location --silent --show-error "$base/$asset" -o "$asset"
done
for asset in "$archive" SHA256SUMS native.sbom.cdx.json; do
  gh attestation verify "$asset" --bundle native.provenance.sigstore.json \
    --repo "$repo" --source-ref refs/heads/main --source-digest "$revision" \
    --signer-digest "$revision" --deny-self-hosted-runners \
    --cert-identity "https://github.com/$repo/.github/workflows/native-distribution.yml@refs/heads/main"
done
awk -v f="$archive" '$2 == f {print; n++} END {if (n != 1) exit 1}' SHA256SUMS > archive.sha256
if command -v sha256sum >/dev/null 2>&1; then
  sha256sum -c archive.sha256
else
  shasum -a 256 -c archive.sha256
fi
```

Use the same provenance verification command on the downloaded source archive or notice files. The SBOM’s `components` records identify each named subject by hash. Its `dependencies` entries map the archive to its build inputs.

## Verify the container and its SBOM

Keep `repo`, `version`, `tag`, `base`, and `revision` from the steps above. Use Docker Buildx to obtain the raw index, jq to read the platform descriptors, and Python 3.11+ to run the verifier from the tagged repository. This command works before pulling any layers.

```sh
for asset in oci.sbom.cdx.json oci.provenance.sigstore.json; do
  curl --fail --location --silent --show-error "$base/$asset" -o "$asset"
done
image="ghcr.io/$repo:$tag"
docker buildx imagetools inspect --raw "$image" > index.json
index_digest=$(python3 -c 'import hashlib; print("sha256:" + hashlib.sha256(open("index.json", "rb").read()).hexdigest())')
platform_digest() {
  jq -er --arg arch "$1" '[.manifests[] | select(.platform.os == "linux" and .platform.architecture == $arch)]
    | if length == 1 then .[0].digest else error("expected one platform") end' index.json
}
git clone --quiet --depth 1 --branch "$tag" "https://github.com/$repo.git" verified-source
test "$(git -C verified-source rev-parse HEAD)" = "$revision"
python3 verified-source/scripts/stable_evidence.py verify-oci --directory . \
  --version "$version" --revision "$revision" --index-digest "$index_digest" \
  --amd64 "$(platform_digest amd64)" --arm64 "$(platform_digest arm64)"
```

The output identifies the signing workflow, Actions run and attempt, revision, and all three OCI digests. Reuse the verified index digest in a pin such as `ghcr.io/enthouan/komodo-mcp@sha256:…`. To verify a mirror, repeat the raw index inspection against its versioned tag and pass its observed digests to the same verifier.

## Maintainer runbook

Both distribution workflows generate their SBOM and in-toto subjects in CI. On trusted main runs, a separate GitHub-hosted job obtains a short-lived Actions OIDC identity and signs with Cosign and Sigstore. The jobs have read access to repository contents. Release and registry publication still run through `scripts/release.py publish`.

Use the native and OCI run IDs for the same validated commit when invoking the [release procedure](https://github.com/enthouan/komodo-mcp/blob/main/.agents/skills/komodo-mcp-release/SKILL.md). Each workflow retains a separate `komodo-mcp-evidence-<native|oci>-<commit>-<attempt>` artifact. The publisher verifies its immutable Actions digest, attestation signatures, expected main workflow, repository revision, invocation, SBOM hash and artifact subjects before writing the tag. It then verifies all twelve anonymously downloadable assets and both public image platforms. Record the v3 receipt, independent verification result and release URLs on the tracking issue. For historical 0.x releases, use their original receipt and verification contract.
