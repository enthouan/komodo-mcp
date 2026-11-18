import { getCollection } from "astro:content";
import type { APIRoute } from "astro";
import { PROJECT_LICENSE, RELEASE_URL } from "../data/project.js";
import { DISCLAIMER } from "../data/publication.js";
import { LICENSE_URL, REPOSITORY_URL } from "../data/repository.js";

export const prerender = true;

function routeForEntry(id: string) {
  const withoutIndex = id.replace(/(?:^|\/)index$/, "");
  return withoutIndex ? `${withoutIndex}/` : "";
}

export const GET: APIRoute = async ({ site }) => {
  if (!site) throw new Error("Astro.site must be configured to generate llms.txt");

  const base = import.meta.env.BASE_URL.endsWith("/")
    ? import.meta.env.BASE_URL
    : `${import.meta.env.BASE_URL}/`;
  const href = (path: string) => new URL(`${base}${path}`.replace(/\/{2,}/g, "/"), site).href;

  const entries = await getCollection("docs");
  const pages = entries
    .filter((entry) => entry.id !== "404")
    .map((entry) => ({
      description: entry.data.description?.trim(),
      title: entry.data.title,
      url: href(routeForEntry(entry.id)),
    }))
    .sort((left, right) => left.url.localeCompare(right.url));

  const body = [
    "# komodo-mcp",
    "",
    "> An independent, self-hosted Rust Model Context Protocol server for Komodo. It runs over stdio and uses the caller's own Komodo Core credentials and permissions.",
    "",
    DISCLAIMER,
    "",
    "## Documentation",
    "",
    ...pages.map(
      ({ description, title, url }) => `- [${title}](${url})${description ? `: ${description}` : ""}`,
    ),
    "",
    "## Source",
    "",
    `- [GitHub repository](${REPOSITORY_URL})`,
    `- [Latest release](${RELEASE_URL})`,
    `- [License (${PROJECT_LICENSE})](${LICENSE_URL})`,
    "",
  ].join("\n");

  return new Response(body, { headers: { "Content-Type": "text/plain; charset=utf-8" } });
};
