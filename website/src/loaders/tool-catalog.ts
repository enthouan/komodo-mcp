import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import type { Loader, LoaderContext } from "astro/loaders";
import { OPERATOR_GUIDES, operatorGuideRoute } from "../data/operator-guides.js";
import { type Catalog, type CatalogTool, standaloneSchema, toolPageId } from "../data/tool-catalog.js";

function escapeHtml(text: string) {
  return text.replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;");
}

function object(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : undefined;
}

function inputFields(schema: Record<string, unknown>) {
  const fields = Object.entries(object(schema.properties) ?? {});
  if (!fields.length) return "";
  const keywords = ["type", "minimum", "exclusiveMinimum", "maximum", "exclusiveMaximum", "minLength", "maxLength", "minItems", "maxItems", "pattern", "default"];
  return "<dl>" + fields.map(([name, value]) => {
    const field = object(value) ?? {};
    const facts = keywords.filter((key) => key in field).map((key) => `<code>${key} ${escapeHtml(JSON.stringify(field[key]))}</code>`);
    if (Array.isArray(schema.required) && schema.required.includes(name)) facts.unshift("Required");
    if (typeof field.$ref === "string") facts.push(`Schema <code>${escapeHtml(field.$ref.split("/").at(-1) ?? field.$ref)}</code>`);
    const description = typeof field.description === "string" ? escapeHtml(field.description) : "";
    return `<dt><code>${escapeHtml(name)}</code></dt><dd>${[description, facts.join(" · ")].filter(Boolean).join(" ") || "See the input schema."}</dd>`;
  }).join("") + "</dl>";
}

function outputFields(schema: Record<string, unknown>) {
  const definitions = object(schema.$defs);
  if (!definitions || !Array.isArray(schema.anyOf)) return "";
  const alternatives = schema.anyOf.flatMap((branch: unknown) => {
    const reference = object(branch)?.$ref;
    if (typeof reference !== "string" || !reference.startsWith("#/$defs/")) return [];
    const name = reference.slice("#/$defs/".length);
    const fields = Object.keys(object(object(definitions[name])?.properties) ?? {});
    if (!fields.length) return [];
    return [`<dt><code>${escapeHtml(name)}</code></dt><dd>${fields.map((field) => `<code>${escapeHtml(field)}</code>`).join(", ")}</dd>`];
  });
  return alternatives.length ? `<dl>${alternatives.join("")}</dl>` : "";
}

function schemaBlock(schema: Record<string, unknown>, label: string) {
  const json = escapeHtml(JSON.stringify(schema, null, 2));
  return `<details><summary>Show ${label.toLowerCase()} JSON Schema</summary><pre tabindex="0" role="region" aria-label="${label} JSON Schema" data-pagefind-ignore><code>${json}</code></pre></details>`;
}

function renderTool(catalog: Catalog, tool: CatalogTool, guides: string[]) {
  const input = standaloneSchema(catalog, tool.inputSchema);
  const output = standaloneSchema(catalog, tool.outputSchema);
  const grants = tool.grantSets.map((set) =>
    set.length ? set.map((grant) => `\`${grant}=true\``).join(" and ") : "Available under the default capability policy",
  ).join(" or ");
  const hints = Object.entries(tool.annotations)
    .filter(([key]) => key.endsWith("Hint"))
    .map(([key, value]) => `- \`${key}\` — \`${value}\``)
    .join("\n");
  const outputNote = tool.outputSource === "resultType"
    ? "This tool omits `outputSchema` in discovery. The shape below comes from its Rust result and tool error types."
    : "The registered output schema covers structured results and tool errors.";
  return [
    `<p>${escapeHtml(tool.description)}</p>`,
    "## Availability", grants,
    "The instance’s tool selection and upstream Core permissions also apply. Some inputs require additional grants.",
    "## Effect annotations", hints,
    "## Inputs", "The registered schema gives field types, alternatives and declared bounds. The description and operator guides also cover checks applied at runtime.",
    inputFields(input), schemaBlock(input, "Input"),
    "## Output", outputNote, outputFields(output), schemaBlock(output, "Output"),
    ...(guides.length ? ["## Operator guides", guides.map((link) => `- ${link}`).join("\n")] : []),
    "[All built-in tools](/reference/tools/#built-in-tools)",
  ].join("\n\n");
}

async function loadTools(context: LoaderContext, directory: URL) {
  const catalog: Catalog = JSON.parse(await readFile(new URL("tool-catalog.json", directory), "utf8"));
  const guides = await Promise.all(OPERATOR_GUIDES.map(async ({ slug, label }) => {
    const source = await readFile(new URL(`${slug}.md`, directory), "utf8");
    const mentions = new Set((source.match(/\b[a-z][a-z0-9_]*\b/g) ?? []).filter((name) => Object.hasOwn(catalog.tools, name)));
    return { mentions, link: `[${label}](${operatorGuideRoute(slug)})` };
  }));
  const ids = new Set<string>();
  for (const [name, tool] of Object.entries(catalog.tools)) {
    const id = toolPageId(name);
    ids.add(id);
    const markdown = renderTool(catalog, tool, guides.filter((guide) => guide.mentions.has(name)).map((guide) => guide.link));
    const digest = context.generateDigest(markdown);
    if (context.store.get(id)?.digest === digest) continue;
    const data = await context.parseData({
      id,
      data: {
        title: name,
        description: `Input bounds, output shape, local grants and effect annotations for the ${name} MCP tool.`,
        lastUpdated: false,
      },
    });
    context.store.set({ id, data, body: markdown, digest, rendered: await context.renderMarkdown(markdown) });
  }
  for (const id of context.store.keys()) {
    if (id.startsWith("reference/tools/") && !ids.has(id)) context.store.delete(id);
  }
}

export function toolCatalogLoader(): Loader {
  return {
    name: "komodo-mcp-tool-catalog",
    load: async (context) => {
      const directory = new URL("../docs/", context.config.root);
      await loadTools(context, directory);
      const path = fileURLToPath(directory);
      context.watcher?.add(path);
      context.watcher?.on("change", (changed) => {
        if (changed === `${path}tool-catalog.json` || (changed.startsWith(path) && changed.endsWith(".md"))) {
          void loadTools(context, directory);
        }
      });
    },
  };
}
