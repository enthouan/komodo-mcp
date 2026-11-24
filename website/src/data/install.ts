import { INSTALLATION_RELEASE_TAG } from "./publication.js";
import { REPOSITORY_URL } from "./repository.js";

export const RELEASE_IMAGE = `ghcr.io/enthouan/komodo-mcp:${INSTALLATION_RELEASE_TAG}`;
const asset = (name: string) => `${REPOSITORY_URL}/releases/download/${INSTALLATION_RELEASE_TAG}/${name}`;
export const INSTALLATION_SOURCE_URL = asset(`komodo-mcp-${INSTALLATION_RELEASE_TAG}-source.tar.gz`);
const source = { label: "Matching Corresponding Source", href: INSTALLATION_SOURCE_URL };
const checksums = { label: "SHA-256 checksums", href: asset("SHA256SUMS") };
const license = { label: "License", href: asset("LICENSE") };
const notices = { label: "Third-party notices", href: asset("THIRD-PARTY-NOTICES") };

const platforms = [
  ["Linux · x86-64", "x86_64-unknown-linux-musl"],
  ["Linux · ARM64", "aarch64-unknown-linux-musl"],
  ["macOS · Intel", "x86_64-apple-darwin"],
  ["macOS · Apple silicon", "aarch64-apple-darwin"],
] as const;

export const INSTALL_METHODS = [
  {
    key: "binary",
    label: "Native stdio",
    summary: "Choose the archive for the machine that runs your MCP client. Verify its checksum, extract the executable, and configure your client to launch it.",
    guide: "/docs/self-hosted-beta/#native-stdio",
    guideLabel: "Install and connect over stdio",
    links: [
      ...platforms.map(([label, target]) => ({ label, href: asset(`komodo-mcp-${INSTALLATION_RELEASE_TAG}-${target}.tar.gz`) })),
      checksums, source, license, notices,
    ],
  },
  {
    key: "docker",
    label: "Docker Compose",
    summary: "Run a persistent authenticated HTTP service. Compose publishes the host port on loopback. Configure a separate inbound bearer token and a Core identity before starting the service.",
    guide: "/docs/self-hosted-beta/#docker-compose",
    guideLabel: "Install and connect over HTTP",
    links: [
      { label: "Versioned Compose file", href: `https://raw.githubusercontent.com/enthouan/komodo-mcp/${INSTALLATION_RELEASE_TAG}/compose.yaml` },
      { label: "Container image on GHCR", href: `${REPOSITORY_URL}/pkgs/container/komodo-mcp` },
      checksums, source, license, notices,
    ],
  },
] as const;

export type InstallMethod = (typeof INSTALL_METHODS)[number];
