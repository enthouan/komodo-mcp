---
name: komodo-mcp-release
description: Prepare, publish, verify, or recover milestone and patch releases of enthouan/komodo-mcp, including Cargo/MCP version alignment, release PR review, exact merged-commit validation, annotated tags, GitHub Releases, and tracking closure.
---

# komodo-mcp release

## Scope and authorization

Current artifacts are an annotated Git tag and a GitHub Release titled exactly
`vX.Y.Z`, with notes from `CHANGELOG.md`. The authoritative publisher is
`python3 scripts/release.py publish`; there is no automatic release workflow.
CI is read-only and cannot publish. Do not introduce a second publisher. Run
one authorized publication sequence at a time.

Honor the user's existing authorization for the exact release:

- Readiness only or an earlier user-defined checkpoint: complete only the
  authorized implementation/readiness work. When approval is required before
  creating the release PR, stop and wait at that point.
- Preparation only: stop at an open, non-draft, reviewed, merge-ready PR.
- Merge only: complete the authorized merge and validation, then stop before
  tag push or Release publication.
- Complete release authorization: continue through merge, exact-commit
  validation, tag push, Release publication, independent verification, evidence,
  and closure of completed tracking. Do not add another normal approval step.

Passing checks or automated review does not grant authorization. History
rewrites, moving/deleting published tags, and replacing conflicting published
state require separate explicit authorization. Preserve repository visibility.

When user feedback establishes a reusable release convention, update the
relevant instruction in this skill during the same task. Keep authorization
for a particular merge or publication in that session's evidence.

## Tools and initial inspection

Use Git, Python 3.11+ (standard library only), the repository's pinned Rust
with Cargo/rustfmt/Clippy, and authenticated GitHub CLI `gh`. Preparation needs
branch/PR/comment write and Actions read access. Publication needs Git tag push
and GitHub Contents write access, including visibility of draft releases.
Tracking closure needs Issues/Projects write access. Node/npm are needed for
the existing benchmark, not the MCP version verifier. Never print credentials.

Read the release issue and comments and repository instructions. Preserve
unrelated work; use an isolated worktree when needed. Refresh live state:

```sh
git status --short --branch
git remote -v
git fetch --no-tags origin
git log -8 --oneline origin/main
git ls-remote --heads --tags origin
gh issue view RELEASE_ISSUE --repo enthouan/komodo-mcp --comments
gh issue list --repo enthouan/komodo-mcp --state all --limit 300 \
  --json number,title,state,milestone,projectItems,url
gh pr list --repo enthouan/komodo-mcp --state open --json number,title,url,headRefName,isDraft
gh api 'repos/enthouan/komodo-mcp/milestones?state=all' --paginate
gh run list --repo enthouan/komodo-mcp --branch main --limit 20 \
  --json name,headSha,status,conclusion,url
gh api repos/enthouan/komodo-mcp/releases --paginate
gh api repos/enthouan/komodo-mcp/branches/main
gh api repos/enthouan/komodo-mcp/branches/main/protection
gh api repos/enthouan/komodo-mcp/rulesets --paginate
```

Inspect Project membership **and field values** through GraphQL or `gh project`.
Check existing implementations, branches, PRs, tags, releases and drafts before
creating overlapping work. An explicit `Branch not protected` 404, combined
with the branch response and rulesets, differs from inaccessible repository or
authentication state. Never copy another repository's protections, labels,
publishing identities, or approval policy. Recheck rules before merge.

The reviewed main policy is documented in `docs/maintenance.md` and
`.github/main-protection.json`; verify hosted read-back rather than assuming the
file is active. It requires PRs, up-to-date branches, resolved conversations,
and GitHub Actions app `15368` checks named `check` and `benchmark-smoke`.
These are the jobs in **CI** and **Inventory benchmark**, whose workflow titles
remain the publisher's main-run identities. The policy includes administrators
and provides no bypass; never use `--admin` to land a release.
Zero required approving reviews supports the solo maintainer's own PRs;
requested code/security reviews still must complete and findings be resolved.
The branch rule applies to `main`; recheck tag rules before authorized tag
publication. Normal reviewed squash merges and the existing manual tag
publisher remain the release path.

For v0.1.0, #1–#5 must be complete. Release tracking issue #58 stays open until
publication is verified and is intentionally exempt from the prerequisite
issue-closure gate. Do not require zero open milestone issues before publishing.

## Version and reviewed notes

Select the version from shipped changes, previous verified releases and SemVer
impact. v0.1.0 is the first release even though Cargo already says `0.1.0`; do
not bump solely because metadata has that version.

Align only `[package].version` in `Cargo.toml` and the source-less root
`komodo-mcp` package in `Cargo.lock`. Preserve unrelated dependency resolutions.
Keep runtime derivation from `CARGO_PKG_VERSION` in `src/server.rs`. The MCP
protocol, `komodo_client`, and observed/fixture Core versions are independent.
Tag and Release title are `v` plus the exact application version. SemVer
prereleases (for example `0.2.0-rc.1`) use `--prerelease` and never become Latest;
build metadata alone does not imply prerelease.

Keep `## Unreleased` above the latest versioned section in `CHANGELOG.md`.
Prepare the release cut by moving its reviewed changes into an exact
`## vX.Y.Z` section and leaving `Unreleased` for subsequent work. If the
versioned notes already exist, establish this boundary without rewriting the
reviewed release scope. Publication dates and commit identities belong in the
GitHub Release and tracking evidence.

Write concise, user-facing release notes. Start directly with applicable
`### Added`, `### Changed` or `### Fixed` headings; omit empty categories. Use a
handful of bullets, each one short sentence about what users can do or what
changed. Group related work. End each bullet with PR links verified against
merged history; issue links do not replace PR links.

Example using shipped komodo-mcp changes:

```markdown
### Added

- Export selected resources and groups as sanitized, inspection-only TOML. ([#105](https://github.com/enthouan/komodo-mcp/pull/105))
- Inspect, refresh and review scoped ResourceSync pending changes before applying them. ([#107](https://github.com/enthouan/komodo-mcp/pull/107))
```

Keep validation commands/results, hashes, fixture/live coverage, permission and
recovery details, benchmark caveats, distribution boilerplate and deferred work
in the relevant docs and release-tracking evidence. Include a brief breaking
change or required migration when it affects users. Apply this format as changes
are added under `Unreleased`, rather than accumulating implementation reports.

For the first release, explicitly summarize shipped scope from history and
completed issues; do not invent a previous tag. Later releases use the previous
**verified published** release's annotated tag and target through the intended
candidate:

```sh
git log --oneline PREVIOUS_VERIFIED_TAG..CANDIDATE_SHA
git diff --stat PREVIOUS_VERIFIED_TAG..CANDIDATE_SHA
python3 scripts/release.py metadata --version X.Y.Z --notes-file target/release-notes.md
```

Review user-facing changes and PR links in the notes, and evidence limits and
distributed artifacts in the supporting docs/tracker. No crates.io, MCP Registry,
GHCR, binary archive, SBOM or provenance publisher is in current scope.
#41 (multi-architecture GHCR/Compose), #42 (native binaries/matching
Corresponding Source), and #51 (stable artifact verification, SBOMs/provenance)
extend this same process when
implemented; they are not current gates. Update the helper, skill, tests and
artifact verification together as those capabilities land.

## Release PR and completed review

Create a dedicated release preparation commit using the changelog cut, even
when Cargo already has the target version or a general tooling PR added the
notes earlier. Reuse an existing correct release-cut commit; do not manufacture
an empty duplicate.

Use `antoine/release-vX.Y.Z` from fresh `origin/main`, keeping related release
work in one PR. The release preparation commit, PR title, and final squash
commit subject are exactly the release name `vX.Y.Z`. This repository convention
takes precedence over ordinary imperative commit subjects for release cuts.
Inspect the full diff, stage only release work, and omit agent attribution.
Describe version alignment, the changelog cut, notes, artifact scope, validation,
and remaining authorization. Include release tooling or skill changes when they
are part of the diff. Use `Tracks #58` (or the applicable tracking issue), never
a closing reference before verified publication.

Run the complete local gate on the exact clean candidate commit:

```sh
release_sha="$(git rev-parse HEAD)"
python3 scripts/release.py validate --version X.Y.Z --commit "$release_sha" \
  --evidence target/release-validation.json
```

This runs:

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
cargo build --locked --release --bin komodo-mcp
cargo build --locked --examples
cargo run --locked --example stacks_on_server -- ./target/release/komodo-mcp
python3 -m unittest discover -s scripts -v
cargo run --locked --example release_version -- ./target/release/komodo-mcp X.Y.Z
```

The final command launches the selected executable with the official rmcp
client and synthetic mock Core, initializes MCP, checks `serverInfo.version`
against the requested version and the computed inventory oracle (`alpha-web`,
`gamma-jobs`, `zeta-metrics`; three pages/six visible stacks), and checks clean
child/mock shutdown. It reuses bounded exchanges, concurrent stderr draining
and failure cleanup. No production `--version` flag is assumed. Evidence records
the commit, notes hash, executable SHA-256, observed version, command results
and timestamp. The helper adds `--message-format=json` to the release build to
require Cargo's actual executable output to match the path it probes; a stale
executable in another target directory cannot pass. Use default layout; remove inherited
`CARGO_TARGET_DIR`/build-target overrides or use a clean build environment.

Run `python3 -m unittest discover -s benchmarks -v` as appropriate and require
**CI** and **Inventory benchmark** on the final head. Do not regenerate baselines
unless relevant code or measured claims change. Recovery tests use temporary
Git repositories and API stubs, never disposable public releases or real tags.

Push and open/update the non-draft PR. Request Codex through the repository's
supported mechanism (currently a PR comment containing `@codex review`). Wait
for all requested, queued and running reviews, not just an eyes reaction. Inspect
submitted reviews, top-level feedback and inline threads. Fix actionable findings,
rerun affected checks, push, resolve addressed threads with evidence, and obtain
follow-up review after substantive changes. Recheck exact head/base, CI and
review state after every new head. A pending review or actionable finding blocks
merge readiness. If authorization is missing, present the concrete reviewed PR
and request only the missing authorization as the final step.

## Authorized merge and exact-commit validation

Immediately before merge, recheck rules, head/base, mergeability, approvals,
checks and threads. Merge through GitHub with an inspected head SHA guard,
respecting any required approvals/queue; never bypass rules. For permitted squash:

```sh
gh pr merge PR_NUMBER --repo enthouan/komodo-mcp --squash \
  --match-head-commit REVIEWED_HEAD_SHA --subject vX.Y.Z
gh pr view PR_NUMBER --repo enthouan/komodo-mcp --json state,mergedAt,mergeCommit,url
git fetch --no-tags origin main
```

Capture the **actual merged SHA** from the PR read-back, require ancestry in
fetched main, and confirm its commit subject is exactly `vX.Y.Z`. Do not assume
a squash/rebase preserves the PR head. Use a clean
checkout/worktree detached at that exact SHA, check metadata and notes, and
rerun `validate` above. Wait for **CI** and **Inventory benchmark** main-push
runs at that exact commit, plus any other applicable required checks/statuses.
Record run URLs/results. Preserve the captured SHA if main advances; do not
silently substitute the newer tip.

## Authoritative publication and read-back

Only with existing authorization for the exact version, from its validated checkout:

```sh
python3 scripts/release.py publish --version X.Y.Z --commit CAPTURED_MERGED_SHA \
  --evidence target/release-validation.json --authorized \
  --output target/release-publication.json
python3 scripts/release.py verify --version X.Y.Z --commit CAPTURED_MERGED_SHA \
  --output target/release-verification.json
```

`--authorized` asserts existing user authorization; it does not acquire approval.
The helper checks clean exact source, evidence/executable hash, origin identity,
green main workflows and main ancestry, and inspects remote tags and all
releases (including drafts) before writes. Its conditional publication commands:

```sh
git tag --no-sign -a vX.Y.Z CAPTURED_MERGED_SHA -m vX.Y.Z
git push --no-follow-tags origin refs/tags/vX.Y.Z:refs/tags/vX.Y.Z
gh release create vX.Y.Z --repo enthouan/komodo-mcp --verify-tag \
  --title vX.Y.Z --notes-file REVIEWED_NOTES_FILE --latest=true
```

`--no-follow-tags` prevents a maintainer's `push.followTags=true` configuration
from publishing unrelated local annotated tags. Prereleases add
`--prerelease --latest=false`. Older stable recovery selects
`--latest=false` when a newer stable release exists and verifies the previous
Latest designation was preserved. A matching published release is never edited
just to alter Latest. Every completed-state recovery and read-only verification
also checks Latest: the newest stable release must be Latest, an older release
must leave Latest with a newer stable release, and a prerelease cannot be Latest.
A mismatch is reported without editing published state. A matching draft uses `gh release edit vX.Y.Z --draft=false
--latest=...` after identity, notes and classification checks. No Actions
publisher is involved; this process does not rely on tag-trigger behavior or
`GITHUB_TOKEN` event chaining.

The helper independently reads `ls-remote` tag/peeled refs, fetches the tag
object without replacing local tags, requires type `tag`, annotation exactly
`vX.Y.Z`, a direct target equal to the captured commit, and accepted main ancestry.
It reads the Release's exact tag/title, notes, published/non-draft state,
stable/prerelease classification and publication timestamp. `target_commitish`
alone is never identity evidence. Current distribution has no uploaded binary
assets; unexpected assets require scope review.

### Authorized editorial note updates

An explicit request to revise published release notes authorizes a body-only
edit, without republishing. Save the original and approved replacement notes
and their hashes in an edit record. Update only the existing Release body, then
independently read it back and verify unchanged Release identity, tag objects,
publication state, assets and Latest designation.

The normal verifier compares notes with the original tagged changelog and will
report a notes mismatch after such an edit. Verify the approved replacement
against the edit record and current changelog instead; keep the original tag
and publication evidence, and never relax the publisher's conflict checks.

## Recovery and completion

Inspect remote state before retrying. Successful empty remote/list responses
confirm absence; authentication, authorization, rate-limit, server and transport
failures do not. The helper makes at most four read-back attempts after uncertain
writes and never blindly retries a write. Identity conflicts stop immediately.

| Observed state | Normal authorized action |
| --- | --- |
| No tag or Release | Validate, create annotated tag, push only it, verify, publish Release, read back. |
| Matching local annotated tag, unpushed | Verify and reuse its annotation, target and object; push without recreation. |
| Matching remote annotated tag, no Release | Verify/reuse its original object; publish only the missing Release. |
| Matching draft Release and remote tag | Verify identity/notes/classification; publish that draft. |
| Matching published Release and tag | Verify completed state; no publication writes. Finish evidence/tracking only. |
| Lightweight/wrong-target/wrong-annotation tag, conflicting local/remote objects or Release, or Release without expected tag | Report observed/expected identities and stop before changing anything. |

Recover the **original** commit from durable validation or the verified tag even
when main advanced. Validate that checkout again if evidence/executable is
missing, then rerun the same `publish` command. Later reviewed tooling can repair
operations while still reading metadata/notes from the original commit. Never
invent a version to bypass a conflict, force-push, recreate tags or replace
Releases. Inconclusive verification leaves tracking open with a precise next step.

After publication, run `verify` again and confirm unchanged tag object/Release
identity. Record durably in the tracking issue/release record: Release and PR
URLs, tag, annotated object SHA/annotation, exact commit, observed MCP version,
executable hash, validation commands/results, main workflow URLs, notes hash,
rerun result, fixture limits and any recovery. Copy/attach JSON evidence there;
do not change the release commit to insert its own SHA.

Only after all acceptance criteria pass, add evidence to #58, update the eventual
PR body with the exact line `Fixes #58`, and explicitly close #58. Recheck every
milestone issue; close milestone 1 only when all remaining work is complete.
Update applicable Project status to Done when accessible. Never close unrelated
work to clear a milestone. For later releases substitute their tracking issue
and milestone; a patch release need not close a milestone. Report verified
remote state, not just command success.

## Official command references

[Git annotated tags](https://git-scm.com/docs/git-tag),
[explicit tag push](https://git-scm.com/docs/git-push),
[GitHub tag objects](https://docs.github.com/en/rest/git/tags),
[GitHub refs](https://docs.github.com/en/rest/git/refs),
[Release creation flags](https://cli.github.com/manual/gh_release_create),
[draft publication flags](https://cli.github.com/manual/gh_release_edit), and
[Release API/Latest behavior](https://docs.github.com/en/rest/releases/releases).
