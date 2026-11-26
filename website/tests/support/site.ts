import { readFileSync } from "node:fs";
import { OPERATOR_GUIDES } from "../../src/data/operator-guides.js";
import { type Catalog, toolPageId } from "../../src/data/tool-catalog.js";

export const CANONICAL_WEBSITE_URL = "https://komodo-mcp.com/";
export const CANONICAL_WEBSITE_ORIGIN = new URL(CANONICAL_WEBSITE_URL).origin;
export const FOOTER_ATTRIBUTION =
  "komodo-mcp is free software released under the GNU General Public License and maintained by Antoine Ménard.";
export const FOOTER_DISCLAIMER =
  "It is not an official Komodo product and is not affiliated with or endorsed by the Komodo project.";
export const COPY_FAILURE_MESSAGE = "Copy failed. Select the code and copy it manually.";
export const HERO_TITLE = "Operate Komodo from your MCP client.";
export const THEME_COLOR = "#047857";
export const SOCIAL_IMAGE_ALT = "komodo-mcp — independent, self-hosted Rust MCP server for Komodo";

export const WEBSITE_ROUTES = [
  "/",
  "/getting-started/",
  "/getting-started/komodo-credentials/",
  "/getting-started/clients/",
  "/getting-started/compatibility/",
  "/getting-started/stdio/",
  "/guides/how-it-works/",
  "/guides/workflows/",
  "/guides/security/",
  "/guides/operations/",
  "/guides/troubleshooting/",
  "/guides/faq/",
  "/reference/",
  "/reference/configuration/",
  "/reference/tools/",
  "/reference/contributing/",
  "/reference/reporting-issues/",
] as const;

export const GUIDE_ROUTES = OPERATOR_GUIDES.map(({ slug }) => `/docs/${slug}/`);

export const TOOL_CATALOG = JSON.parse(readFileSync(new URL("../../../docs/tool-catalog.json", import.meta.url), "utf8")) as Catalog;
export const TOOL_ROUTES = Object.keys(TOOL_CATALOG.tools).map((name) => `/${toolPageId(name)}/`);
export const DOCUMENT_ROUTES = [...WEBSITE_ROUTES, ...GUIDE_ROUTES, ...TOOL_ROUTES];
export const PUBLIC_ROUTES = [...DOCUMENT_ROUTES, "/404.html"];

export const PRIMARY_NAVIGATION = [
  ["Install and run", "/getting-started/"],
  ["Komodo Core credentials", "/getting-started/komodo-credentials/"],
  ["Set up your MCP client", "/getting-started/clients/"],
  ["Compatibility", "/getting-started/compatibility/"],
  ["Stable support policy", "/docs/stable-support/"],
  ["Upgrade and rollback", "/docs/upgrade-v1/"],
  ["Build from source", "/getting-started/stdio/"],
  ["How it works", "/guides/how-it-works/"],
  ["Workflows", "/guides/workflows/"],
  ["Security & Data", "/guides/security/"],
  ["Operations", "/guides/operations/"],
  ["Troubleshooting", "/guides/troubleshooting/"],
  ["FAQ", "/guides/faq/"],
  ["Overview", "/reference/"],
  ["Configuration", "/reference/configuration/"],
  ["Tool reference", "/reference/tools/"],
  ["Contributing", "/reference/contributing/"],
  ["Reporting issues and support", "/reference/reporting-issues/"],
  ["Repository maintenance", "/docs/maintenance/"],
] as const;

export const RESPONSIVE_VIEWPORTS = [
  { name: "desktop-1440", width: 1440, height: 900 },
  { name: "tablet-820", width: 820, height: 1180 },
  { name: "mobile-360", width: 360, height: 780 },
] as const;

export const REPRESENTATIVE_ACCESSIBILITY_ROUTES = [
  "/",
  "/getting-started/",
  "/getting-started/clients/",
  "/getting-started/compatibility/",
  "/guides/how-it-works/",
  "/guides/troubleshooting/",
  "/reference/configuration/",
  "/reference/tools/",
  "/docs/diagnostics/",
  "/reference/tools/list_stacks/",
  "/404.html",
] as const;
