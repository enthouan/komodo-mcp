import { REPOSITORY_URL } from "../../src/data/repository.js";
import { HERO_TITLE, PRIMARY_NAVIGATION, RESPONSIVE_VIEWPORTS } from "../support/site.js";
import { assertNoPageOverflow, expect, gotoLoaded, monitorBrowserProblems, test } from "./support.js";

test("homepage presents the Komodo hero, actions, and independence statement", async ({ page }) => {
  const problems = monitorBrowserProblems(page);
  await gotoLoaded(page, "/");
  await expect(page.getByRole("heading", { level: 1 })).toHaveAccessibleName(HERO_TITLE);
  await expect(page.getByRole("link", { name: "Get started", exact: true })).toHaveAttribute(
    "href",
    "/getting-started/",
  );
  const github = page.locator(".hero [data-github-action]");
  await expect(github).toHaveAccessibleName("View on GitHub");
  await expect(github).toHaveAttribute("href", REPOSITORY_URL);
  await expect(page.locator("footer")).toContainText("not an official Komodo product");
  await expect(page.locator("main")).toContainText("List the stacks I can access in Komodo");
  await expect(page.locator("header [data-repository-navigation]:visible")).toHaveAccessibleName(
    "komodo-mcp source repository, 1.2K stars",
  );
  expect(problems).toEqual([]);
});

test("desktop sidebar exposes every primary destination", async ({ page }) => {
  await gotoLoaded(page, "/getting-started/");
  const sidebar = page.locator("#starlight__sidebar");
  for (const [label, href] of PRIMARY_NAVIGATION) {
    await expect(sidebar.locator(`a[href="${href}"]`).first(), label).toHaveText(label);
  }
  await expect(sidebar.locator('a[aria-current="page"]')).toHaveText("Install and run");
});

for (const viewport of RESPONSIVE_VIEWPORTS) {
  for (const route of ["/", "/getting-started/clients/", "/docs/diagnostics/", "/reference/configuration/"]) {
    test(`${route} reflows without page overflow at ${viewport.name}`, async ({ page }) => {
      await page.setViewportSize(viewport);
      await gotoLoaded(page, route);
      await assertNoPageOverflow(page, `${route} at ${viewport.name}`);
    });
  }
}

test("mobile menu opens with the keyboard and reaches documentation", async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 780 });
  await gotoLoaded(page, "/getting-started/");
  const button = page.locator(".sl-menu-button").first();
  await expect(button).toBeVisible();
  await button.focus();
  await page.keyboard.press("Enter");
  await expect(page.locator("#starlight__sidebar:popover-open")).toHaveCount(1);
  const link = page.locator('#starlight__sidebar a[href="/getting-started/stdio/"]:visible').first();
  await expect(link).toBeVisible();
  await link.click();
  await expect(page).toHaveURL(/\/getting-started\/stdio\/$/);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Build from source");
});

test("theme selection persists and applies the green palette", async ({ page }) => {
  await gotoLoaded(page, "/");
  const themeSelect = page.locator("starlight-theme-select select:visible").first();
  const expected = { dark: "rgb(52, 211, 153)", light: "rgb(4, 120, 87)" } as const;
  for (const theme of ["dark", "light"] as const) {
    await themeSelect.selectOption(theme);
    await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
    const accent = await page.evaluate(() =>
      getComputedStyle(document.documentElement).getPropertyValue("--sl-color-accent").trim(),
    );
    const resolved = await page.evaluate((value) => {
      const probe = document.createElement("span");
      probe.style.color = value;
      document.body.append(probe);
      const color = getComputedStyle(probe).color;
      probe.remove();
      return color;
    }, accent);
    expect(resolved).toBe(expected[theme]);
    await page.reload({ waitUntil: "domcontentloaded" });
    await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
  }
});

test("repository star falls back to the icon without a count", async ({ page }) => {
  const problems = monitorBrowserProblems(page, {
    allowConsole: () => true,
    allowResponse: (response) => response.url().startsWith("https://api.github.com/"),
  });
  await page.unroute("https://api.github.com/repos/enthouan/komodo-mcp");
  await page.route("https://api.github.com/repos/enthouan/komodo-mcp", (route) =>
    route.fulfill({ status: 404, body: '{"message":"Not Found"}' }),
  );
  await gotoLoaded(page, "/");
  const link = page.locator("header [data-repository-navigation]:visible");
  await expect(link).toHaveAttribute("data-repository-star-state", "complete");
  await expect(link).toHaveAccessibleName("komodo-mcp source repository");
  const slot = link.locator("[data-repository-star-slot]");
  await expect(slot).toBeVisible();
  await expect(slot.locator("svg")).toBeVisible();
  await expect(slot.locator("[data-repository-star-value]")).toHaveText("");
  // The star stands alone: no space stays reserved for a missing number.
  const width = await slot.evaluate((element) => element.getBoundingClientRect().width);
  expect(width).toBeLessThan(24);
  await expect(link).toHaveAttribute("href", REPOSITORY_URL);
  expect(problems.filter(({ kind }) => kind === "page")).toEqual([]);
});

test("repository star shows the formatted count when GitHub answers", async ({ page }) => {
  await gotoLoaded(page, "/getting-started/");
  const slot = page.locator("header [data-repository-navigation]:visible [data-repository-star-slot]");
  await expect(slot).toHaveAttribute("data-repository-star-count", "");
  await expect(slot.locator("[data-repository-star-value]")).toHaveText("1.2K");
  await expect(slot.locator("svg")).toBeVisible();
});
