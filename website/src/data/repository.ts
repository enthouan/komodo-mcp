export const REPOSITORY_URL = "https://github.com/enthouan/komodo-mcp";

const GITHUB_API_ORIGIN = "https://api.github.com/";
const repositoryPath = new URL(REPOSITORY_URL).pathname;
const compactNumberFormatter = new Intl.NumberFormat("en-US", {
  notation: "compact",
  compactDisplay: "short",
  maximumFractionDigits: 1,
});

export const REPOSITORY_API_URL = new URL(
  `repos${repositoryPath}`,
  GITHUB_API_ORIGIN,
).href;

export function isRepositoryStarCount(value: unknown): value is number {
  return (
    typeof value === "number" &&
    Number.isFinite(value) &&
    Number.isSafeInteger(value) &&
    value >= 0 &&
    !Object.is(value, -0)
  );
}

export function formatRepositoryStarCount(
  starCount: number,
): string | undefined {
  return isRepositoryStarCount(starCount)
    ? compactNumberFormatter.format(starCount)
    : undefined;
}

export function formatRepositoryStarCountLabel(
  starCount: number,
): string | undefined {
  if (!isRepositoryStarCount(starCount)) return undefined;

  return `${compactNumberFormatter.format(starCount)} ${starCount === 1 ? "star" : "stars"}`;
}

/** Repository-relative links in rendered documentation point at this ref. */
export const REPOSITORY_DOCUMENTATION_REF = "main";

export function repositoryFileUrl(path: string, kind: "blob" | "tree" = "blob") {
  return `${REPOSITORY_URL}/${kind}/${REPOSITORY_DOCUMENTATION_REF}/${path.replace(/^\/+/, "")}`;
}

export const ISSUES_URL = `${REPOSITORY_URL}/issues`;
export const RELEASES_URL = `${REPOSITORY_URL}/releases`;
export const LICENSE_URL = repositoryFileUrl("LICENSE");
export const CHANGELOG_URL = repositoryFileUrl("CHANGELOG.md");
