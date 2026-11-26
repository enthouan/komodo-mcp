import { REPRESENTATIVE_ACCESSIBILITY_ROUTES } from "../support/site.js";
import {
  assertHeadingAndLandmarkBasics,
  assertNoSeriousAccessibilityViolations,
  expect,
  gotoLoaded,
  test,
} from "./support.js";

for (const theme of ["light", "dark"] as const) {
  test.describe(`${theme} theme`, () => {
    test.use({ colorScheme: theme });
    for (const route of REPRESENTATIVE_ACCESSIBILITY_ROUTES) {
      test(`${route} has no serious accessibility violations`, async ({ page }) => {
        await gotoLoaded(page, route);
        await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
        if (route !== "/404.html") await assertHeadingAndLandmarkBasics(page, route);
        await assertNoSeriousAccessibilityViolations(page, `${route} (${theme})`);
      });
    }
  });
}

test("keyboard users can skip to content and see focus", async ({ page }) => {
  await gotoLoaded(page, "/getting-started/");
  await page.keyboard.press("Tab");
  const skip = page.getByRole("link", { name: /skip to content/i });
  await expect(skip).toBeFocused();
  await page.keyboard.press("Tab");
  const focused = page.locator(":focus-visible");
  await expect(focused).toHaveCount(1);
  await expect(focused).toHaveCSS("outline-width", "3px");
});

test("installation choices are named and keyboard navigable", async ({ page }) => {
  await gotoLoaded(page, "/getting-started/");
  const native = page.locator('main p a[href="#native-stdio"]').first();
  await native.focus();
  await page.keyboard.press("Tab");
  await expect(page.locator('main p a[href="#docker-compose"]').first()).toBeFocused();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/#docker-compose$/);
});
