#!/usr/bin/env python3
"""One manual tag-to-Release publisher. Validation never calls the publisher.

Requires Python 3.11+, Git, authenticated gh and the packaged release adapters.
See .agents/skills/komodo-mcp-release/SKILL.md for authorization and merge policy.
"""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time
import tomllib

import stable_evidence

from release_artifacts import (REPO, CommandError, Packages, ReleaseError, api_file,
                               hash_stream, require, verify_public_source)
SCHEMA = "komodo-release-validation/v1"
BUILD_COMMAND = ["cargo", "build", "--locked", "--release", "--bin", "komodo-mcp", "--message-format=json"]
CHECKS = [
    ["cargo", "fmt", "--all", "--", "--check"],
    ["cargo", "clippy", "--locked", "--all-targets", "--", "-D", "warnings"],
    ["cargo", "test", "--locked", "--all-targets"],
    BUILD_COMMAND,
    ["cargo", "build", "--locked", "--examples"],
    ["cargo", "run", "--locked", "--example", "stacks_on_server", "--", "./target/release/komodo-mcp"],
    ["python3", "-m", "unittest", "discover", "-s", "scripts", "-v"],
]
VERSION_COMMAND = [
    "cargo", "run", "--locked", "--example", "release_version", "--",
    "./target/release/komodo-mcp",
]
SEMVER = re.compile(
    r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
    r"(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?"
)


def version_info(version):
    match = SEMVER.fullmatch(version)
    require(match is not None, f"invalid SemVer: {version}")
    pre = match[4]
    require(not pre or all(not p.isdigit() or p == "0" or not p.startswith("0")
                          for p in pre.split(".")), f"invalid prerelease: {version}")
    return tuple(int(match[i]) for i in (1, 2, 3)), bool(pre)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def normalized_notes(notes):
    return notes.replace("\r\n", "\n").strip() + "\n"


def metadata(manifest, lockfile, changelog, version):
    _, prerelease = version_info(version)
    package = tomllib.loads(manifest)["package"]
    roots = [p for p in tomllib.loads(lockfile)["package"]
             if p["name"] == "komodo-mcp" and "source" not in p]
    require(package["name"] == "komodo-mcp" and package["version"] == version,
            f"manifest version mismatch: expected {version}, observed {package}")
    require(len(roots) == 1 and roots[0]["version"] == version,
            f"root lockfile version mismatch: expected {version}, observed {roots}")
    heading = f"## v{version}"
    sections = changelog.splitlines()
    require(sections.count(heading) == 1, f"expected exactly one {heading} changelog section")
    start = sections.index(heading) + 1
    end = next((i for i in range(start, len(sections)) if sections[i].startswith("## ")), len(sections))
    notes = normalized_notes("\n".join(sections[start:end]))
    require(notes.strip(), f"empty notes for v{version}")
    return {"version": version, "tag": f"v{version}", "prerelease": prerelease,
            "notes": notes, "notes_sha256": digest(notes.encode())}


class Git:
    def __init__(self, root):
        self.root = Path(root)

    def run(self, *args, capture=True):
        try:
            result = subprocess.run(["git", *args], cwd=self.root, text=True,
                                    capture_output=capture, timeout=120)
        except subprocess.SubprocessError as error:
            raise CommandError(f"git {' '.join(args)} did not complete: {error}") from error
        if result.returncode:
            raise CommandError(f"git {' '.join(args)} failed: {result.stderr}")
        return result.stdout.strip() if capture else None

    def source(self, path, commit=None):
        if commit:
            return self.run("show", f"{commit}:{path}")
        return (self.root / path).read_text()

    def metadata(self, version, commit=None):
        return metadata(*(self.source(p, commit) for p in
                          ("Cargo.toml", "Cargo.lock", "CHANGELOG.md")), version)

    def exact_checkout(self, commit):
        require(re.fullmatch(r"[0-9a-f]{40}", commit), "use the full captured commit SHA")
        require(self.run("rev-parse", "HEAD") == commit, f"checkout must be at {commit}")
        require(not self.run("status", "--porcelain", "--untracked-files=all"),
                "release validation/publication requires a clean checkout (use target/ for evidence)")

    def origin(self):
        url = self.run("remote", "get-url", "--push", "--all", "origin")
        fetch = self.run("remote", "get-url", "--all", "origin")
        allowed = {f"git@github.com:{REPO}.git", f"https://github.com/{REPO}.git", f"https://github.com/{REPO}"}
        # Rejected URLs may contain credentials in userinfo, queries or fragments.
        # Do not echo them into retained release logs.
        require(url in allowed and fetch in allowed,
                f"unexpected origin: expected one credential-free fetch/push URL for github.com/{REPO}")

    def accepted_main(self, commit):
        self.run("fetch", "--no-tags", "origin", "main")
        # Check the freshly fetched commit rather than relying on a fetch refspec.
        self.run("merge-base", "--is-ancestor", commit, "FETCH_HEAD")

    def check_tag(self, oid, tag, commit):
        require(self.run("cat-file", "-t", oid) == "tag",
                f"{tag}: expected annotated tag at {commit}, observed lightweight object {oid}")
        raw = self.run("cat-file", "tag", oid)
        headers, annotation = raw.split("\n\n", 1)
        observed = dict(line.split(" ", 1) for line in headers.splitlines())
        require(observed.get("object") == commit and observed.get("type") == "commit"
                and observed.get("tag") == tag and annotation.strip() == tag,
                f"{tag}: expected commit {commit}, annotation {tag!r}; observed {raw}")
        return oid

    def local_tag(self, tag, commit):
        refs = self.run("for-each-ref", "--format=%(objectname) %(refname)", f"refs/tags/{tag}")
        exact = [line.split()[0] for line in refs.splitlines() if line.endswith(f" refs/tags/{tag}")]
        return self.check_tag(exact[0], tag, commit) if exact else None

    def remote_tag(self, tag, commit):
        # A successful ls-remote with no exact ref confirms absence. Any failure stops.
        output = self.run("ls-remote", "origin", f"refs/tags/{tag}", f"refs/tags/{tag}^{{}}")
        refs = dict((ref, sha) for sha, ref in (line.split() for line in output.splitlines()))
        oid = refs.get(f"refs/tags/{tag}")
        if not oid:
            require(not refs, f"inconsistent remote refs for {tag}: {refs}")
            return None
        require(refs.get(f"refs/tags/{tag}^{{}}") == commit,
                f"{tag}: expected annotated tag targeting {commit}, observed remote refs {refs}")
        # Fetch the object without overwriting a local tag, then inspect its contents.
        self.run("fetch", "--no-tags", "origin", f"refs/tags/{tag}")
        require(self.run("rev-parse", "FETCH_HEAD") == oid, f"{tag} changed during fetch; inspect remote state")
        return self.check_tag(oid, tag, commit)

    def create_tag(self, tag, commit):
        self.run("tag", "--no-sign", "-a", tag, commit, "-m", tag)
        return self.local_tag(tag, commit)

    def push_tag(self, tag):
        self.run("push", "--no-follow-tags", "origin", f"refs/tags/{tag}:refs/tags/{tag}")


class GitHub:
    def call(self, *args):
        try:
            timeout = 1800 if args[:2] == ("release", "upload") else 120
            result = subprocess.run(["gh", *args], text=True, capture_output=True, timeout=timeout)
        except subprocess.SubprocessError as error:
            raise CommandError(f"gh {' '.join(args)} did not complete: {error}") from error
        if result.returncode:
            raise CommandError(f"gh {' '.join(args)} failed: {result.stderr}")
        return result.stdout

    def api(self, endpoint, pages=False, key=None):
        args = ["api", f"repos/{REPO}/{endpoint}".rstrip("/")]
        if pages:
            args += ["--paginate", "--slurp"]
        data = json.loads(self.call(*args))
        return [item for page in data for item in (page[key] if key else page)] if pages else data

    def preflight(self):
        repo = self.api("")
        require(repo["full_name"] == REPO and repo.get("permissions", {}).get("push"),
                "gh needs push access so draft releases and private state are visible")
        return repo["visibility"]

    def releases(self):
        return self.api("releases?per_page=100", pages=True)

    def latest(self):
        return self.api("releases/latest")["id"]

    def main_checks(self, commit):
        runs = self.api(f"actions/runs?head_sha={commit}&event=push&per_page=100", pages=True, key="workflow_runs")
        evidence = []
        for name in ("CI", "Inventory benchmark"):
            matching = [r for r in runs if r["name"] == name and r["head_branch"] == "main"
                        and r["head_sha"] == commit]
            require(matching, f"missing {name} main run for {commit}")
            latest = max(matching, key=lambda r: r["id"])
            require(latest["status"] == "completed" and latest["conclusion"] == "success",
                    f"{name} is not green for {commit}: {latest['html_url']}")
            evidence.append({"name": name, "url": latest["html_url"], "commit": commit,
                             "conclusion": latest["conclusion"]})
        return evidence

    def create(self, release, latest, *, draft=False):
        with tempfile.TemporaryDirectory(prefix="komodo-release-") as directory:
            notes = Path(directory) / "notes.md"
            notes.write_text(release["notes"])
            args = ["release", "create", release["tag"], "--repo", REPO, "--verify-tag",
                    "--title", release["tag"], "--notes-file", str(notes),
                    f"--latest={str(latest).lower()}"]
            if draft:
                args.append("--draft")
            if release["prerelease"]:
                args.append("--prerelease")
            self.call(*args)

    def publish_draft(self, release, latest):
        self.call("release", "edit", release["tag"], "--repo", REPO, "--draft=false",
                  f"--latest={str(latest).lower()}")

    def assets(self, release_id):
        return self.api(f"releases/{release_id}/assets?per_page=100", pages=True)

    def asset_hash(self, asset_id):
        with tempfile.TemporaryDirectory(prefix="komodo-asset-") as scratch:
            path = Path(scratch) / "asset"
            api_file(f"releases/assets/{asset_id}", path, binary=True)
            with path.open("rb") as stream:
                return hash_stream(stream)

    def upload(self, tag, path):
        self.call("release", "upload", tag, str(path), "--repo", REPO)


def find_release(releases, expected):
    matches = [r for r in releases if r["tag_name"] == expected["tag"]]
    require(len(matches) <= 1, f"multiple releases for {expected['tag']}")
    release = matches[0] if matches else None
    if release:
        require(release["name"] == expected["tag"]
                and release["prerelease"] is expected["prerelease"]
                and normalized_notes(release["body"] or "") == expected["notes"],
                f"release conflict: expected {expected['tag']}, prerelease={expected['prerelease']}, "
                f"notes SHA256={expected['notes_sha256']}; observed {json.dumps(release)}")
        if not packaged_version(expected["version"]):
            require(release.get("assets") == [], "unexpected release assets; inspect scope before proceeding")
    return release


def inspect(git, github, expected, commit):
    local = git.local_tag(expected["tag"], commit)
    remote = git.remote_tag(expected["tag"], commit)
    require(not (local and remote) or local == remote,
            f"tag object conflict: local {local}, remote {remote}")
    releases = github.releases()  # Includes drafts with the preflight's push access.
    release = find_release(releases, expected)
    require(not release or remote, f"release {release and release['html_url']} exists without expected remote tag")
    return local, remote, release, releases


def stable_policy(releases, expected):
    stable = [r for r in releases if not r["draft"] and not r["prerelease"]]
    newer = []
    for item in stable:
        base, pre = version_info(item["tag_name"].removeprefix("v"))
        require(not pre, f"stable release has prerelease tag: {item['tag_name']}")
        if base > version_info(expected["version"])[0]:
            newer.append(item["id"])
    return stable, newer


def verify_latest(github, expected, release, releases):
    stable, newer = stable_policy(releases, expected)
    latest_id = github.latest() if stable else None
    if expected["prerelease"]:
        require(latest_id != release["id"], "prerelease is incorrectly Latest")
    elif newer:
        require(latest_id in newer,
                f"Latest conflict: expected a newer stable release {newer}, observed {latest_id}")
    else:
        require(latest_id == release["id"],
                f"Latest conflict: expected newest stable release {release['id']}, observed {latest_id}")
    return latest_id


def packaged_version(version):
    return version_info(version)[0] >= (0, 8, 0)


def reconcile(git, github, expected, commit, publish=False, sleep=time.sleep):
    """Inspect before any write. Lost write responses get reads, never a write retry."""
    require(not packaged_version(expected["version"]), "packaged releases require verified distribution candidates")
    visibility = github.preflight()
    git.accepted_main(commit)
    local, remote, release, releases = inspect(git, github, expected, commit)
    if not publish:
        require(remote and release and not release["draft"] and release["published_at"],
                "publication is incomplete; verify does not publish")
    elif not (release and not release["draft"]):
        if not remote:
            local = local or git.create_tag(expected["tag"], commit)
            write_error = None
            try:
                git.push_tag(expected["tag"])
            except ReleaseError as error:
                write_error = error
            # A failed push response does not establish whether the server accepted it.
            remote = read_back(lambda: git.remote_tag(expected["tag"], commit), sleep, write_error)
            require(remote == local, f"tag push not verified; original error: {write_error}")
        # Re-read all state after tag publication and immediately before release creation.
        _, remote, release, releases = inspect(git, github, expected, commit)
        stable, newer = stable_policy(releases, expected)
        latest = not expected["prerelease"] and not newer
        previous_latest = github.latest() if stable else None
        if not (release and not release["draft"]):
            write_error = None
            try:
                if release:
                    github.publish_draft(expected, latest)
                else:
                    github.create(expected, latest)
            except ReleaseError as error:
                write_error = error
            def published():
                observed = find_release(github.releases(), expected)
                return observed if observed and not observed["draft"] and observed["published_at"] else None
            release = read_back(published, sleep, write_error)
            require(release, f"release publication not verified; original error: {write_error}")
            if latest:
                require(github.latest() == release["id"], "published stable release is not Latest")
            elif previous_latest is not None:
                require(github.latest() == previous_latest, "recovery changed the existing Latest release")
    # Independent final remote reads, including visibility. target_commitish is not identity evidence.
    _, final_tag, release, releases = inspect(git, github, expected, commit)
    require(final_tag == remote and release and not release["draft"] and release["published_at"],
            "final remote publication identity is incomplete or changed")
    latest_id = verify_latest(github, expected, release, releases)
    require(github.preflight() == visibility, "repository visibility changed during publication")
    return {"repository": REPO, "tag": expected["tag"], "tag_object": final_tag,
            "annotation": expected["tag"], "commit": commit, "release_url": release["html_url"],
            "release_id": release["id"], "latest_release_id": latest_id, "prerelease": expected["prerelease"],
            "notes_sha256": expected["notes_sha256"], "visibility": visibility,
            "verified_at": datetime.now(timezone.utc).isoformat()}


def read_back(read, sleep, write_error=None):
    errors = []
    for attempt in range(4):
        try:
            result = read()
            if result:
                return result
        except CommandError as error:
            errors.append(str(error))
        if attempt < 3:
            sleep(attempt + 1)
    raise ReleaseError(f"bounded read-back could not verify publication; original write error: {write_error}; "
                       f"inspect before rerun: {errors}")


def reconcile_packages(git, github, expected, commit, packages, previous=None,
                       publish=False, save=lambda state: None, sleep=time.sleep):
    """Resume only from proven state. Preserve immutable asset IDs across recovery."""
    tag, version = expected["tag"], expected["version"]
    require(packaged_version(version), "historical releases cannot acquire package assets")
    require(not publish or (packages.native_dir and packages.oci_dir), "publication requires exact candidate files")
    state = dict(previous or {})
    schema = "komodo-release-publication/v3" if stable_evidence.required(version) else "komodo-release-publication/v2"
    identity = {"schema": schema, "repository": REPO,
                "tag": tag, "commit": commit, "notes_sha256": expected["notes_sha256"],
                "package_identity": packages.identity}
    require(not state or all(state.get(k) == value for k, value in identity.items()),
            "saved release evidence conflicts with this candidate; recover the original run and commit")
    state.update(identity)
    visibility = github.preflight()
    require(visibility == "public", "packaged release requires a publicly accessible source repository")
    git.accepted_main(commit)
    expected_names = {f["name"] for f in packages.files}
    source_name = next(f["name"] for f in packages.identity["distribution"]["files"] if f["kind"] == "source")
    receipt = packages.identity["container"]

    def observe():
        local, remote, release, releases = inspect(git, github, expected, commit)
        require(not state.get("tag_object") or state["tag_object"] == remote,
                "original remote tag object is missing or changed")
        require(not state.get("release_id") or (release and state["release_id"] == release["id"]),
                "original Release ID is missing or changed")
        require(not state.get("published_at") or (release and release["published_at"] == state["published_at"]
                and not release["draft"]), "published Release changed publication state")
        assets = packages.inventory(github, release, version, state.get("assets", {})) if release else {}
        if remote:
            state["tag_object"] = remote
        if release:
            state.update(release_id=release["id"], release_url=release["html_url"], assets=assets)
            if not release["draft"]:
                require(release["published_at"], "published Release has no publication timestamp")
                state["published_at"] = release["published_at"]
                state["latest_release_id"] = verify_latest(github, expected, release, releases)
        if publish:
            save(state)
        return local, remote, release, releases

    def write_once(write, read):
        error = None
        try:
            write()
        except CommandError as exc:
            error = exc
        return read_back(read, sleep, error)

    local, remote, item, releases = observe()
    # Inspect both services before changing either. An existing OCI version needs
    # a published, verified source predecessor even when the image identity matches.
    def image_state():
        image = packages.registry.read_published(receipt=receipt)
        if state.get("authenticated_image"):
            require(state["authenticated_image"] == image, "previously observed registry tag disappeared or changed")
        if image:
            state["authenticated_image"] = image
            if publish:
                save(state)
        return image

    image = image_state()
    require(image is not None or not state.get("published_image"), "previously published registry tag disappeared")
    if item is None or item["draft"]:
        _, newer = stable_policy(releases, expected)
        if newer and not expected["prerelease"]:
            require(github.latest() in newer, "Latest conflict before recovering an older stable release")
    if image is not None:
        require(item and not item["draft"] and source_name in state["assets"],
                "registry version exists without its published Corresponding Source")
        verify_public_source(state["assets"][source_name])
    if not publish:
        require(remote and item and not item["draft"] and expected_names == state["assets"].keys()
                and image is not None, "packaged publication is incomplete; verify does not publish")
    if publish and not remote:
        local = local or git.create_tag(tag, commit)
        seen = write_once(lambda: git.push_tag(tag), lambda: git.remote_tag(tag, commit))
        require(seen == local, "remote tag object conflicts with the original local tag")
        _, remote, item, releases = observe()
    if publish and not item:
        item = write_once(lambda: github.create(expected, False, draft=True), lambda: observe()[2])
    if publish:
        for name in sorted(expected_names - state["assets"].keys()):
            _, _, item, _ = observe()
            if name in state["assets"]:
                continue
            require(item["draft"] or item.get("immutable") is False,
                    "cannot append missing assets to an immutable or unconfirmed published Release")
            write_once(lambda: github.upload(tag, packages.upload_path(name)),
                       lambda: observe()[2] and state["assets"].get(name))
        _, _, item, releases = observe()
        require(expected_names == state["assets"].keys(), "incomplete release asset set")
        if item["draft"]:
            stable, newer = stable_policy(releases, expected)
            previous_latest = github.latest() if stable else None
            latest = not expected["prerelease"] and not newer
            require(not newer or previous_latest in newer or expected["prerelease"],
                    "Latest conflict before publishing older stable release")
            write_once(lambda: github.publish_draft(expected, latest),
                       lambda: observe()[2] and state.get("published_at"))
            if not latest and previous_latest is not None:
                require(github.latest() == previous_latest, "draft publication changed the existing Latest release")

    def published_source():
        _, _, item, _ = observe()
        require(item and not item["draft"] and expected_names == state["assets"].keys(),
                "public Release or its complete asset set is missing")
        if stable_evidence.required(version):
            state["verified_provenance"] = packages.verify_stable_downloads(state["assets"], version, commit)
        else:
            verify_public_source(state["assets"][source_name])

    published_source()
    if publish and image is None:
        if image_state() is None:
            packages.registry.publish_candidate(packages.oci_dir, receipt=receipt)
            require(image_state(), "promoted image is missing from the authenticated registry")
    public_image = packages.registry.read_published(receipt=receipt, anonymous=True)
    require(public_image, "released image and platform descriptors are not anonymously verifiable")
    require(not state.get("published_image") or state["published_image"] == public_image,
            "published registry identities differ from the original receipt")
    # Verify that the same tag, Release, assets, Latest and visibility still hold.
    observe()
    require(github.preflight() == visibility, "repository visibility changed during publication")
    state.update(annotation=tag, prerelease=expected["prerelease"], visibility=visibility,
                 published_image=public_image, verified_at=datetime.now(timezone.utc).isoformat())
    if publish:
        save(state)
    return state


def validation(git, version, commit, output):
    git.exact_checkout(commit)
    require(not any(os.environ.get(name) for name in ("CARGO_TARGET_DIR", "CARGO_BUILD_TARGET")),
            "unset CARGO_TARGET_DIR/CARGO_BUILD_TARGET for the documented release executable layout")
    expected = git.metadata(version, commit)
    checks = []
    for command in CHECKS:
        result = subprocess.run(command, cwd=git.root, check=True, text=True,
                                stdout=subprocess.PIPE, timeout=900)
        if command == BUILD_COMMAND:
            artifacts = [json.loads(line) for line in result.stdout.splitlines()]
            executables = [Path(a["executable"]).resolve() for a in artifacts
                           if a.get("reason") == "compiler-artifact" and a.get("executable")
                           and a["target"]["name"] == "komodo-mcp" and "bin" in a["target"]["kind"]]
            require(executables == [(git.root / "target/release/komodo-mcp").resolve()],
                    f"Cargo built a different executable: {executables}; use the documented default target layout")
        elif result.stdout:
            print(result.stdout, file=sys.stderr, end="")
        checks.append({"command": command, "result": "passed"})
    command = VERSION_COMMAND + [version]
    result = subprocess.run(command, cwd=git.root, check=True, text=True,
                            stdout=subprocess.PIPE, timeout=180)
    runtime = json.loads(result.stdout)
    executable = git.root / "target/release/komodo-mcp"
    require(runtime["observed_version"] == version and runtime["child_exit_success"]
            and not runtime["forced_kill"] and runtime["mock_stopped"], "runtime verification failed")
    git.exact_checkout(commit)
    evidence = {"schema": SCHEMA, "repository": REPO, "commit": commit, "version": version,
                "notes_sha256": expected["notes_sha256"], "checks": checks,
                "version_command": command, "runtime": runtime,
                "executable_sha256": digest(executable.read_bytes()),
                "validated_at": datetime.now(timezone.utc).isoformat()}
    write_json(output, evidence)
    return evidence


def require_evidence(git, expected, commit, path):
    evidence = json.loads(path.read_text())
    require(evidence["schema"] == SCHEMA and evidence["repository"] == REPO
            and evidence["commit"] == commit and evidence["version"] == expected["version"]
            and evidence["notes_sha256"] == expected["notes_sha256"]
            and evidence["checks"] == [{"command": c, "result": "passed"} for c in CHECKS]
            and evidence["version_command"] == VERSION_COMMAND + [expected["version"]]
            and evidence["runtime"]["observed_version"] == expected["version"]
            and evidence["runtime"]["child_exit_success"] is True
            and evidence["runtime"]["forced_kill"] is False
            and evidence["runtime"]["mock_stopped"] is True
            and evidence["executable_sha256"] == digest((git.root / "target/release/komodo-mcp").read_bytes()),
            "validation evidence does not match the commit, notes, checks or release executable; revalidate")
    return evidence


def write_json(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, delete=False) as output:
        json.dump(data, output, indent=2)
        output.write("\n")
    os.replace(output.name, path)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("metadata", "validate", "publish", "verify", "mirror"))
    parser.add_argument("--version", required=True, help="application SemVer without v")
    parser.add_argument("--commit", help="full validated release commit SHA; never a moving ref")
    parser.add_argument("--evidence", type=Path, help="validation JSON, required for validate/publish")
    parser.add_argument("--output", type=Path, help="write publication verification JSON")
    parser.add_argument("--native-run", type=int, help="successful Native distribution main Actions run ID")
    parser.add_argument("--oci-run", type=int, help="successful Container distribution main Actions run ID")
    parser.add_argument("--receipt", type=Path, help="archived packaged publication JSON for independent verification")
    parser.add_argument("--notes-file", type=Path, help="export reviewed changelog section")
    parser.add_argument("--authorized", action="store_true", help="assert existing user authorization for publication")
    args = parser.parse_args(argv)
    git = Git(Path.cwd())
    expected = git.metadata(args.version, args.commit)
    if args.notes_file:
        args.notes_file.parent.mkdir(parents=True, exist_ok=True)
        args.notes_file.write_text(expected["notes"])
    if args.mode == "metadata":
        print(json.dumps(expected, indent=2))
        return
    require(args.commit, "--commit is required")
    require(re.fullmatch(r"[0-9a-f]{40}", args.commit), "--commit must be a full SHA")
    if args.mode == "validate":
        require(args.evidence, "--evidence is required")
        print(json.dumps(validation(git, args.version, args.commit, args.evidence), indent=2))
        return
    git.origin()
    github = GitHub()
    workflows = None
    if args.mode == "publish":
        require(args.authorized, "publication requires existing user authorization and --authorized")
        require(args.evidence, "--evidence is required")
        git.exact_checkout(args.commit)
        require_evidence(git, expected, args.commit, args.evidence)
        workflows = github.main_checks(args.commit)
    if args.mode == "mirror":
        require(version_info(args.version)[0] >= (1, 0, 0), "the Hub mirror begins at v1.0.0")
        require(args.receipt and not (args.output or args.native_run or args.oci_run or args.evidence),
                "use the existing --receipt for the mirror")
    if packaged_version(args.version):
        if args.mode == "publish":
            require(args.output and not args.receipt, "packaged publication requires its original --output path")
            previous = json.loads(args.output.read_text()) if args.output.exists() else None
            if previous:
                saved = previous["package_identity"]
                require(not args.native_run or args.native_run == saved["native_run"]["run_id"],
                        "use the original native candidate run when resuming")
                require(not args.oci_run or args.oci_run == saved["oci_run"]["run_id"],
                        "use the original OCI candidate run when resuming")
                args.native_run = saved["native_run"]["run_id"]
                args.oci_run = saved["oci_run"]["run_id"]
            if previous and previous.get("published_image") and previous.get("verified_at"):
                packages = Packages.from_receipt(saved, args.commit, args.version)
                result = reconcile_packages(git, github, expected, args.commit, packages, previous)
            else:
                require(args.native_run and args.oci_run, "supply both --native-run and --oci-run")
                with tempfile.TemporaryDirectory(prefix="komodo-candidates-") as scratch:
                    packages = Packages.download(github, Path(scratch), args.commit, args.version,
                                                 args.native_run, args.oci_run)
                    result = reconcile_packages(git, github, expected, args.commit, packages, previous, publish=True,
                                                save=lambda state: write_json(args.output, state))
        else:
            require(args.receipt, "packaged verification requires the original --receipt")
            previous = json.loads(args.receipt.read_text())
            packages = Packages.from_receipt(previous["package_identity"], args.commit, args.version)
            result = reconcile_packages(git, github, expected, args.commit, packages, previous)
    else:
        require(not (args.native_run or args.oci_run or args.receipt),
                "historical release verification uses the original tag and asset-free Release")
        result = reconcile(git, github, expected, args.commit, publish=args.mode == "publish")
    if args.mode == "mirror":
        image = result["published_image"]
        index = image["index_digest"]
        require(result["verified_provenance"]["oci"]["subjects"].get("ghcr.io/" + REPO) == index.removeprefix("sha256:"),
                "verified OCI provenance differs from the anonymous GHCR index")
        # Only the protected environment receives the Hub credential. A separate dispatch recovers the mirror.
        dispatch = github.call("workflow", "run", "docker-hub-mirror.yml", "--repo", REPO, "--ref", "main",
                               "-f", "tag=" + result["tag"], "-f", "revision=" + result["commit"],
                               "-f", "digest=" + index, "-f", "tag_object=" + result["tag_object"],
                               "-f", "publish=" + str(args.authorized).lower())
        print(dispatch.strip() or "Docker Hub mirror dispatched; inspect its preview and deployment in GitHub Actions")
        return
    if workflows:
        result["main_workflows"] = workflows
    if args.output:
        write_json(args.output, result)
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (RuntimeError, subprocess.SubprocessError, OSError, ValueError, KeyError) as error:
        print(f"release: {error}", file=sys.stderr)
        sys.exit(1)
