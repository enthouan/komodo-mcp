import { existsSync, statSync } from "node:fs";
import { posix } from "node:path";
import GithubSlugger from "github-slugger";
import { OPERATOR_GUIDES, operatorGuideRoute } from "../data/operator-guides.js";
import { REPOSITORY_SECTIONS } from "../data/repository-sections.js";
import { repositoryFileUrl } from "../data/repository.js";

const FENCE = /^\s*(```|~~~)/;
const HEADING = /^(#{1,6})\s+(.+?)\s*#*\s*$/;
const INLINE_LINK = /\]\(([^)\s]+)((?:\s+"[^"]*")?)\)/g;

/** Applies `transform` to Markdown prose lines while leaving fenced code untouched. */
function mapProseLines(markdown: string, transform: (line: string) => string) {
  let fence: string | undefined;
  return markdown
    .split("\n")
    .map((line) => {
      const marker = line.match(FENCE)?.[1];
      if (marker && (fence === undefined || fence === marker)) {
        fence = fence === undefined ? marker : undefined;
        return line;
      }
      return fence === undefined ? transform(line) : line;
    })
    .join("\n");
}

export function headingSlug(text: string) {
  return new GithubSlugger().slug(text);
}

/** Maps README anchors embedded on website pages to those pages. */
const README_SECTION_ROUTES = new Map(
  REPOSITORY_SECTIONS.map(({ heading, route }) => [headingSlug(heading), route]),
);
const GUIDE_SLUGS = new Set(OPERATOR_GUIDES.map(({ slug }) => slug));

/**
 * Rewrites a relative link found in repository Markdown so it works on the
 * website: published guides become website routes, README anchors embedded on
 * the website become page anchors, and every other repository path becomes a
 * GitHub URL. A link to a missing repository path fails the build.
 */
export function rewriteRepositoryLink(
  target: string,
  sourcePath: string,
  repositoryRoot: string,
): string {
  if (/^[a-z][a-z0-9+.-]*:/i.test(target) || target.startsWith("//")) return target;

  const hashIndex = target.indexOf("#");
  const pathPart = hashIndex === -1 ? target : target.slice(0, hashIndex);
  const fragment = hashIndex === -1 ? "" : target.slice(hashIndex);

  if (pathPart === "") {
    if (sourcePath === "README.md") {
      const route = README_SECTION_ROUTES.get(fragment.slice(1));
      return route ? `${route}${fragment}` : repositoryFileUrl(`README.md${fragment}`);
    }
    return fragment;
  }

  const resolved = posix.normalize(posix.join(posix.dirname(sourcePath), pathPart));
  if (resolved.startsWith("../") || resolved === "..") {
    throw new Error(`${sourcePath} links outside the repository: ${target}`);
  }

  const guide = resolved.match(/^docs\/([a-z0-9-]+)\.md$/)?.[1];
  if (guide && GUIDE_SLUGS.has(guide)) return `${operatorGuideRoute(guide)}${fragment}`;

  if (resolved === "README.md") {
    const route = README_SECTION_ROUTES.get(fragment.slice(1));
    if (route) return `${route}${fragment}`;
  }

  const absolute = posix.join(repositoryRoot, resolved);
  if (!existsSync(absolute)) {
    throw new Error(`${sourcePath} links to a missing repository path: ${target}`);
  }
  const kind = statSync(absolute).isDirectory() ? "tree" : "blob";
  return repositoryFileUrl(`${resolved}${fragment}`, kind);
}

export function rewriteRepositoryLinks(
  markdown: string,
  sourcePath: string,
  repositoryRoot: string,
) {
  return mapProseLines(markdown, (line) =>
    line.replace(
      INLINE_LINK,
      (_match, target: string, title: string) =>
        `](${rewriteRepositoryLink(target, sourcePath, repositoryRoot)}${title})`,
    ),
  );
}

/** Removes the document title; Starlight renders the page title as the h1. */
export function splitTitle(markdown: string, sourcePath: string) {
  const lines = markdown.split("\n");
  const title = lines[0]?.match(/^#\s+(.+)$/)?.[1]?.trim();
  if (!title) throw new Error(`${sourcePath} must begin with a level-one heading`);
  return { title, body: lines.slice(1).join("\n").replace(/^\n+/, "") };
}

/**
 * Extracts one README section (without its heading). Subheadings are shifted so
 * the section nests under an h2 on the embedding page. With `stopAtSubheading`,
 * extraction ends at the first nested heading.
 */
export function extractSection(
  markdown: string,
  heading: string,
  sourcePath: string,
  { stopAtSubheading = false } = {},
) {
  const lines = markdown.split("\n");
  let fence: string | undefined;
  let level: number | undefined;
  const collected: string[] = [];

  for (const line of lines) {
    const marker = line.match(FENCE)?.[1];
    if (marker && (fence === undefined || fence === marker)) {
      fence = fence === undefined ? marker : undefined;
      if (level !== undefined) collected.push(line);
      continue;
    }
    const match = fence === undefined ? line.match(HEADING) : null;
    if (match) {
      const depth = match[1]?.length ?? 0;
      if (level === undefined) {
        if (match[2] === heading) level = depth;
        continue;
      }
      if (depth <= level || stopAtSubheading) break;
      collected.push(`${"#".repeat(depth - level + 2)} ${match[2]}`);
      continue;
    }
    if (level !== undefined) collected.push(line);
  }

  if (level === undefined) {
    throw new Error(`${sourcePath} no longer contains the heading “${heading}”`);
  }
  return collected.join("\n").trim();
}
