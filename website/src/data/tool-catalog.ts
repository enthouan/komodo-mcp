export interface CatalogTool {
  description: string;
  grantSets: string[][];
  annotations: Record<string, boolean | string>;
  inputSchema: string;
  outputSchema: string;
  outputSource?: "resultType";
}

export interface Catalog {
  tools: Record<string, CatalogTool>;
  schemas: Record<string, Record<string, unknown>>;
  schemaDefinitions: Record<string, Record<string, string>>;
  definitions: Record<string, unknown>;
}

/** Preserve familiar registered names and keep other names in a single, unique URL segment. */
export function toolPageId(name: string) {
  const slug = /^[a-z0-9][a-z0-9_-]*$/.test(name) ? name : `~${Buffer.from(name, "utf8").toString("base64url")}`;
  return `reference/tools/${slug}`;
}

/** Restore the registered document's definition names before resolving its local references. */
export function standaloneSchema(catalog: Catalog, key: string): Record<string, unknown> {
  const root = catalog.schemas[key];
  if (!root) throw new Error(`Missing catalog schema ${key}`);
  const names = catalog.schemaDefinitions[key];
  if (!names) return root;
  const definitions = Object.entries(names).map(([name, shared]) => {
    if (!Object.hasOwn(catalog.definitions, shared)) throw new Error(`Missing catalog definition ${shared}`);
    return [name, catalog.definitions[shared]] as const;
  });
  return { ...root, $defs: Object.fromEntries(definitions) };
}
