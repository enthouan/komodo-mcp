import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import type { Loader, LoaderContext } from "astro/loaders";
import { OPERATOR_GUIDES } from "../data/operator-guides.js";
import { REPOSITORY_SECTIONS } from "../data/repository-sections.js";
import { extractSection, rewriteRepositoryLinks, splitTitle } from "./repository-markdown.js";

function repositoryRoot(context: LoaderContext) {
  return fileURLToPath(new URL("../", context.config.root));
}

async function loadGuide(context: LoaderContext, slug: string) {
  const root = repositoryRoot(context);
  const sourcePath = `docs/${slug}.md`;
  const fileURL = new URL(sourcePath, `file://${root}`);
  const source = await readFile(fileURL, "utf8");
  const guide = OPERATOR_GUIDES.find((candidate) => candidate.slug === slug);
  if (!guide) throw new Error(`No presentation metadata for ${sourcePath}`);

  const { title, body } = splitTitle(source, sourcePath);
  const markdown = rewriteRepositoryLinks(body, sourcePath, root);
  const id = `docs/${slug}`;
  const data = await context.parseData({
    id,
    data: {
      title,
      description: guide.description,
      sidebar: { label: guide.label },
      lastUpdated: false,
    },
  });
  context.store.set({
    id,
    data,
    body: markdown,
    rendered: await context.renderMarkdown(markdown, { fileURL }),
    digest: context.generateDigest(source),
  });
}

/**
 * Adds the registered repository guides in `docs/` to Starlight's docs collection.
 * Content is rendered from the versioned Markdown at build time; nothing is copied.
 */
export function repositoryGuidesLoader(): Loader {
  return {
    name: "komodo-mcp-repository-guides",
    load: async (context) => {
      await Promise.all(OPERATOR_GUIDES.map(({ slug }) => loadGuide(context, slug)));
      const docsDirectory = `${repositoryRoot(context)}docs/`;
      context.watcher?.add(docsDirectory);
      context.watcher?.on("change", (changed) => {
        const slug = changed.startsWith(docsDirectory)
          ? changed.slice(docsDirectory.length).replace(/\.md$/, "")
          : undefined;
        if (slug && OPERATOR_GUIDES.some((guide) => guide.slug === slug)) {
          void loadGuide(context, slug);
        }
      });
    },
  };
}

/** Loads repository sections that website pages render verbatim. */
export function repositorySectionsLoader(): Loader {
  return {
    name: "komodo-mcp-repository-sections",
    load: async (context) => {
      const root = repositoryRoot(context);
      context.store.clear();
      for (const section of REPOSITORY_SECTIONS) {
        const fileURL = new URL(section.file, `file://${root}`);
        const source = await readFile(fileURL, "utf8");
        const extracted = extractSection(source, section.heading, section.file, {
          stopAtSubheading: "stopAtSubheading" in section && section.stopAtSubheading,
        });
        const markdown = rewriteRepositoryLinks(extracted, section.file, root);
        const data = await context.parseData({
          id: section.id,
          data: { file: section.file, heading: section.heading },
        });
        context.store.set({
          id: section.id,
          data,
          body: markdown,
          rendered: await context.renderMarkdown(markdown, { fileURL }),
          digest: context.generateDigest(markdown),
        });
      }
    },
  };
}
