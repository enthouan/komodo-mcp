import { mkdir } from "node:fs/promises";
import { expect, gotoLoaded, test } from "./browser/support.js";

/** Optional design-review capture (`pnpm website:visual`); not part of the PR gate. */
const ROUTES = { home: "/", clients: "/getting-started/clients/", guide: "/docs/diagnostics/" } as const;
const VIEWPORTS = {
  desktop: { width: 1440, height: 900 },
  tablet: { width: 820, height: 1180 },
  mobile: { width: 390, height: 844 },
} as const;
const output = new URL("../artifacts/visual/", import.meta.url);

for (const [name, route] of Object.entries(ROUTES)) {
  for (const [viewportName, viewport] of Object.entries(VIEWPORTS)) {
    for (const theme of ["light", "dark"] as const) {
      test(`${name} ${viewportName} ${theme}`, async ({ page }) => {
        await mkdir(output, { recursive: true });
        await page.emulateMedia({ colorScheme: theme, reducedMotion: "reduce" });
        await page.setViewportSize(viewport);
        await gotoLoaded(page, route);
        await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
        await page.screenshot({
          path: new URL(`${name}-${viewportName}-${theme}.png`, output).pathname,
          fullPage: name !== "guide",
        });
      });
    }
  }
}
