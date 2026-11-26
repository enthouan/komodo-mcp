/**
 * Repository sections rendered verbatim on website pages. A build fails when a
 * listed heading disappears, so the website cannot silently drift from it.
 */
export type RepositorySection = {
  id: string;
  file: string;
  heading: string;
  /** Website page that embeds the section under an h2 with the same slug. */
  route: string;
  /** End the section at its first nested heading. */
  stopAtSubheading?: boolean;
};

export const REPOSITORY_SECTIONS = [
  { id: "source-fresh-checkout-demo", file: "docs/reference/building.md", heading: "Fresh-checkout demo (synthetic Core)", route: "/getting-started/stdio/" },
  { id: "source-build-and-launch", file: "docs/reference/building.md", heading: "Build and launch", route: "/getting-started/stdio/", stopAtSubheading: true },
  { id: "source-streamable-http", file: "docs/reference/runtime.md", heading: "Authenticated Streamable HTTP", route: "/reference/configuration/" },
  { id: "source-tool-selection", file: "docs/reference/runtime.md", heading: "Select individual tools", route: "/reference/configuration/" },
  { id: "source-local-capabilities", file: "docs/reference/runtime.md", heading: "Local capabilities and Core permissions", route: "/reference/configuration/" },
  { id: "source-inventory-tools", file: "docs/reference/tools.md", heading: "Inventory tools", route: "/reference/tools/" },
  { id: "source-workload-operations", file: "docs/reference/tools.md", heading: "Workload operations", route: "/reference/tools/" },
  { id: "source-fleet-health", file: "docs/reference/tools.md", heading: "Fleet health", route: "/reference/tools/" },
  { id: "source-client-and-error-foundation", file: "docs/reference/tools.md", heading: "Client and error foundation", route: "/reference/tools/" },
  { id: "source-version-evidence", file: "docs/reference/development.md", heading: "Version evidence", route: "/getting-started/compatibility/" },
  { id: "source-development", file: "docs/reference/development.md", heading: "Development", route: "/reference/contributing/" },
  { id: "source-license", file: "README.md", heading: "License", route: "/reference/" },
] as const satisfies readonly RepositorySection[];
