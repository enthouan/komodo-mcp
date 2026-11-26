#!/usr/bin/env python3
"""Preview or recover the secondary Docker Hub tags from an independently verified release."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import time

from container_release import INDEX, REPOSITORY, check, describe, digest, public_ref
from mirror_registry import DESTINATION, SOURCE, Missing, Reader, graph, repository_public, tag_digest
from release import Git, GitHub, find_release, stable_policy, verify_latest, version_info

ROOT = Path(__file__).resolve().parents[1]


def readback(operation, inspect, sleep=time.sleep):
    """One mutation, followed only by reads, even when the mutation's response is lost."""
    try:
        operation()
    except (OSError, subprocess.SubprocessError):
        pass
    for attempt in range(4):
        try:
            return inspect()
        except (Missing, OSError, RuntimeError, subprocess.SubprocessError):
            if attempt == 3:
                raise
            sleep(2 ** attempt)


def equivalent(source, target):
    if source == target:
        return
    for key, attached in target["referrers"].items():
        check(key in source["referrers"] and set(attached) <= set(source["referrers"][key]),
              "unexpected destination referrer; inspect both registries before recovery")
    if any(set(links) - set(target["referrers"].get(key, [])) for key, links in source["referrers"].items()):
        raise Missing("Docker Hub is missing source referrers")
    raise ValueError("destination graph differs from GHCR")


class Mirror:
    def __init__(self, args, directory):
        self.version, self.revision, self.identifier = args.tag[1:], args.revision, args.digest
        self.hub_tag = public_ref(self.version).rsplit(":", 1)[1][1:]
        check(version_info(self.version)[0] >= (1, 0, 0), "0.x backfill is outside the stable mirror")
        check(re.fullmatch(r"[a-f0-9]{40}", self.revision) and digest(self.identifier), "invalid source identity")
        self.git, self.github = Git(ROOT), GitHub()
        self.directory = directory
        self.env = {}
        for role in ("anonymous", "writer"):
            path = directory / (role + ".json")
            path.write_text(json.dumps({"incDockerCred": False, "incDockerCert": False}))
            self.env[role] = {"PATH": os.environ["PATH"], "REGCTL_CONFIG": str(path),
                              "HOME": str(directory), "DOCKER_CONFIG": str(directory)}
        self.source = Reader(SOURCE, self.env["anonymous"])
        self.hub = Reader(DESTINATION, self.env["anonymous"])
        self.tag_object = args.tag_object

    def release_state(self):
        expected = self.git.metadata(self.version, self.revision)
        self.git.accepted_main(self.revision)
        oid = self.git.remote_tag(expected["tag"], self.revision)
        check(oid and (not self.tag_object or oid == self.tag_object), "annotated release tag is missing or changed")
        self.tag_object = oid
        releases = self.github.releases()
        release = find_release(releases, expected)
        check(release and not release["draft"] and release["published_at"], "expected a published release")
        latest_id = verify_latest(self.github, expected, release, releases)
        stable, newer = stable_policy(releases, expected)
        eligible = not expected["prerelease"] and not newer and latest_id == release["id"]
        return eligible, stable

    def download_evidence(self):
        self.github.call("release", "download", "v" + self.version, "--repo", REPOSITORY,
                         "--dir", str(self.directory), "--pattern", "oci.sbom.cdx.json",
                         "--pattern", "oci.provenance.sigstore.json")

    def verify_evidence(self, identity):
        subprocess.run([sys.executable, str(ROOT / "scripts/stable_evidence.py"), "verify-oci",
                        "--directory", str(self.directory), "--version", self.version, "--revision", self.revision,
                        "--index-digest", identity["index_digest"], "--amd64", identity["platforms"]["linux/amd64"],
                        "--arm64", identity["platforms"]["linux/arm64"]], check=True)

    def check_first_version(self):
        if self.version != "1.0.0":
            baseline = tag_digest(self.hub, "1.0.0")
            check(baseline and baseline == tag_digest(self.source, "v1.0.0"),
                  "first complete the verified v1.0.0 mirror before later versions")

    def source_graph(self):
        check(tag_digest(self.source, "v" + self.hub_tag) == self.identifier,
              "GHCR version differs from the verified release digest")
        identity = graph(self.source, self.identifier, self.version, self.revision)
        self.verify_evidence(identity)
        return identity

    def exact(self):
        current = tag_digest(self.hub, self.hub_tag)
        check(current is None or current == self.identifier, "conflicting immutable Docker Hub version digest")
        return current

    def checked_destination(self, source):
        if not self.exact():
            raise Missing("Docker Hub exact tag has not appeared")
        identity = graph(self.hub, self.identifier, self.version, self.revision)
        equivalent(source, identity)
        return identity

    def latest(self):
        eligible, stable = self.release_state()
        current = tag_digest(self.hub, "latest")
        if not eligible or current == self.identifier:
            return current, False
        if current is not None:
            # A mismatched alias can only move forward from another verified, older version pin.
            target = version_info(self.version)[0]
            older = (r["tag_name"][1:] for r in stable if version_info(r["tag_name"][1:])[0] < target)
            for version in older:
                tag = public_ref(version).rsplit(":", 1)[1]
                if tag_digest(self.source, tag) == current and tag_digest(self.hub, tag[1:]) == current:
                    break
            else:
                raise ValueError("latest does not identify a known older mirrored release")
        return current, True

    def writer(self, *args, data=None):
        subprocess.run(["regctl", *args], env=self.env["writer"], input=data, check=True,
                       stderr=subprocess.DEVNULL, stdout=subprocess.DEVNULL, timeout=900)

    def login(self):
        token = os.environ.get("DOCKERHUB_TOKEN")
        check(token, "configure the dedicated dockerhub environment token")
        account = os.environ.get("DOCKERHUB_USERNAME") or "enthouan"
        check(re.fullmatch("[a-z0-9][a-z0-9_-]{0,63}", account), "invalid dockerhub environment username")
        self.writer("registry", "login", "docker.io", "--user", account, "--pass-stdin", data=token.encode())

    def copy(self):
        self.writer("image", "copy", "--force-recursive", "--referrers", SOURCE + "@" + self.identifier,
                    DESTINATION + ":" + self.hub_tag)

    def put_latest(self, raw):
        self.writer("manifest", "put", DESTINATION + ":latest", "--content-type", INDEX, data=raw)


def reconcile(mirror, write):
    mirror.check_first_version()
    source = mirror.source_graph()
    exact = mirror.exact()
    try:
        identity = mirror.checked_destination(source)
        copy = False
    except Missing:
        copy = True
    latest, advance = mirror.latest()
    plan = {"tag": "v" + mirror.version, "revision": mirror.revision, "tag_object": mirror.tag_object,
            "source": SOURCE, "verified_source": source, "destination": DESTINATION, "exact_tag": mirror.hub_tag,
            "current_exact": exact, "current_latest": latest, "copy_or_repair_exact": copy, "advance_latest": advance}
    print(json.dumps(plan, indent=2), flush=True)
    if not write:
        return plan
    if copy or advance:
        mirror.login()
    if copy:
        mirror.release_state()
        try:
            identity = mirror.checked_destination(source)
        except Missing:
            identity = readback(mirror.copy, lambda: mirror.checked_destination(source))
    mirror.verify_evidence(identity)
    raw = mirror.hub.manifest(mirror.hub_tag)
    check(describe(raw) == mirror.identifier, "immutable tag changed before latest decision")
    current, advance_now = mirror.latest()
    if advance_now:
        check(advance and current == latest, "latest state changed; preview the mirror again")

        def checked_latest():
            observed = tag_digest(mirror.hub, "latest")
            if observed == current:
                raise Missing("latest update has not appeared")
            check(observed == mirror.identifier, "unexpected latest digest after write")
            return observed

        current = readback(lambda: mirror.put_latest(raw), checked_latest)
    print(json.dumps({"verified_hub": identity, "latest": current, "exact_tag": mirror.hub_tag}, indent=2))
    return plan


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=["preview", "write"])
    parser.add_argument("--tag", required=True)
    parser.add_argument("--revision", required=True)
    parser.add_argument("--digest", required=True)
    parser.add_argument("--tag-object", help="original annotated tag object from the preview")
    args = parser.parse_args()
    check(args.tag.startswith("v"), "expected a v-prefixed release tag")
    if args.mode == "write":
        check(args.tag_object and re.fullmatch("[0-9a-f]{40}", args.tag_object), "writer requires preview tag object")
    with tempfile.TemporaryDirectory(prefix="komodo-hub-") as directory:
        mirror = Mirror(args, Path(directory))
        mirror.release_state()
        repository_public()
        mirror.download_evidence()
        plan = reconcile(mirror, args.mode == "write")
        if args.mode == "preview" and os.environ.get("GITHUB_OUTPUT"):
            with open(os.environ["GITHUB_OUTPUT"], "a") as out:
                print("tag_object=" + plan["tag_object"], file=out)


if __name__ == "__main__":
    main()
