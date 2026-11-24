"""Acquire the exact standalone Native distribution artifact for a container candidate."""

import argparse
from dataclasses import dataclass
import json
import os
from pathlib import Path
import re
import subprocess
import time
import tomllib
from urllib.parse import urlencode

from container_release import REPOSITORY, ROOT, check

WORKFLOW = ".github/workflows/native-distribution.yml"
FIELDS = ("run_id", "run_attempt", "workflow", "artifact_id", "artifact_digest")
# The native source, platform matrix and aggregation run in sequence (45 + 90 + 30 minutes).
# Allow another 45 minutes for runner queues before giving up on an unfinished candidate.
NATIVE_WAIT_MINUTES = 210


@dataclass(frozen=True)
class Source:
    revision: str
    branch: str
    repo: str
    event: str
    pr: int | None = None

    def matches(self, run):
        if not (
            run["path"] == WORKFLOW
            and run["repository"]["full_name"] == REPOSITORY
            and (run.get("head_repository") or {}).get("full_name") == self.repo
            and run["head_sha"] == self.revision
            and run["head_branch"] == self.branch
        ):
            return False
        if self.event == "workflow_dispatch":
            return run["event"] in {"workflow_dispatch", "push"}
        if run["event"] != self.event:
            return False
        if self.pr is not None:
            return any(item["number"] == self.pr for item in run.get("pull_requests", ()))
        return True


def source(event, payload, revision):
    check(
        re.fullmatch("[a-f0-9]{40}", revision) and payload["repository"]["full_name"] == REPOSITORY,
        "candidate checkout or repository identity differs",
    )
    if event == "pull_request":
        pr = payload["pull_request"]
        head = pr["head"]
        check(
            pr["base"]["repo"]["full_name"] == REPOSITORY
            and pr["base"]["ref"] == "main"
            and head["sha"] == revision,
            "unexpected candidate pull request revision or base",
        )
        return Source(revision, head["ref"], head["repo"]["full_name"], event, pr["number"])
    check(
        event in {"push", "workflow_dispatch"} and payload["ref"] in {"main", "refs/heads/main"},
        "main branch required for standalone container candidates",
    )
    if event == "push":
        check(payload["after"] == revision, "push checkout differs from event")
    return Source(revision, "main", REPOSITORY, event)


def select(github, expected, *, run_id=None, attempt=None):
    if expected.event == "workflow_dispatch":
        check(
            type(run_id) is int and run_id > 0 and type(attempt) is int and attempt > 0,
            "dispatch requires an exact native run ID and attempt",
        )
        run = github.api(f"actions/runs/{run_id}")
        check(
            expected.matches(run) and run["id"] == run_id and run["run_attempt"] == attempt,
            "selected native run has the wrong source, workflow, or attempt",
        )
    else:
        params = urlencode({"head_sha": expected.revision, "event": expected.event, "per_page": 100})
        found = github.api(
            "actions/workflows/native-distribution.yml/runs?" + params, pages=True, key="workflow_runs"
        )
        matches = [r for r in found if expected.matches(r)]
        if not matches:
            return None
        run = max(matches, key=lambda r: r["id"])
    if run["status"] != "completed":
        return None
    check(
        run["conclusion"] == "success", "matching Native distribution failed; complete its run before retry"
    )
    return run


def acquire(args):
    from release import GitHub
    from release_artifacts import acquire_run_artifact, asset_names

    github = GitHub()
    revision = subprocess.check_output(["git", "-C", ROOT, "rev-parse", "HEAD"], text=True).strip()
    expected = source(
        os.environ["GITHUB_EVENT_NAME"],
        json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_bytes()),
        revision,
    )
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    deadline = time.monotonic() + NATIVE_WAIT_MINUTES * 60
    while time.monotonic() < deadline:
        run = select(github, expected, run_id=args.native_run, attempt=args.native_attempt)
        if run is not None:
            break
        time.sleep(30)
    else:
        raise TimeoutError(
            f"no successful matching Native distribution completed within {NATIVE_WAIT_MINUTES} minutes"
        )
    identity = acquire_run_artifact(
        github,
        args.output_dir,
        run,
        "komodo-mcp-distribution-" + revision,
        set(asset_names(version)) | {"distribution-manifest.json"},
    )
    args.identity.write_text(json.dumps({key: identity[key] for key in FIELDS}, indent=2) + "\n")
    print(
        f"Verified native workflow run {identity['run_id']}, attempt {identity['run_attempt']}, "
        f"artifact {identity['artifact_id']} at {identity['artifact_digest']}"
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--identity", type=Path, required=True)
    parser.add_argument("--native-run", type=int)
    parser.add_argument("--native-attempt", type=int)
    acquire(parser.parse_args())
