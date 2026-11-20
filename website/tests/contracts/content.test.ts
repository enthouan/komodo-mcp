import { readdir, readFile } from "node:fs/promises";
import { describe, expect, it } from "vitest";
import { CLIENT_SETUPS, EXECUTABLE_PLACEHOLDER, VERIFY_PROMPT } from "../../src/data/clients.js";
import { CAPABILITY_VARIABLES, CONFIGURATION_VARIABLES } from "../../src/data/configuration.js";
import { INSTALL_METHODS } from "../../src/data/install.js";
import { OPERATOR_GUIDES } from "../../src/data/operator-guides.js";
import {
  KOMODO_CLIENT_VERSION,
  PROJECT_LICENSE,
  PROJECT_VERSION,
  RUST_TOOLCHAIN,
} from "../../src/data/project.js";
import { REPOSITORY_SECTIONS } from "../../src/data/repository-sections.js";
import { extractSection, rewriteRepositoryLinks } from "../../src/loaders/repository-markdown.js";
import { DOCUMENT_ROUTES, PUBLIC_ROUTES } from "../support/site.js";
import { normalizedVisibleText, readRoute, repositoryRoot } from "./html.js";

const repo = (path: string) => readFile(new URL(path, `file://${repositoryRoot}`), "utf8");

async function websiteSources() {
  const root = new URL("../../src/", import.meta.url);
  const files = (await readdir(root, { recursive: true })).filter((file) => /\.(?:mdx?|astro|ts)$/.test(file));
  return Promise.all(
    files.map(async (file) => ({ file, source: await readFile(new URL(file, root), "utf8") })),
  );
}

/** Tool names registered in the production router. */
async function registeredTools() {
  const server = await repo("src/server.rs");
  const names = new Set<string>();
  for (const match of server.matchAll(/"([a-z]+(?:_[a-z]+)+)"/g)) names.add(match[1] ?? "");
  for (const match of server.matchAll(/async fn ([a-z]+(?:_[a-z]+)+)\(&self/g)) names.add(match[1] ?? "");
  return names;
}

const TOOL_PREFIX =
  /^(?:list|get|preview|apply|deploy|start|stop|restart|run|cancel|wait|reconcile|stack|fleet|export|find|review|refresh|observe|diagnose|execute|test|clone|pull)_[a-z_]+$/;

describe("configuration accuracy", () => {
  it("documents exactly the variables in .env.example and src/config.rs", async () => {
    const example = await repo(".env.example");
    const config = await repo("src/config.rs");
    const exampleNames = [...example.matchAll(/^([A-Z_]+)=/gm)].map((match) => match[1]);
    expect(CONFIGURATION_VARIABLES.map(({ name }) => name).sort()).toEqual([...exampleNames].sort());
    for (const { name } of CONFIGURATION_VARIABLES) expect(config).toContain(`"${name}"`);
  });

  it("keeps every capability grant default-off and strictly boolean", async () => {
    const example = await repo(".env.example");
    const capabilities = await repo("src/capabilities.rs");
    expect(capabilities).toContain('None | Some("false") => Ok(false)');
    expect(capabilities).toContain('Some("true") => Ok(true)');
    for (const variable of CAPABILITY_VARIABLES) {
      expect(variable.defaultValue).toBe("false");
      expect(example).toContain(`${variable.name}=false`);
    }
  });

  it("never claims the executable loads .env files", async () => {
    const config = await repo("src/config.rs");
    const main = await repo("src/main.rs");
    expect(`${config}${main}`).not.toMatch(/dotenv/i);
    const sources = await websiteSources();
    for (const { file, source } of sources) {
      const claims = source
        .split("\n")
        .filter((line) => /(?:loads?|reads?) (?:the |a |your )?\.env files?/i.test(line))
        .filter((line) => !/\b(?:not|never|no)\b/i.test(line));
      expect(claims, file).toEqual([]);
    }
  });
});

describe("installation and client accuracy", () => {
  it("advertises only the source-built stdio path as available", () => {
    expect(INSTALL_METHODS.filter(({ status }) => status === "available").map(({ key }) => key)).toEqual(["stdio"]);
    for (const method of INSTALL_METHODS.filter(({ status }) => status === "planned")) {
      expect(method).not.toHaveProperty("code");
      expect(method.tracking?.length).toBeGreaterThan(0);
    }
  });

  it("uses the pinned toolchain and the locked release build", async () => {
    expect((await repo("rust-toolchain.toml")).includes(`channel = "${RUST_TOOLCHAIN}"`)).toBe(true);
    expect(await repo(".tool-versions")).toContain(`rust ${RUST_TOOLCHAIN}`);
    const stdio = INSTALL_METHODS.find(({ key }) => key === "stdio");
    expect(stdio?.code).toContain("cargo build --locked --release --bin komodo-mcp");
    expect(await repo(".github/workflows/ci.yml")).toContain("cargo build --locked --release --bin komodo-mcp");
  });

  it("keeps every client recipe on stdio with read-only grants and no literal credentials", () => {
    expect(CLIENT_SETUPS.map(({ key }) => key)).toEqual(["codex", "claude-code", "claude-desktop", "vscode", "opencode"]);
    for (const client of CLIENT_SETUPS) {
      expect(client.code, client.key).toContain(EXECUTABLE_PLACEHOLDER);
      for (const { name } of CAPABILITY_VARIABLES) {
        expect(client.code, `${client.key} ${name}`).toMatch(new RegExp(`${name}"?\\s*[:=]\\s*"false"`));
      }
      expect(client.code, client.key).not.toMatch(/https?:\/\/127\.0\.0\.1|\/mcp"|bearer|Authorization/i);
      expect(client.reviewed).toMatch(/^\d{4}-\d{2}-\d{2}$/);
      if (client.key !== "claude-desktop") {
        expect(client.code, client.key).not.toContain("replace-in-this-user-local-file");
      }
    }
  });

  it("makes listing stacks the first verification read", () => {
    expect(VERIFY_PROMPT).toContain("list_stacks");
    expect(VERIFY_PROMPT).toMatch(/do not change anything/i);
  });

  it("names only tools registered by the production router", async () => {
    const tools = await registeredTools();
    const unknown = new Set<string>();
    for (const { file, source } of await websiteSources()) {
      for (const match of source.matchAll(/[`"]([a-z]+(?:_[a-z]+)+)[`"]/g)) {
        const name = match[1] ?? "";
        if (TOOL_PREFIX.test(name) && !tools.has(name)) unknown.add(`${file}: ${name}`);
      }
      for (const chain of source.matchAll(/`([a-z_]+(?: → [a-z_]+)+)`/g)) {
        for (const name of (chain[1] ?? "").split(" → ")) {
          if (!tools.has(name)) unknown.add(`${file}: ${name}`);
        }
      }
    }
    expect([...unknown]).toEqual([]);
    expect(tools.has("list_stacks")).toBe(true);
    expect(tools.has("stack_diagnose")).toBe(true);
  });

  it("does not invent unavailable distribution or HTTP details", async () => {
    for (const { file, source } of await websiteSources()) {
      const withoutUrls = source.replace(/https?:\/\/[^\s"'`)<]+/g, "");
      expect(withoutUrls, file).not.toMatch(/ghcr\.io|docker compose up|docker run|MCP_AUTH_TOKEN|:\d+\/mcp\b/);
      expect(source, file).not.toMatch(/releases\/download|ghcr\.io/);
    }
  });
});

describe("canonical documentation sources", () => {
  it("publishes every repository guide exactly once", async () => {
    const files = (await readdir(new URL("docs/", `file://${repositoryRoot}`)))
      .filter((file) => file.endsWith(".md"))
      .map((file) => file.replace(/\.md$/, ""))
      .sort();
    const slugs = OPERATOR_GUIDES.map(({ slug }) => slug);
    expect(new Set(slugs).size).toBe(slugs.length);
    expect([...slugs].sort()).toEqual(files);
  });

  it("renders each guide from its versioned Markdown", async () => {
    for (const { slug } of OPERATOR_GUIDES) {
      const source = await repo(`docs/${slug}.md`);
      const title = source.split("\n")[0]?.replace(/^#\s+/, "") ?? "";
      const { document } = await readRoute(`/docs/${slug}/`);
      const text = normalizedVisibleText(document);
      expect(text).toContain(title);
      const firstHeading = source.match(/^## (.+)$/m)?.[1];
      if (firstHeading) expect(text).toContain(firstHeading.replace(/`/g, ""));
    }
  });

  it("finds every embedded README section and rewrites its links", async () => {
    const readme = await repo("README.md");
    for (const section of REPOSITORY_SECTIONS) {
      const extracted = extractSection(readme, section.heading, "README.md", {
        stopAtSubheading: "stopAtSubheading" in section && section.stopAtSubheading,
      });
      expect(extracted.length).toBeGreaterThan(50);
      const rewritten = rewriteRepositoryLinks(extracted, "README.md", repositoryRoot);
      expect(rewritten).not.toMatch(/\]\((?!https?:|\/|#)/);
    }
  });

  it("rejects links to missing repository paths", () => {
    expect(() => rewriteRepositoryLinks("[x](docs/missing.md)", "README.md", repositoryRoot)).toThrow(
      /missing repository path/,
    );
  });

  it("states versions and the license from Cargo metadata", async () => {
    expect(PROJECT_LICENSE).toBe("GPL-3.0-or-later");
    expect(await repo("LICENSE")).toContain("GNU GENERAL PUBLIC LICENSE");
    const { document } = await readRoute("/reference/");
    const text = normalizedVisibleText(document);
    expect(text).toContain(`v${PROJECT_VERSION}`);
    expect(text).toContain(`komodo_client ${KOMODO_CLIENT_VERSION}`);
    const faq = normalizedVisibleText((await readRoute("/guides/faq/")).document);
    expect(faq).toContain(`Komodo Core ${KOMODO_CLIENT_VERSION}`);
  });
});

describe("visitor-facing output", () => {
  it("interpolates every build-time value", async () => {
    for (const route of PUBLIC_ROUTES) {
      const text = normalizedVisibleText((await readRoute(route)).document);
      // `${NAME}` is client substitution syntax shown deliberately; a bare `{NAME}` is a missed MDX expression.
      expect(text.match(/(?<!\$)\{[A-Z][A-Z0-9_]{2,}\}/g) ?? [], route).toEqual([]);
    }
  });

  it("contains no copied reference-project identity, claims, or license", async () => {
    for (const route of PUBLIC_ROUTES) {
      const { source } = await readRoute(route);
      expect(source, route).not.toMatch(/trello|simplelogin|atlassian|proton/i);
      expect(source, route).not.toMatch(/MIT License|MIT licensed/);
      expect(source, route).not.toMatch(/\bTested over\b/);
    }
  });

  it("introduces komodo-mcp as independent and makes stacks the first read", async () => {
    const text = normalizedVisibleText((await readRoute("/")).document);
    expect(text).toContain("not an official Komodo product");
    expect(text).toContain("independent, self-hosted Rust Model Context Protocol (MCP) server for Komodo");
    expect(text).toMatch(/Start by listing your stacks[\s\S]*Diagnose a failed stack/);
    expect(text).toContain("stack_diagnose");
  });

  it("keeps planned paths visibly unavailable", async () => {
    const text = normalizedVisibleText((await readRoute("/getting-started/")).document);
    expect(text).toContain("Planned — not available yet");
    expect(text).toContain("Available now");
    for (const route of ["/getting-started/docker/", "/getting-started/http/"]) {
      expect(DOCUMENT_ROUTES).not.toContain(route);
    }
  });
});
