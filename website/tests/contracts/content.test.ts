import { readdir, readFile } from "node:fs/promises";
import { describe, expect, it } from "vitest";
import { CLIENT_SETUPS, EXECUTABLE_PLACEHOLDER, VERIFY_PROMPT } from "../../src/data/clients.js";
import { CAPABILITY_VARIABLES, CONFIGURATION_VARIABLES } from "../../src/data/configuration.js";
import { checkPagesPublication, INSTALLATION_RELEASE_TAG } from "../../src/data/publication.js";
import { CHECKSUMS_URL, COMPOSE_URL, INSTALLATION_SOURCE_URL, LICENSE_URL, NATIVE_DOWNLOADS, NOTICES_URL, RELEASE_IMAGE } from "../../src/data/install.js";
import { installationCandidate } from "../../src/data/installation-preview.js";
import { OPERATOR_GUIDES } from "../../src/data/operator-guides.js";
import {
  KOMODO_CLIENT_VERSION,
  PROJECT_LICENSE,
  PROJECT_VERSION,
  RELEASE_TAG,
  RELEASE_URL,
  RUST_TOOLCHAIN,
} from "../../src/data/project.js";
import { REPOSITORY_SECTIONS } from "../../src/data/repository-sections.js";
import { extractSection, rewriteRepositoryLinks } from "../../src/loaders/repository-markdown.js";
import { PUBLIC_ROUTES, TOOL_CATALOG } from "../support/site.js";
import { anchorHrefs, normalizedVisibleText, readRoute, repositoryRoot } from "./html.js";

const repo = (path: string) => readFile(new URL(path, `file://${repositoryRoot}`), "utf8");

async function websiteSources() {
  const root = new URL("../../src/", import.meta.url);
  const files = (await readdir(root, { recursive: true })).filter((file) => /\.(?:mdx?|astro|ts)$/.test(file));
  return Promise.all(
    files.map(async (file) => ({ file, source: await readFile(new URL(file, root), "utf8") })),
  );
}

const TOOL_PREFIX =
  /^(?:list|get|preview|apply|deploy|start|stop|restart|run|cancel|wait|reconcile|stack|fleet|export|find|review|refresh|observe|diagnose|execute|test|clone|pull)_[a-z_]+$/;

describe("configuration accuracy", () => {
  it("documents exactly the variables in .env.example and src/config.rs", async () => {
    const example = await repo(".env.example");
    const config = await repo("src/config.rs");
    const exampleNames = [...example.matchAll(/^(?:# )?([A-Z_]+)=/gm)].map((match) => match[1]);
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
  it("serves version-matched packages, checksums, Compose, legal files and source from the guide", async () => {
    const guide = await readRoute("/getting-started/");
    const links = anchorHrefs(guide.document);
    const base = `https://github.com/enthouan/komodo-mcp/releases/download/${INSTALLATION_RELEASE_TAG}`;
    const targets = ["x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl", "x86_64-apple-darwin", "aarch64-apple-darwin"];
    expect(NATIVE_DOWNLOADS).toHaveLength(targets.length);
    for (const target of targets) {
      expect(links).toContain(`${base}/komodo-mcp-${INSTALLATION_RELEASE_TAG}-${target}.tar.gz`);
    }
    for (const href of [CHECKSUMS_URL, COMPOSE_URL, INSTALLATION_SOURCE_URL, LICENSE_URL, NOTICES_URL]) {
      expect(links).toContain(href);
    }
    expect(COMPOSE_URL).toContain(`/${INSTALLATION_RELEASE_TAG}/compose.yaml`);
    expect(INSTALLATION_SOURCE_URL).toBe(`${base}/komodo-mcp-${INSTALLATION_RELEASE_TAG}-source.tar.gz`);
    expect(normalizedVisibleText(guide.document)).toContain(`KOMODO_MCP_IMAGE=${RELEASE_IMAGE}`);
    if (INSTALLATION_RELEASE_TAG === RELEASE_TAG) {
      expect(RELEASE_IMAGE).toMatch(/@sha256:[0-9a-f]{64}$/);
      expect(await repo("docs/container.md")).toContain(`KOMODO_MCP_IMAGE=${RELEASE_IMAGE}`);
    }
    expect(links).toContain("https://github.com/users/enthouan/packages/container/package/komodo-mcp");
  });

  it("allows installation previews and requires verified publication on the production branch", () => {
    const production = { CF_PAGES: "1", CF_PAGES_BRANCH: "main" };
    const preview = { CF_PAGES: "1", CF_PAGES_BRANCH: "antoine/beta-site" };
    expect(() => checkPagesPublication(preview, "v1.1.0", "v1.0.0")).not.toThrow();
    expect(() => checkPagesPublication(production, "v1.1.0", "v1.0.0")).toThrow("verified published release");
    expect(() => checkPagesPublication(production, "v1.1.0", "v1.1.0")).not.toThrow();
    expect(() => checkPagesPublication({ CF_PAGES: "1" })).toThrow("deployment branch");
  });

  it("exposes rehearsal packages only on the documentation review deployment", () => {
    expect(installationCandidate({})).toBeUndefined();
    expect(installationCandidate({ CF_PAGES: "1", CF_PAGES_BRANCH: "main" })).toBeUndefined();
    expect(installationCandidate({ CF_PAGES: "1", CF_PAGES_BRANCH: "antoine/unrelated" })).toBeUndefined();
    const build = installationCandidate({ CF_PAGES: "1", CF_PAGES_BRANCH: "antoine/stable-operator-docs" });
    expect(build?.revision).toMatch(/^[0-9a-f]{40}$/);
    expect(build?.checksumsSha256).toMatch(/^[0-9a-f]{64}$/);
    expect(build?.archiveSha256).toMatch(/^[0-9a-f]{64}$/);
  });

  it("uses the pinned toolchain and the locked release build", async () => {
    expect((await repo("rust-toolchain.toml")).includes(`channel = "${RUST_TOOLCHAIN}"`)).toBe(true);
    expect(await repo(".tool-versions")).toContain(`rust ${RUST_TOOLCHAIN}`);
    expect(normalizedVisibleText((await readRoute("/getting-started/stdio/")).document)).toContain("cargo build --locked --release --bin komodo-mcp");
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
    const tools = new Set(Object.keys(TOOL_CATALOG.tools));
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

  it("keeps release download URLs and image tags versioned", async () => {
    for (const { file, source } of await websiteSources()) {
      expect(source, file).not.toMatch(/releases\/latest\/download|ghcr\.io\/enthouan\/komodo-mcp:latest\b|MCP_AUTH_TOKEN/);
    }
  });
});

describe("canonical documentation sources", () => {
  it("registers every repository guide exactly once except the GitHub installation stub", async () => {
    const files = (await readdir(new URL("docs/", `file://${repositoryRoot}`)))
      .filter((file) => file.endsWith(".md"))
      .map((file) => file.replace(/\.md$/, ""))
      .sort();
    const slugs = OPERATOR_GUIDES.map(({ slug }) => slug);
    expect(new Set(slugs).size).toBe(slugs.length);
    expect(files).toContain("self-hosted-beta");
    expect([...slugs].sort()).toEqual(files.filter((slug) => slug !== "self-hosted-beta"));
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

  it("keeps the three-tool walkthrough scoped and its examples tied to the incident fixture", async () => {
    const guide = await repo("docs/operator-quickstart.md");
    const fixture = JSON.parse(await repo("tests/fixtures/core-2.3.2/incident.json"));
    const parts = guide.split(/^## /m);
    const selection = (section: string) =>
      section.match(/^KOMODO_ENABLED_TOOLS=(.+)$/m)?.[1]?.split(",") ?? [];
    const triage = selection(parts.find((section) => section.startsWith("Discover and diagnose")) ?? "");
    const staging = selection(parts.find((section) => section.startsWith("Staging deployment")) ?? "");
    expect(triage).toEqual(["list_stacks", "get_stack", "stack_diagnose"]);
    expect(staging).toEqual(expect.arrayContaining([...triage, "deploy_stack", "get_update", "wait_for_update", "reconcile_operation"]));
    for (const name of staging) expect(TOOL_CATALOG.tools, name).toHaveProperty(name);
    for (const name of triage) expect(TOOL_CATALOG.tools[name]?.grantSets, name).toEqual([[]]);

    const calls = guide.split("\n").map((line) => line.trim())
      .filter((line) => line.startsWith('{"name"')).map((line) => JSON.parse(line));
    const args = Object.fromEntries(calls.map((call) => [call.name, call.arguments]));
    expect(triage).not.toContain("list_deployments");
    expect(args.list_deployments).toEqual({});
    expect(args.get_stack.id).toBe(fixture.stack.id);
    expect(args.stack_diagnose.id).toBe(args.get_stack.id);
    expect(args.deploy_stack.target.id).toBe(args.get_stack.id);
    expect(args.wait_for_update.id).toBe(fixture.update.id);
    expect(args.reconcile_operation.target.id).toBe(args.deploy_stack.target.id);
    expect(args.reconcile_operation.requested_scope).toEqual(args.deploy_stack.scope);
    expect(args.reconcile_operation.submission_window).toEqual({
      started_ts: fixture.update.start_ts,
      ended_ts: fixture.update.end_ts,
    });
  });

  it("finds every embedded repository section and rewrites its links", async () => {
    for (const section of REPOSITORY_SECTIONS) {
      const extracted = extractSection(await repo(section.file), section.heading, section.file, {
        stopAtSubheading: "stopAtSubheading" in section && section.stopAtSubheading,
      });
      expect(extracted.length).toBeGreaterThan(50);
      const rewritten = rewriteRepositoryLinks(extracted, section.file, repositoryRoot);
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
    const { document, source } = await readRoute("/reference/");
    const text = normalizedVisibleText(document);
    expect(text).toContain(`Package version on main ${PROJECT_VERSION}`);
    expect(text).toContain(`Latest release ${RELEASE_TAG}`);
    expect(source).toContain(`href="${RELEASE_URL}"`);
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
    expect(text).toContain("A self-hosted Model Context Protocol (MCP) server for Komodo Core users.");
    expect(text).toContain("List Stacks, check server health, and diagnose failed rollouts from a server you run.");
    expect(text).toContain("Start by listing your stacks");
    expect(text).toContain("stack_diagnose");
  });

  it("takes GitHub readers and homepage visitors to the same complete guide", async () => {
    expect(await repo("README.md")).toContain("https://komodo-mcp.com/getting-started/");
    const home = anchorHrefs((await readRoute("/")).document);
    expect(home).toContain("/getting-started/");
    for (const anchor of ["native-stdio", "docker-compose"]) {
      expect(home).toContain(`/getting-started/#${anchor}`);
    }
    const page = normalizedVisibleText((await readRoute("/getting-started/")).document);
    expect(page).toContain("tools/list");
    expect(page).toContain("list_stacks");
    expect(page).toContain("Authorization: Bearer");
    expect(page).toContain("up -d --wait komodo-mcp");
    expect(page).toContain("SHA256SUMS");
  });
});
