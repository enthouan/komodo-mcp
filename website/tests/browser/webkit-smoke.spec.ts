import { HERO_TITLE } from "../support/site.js";
import { assertNoPageOverflow, expect, gotoLoaded, monitorBrowserProblems, test } from "./support.js";

test("homepage loads without runtime or layout errors", async ({ page }) => {
  const problems = monitorBrowserProblems(page);
  await gotoLoaded(page, "/");
  await expect(page.getByRole("heading", { level: 1 })).toHaveAccessibleName(HERO_TITLE);
  await expect(page.locator("header [data-repository-navigation]:visible")).toHaveAccessibleName(
    "komodo-mcp source repository, 1.2K stars",
  );
  await assertNoPageOverflow(page, "WebKit homepage");
  expect(problems).toEqual([]);
});

test("search and client deep links work in WebKit", async ({ page }) => {
  await gotoLoaded(page, "/getting-started/clients/#opencode");
  await expect(page.locator('[data-client-setups] [role="tab"][aria-selected="true"]')).toHaveText("OpenCode");
  await page.locator("button[data-open-modal]:visible").first().click();
  const input = page.locator(".pagefind-ui__search-input:visible").first();
  await input.fill("list_stacks");
  await expect(page.locator(".pagefind-ui__result-link:visible").first()).toBeVisible();
});
