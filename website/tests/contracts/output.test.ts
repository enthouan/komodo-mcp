import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { describe, expect, it } from "vitest";
import { DISCLAIMER } from "../../src/data/publication.js";
import { LICENSE_URL, REPOSITORY_URL } from "../../src/data/repository.js";
import { standaloneSchema, toolPageId } from "../../src/data/tool-catalog.js";
import {
  CANONICAL_WEBSITE_ORIGIN,
  CANONICAL_WEBSITE_URL,
  DOCUMENT_ROUTES,
  FOOTER_ATTRIBUTION,
  FOOTER_DISCLAIMER,
  PUBLIC_ROUTES,
  SOCIAL_IMAGE_ALT,
  THEME_COLOR,
  TOOL_CATALOG,
} from "../support/site.js";
import {
  anchorHrefs,
  attribute,
  elements,
  findByAttribute,
  findById,
  linkHref,
  metadataContent,
  normalizedText,
  readDist,
  readDistBuffer,
  readRoute,
  required,
  routeOutputPath,
  textContent,
} from "./html.js";

it("publishes every registered tool with its grants, effect hints and complete schemas", async () => {
  const index = await readRoute("/reference/tools/");
  const links = new Set(anchorHrefs(index.document));
  for (const [name, tool] of Object.entries(TOOL_CATALOG.tools)) {
    const route = `/${toolPageId(name)}/`;
    expect(links.has(route), name).toBe(true);
    const { document } = await readRoute(route);
    const text = normalizedText(document);
    for (const grant of tool.grantSets.flat()) expect(text, name).toContain(grant);
    for (const [hint, value] of Object.entries(tool.annotations)) {
      if (hint.endsWith("Hint")) expect(text, name).toContain(`${hint} — ${value}`);
    }
    for (const [label, key] of [["Input", tool.inputSchema], ["Output", tool.outputSchema]] as const) {
      const block = required(findByAttribute(document, "pre", "aria-label", `${label} JSON Schema`)[0], name);
      expect(JSON.parse(textContent(block)), name).toEqual(standaloneSchema(TOOL_CATALOG, key));
    }
    if (name === "execute_terminal") expect(text).toContain("omits outputSchema in discovery");
  }
});

describe("production route contracts", () => {
  for (const route of PUBLIC_ROUTES) {
    it(`${route} is generated with the public shell and metadata`, async () => {
      const { document, source } = await readRoute(route);
      expect(elements(document, (element) => element.tagName === "main")).toHaveLength(1);
      expect(
        elements(document, (element) => element.tagName === "h1" && attribute(element, "id") === "_top"),
      ).toHaveLength(1);
      expect(source).not.toContain("Edit page");
      expect(normalizedText(document)).toContain(FOOTER_ATTRIBUTION);
      expect(normalizedText(document)).toContain(FOOTER_DISCLAIMER);

      const runtimeAssets = elements(document, (element) =>
        ["img", "script", "source"].includes(element.tagName),
      )
        .flatMap((element) => [attribute(element, "src"), attribute(element, "srcset")])
        .filter((value): value is string => value !== undefined);
      expect(runtimeAssets.filter((value) => /^https?:\/\//.test(value))).toEqual([]);

      if (route === "/404.html") return;

      const title = normalizedText(
        required(elements(document, (element) => element.tagName === "title")[0], "title"),
      );
      expect(title).toMatch(/^.+ — komodo-mcp$/);
      const [description] = metadataContent(document, "name", "description");
      expect(description?.length ?? 0).toBeGreaterThan(40);
      expect(metadataContent(document, "property", "og:description")).toEqual([description]);
      expect(metadataContent(document, "property", "og:title")).toEqual([title.replace(/ — komodo-mcp$/, "")]);

      const canonical = linkHref(document, "canonical");
      expect(canonical).toHaveLength(1);
      const canonicalUrl = new URL(canonical[0] ?? "http://invalid.invalid/");
      expect(canonicalUrl.origin).toBe(CANONICAL_WEBSITE_ORIGIN);
      expect(canonicalUrl.protocol).toBe("https:");
      expect(canonicalUrl.hostname).toBe("komodo-mcp.com");
      expect(canonicalUrl.pathname).toBe(route);
      expect(canonicalUrl.search + canonicalUrl.hash).toBe("");
      expect(metadataContent(document, "property", "og:url")).toEqual([canonicalUrl.href]);
      expect(metadataContent(document, "name", "robots")).toEqual(["index, follow"]);
    });
  }

  it("gives every document a unique title and description", async () => {
    const pages = await Promise.all(DOCUMENT_ROUTES.map((route) => readRoute(route)));
    const titles = pages.map(({ document }) =>
      normalizedText(required(elements(document, (element) => element.tagName === "title")[0], "title")),
    );
    const descriptions = pages.map(({ document }) => metadataContent(document, "name", "description")[0]);
    expect(new Set(titles).size).toBe(titles.length);
    expect(new Set(descriptions).size).toBe(descriptions.length);
  });

  it("keeps the generated 404 out of search and canonical indexing", async () => {
    const { document } = await readRoute("/404.html");
    expect(metadataContent(document, "name", "robots")).toEqual(["noindex, nofollow"]);
    expect(linkHref(document, "canonical")).toEqual([]);
    expect(metadataContent(document, "property", "og:url")).toEqual([]);
    expect(anchorHrefs(document)).toContain("/");
  });
});

describe("internal links and fragments", () => {
  const routes = new Map<string, { hrefs: string[]; ids: Set<string> }>();
  async function linksAndIds(route: string) {
    const cached = routes.get(route);
    if (cached) return cached;
    const { document } = await readRoute(route);
    const found = {
      hrefs: anchorHrefs(document),
      ids: new Set(elements(document, (element) => attribute(element, "id") !== undefined)
        .map((element) => attribute(element, "id") ?? "")),
    };
    routes.set(route, found);
    return found;
  }

  it("resolves every internal link and fragment in the artifact", async () => {
    const problems: string[] = [];
    const checked = new Set<string>();
    for (const route of PUBLIC_ROUTES) {
      for (const href of (await linksAndIds(route)).hrefs) {
        if (/^(?:[a-z]+:|\/\/)/i.test(href)) continue;
        const url = new URL(href, `${CANONICAL_WEBSITE_URL.replace(/\/$/, "")}${route}`);
        // Every repeated navigation link resolves to the same static target.
        const key = url.pathname + url.hash;
        if (checked.has(key)) continue;
        checked.add(key);
        const target = url.pathname;
        if (!/\/$|\.html$/.test(target)) {
          if (!existsSync(new URL(`../../dist${target}`, import.meta.url))) {
            problems.push(`${route} → ${href} (missing asset)`);
          }
          continue;
        }
        if (!existsSync(routeOutputPath(target))) {
          problems.push(`${route} → ${href} (missing page)`);
          continue;
        }
        const fragment = decodeURIComponent(url.hash.slice(1));
        if (fragment && !(await linksAndIds(target)).ids.has(fragment)) {
          problems.push(`${route} → ${href} (missing fragment)`);
        }
      }
    }
    expect(problems).toEqual([]);
  });

  it("links the maintainer's GitHub resources only inside the komodo-mcp repository", async () => {
    const githubLinks = new Set<string>();
    for (const route of PUBLIC_ROUTES) {
      for (const href of (await linksAndIds(route)).hrefs) {
        if (href.startsWith("https://github.com/")) githubLinks.add(href);
      }
    }
    for (const href of githubLinks) {
      if (href.startsWith("https://github.com/enthouan/")) {
        expect(href.startsWith(`${REPOSITORY_URL}/`) || href === REPOSITORY_URL, href).toBe(true);
      }
    }
    expect(githubLinks.has(LICENSE_URL)).toBe(true);
    expect([...githubLinks].some((href) => href.startsWith(`${REPOSITORY_URL}/releases/tag/v`))).toBe(true);
  });
});

describe("SEO and machine-readable publication output", () => {
  it("emits homepage social metadata and WebSite JSON-LD", async () => {
    const { document } = await readRoute("/");
    expect(metadataContent(document, "name", "theme-color")).toEqual([THEME_COLOR]);
    expect(metadataContent(document, "property", "og:type")).toEqual(["website"]);
    expect(metadataContent(document, "property", "og:image")).toEqual([`${CANONICAL_WEBSITE_URL}social-card.png`]);
    expect(metadataContent(document, "name", "twitter:image")).toEqual([`${CANONICAL_WEBSITE_URL}social-card.png`]);
    expect(metadataContent(document, "name", "twitter:card")).toEqual(["summary_large_image"]);
    expect(metadataContent(document, "property", "og:image:width")).toEqual(["1200"]);
    expect(metadataContent(document, "property", "og:image:height")).toEqual(["630"]);
    expect(metadataContent(document, "property", "og:image:alt")).toEqual([SOCIAL_IMAGE_ALT]);

    const structuredData = findByAttribute(document, "script", "type", "application/ld+json");
    expect(structuredData).toHaveLength(1);
    const schema = JSON.parse(normalizedText(required(structuredData[0], "homepage JSON-LD")));
    expect(schema).toMatchObject({
      "@context": "https://schema.org",
      "@type": "WebSite",
      name: "komodo-mcp",
      url: CANONICAL_WEBSITE_URL,
    });
  });

  it("keeps document pages as articles without JSON-LD", async () => {
    const { document } = await readRoute("/getting-started/");
    expect(metadataContent(document, "property", "og:type")).toEqual(["article"]);
    expect(findByAttribute(document, "script", "type", "application/ld+json")).toHaveLength(0);
  });

  it("ships the generated 1200 × 630 social image", async () => {
    const image = await readDistBuffer("social-card.png");
    expect(image.subarray(1, 4).toString()).toBe("PNG");
    expect(image.readUInt32BE(16)).toBe(1200);
    expect(image.readUInt32BE(20)).toBe(630);
    const template = await readFile(new URL("../../og-image.html", import.meta.url), "utf8");
    expect(template).toContain("--og-accent: #047857;");
    expect(template).toContain("komodo-mcp");
    expect(template).not.toMatch(/trello|simplelogin|#0052cc/i);
  });

  it("publishes robots, sitemap, and llms.txt from canonical routes", async () => {
    const robots = await readDist("robots.txt");
    const sitemapIndex = await readDist("sitemap-index.xml");
    const sitemap = await readDist("sitemap-0.xml");
    const llms = await readDist("llms.txt");

    expect(robots.trimEnd().split("\n").slice(0, 2)).toEqual(["User-agent: *", "Allow: /"]);
    expect(robots).not.toMatch(/Disallow|noindex/i);
    expect(robots).toContain(`Sitemap: ${CANONICAL_WEBSITE_URL}sitemap-index.xml`);
    expect(sitemapIndex).toContain(`${CANONICAL_WEBSITE_URL}sitemap-0.xml`);

    const locations = [...sitemap.matchAll(/<loc>([^<]+)<\/loc>/g)].map((match) => new URL(match[1] ?? ""));
    for (const location of locations) expect(location.origin).toBe(CANONICAL_WEBSITE_ORIGIN);
    expect(locations.map(({ pathname }) => pathname).sort()).toEqual([...DOCUMENT_ROUTES].sort());

    expect(llms).toContain("# komodo-mcp");
    expect(llms).toContain(`\n${DISCLAIMER}\n`);
    expect(llms).toContain(`- [GitHub repository](${REPOSITORY_URL})`);
    for (const route of DOCUMENT_ROUTES) {
      expect(llms).toContain(`(${CANONICAL_WEBSITE_URL}${route.slice(1)})`);
    }
    expect(llms).not.toMatch(/\]\(\//);
    expect(llms).not.toContain("undefined");
    expect(llms).not.toContain("404");
    expect(llms).not.toContain("/docs/self-hosted-beta");
    const search = JSON.parse(await readDist("pagefind/pagefind-entry.json"));
    expect(search.languages.en.page_count).toBe(DOCUMENT_ROUTES.length);
  });
});

describe("Cloudflare-static publication contracts", () => {
  it("copies security headers and never publishes a global noindex rule", async () => {
    const headers = (await readDist("_headers")).trim();
    expect(headers).toBe(
      [
        "/*",
        "  X-Content-Type-Options: nosniff",
        "  Referrer-Policy: strict-origin-when-cross-origin",
        "  X-Frame-Options: DENY",
        "  Permissions-Policy: camera=(), geolocation=(), microphone=(), payment=(), usb=()",
      ].join("\n"),
    );
    expect(headers).not.toMatch(/X-Robots-Tag/i);
  });

  it("redirects the retired installation route to the current guide and preserves its anchors", async () => {
    const retired = "/docs/self-hosted-beta";
    expect((await readDist("_redirects")).trim().split(/\r?\n/)).toEqual([
      `${retired} /getting-started/ 301`,
      `${retired}/ /getting-started/ 301`,
    ]);
    expect(existsSync(routeOutputPath(`${retired}/`))).toBe(false);
    const { document } = await readRoute("/getting-started/");
    for (const fragment of ["native-stdio", "docker-compose"]) {
      expect(findById(document, fragment)).toHaveLength(1);
    }
  });
});

describe("production asset safety", () => {
  it("ships the local project mark and accessible request-flow diagram", async () => {
    const favicon = await readDist("favicon.svg");
    expect(favicon).toContain("komodo-mcp stacked-status mark");
    expect(favicon).toContain("#047857");
    expect(favicon).not.toMatch(/<script|href=["']https?:/i);

    const flow = await readDist("request-flow.svg");
    for (const marker of [
      "How a komodo-mcp tool call travels",
      "holds Core API key + secret",
      "Komodo Core remains the source of truth",
      "@media (prefers-color-scheme: dark)",
    ]) {
      expect(flow).toContain(marker);
    }
  });

  it("generates local Font Awesome client masks with attribution", async () => {
    const css = await readDist("client-icons.css");
    expect(css).toContain("Font Awesome Free 7.3.1");
    expect(css).toContain("https://fontawesome.com/license/free");
    expect(css).not.toMatch(/url\(["']?https?:/);
    for (const name of ["claude", "evidence", "inspector", "openai", "opencode", "vscode"]) {
      expect(css).toContain(`--client-icon-${name}:`);
    }
  });

  it("leaves analytics injection to the host and keeps credentials out of the generated pages", async () => {
    const sources = await Promise.all(PUBLIC_ROUTES.map((route) => readRoute(route)));
    const combined = sources.map(({ source }) => source).join("\n");
    const bundledScripts = sources.flatMap(({ document }) =>
      elements(document, (element) => element.tagName === "script")
        .map((script) => [attribute(script, "src"), normalizedText(script)].join(" ")),
    );
    expect(bundledScripts.join("\n")).not.toMatch(
      /googletagmanager|google-analytics|plausible\.io|posthog|segment\.com|mixpanel|hotjar|cloudflareinsights/i,
    );
    expect(combined).not.toMatch(/KOMODO_API_(?:KEY|SECRET)[^<\n]{0,8}=\s*[A-Za-z0-9]{24,}/);
    for (const route of DOCUMENT_ROUTES) {
      expect(findById((await readRoute(route)).document, "_top")).toHaveLength(1);
    }
  });
});
