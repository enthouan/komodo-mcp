import { parse } from "smol-toml";
import cargoManifest from "../../../Cargo.toml?raw";
import rustToolchain from "../../../rust-toolchain.toml?raw";
import { PUBLISHED_RELEASE_TAG } from "./publication.js";
import { RELEASES_URL, REPOSITORY_URL } from "./repository.js";

type CargoManifest = {
  package: { license: string; name: string; version: string; "rust-version": string };
  dependencies: Record<string, string | { version?: string }>;
};

const manifest = parse(cargoManifest) as unknown as CargoManifest;
const toolchain = parse(rustToolchain) as unknown as { toolchain: { channel: string } };

function exactDependency(name: string): string {
  const dependency = manifest.dependencies[name];
  const requirement = typeof dependency === "string" ? dependency : dependency?.version;
  const exact = requirement?.match(/^=(\d+\.\d+\.\d+)$/)?.[1];
  if (!exact) throw new Error(`Cargo.toml must pin ${name} exactly`);
  return exact;
}

/** Facts derived from versioned repository sources at build time. */
export const PROJECT_NAME = manifest.package.name;
export const PROJECT_VERSION = manifest.package.version;
export const PROJECT_LICENSE = manifest.package.license;
export const RUST_TOOLCHAIN = toolchain.toolchain.channel;
export const KOMODO_CLIENT_VERSION = exactDependency("komodo_client");
export const RMCP_VERSION = exactDependency("rmcp");
/** The MCP protocol revision exercised by the repository's compiled-stdio tests. */
export const MCP_PROTOCOL_VERSION = "2025-11-25";
export const RELEASE_TAG = PUBLISHED_RELEASE_TAG;
export const RELEASE_URL = `${RELEASES_URL}/tag/${RELEASE_TAG}`;
export const RELEASE_SOURCE_ARCHIVE_URL = `${REPOSITORY_URL}/releases/download/${RELEASE_TAG}/komodo-mcp-${RELEASE_TAG}-source.tar.gz`;
