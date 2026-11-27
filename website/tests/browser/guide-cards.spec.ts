import type { Locator } from "@playwright/test";
import sharp from "sharp";
import { expect, gotoLoaded, test } from "./support.js";

async function expectPaintedIcon(card: Locator, state: string) {
  // The path crop excludes the SVG's colored badge and border.
  const glyph = card.locator(".title svg.icon path").first();
  const fill = await glyph.evaluate((path) => getComputedStyle(path).fill);
  const rgb = fill.match(/^rgb\((\d+), (\d+), (\d+)\)$/)?.slice(1).map(Number);
  if (!rgb) throw new Error(`Expected an RGB icon fill, got ${fill}`);

  const pixels = await sharp(await glyph.screenshot()).removeAlpha().raw().toBuffer();
  let ink = 0;
  for (let offset = 0; offset < pixels.length; offset += 3) {
    if (rgb.every((channel, i) => Math.abs(pixels.readUInt8(offset + i) - channel) <= 20)) ink++;
  }
  const title = await card.locator(".title").innerText();
  expect(ink, `${title} icon disappeared ${state}`).toBeGreaterThan(4);
}

for (const theme of ["light", "dark"] as const) {
  test(`homepage guide icons stay painted in the ${theme} theme`, async ({ page }) => {
    await page.emulateMedia({ colorScheme: theme });
    await gotoLoaded(page, "/");
    await expect(page.locator("html")).toHaveAttribute("data-theme", theme);
    const cards = page.locator("main .card:has(.guide-link)");
    await expect(cards).toHaveCount(4);

    for (const card of await cards.all()) {
      await page.mouse.move(0, 0);
      await expectPaintedIcon(card, "at rest");
      await card.locator(".guide-link").hover();
      await expectPaintedIcon(card, "on hover");
    }

    await page.mouse.move(0, 0);
    await cards.first().locator(".guide-link").focus();
    await page.keyboard.press("Shift+Tab");
    for (const card of await cards.all()) {
      await page.keyboard.press("Tab");
      await expect(card.locator(".guide-link:focus-visible")).toBeFocused();
      await expectPaintedIcon(card, "with keyboard focus");
    }
  });
}
