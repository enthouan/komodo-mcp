export type InstallMethodKey = "stdio" | "binary" | "docker";

export type InstallMethod = {
  key: InstallMethodKey;
  label: string;
  status: "available" | "planned";
  summary: string;
  href?: string;
  title?: string;
  code?: string;
  tracking?: { label: string; href: string }[];
};

const ISSUES = "https://github.com/enthouan/komodo-mcp/issues";

/** Installation paths shown on the website, with their actual availability. */
export const INSTALL_METHODS: readonly InstallMethod[] = [
  {
    key: "stdio",
    label: "Native stdio · source build",
    status: "available",
    summary:
      "Build the locked release executable with the pinned Rust toolchain. Your MCP client launches it as a child process over stdio; it opens no network listener.",
    href: "/getting-started/stdio/",
    title: "Build from source",
    code: `git clone https://github.com/enthouan/komodo-mcp.git
cd komodo-mcp
cargo build --locked --release --bin komodo-mcp
# Executable: ./target/release/komodo-mcp`,
  },
  {
    key: "binary",
    label: "Native binary download",
    status: "planned",
    summary:
      "Prebuilt executables with checksums, license text, notices, and matching Corresponding Source are not published yet. Releases currently provide source only.",
    tracking: [{ label: "#42 Distribute native binaries", href: `${ISSUES}/42` }],
  },
  {
    key: "docker",
    label: "Docker Compose · authenticated HTTP",
    status: "planned",
    summary:
      "The executable currently serves stdio only. A container image, Compose configuration, and authenticated Streamable HTTP are planned, not available.",
    tracking: [
      { label: "#40 Authenticated Streamable HTTP", href: `${ISSUES}/40` },
      { label: "#41 GHCR images and Docker Compose", href: `${ISSUES}/41` },
    ],
  },
];

export function getInstallMethod(key: InstallMethodKey): InstallMethod {
  const method = INSTALL_METHODS.find((candidate) => candidate.key === key);
  if (!method) throw new Error(`Unknown install method: ${key}`);
  return method;
}
