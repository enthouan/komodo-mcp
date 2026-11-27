import { INSTALLATION_RELEASE_TAG, PUBLISHED_RELEASE_TAG } from "./publication.js";
import { REPOSITORY_URL } from "./repository.js";

// Add a digest after receipt-based public verification on the release tracking issue.
const verifiedImageDigests: Readonly<Record<string, string>> = {
  "v0.9.0": "sha256:0dac5c0d80dd463c8d4eed87a8538606f2059988eb90862f8404177016ab3b21",
};
const imageDigest = verifiedImageDigests[INSTALLATION_RELEASE_TAG];
if (!imageDigest && INSTALLATION_RELEASE_TAG === PUBLISHED_RELEASE_TAG) {
  throw new Error(`Missing verified GHCR digest for ${INSTALLATION_RELEASE_TAG}`);
}
const taggedImage = `ghcr.io/enthouan/komodo-mcp:${INSTALLATION_RELEASE_TAG}`;
export const RELEASE_IMAGE = imageDigest ? `${taggedImage}@${imageDigest}` : taggedImage;
const releaseAsset = (name: string) => `${REPOSITORY_URL}/releases/download/${INSTALLATION_RELEASE_TAG}/${name}`;
export const INSTALLATION_SOURCE_URL = releaseAsset(`komodo-mcp-${INSTALLATION_RELEASE_TAG}-source.tar.gz`);
export const CHECKSUMS_URL = releaseAsset("SHA256SUMS");
export const LICENSE_URL = releaseAsset("LICENSE");
export const NOTICES_URL = releaseAsset("THIRD-PARTY-NOTICES");
export const COMPOSE_URL = `https://raw.githubusercontent.com/enthouan/komodo-mcp/${INSTALLATION_RELEASE_TAG}/compose.yaml`;

const platforms = [
  ["Linux x86-64", "x86_64-unknown-linux-musl"],
  ["Linux arm64", "aarch64-unknown-linux-musl"],
  ["macOS Intel", "x86_64-apple-darwin"],
  ["macOS Apple silicon", "aarch64-apple-darwin"],
] as const;

export const NATIVE_DOWNLOADS = platforms.map(([label, target]) => {
  const name = `komodo-mcp-${INSTALLATION_RELEASE_TAG}-${target}.tar.gz`;
  return { label, name, href: releaseAsset(name) };
});
