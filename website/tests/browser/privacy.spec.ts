import type { Request } from "@playwright/test";
import { REPOSITORY_API_URL } from "../../src/data/repository.js";
import { expect, gotoLoaded, test } from "./support.js";

const analyticsCookies = new Set([
  "cf_zaraz_client",
  "cfz_google-analytics_v4",
  "cfzs_google-analytics_v4",
]);

function isExpectedRequest(request: Request, siteOrigin: string) {
  const url = new URL(request.url());
  if (url.origin === siteOrigin || url.protocol === "data:" || url.href === REPOSITORY_API_URL) return true;
  if (url.origin === "https://static.cloudflareinsights.com") {
    return request.method() === "GET" && request.resourceType() === "script" &&
      /^\/beacon\.min\.js(?:\/|$)/.test(url.pathname);
  }
  return url.origin === "https://cloudflareinsights.com" && url.pathname === "/cdn-cgi/rum" &&
    request.method() === "POST";
}

test("the site fetches GitHub stars anonymously and uses only expected cookies and services", async ({ page, context, baseURL }) => {
  const site = new URL(baseURL!);
  // Local CI stubs GitHub. A deployment check exercises the real request.
  if (process.env.PLAYWRIGHT_BASE_URL) await page.unroute(REPOSITORY_API_URL);
  const requests: Request[] = [];
  page.on("request", (request) => requests.push(request));

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

  const unexpectedCookies = (await context.cookies()).filter(({ name, domain }) =>
    domain.replace(/^\./, "") !== site.hostname ||
    !(analyticsCookies.has(name) || /^_ga(?:_[A-Z0-9]+)?$/.test(name)),
  );
  expect(unexpectedCookies.map(({ name }) => name)).toEqual([]);

  const unexpected = requests.filter((request) => !isExpectedRequest(request, site.origin));
  expect(unexpected.map((request) => new URL(request.url()).origin)).toEqual([]);

  const github = requests.filter((request) => request.url() === REPOSITORY_API_URL);
  expect(github).toHaveLength(1);
  expect(github[0]!.method()).toBe("GET");
  const headers = await github[0]!.allHeaders();
  for (const name of ["cookie", "authorization", "referer"]) {
    expect(Object.hasOwn(headers, name), name).toBe(false);
  }
});
