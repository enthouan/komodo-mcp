import { URL } from "node:url";
import { unified } from "@astrojs/markdown-remark";
import starlight from "@astrojs/starlight";
import { defineConfig } from "astro/config";
import { OPERATOR_GUIDE_GROUPS } from "./src/data/operator-guides.js";
import { CANONICAL_WEBSITE_URL, checkPagesPublication } from "./src/data/publication.js";
import { REPOSITORY_URL } from "./src/data/repository.js";

checkPagesPublication(process.env);

const THEME_COLOR = "#047857";
const SOCIAL_IMAGE_ALT = "komodo-mcp — independent, self-hosted Rust MCP server for Komodo";
const socialImageUrl = new URL("social-card.png", CANONICAL_WEBSITE_URL).href;
/** @type {Array<{ tag: "meta"; attrs: Record<string, string> }>} */
const socialImageHead = [
  { tag: "meta", attrs: { property: "og:image", content: socialImageUrl } },
  { tag: "meta", attrs: { property: "og:image:alt", content: SOCIAL_IMAGE_ALT } },
  { tag: "meta", attrs: { property: "og:image:type", content: "image/png" } },
  { tag: "meta", attrs: { property: "og:image:width", content: "1200" } },
  { tag: "meta", attrs: { property: "og:image:height", content: "630" } },
  { tag: "meta", attrs: { name: "twitter:image", content: socialImageUrl } },
  { tag: "meta", attrs: { name: "twitter:image:alt", content: SOCIAL_IMAGE_ALT } },
];

function makeScrollableTablesKeyboardAccessible() {
  return (tree) => {
    const visit = (node) => {
      if (!node || typeof node !== "object") return;
      if (node.type === "element" && node.tagName === "table") {
        node.properties ??= {};
        node.properties.tabIndex = 0;
      }
      if (Array.isArray(node.children)) {
        for (const child of node.children) visit(child);
      }
    };
    visit(tree);
  };
}

const operatorGuideSidebar = OPERATOR_GUIDE_GROUPS.filter(({ label }) => label !== "Project" && label !== "Support and upgrades").map(
  ({ label, guides }) => ({
    label,
    collapsed: true,
    items: guides.map(({ slug, label: guideLabel }) => ({ slug: `docs/${slug}`, label: guideLabel })),
  }),
);

export default defineConfig({
  output: "static",
  // Preserve component selector specificity used before Starlight 0.43.
  scopedStyleStrategy: "where",
  site: CANONICAL_WEBSITE_URL,
  trailingSlash: "always",
  markdown: {
    processor: unified({
      rehypePlugins: [makeScrollableTablesKeyboardAccessible],
    }),
  },
  integrations: [
    starlight({
      title: "komodo-mcp",
      titleDelimiter: "—",
      description:
        "Documentation for komodo-mcp, an independent, self-hosted Rust Model Context Protocol server for Komodo.",
      favicon: "/favicon.svg",
      logo: {
        src: "./public/favicon.svg",
        alt: "",
        replacesTitle: false,
      },
      head: [
        { tag: "link", attrs: { rel: "stylesheet", href: "/client-icons.css" } },
        { tag: "meta", attrs: { name: "robots", content: "index, follow" } },
        { tag: "meta", attrs: { name: "theme-color", content: THEME_COLOR } },
        { tag: "meta", attrs: { name: "twitter:card", content: "summary_large_image" } },
        ...socialImageHead,
      ],
      customCss: ["./src/styles/custom.css"],
      routeMiddleware: "./src/starlightRouteData.ts",
      components: {
        Footer: "./src/components/Footer.astro",
        SocialIcons: "./src/components/RepositorySocialLink.astro",
      },
      social: [
        {
          icon: "github",
          label: "komodo-mcp source repository",
          href: REPOSITORY_URL,
        },
      ],
      sidebar: [
        {
          label: "Get started",
          items: [
            { slug: "getting-started", label: "Install and run" },
            { slug: "getting-started/komodo-credentials", label: "Komodo Core credentials" },
            { slug: "getting-started/clients", label: "Set up your MCP client" },
            { slug: "getting-started/compatibility", label: "Compatibility" },
            { slug: "docs/stable-support", label: "Stable support policy" },
            { slug: "docs/upgrade-v1", label: "Upgrade and rollback" },
          ],
        },
        {
          label: "Guides",
          items: [
            { slug: "guides/how-it-works", label: "How it works" },
            { slug: "guides/workflows", label: "Workflows" },
            { slug: "guides/security", label: "Security & Data" },
            { slug: "guides/operations", label: "Operations" },
            { slug: "guides/troubleshooting", label: "Troubleshooting" },
            { slug: "guides/faq", label: "FAQ" },
            { label: "Operator guides", collapsed: true, items: operatorGuideSidebar },
          ],
        },
        {
          label: "Reference",
          items: [
            { slug: "reference", label: "Overview" },
            { slug: "reference/configuration", label: "Configuration" },
            { slug: "reference/tools", label: "Tool reference" },
            { slug: "getting-started/stdio", label: "Build from source" },
            { slug: "reference/contributing", label: "Contributing" },
            { slug: "reference/reporting-issues", label: "Reporting issues and support" },
            { slug: "docs/maintenance", label: "Repository maintenance" },
          ],
        },
      ],
      lastUpdated: true,
      pagination: true,
      pagefind: true,
      credits: false,
    }),
  ],
});
