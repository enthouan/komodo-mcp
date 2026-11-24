/**
 * README sections rendered verbatim on website pages. A build fails when a
 * listed heading disappears, so the website cannot silently drift from it.
 */
export type RepositorySection = {
  id: string;
  file: "README.md";
  heading: string;
  /** Website page that embeds the section under an h2 with the same slug. */
  route: string;
  /** End the section at its first nested heading. */
  stopAtSubheading?: boolean;
};

export const REPOSITORY_SECTIONS = [
  { id: "readme-fresh-checkout-demo", file: "README.md", heading: "Fresh-checkout demo (synthetic Core)", route: "/getting-started/stdio/" },
  { id: "readme-build-and-launch", file: "README.md", heading: "Build and launch", route: "/getting-started/stdio/", stopAtSubheading: true },
  { id: "readme-streamable-http", file: "README.md", heading: "Authenticated Streamable HTTP", route: "/reference/configuration/" },
  { id: "readme-tool-selection", file: "README.md", heading: "Select individual tools", route: "/reference/configuration/" },
  { id: "readme-local-capabilities", file: "README.md", heading: "Local capabilities and Core permissions", route: "/reference/configuration/" },
  { id: "readme-inventory-tools", file: "README.md", heading: "Inventory tools", route: "/reference/tools/" },
  { id: "readme-workload-operations", file: "README.md", heading: "Workload operations", route: "/reference/tools/" },
  { id: "readme-fleet-health", file: "README.md", heading: "Fleet health", route: "/reference/tools/" },
  { id: "readme-client-and-error-foundation", file: "README.md", heading: "Client and error foundation", route: "/reference/tools/" },
  { id: "readme-version-evidence", file: "README.md", heading: "Version evidence", route: "/getting-started/compatibility/" },
  { id: "readme-development", file: "README.md", heading: "Development", route: "/reference/contributing/" },
  { id: "readme-license", file: "README.md", heading: "License", route: "/reference/" },
] as const satisfies readonly RepositorySection[];
