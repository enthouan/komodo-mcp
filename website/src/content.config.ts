import { defineCollection } from "astro:content";
import type { Loader } from "astro/loaders";
import { docsLoader } from "@astrojs/starlight/loaders";
import { docsSchema } from "@astrojs/starlight/schema";
import { z } from "astro/zod";
import { repositoryGuidesLoader, repositorySectionsLoader } from "./loaders/repository.js";
import { toolCatalogLoader } from "./loaders/tool-catalog.js";

/** Website pages from src/content/docs plus guides rendered from the repository's docs/. */
function websiteDocsLoader(): Loader {
  const pages = docsLoader();
  const guides = repositoryGuidesLoader();
  const catalog = toolCatalogLoader();
  return {
    name: "komodo-mcp-docs",
    load: async (context) => {
      await pages.load(context);
      await guides.load(context);
      await catalog.load(context);
    },
  };
}

export const collections = {
  docs: defineCollection({ loader: websiteDocsLoader(), schema: docsSchema() }),
  repositorySections: defineCollection({
    loader: repositorySectionsLoader(),
    schema: z.object({ file: z.string(), heading: z.string() }),
  }),
};
