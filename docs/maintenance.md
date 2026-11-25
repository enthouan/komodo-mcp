# Repository maintenance

## Dependency updates

[Dependabot](../.github/dependabot.yml) checks Cargo on Mondays at 09:00 and
GitHub Actions at 09:30, in `America/Los_Angeles` (including daylight saving).
Cargo covers the root `Cargo.toml` and `Cargo.lock`; GitHub Actions covers the
workflow files under `.github/workflows`. The npm ecosystem checks the website's
pnpm workspace (`package.json`, `website/package.json`, `pnpm-lock.yaml`) on
Mondays at 10:00. Those packages build only the static website in `website/`;
the Rust executable never depends on Node.

Minor and patch updates are grouped by ecosystem. Major updates remain separate.
Cargo allows five open version-update PRs; Actions and npm allow three each. Security
updates use GitHub's separate limits. No dependency PR is automatically merged.

The exact `komodo_client` and `rmcp` pins in `Cargo.toml` are excluded from the
compatible-update group so proposed SDK changes receive their own review.
Before merging a Core SDK update, inspect that version's authoritative read API,
update compatibility/permission fixtures, and rerun the real stdio tests. An MCP
SDK change also needs lifecycle, tool-schema, and subprocess verification.

`dtolnay/rust-toolchain` updates are separate. Both workflows pin an action
commit from its input-capable `master` branch and explicitly select Rust 1.99.1
with `toolchain`. Version-named action revisions hard-code their compiler and
can name Rust versions that have not been released. Review action implementation
updates independently; deliberate compiler updates must keep both workflows,
`rust-toolchain.toml`, `.tool-versions`, and the documented compiler aligned.
A 0.x minor dependency update can change compatibility and still requires review.

The benchmark reference is an external checkout pinned in
[`benchmarks/bench.py`](../benchmarks/bench.py), not a local npm dependency.
Changing its release/commit requires explicit comparator review, corresponding
oracle updates, and fresh measurements with exact source/executable identities.
Keep raw reports in the ignored `target/benchmark/` directory and publish a
short summary of the findings. [Benchmark reproduction](../benchmarks/README.md#reproduce)

The separate `Website` workflow builds the site and runs its contracts and
browser checks; it is reported on every pull request but is not one of the
required checks below. Its failures still block a ready-to-merge assessment.
Website deployment belongs to Cloudflare Pages, not CI; see `website/README.md`.

Both required workflows use `pull_request`, `contents: read`, no Actions secrets,
and synthetic Core credentials. There is no actor filter or privileged
`pull_request_target` path. The public benchmark reference is fetched without
private registry credentials. Dependabot PRs therefore use the same jobs with
their restricted token. Record actual ordinary and dependency-PR run URLs in
the maintenance issue; a workflow run alone does not prove branch enforcement.

## Protecting main

The reviewed target policy is
[`.github/main-protection.json`](../.github/main-protection.json). GitHub does
not apply that file automatically. An authorized administrator applies it with:

```sh
gh api --method PUT repos/enthouan/komodo-mcp/branches/main/protection \
  --input .github/main-protection.json
gh api repos/enthouan/komodo-mcp/branches/main/protection
gh api repos/enthouan/komodo-mcp/rulesets --paginate
```

Read existing rules before applying; reconcile any intervening policy changes.
Require PRs, branches current with main, passing `check` and `benchmark-smoke`
checks from GitHub Actions app `15368`, and resolved review conversations.
These names come from actual check runs: `CI / check` and
`Inventory benchmark / benchmark-smoke`. Workflow titles remain unchanged for
the release helper's independent main-run verification.

The policy applies to administrators, restricts force pushes and deletion, and
has no bypass allowance. Required approving reviews are zero so the solo
maintainer can merge their own passing PR. Code-owner and last-pusher approval
requirements are disabled. Requested code/security reviews still must finish,
and actionable findings must be resolved before an authorized merge.

Issue #64 records the active API read-back, required-check identities, ordinary
and Dependabot workflow evidence, and any access/platform limitation. A proposed
policy file is not evidence that hosted protection is active.

## Release compatibility

The [v1 MCP tool contract](contracts/README.md) records the running server’s
public surface, profile visibility and representative responses. It includes
the drift check and the review procedure for intentional changes.

Release PRs use the same normal, SHA-guarded squash merge after checks and
reviews pass, without an administrator bypass. A separate explicit approval
can be required before creating the release PR; honor that checkpoint.
The branch rule targets `main`, so the existing authorized annotated-tag
publisher can push `refs/tags/vX.Y.Z` after merged-commit validation and main CI.
Recheck tag rules and publication permissions when that work is authorized.
CI remains read-only and does not publish. Follow the
[release skill](../.agents/skills/komodo-mcp-release/SKILL.md).

References: GitHub's [Dependabot options](https://docs.github.com/en/code-security/reference/supply-chain-security/dependabot-options-reference),
[Dependabot Actions restrictions](https://docs.github.com/en/code-security/reference/supply-chain-security/dependabot-on-actions),
and [protected branches](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-protected-branches/about-protected-branches).
