# Website

The public documentation site for komodo-mcp, published at <https://komodo-mcp.com>. It is a static
[Astro](https://astro.build/) [Starlight](https://starlight.astro.build/) site that follows the same
structure, components, and interaction patterns as the trello-mcp and simplelogin-mcp websites, with
Komodo content and a green theme.

Node and pnpm are **website build dependencies only**. They are not needed to build or run the Rust
komodo-mcp executable, and nothing in this directory ships with it. The `"private": true` setting in
`package.json` prevents accidental registry publication.

## Layout

| Path | Purpose |
| --- | --- |
| `astro.config.mjs` | Static output, canonical origin, Starlight shell, sidebar, metadata |
| `src/content/docs/` | Website-authored pages (MDX) |
| `src/loaders/` | Content loaders that render the repository's `docs/*.md` and README sections |
| `src/data/` | Typed presentation data: publication and repository metadata, project facts, configuration, clients, install paths, guide map |
| `src/components/` | Footer, GitHub link, client and install tabs, code-block enhancements, repository sections |
| `src/styles/custom.css` | Theme tokens (Starlight accent variables plus `--komodo-mcp-*`) |
| `src/pages/` | `llms.txt`, `robots.txt`, and generated client icon CSS |
| `public/` | Favicon/mark, request-flow diagram, social card, Cloudflare `_headers` |
| `og-image.html`, `scripts/render-og-image.ts` | Social-card template and renderer |
| `tests/contracts/` | Vitest contracts against the built `dist/` artifact |
| `tests/browser/` | Playwright browser checks (Chromium matrix and WebKit smoke) |

## Content sources

The repository README and `docs/*.md` are the canonical documentation. The site renders them at
build time instead of keeping copies:

- Every file in `docs/` becomes `/docs/<name>/`. `src/data/operator-guides.ts` holds only its
  sidebar label, group, and description; a contract fails if a guide is added, removed, or renamed
  without updating it.
- Selected README sections listed in `src/data/repository-sections.ts` are rendered verbatim on
  website pages. The build fails if a listed heading disappears.
- Relative links are rewritten: published guides become site routes, README anchors embedded on the
  site become page anchors, and any other repository path becomes a GitHub URL on `main`. A link to a
  missing repository path fails the build.
- Versions, the toolchain, and the license come from `Cargo.toml` and `rust-toolchain.toml`.
  Environment variable names are checked against `.env.example` and `src/config.rs`, and tool names
  used in website pages are checked against the production router in `src/server.rs`.

The generated tool catalog belongs to issue #43. Until it lands, the site publishes no tool counts and
no hand-maintained tool list; it renders the README's tool contracts instead.

## Develop and check

Run from the repository root with Node 24 (`.nvmrc`) and the pinned pnpm (`packageManager`):

```sh
corepack pnpm install --frozen-lockfile
corepack pnpm --dir website exec playwright install --with-deps chromium webkit
corepack pnpm website:dev
corepack pnpm website:check
```

`website:check` verifies that the social image matches its template, runs Astro diagnostics, builds
one production artifact, then runs the Vitest output/content contracts and the Playwright browser
checks against that same artifact. Individual stages:

```sh
corepack pnpm website:og:check
corepack pnpm website:typecheck
corepack pnpm website:build
corepack pnpm website:contracts   # builds first
corepack pnpm website:test        # builds first
corepack pnpm website:preview
```

`website:contracts:run` and `website:test:run` reuse an existing `website/dist`. For design review,
`corepack pnpm website:visual` writes light/dark desktop, tablet, and mobile screenshots to
`website/artifacts/visual/`.

If a version manager shim intercepts the nested `pnpm` calls, run `corepack enable pnpm` (or put
corepack's pnpm first on `PATH`).

## Social image

`public/social-card.png` is rendered from `og-image.html` with Chromium. After changing the template
or renderer, run `corepack pnpm website:og`. `website:og:check` fails when the committed image or its
`og-image.manifest.json` no longer matches the sources.

## Theme

Colors are set centrally in `src/styles/custom.css`: Starlight's `--sl-color-accent-low`,
`--sl-color-accent`, and `--sl-color-accent-high` per theme, plus `--komodo-mcp-green` and the button
hover ring/shadow tokens. Dark mode uses neutral surfaces with emerald `#34d399` accents; light mode
uses pale green-neutral surfaces with forest `#047857` links and actions. Informational callouts use
the accent; caution and danger callouts keep their semantic colors. The favicon, request-flow diagram,
social card, and `theme-color` (`#047857`) use the same palette.

Client marks are generated at build time from the pinned Font Awesome Free packages into
`client-icons.css`, which carries the Font Awesome Free license attribution. No Font Awesome script,
font, or CDN request is made.

The workspace patches Starlight 0.43.1's search button so its accessible name is built from the visible
label and shortcut (`patches/`). The browser suite checks the resulting name; drop the patch only when
an upgraded Starlight provides the same behavior.

## Publication (Cloudflare Pages)

Every build targets the canonical origin `https://komodo-mcp.com/` with no `www`. Canonical and Open
Graph URLs, the sitemap, `robots.txt`, and absolute `llms.txt` links are emitted at build time without
any website environment variable. Repository CI builds and verifies the artifact; publication belongs
to the Cloudflare Pages Git integration.

| Setting | Value |
| --- | --- |
| Project | `komodo-mcp` (Pages, connected to `enthouan/komodo-mcp`) |
| Production branch | `main` |
| Root directory | repository root |
| Build command | `pnpm website:build` |
| Build output directory | `website/dist` |
| Environment variables | `NODE_VERSION=24.17.0`, `PNPM_VERSION=10.34.1` |
| Preview deployments | Pull-request branches (Cloudflare default) |

Cloudflare installs dependencies from `pnpm-lock.yaml` before the build command. `public/_headers`
adds conservative security headers. Cloudflare Pages [sends `X-Robots-Tag: noindex` on preview
deployments](https://developers.cloudflare.com/pages/configuration/preview-deployments/), so previews
can use the same canonical artifact without becoming duplicate search results; the production
artifact never sets a global `noindex`.

### Domain and HTTPS

The `komodo-mcp.com` zone is delegated to Cloudflare. Preserve any existing unrelated DNS records.

1. In the Pages project, add the custom domain `komodo-mcp.com`. Cloudflare creates the proxied apex
   record and issues the certificate.
2. To support `www`, add a proxied `www` DNS record and a single redirect rule
   `https://www.komodo-mcp.com/*` → `https://komodo-mcp.com/${1}` (301, preserve query string). Never
   redirect the apex to `www`.
3. Enable **Always Use HTTPS** for the zone.

### Verify a deployment

```sh
curl -sSI https://komodo-mcp.com/ | grep -Ei '^(HTTP|x-content-type-options|x-robots-tag)'
curl -sSI https://www.komodo-mcp.com/ | grep -Ei '^(HTTP|location)'
curl -sSI http://komodo-mcp.com/ | grep -Ei '^(HTTP|location)'
curl -sSI https://<preview>.komodo-mcp.pages.dev/ | grep -Ei '^x-robots-tag'
curl -sS https://komodo-mcp.com/ | grep -o '<link rel="canonical"[^>]*>'
```

Expect `200` with `X-Content-Type-Options: nosniff` and **no** `X-Robots-Tag` on the apex, a `301`
to `https://komodo-mcp.com/` from `www` and from HTTP, `X-Robots-Tag: noindex` on preview URLs, and a
canonical link on the apex origin. A green local build does not prove publication.

### Deployment troubleshooting

- **Build fails installing dependencies:** confirm `PNPM_VERSION` matches `packageManager` and the
  lockfile is committed; never switch to a non-frozen install.
- **Wrong Node version:** set `NODE_VERSION` to the `.nvmrc` value.
- **Last-updated dates missing:** the build reads Git history; Cloudflare's clone must include it.
- **GitHub links return 404 for visitors:** the source repository must be public for the GitHub,
  release, source, and license links to work.

The site makes no request to a user's Komodo Core and collects no input. The optional GitHub star count
is fetched once per browser session from the public GitHub API without credentials or a referrer, and
the repository link shows the star without a number when that request fails, including while the
repository is private.
