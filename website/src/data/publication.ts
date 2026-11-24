export const CANONICAL_WEBSITE_URL = "https://komodo-mcp.com/";

/** Advance once the release and its advertised downloads are publicly verified. */
export const PUBLISHED_RELEASE_TAG = "v0.7.0";
/** Version targeted by the installation guide, including the preview before publication. */
export const INSTALLATION_RELEASE_TAG: string = "v0.8.0";

export function checkPagesPublication(
  env: Readonly<Record<string, string | undefined>>,
  installationTag: string = INSTALLATION_RELEASE_TAG,
  publishedTag: string = PUBLISHED_RELEASE_TAG,
) {
  if (env.CF_PAGES === "1" && !env.CF_PAGES_BRANCH) {
    throw new Error("Cloudflare Pages must identify the deployment branch.");
  }
  if (env.CF_PAGES_BRANCH === "main" && installationTag !== publishedTag) {
    throw new Error(`The installation guide targets ${installationTag}, but the verified published release is ${publishedTag}.`);
  }
}

export const DISCLAIMER =
  "komodo-mcp is an independent, community-maintained project. It is not an official Komodo product and is not affiliated with, endorsed by, or sponsored by the Komodo project.";
