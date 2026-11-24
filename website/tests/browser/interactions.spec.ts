import { COPY_FAILURE_MESSAGE } from "../support/site.js";
import { expect, gotoLoaded, monitorBrowserProblems, test } from "./support.js";

test("filtering the catalog opens a registered tool and its schemas", async ({ page }) => {
  await gotoLoaded(page, "/reference/tools/");
  const catalog = page.locator("[data-tool-catalog]");
  const filter = catalog.getByRole("searchbox", { name: "Filter by tool name" });
  await filter.fill("STACK DIAGNOSE");
  await expect(catalog.getByRole("status")).toHaveText("1 tool");
  await catalog.getByRole("link", { name: "stack_diagnose", exact: true }).click();
  await expect(page).toHaveURL(/\/reference\/tools\/stack_diagnose\/$/);
  await page.getByText("Show input JSON Schema", { exact: true }).click();
  await expect(page.locator('pre[aria-label="Input JSON Schema"]')).toBeVisible();
});

test("keyboard search finds tool references and repository guides", async ({ page }) => {
  const problems = monitorBrowserProblems(page);
  await gotoLoaded(page, "/");
  await page.keyboard.press("Control+k");
  const input = page.locator(".pagefind-ui__search-input:visible").first();
  await expect(input).toBeFocused();
  await input.fill("stack_diagnose");
  await expect(page.locator('.pagefind-ui__result-link[href="/reference/tools/stack_diagnose/"]').first()).toBeVisible();
  const result = page.locator('.pagefind-ui__result-link[href^="/docs/diagnostics/"]:visible').first();
  const more = page.getByRole("button", { name: "Load more results" });
  while (!(await result.isVisible()) && await more.isVisible()) {
    await more.click();
  }
  await expect(result).toBeVisible();
  await result.click();
  await expect(page).toHaveURL(/\/docs\/diagnostics\//);
  expect(problems).toEqual([]);
});

test("search button has an accessible name matching its visible label", async ({ page }) => {
  await gotoLoaded(page, "/getting-started/");
  const button = page.locator("button[data-open-modal]:visible").first();
  await expect(button).toHaveAccessibleName(/^Search\b/);
  await button.click();
  await expect(page.locator(".pagefind-ui__search-input:visible").first()).toBeFocused();
});

test("client tabs honor deep links and switch recipes", async ({ page }) => {
  await gotoLoaded(page, "/getting-started/clients/#claude-desktop");
  const selected = page.locator('[data-client-setups] [role="tab"][aria-selected="true"]');
  await expect(selected).toHaveText("Claude Desktop");
  await expect(selected).toHaveAttribute("data-client-icon", "claude");
  const panel = page.locator('[data-client-setups] [role="tabpanel"]:visible');
  await expect(panel).toContainText("claude_desktop_config.json");
  await expect(panel).toContainText("KOMODO_ALLOW_OPERATE");

  await page.locator('[data-client-setups] [role="tab"]', { hasText: "VS Code" }).click();
  await expect(page.locator('[data-client-setups] [role="tabpanel"]:visible')).toContainText(
    "~/.copilot/mcp-config.json",
  );
  await page.keyboard.press("ArrowRight");
  await expect(selected).toHaveText("OpenCode");
  await expect(page.locator('[data-client-setups] [role="tabpanel"]:visible')).toContainText("{env:KOMODO_API_KEY}");
});

test("homepage client cards deep-link to the matching tab", async ({ page }) => {
  await gotoLoaded(page, "/");
  await page.locator('a[href="/getting-started/clients/#vscode"]').click();
  await expect(page.locator('[data-client-setups] [role="tab"][aria-selected="true"]')).toHaveText("VS Code");
});

test("install tabs lead to both beta guides and matching source", async ({ page }) => {
  for (const route of ["/", "/getting-started/"]) {
    for (const [label, anchor] of [["Native stdio", "native-stdio"], ["Docker Compose", "docker-compose"]] as const) {
      await gotoLoaded(page, route);
      const methods = page.locator("[data-install-methods]");
      await methods.getByRole("tab", { name: label }).click();
      const panel = methods.getByRole("tabpanel").filter({ visible: true });
      await expect(panel.getByRole("link", { name: /Install and connect/ })).toHaveAttribute("href", `/docs/self-hosted-beta/#${anchor}`);
      await expect(panel.getByRole("link", { name: "Matching Corresponding Source" })).toHaveAttribute("href", /-source\.tar\.gz$/);
      await panel.getByRole("link", { name: /Install and connect/ }).click();
      await expect(page).toHaveURL(new RegExp(`/docs/self-hosted-beta/#${anchor}$`));
      await expect(page.locator(`h2#${anchor}`)).toBeVisible();
      await page.locator('main a[href="/docs/operator-quickstart/#discover-and-diagnose-with-three-tools"]').first().click();
      await expect(page).toHaveURL(/\/docs\/operator-quickstart\/#discover-and-diagnose-with-three-tools$/);
      await expect(page.locator("main")).toContainText("list_stacks,get_stack,stack_diagnose");
    }
  }
});

test("overflowing code blocks are keyboard scrollable", async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 780 });
  await gotoLoaded(page, "/getting-started/stdio/");
  const overflowing = await page.locator(".expressive-code pre").evaluateAll((blocks) =>
    blocks.filter((block) => block.scrollWidth > block.clientWidth).map((block) => block.getAttribute("tabindex")),
  );
  expect(overflowing.length).toBeGreaterThan(0);
  expect(overflowing.every((value) => value === "0")).toBe(true);
});

test("code blocks copy their contents", async ({ page, context, browserName }) => {
  test.skip(browserName !== "chromium", "Clipboard permissions are Chromium-specific");
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await gotoLoaded(page, "/");
  const block = page.locator(".expressive-code").filter({ hasText: "Use list_stacks and do not change anything." });
  const copy = block.locator(".copy button");
  await expect(copy).toHaveAccessibleName(/copy/i);
  await copy.click();
  const copied = await page.evaluate(() => navigator.clipboard.readText());
  expect(copied).toContain("Use list_stacks and do not change anything.");
});

test("copy failures are announced", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: {
        writeText: async () => {
          throw new DOMException("Clipboard write rejected", "NotAllowedError");
        },
      },
    });
    Object.defineProperty(Document.prototype, "execCommand", {
      configurable: true,
      value: () => false,
    });
  });
  await gotoLoaded(page, "/");
  await page.locator(".expressive-code").filter({ hasText: "Use list_stacks and do not change anything." }).locator(".copy button").click();
  await expect(page.getByText(COPY_FAILURE_MESSAGE).first()).toBeVisible();
});

test("tables in rendered repository guides are keyboard scrollable", async ({ page }) => {
  await gotoLoaded(page, "/docs/mock-stdio/");
  const table = page.locator("main table").first();
  await table.focus();
  await expect(table).toBeFocused();
  await expect(table).toHaveCSS("outline-style", "solid");
});

test.describe("reduced motion", () => {
  test.use({ reducedMotion: "reduce" });
  test("removes button and copy transitions", async ({ page }) => {
    await gotoLoaded(page, "/");
    await expect(page.locator(".sl-link-button.primary").first()).toHaveCSS("transition-duration", "0s");
  });
});

test.describe("without JavaScript", () => {
  test.use({ javaScriptEnabled: false });

  test("client setup shows every recipe", async ({ page }) => {
    await page.goto("/getting-started/clients/");
    const setups = page.locator("[data-client-setups]");
    await expect(setups.locator(':scope > starlight-tabs[data-sync-key="mcp-client"]')).toBeHidden();
    const fallbacks = setups.locator('section[aria-labelledby^="no-script-"]');
    await expect(fallbacks).toHaveCount(5);
    for (const label of ["Codex", "Claude Code", "Claude Desktop", "VS Code", "OpenCode"]) {
      await expect(page.getByRole("heading", { level: 3, name: label, exact: true })).toBeVisible();
    }
    await expect(fallbacks.filter({ hasText: "VS Code" })).toContainText("~/.copilot/mcp-config.json");
  });

  test("the repository link shows the star without a count", async ({ page }) => {
    await page.goto("/");
    const slot = page.locator("header [data-repository-navigation]:visible [data-repository-star-slot]");
    await expect(slot.locator("svg")).toBeVisible();
    await expect(slot.locator("[data-repository-star-value]")).toHaveText("");
  });

  test("installation paths and stdio instructions remain readable", async ({ page }) => {
    for (const route of ["/", "/getting-started/"]) {
      await page.goto(route);
      const methods = page.locator("[data-install-methods]");
      for (const [label, section] of [["Native stdio", "native-stdio"], ["Docker Compose", "docker-compose"]] as const) {
        const fallback = methods.getByRole("region", { name: label, exact: true });
        await expect(fallback.getByRole("heading", { name: label })).toBeVisible();
        await expect(fallback.locator(`a[href="/docs/self-hosted-beta/#${section}"]`)).toBeVisible();
      }
    }
    await expect(page.locator("main")).toContainText("cargo build --locked --release --bin komodo-mcp");
    await page.goto("/getting-started/stdio/");
    await expect(page.locator("main")).toContainText("KOMODO_API_SECRET");
  });
});
