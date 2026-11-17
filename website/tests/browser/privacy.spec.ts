import { DISCLOSED_STORAGE, DISCLOSED_THIRD_PARTY_ORIGINS } from "../support/site.js";
import { expect, gotoLoaded, test } from "./support.js";

/**
 * Exercises the interactive features that can store state or make requests and
 * compares the result with the Security & Data disclosure. Point
 * PLAYWRIGHT_BASE_URL at a Cloudflare preview or production to also catch
 * hosting-injected scripts such as analytics beacons.
 */
test("requests, cookies, and storage match the published disclosure", async ({ page, context, baseURL }) => {
  const siteOrigin = new URL(baseURL ?? "http://127.0.0.1:4321").origin;
  const requests: { origin: string; url: string; headers: Record<string, string> }[] = [];
  page.on("request", (request) => {
    const url = new URL(request.url());
    if (url.protocol === "data:") return;
    requests.push({ origin: url.origin, url: request.url(), headers: request.headers() });
  });

  await gotoLoaded(page, "/");
  await gotoLoaded(page, "/getting-started/clients/");
  await page.locator('[data-client-setups] [role="tab"]', { hasText: "VS Code" }).click();
  await gotoLoaded(page, "/getting-started/");
  await page.locator('[data-install-methods] [role="tab"]').nth(1).click();
  await page.locator("starlight-theme-select select:visible").first().selectOption("dark");
  await page.keyboard.press("Control+k");
  await page.locator(".pagefind-ui__search-input:visible").first().fill("stack");
  await expect(page.locator(".pagefind-ui__result-link:visible").first()).toBeVisible();
  await gotoLoaded(page, "/guides/security/");

  expect(await context.cookies()).toEqual([]);

  const storage = await page.evaluate(() => ({
    local: Object.keys(localStorage),
    session: Object.keys(sessionStorage),
  }));
  expect(storage.local.filter((key) => !(DISCLOSED_STORAGE.local as readonly string[]).includes(key))).toEqual([]);
  expect(storage.session.filter((key) => !(DISCLOSED_STORAGE.session as readonly string[]).includes(key))).toEqual([]);
  expect(storage.local).toEqual(expect.arrayContaining([...DISCLOSED_STORAGE.local]));

  const allowed = new Set<string>([siteOrigin, ...DISCLOSED_THIRD_PARTY_ORIGINS]);
  expect(requests.filter(({ origin }) => !allowed.has(origin)).map(({ url }) => url)).toEqual([]);

  const github = requests.filter(({ origin }) => origin === "https://api.github.com");
  expect(github).toHaveLength(1);
  expect(github[0]?.headers.cookie).toBeUndefined();
  expect(github[0]?.headers.authorization).toBeUndefined();
  expect(github[0]?.headers.referer).toBeUndefined();

  const scripts = await page.locator("script[src]").evaluateAll((elements) =>
    elements.map((element) => (element as HTMLScriptElement).src),
  );
  expect(scripts.filter((src) => new URL(src).origin !== siteOrigin)).toEqual([]);
  expect((await page.content()).toLowerCase()).not.toMatch(/cloudflareinsights|beacon\.min\.js|zaraz/);
});
