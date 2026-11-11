"""Release safety tests use disposable local Git repositories and API responses."""
import copy
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import release


MANIFEST = '[package]\nname = "komodo-mcp"\nversion = "0.1.0"\n'
LOCK = '[[package]]\nname = "komodo-mcp"\nversion = "0.1.0"\n'
CHANGELOG = '# Changelog\n\n## v0.1.0\n\nFirst release.\n'


def expected(version="0.1.0"):
    return release.metadata(MANIFEST.replace("0.1.0", version), LOCK.replace("0.1.0", version),
                            CHANGELOG.replace("0.1.0", version), version)


def api_release(spec=None, draft=False, identity=1):
    spec = spec or expected()
    return {"id": identity, "tag_name": spec["tag"], "name": spec["tag"], "body": spec["notes"],
            "draft": draft, "prerelease": spec["prerelease"], "assets": [],
            "published_at": None if draft else "2026-11-11T00:00:00Z",
            "html_url": f"https://github.com/{release.REPO}/releases/tag/{spec['tag']}"}


class FakeGitHub:
    def __init__(self, releases=None):
        self.items = releases or []
        self.writes = []
        self.latest_id = next((r["id"] for r in self.items if not r["draft"] and not r["prerelease"]), None)
        self.lost_create_response = False
        self.failed_create = False

    def preflight(self):
        return "private"

    def releases(self):
        return copy.deepcopy(self.items)

    def latest(self):
        return self.latest_id

    def create(self, spec, latest):
        self.writes.append(("create", spec["tag"], latest))
        if self.failed_create:
            raise release.CommandError("network unavailable before response")
        item = api_release(spec, identity=99)
        self.items.append(item)
        if latest:
            self.latest_id = item["id"]
        if self.lost_create_response:
            raise release.CommandError("response lost after accepted write")

    def publish_draft(self, spec, latest):
        self.writes.append(("publish_draft", spec["tag"], latest))
        item = next(r for r in self.items if r["tag_name"] == spec["tag"])
        item.update(draft=False, published_at="2026-11-11T00:00:00Z")
        if latest:
            self.latest_id = item["id"]


class MetadataTests(unittest.TestCase):
    def test_first_release_notes_and_root_metadata(self):
        spec = expected()
        self.assertEqual(spec["notes"], "First release.\n")
        self.assertFalse(spec["prerelease"])
        # The independent dependency version is not an application version.
        lock = LOCK + '\n[[package]]\nname = "komodo_client"\nversion = "2.3.2"\n'
        self.assertEqual(release.metadata(MANIFEST, lock, CHANGELOG, "0.1.0"), spec)

    def test_manifest_lock_and_notes_mismatches(self):
        for manifest, lock, notes in [
            (MANIFEST.replace("0.1.0", "0.1.1"), LOCK, CHANGELOG),
            (MANIFEST, LOCK.replace("0.1.0", "0.1.1"), CHANGELOG),
            (MANIFEST, LOCK + LOCK, CHANGELOG),
            (MANIFEST, LOCK, "# No release yet"),
            (MANIFEST, LOCK, CHANGELOG + CHANGELOG),
            (MANIFEST, LOCK, "## v0.1.0\n\n## v0.0.1\nEarlier notes"),
        ]:
            with self.subTest(manifest=manifest, lock=lock, notes=notes):
                with self.assertRaises(release.ReleaseError):
                    release.metadata(manifest, lock, notes, "0.1.0")

    def test_subsequent_notes_are_one_reviewed_section(self):
        notes = "# Changelog\n\n## v0.1.1\n\nFix pagination.\n\n" + CHANGELOG[CHANGELOG.index("## "):]
        spec = release.metadata(MANIFEST.replace("0.1.0", "0.1.1"),
                                LOCK.replace("0.1.0", "0.1.1"), notes, "0.1.1")
        self.assertEqual(spec["notes"], "Fix pagination.\n")

    def test_semver_stable_prerelease_and_invalid_versions(self):
        self.assertTrue(expected("1.2.3-rc.1")["prerelease"])
        self.assertFalse(expected("1.2.3+build.7")["prerelease"])
        for version in ["v0.1.0", "01.1.0", "0.1", "1.0.0-rc.01", "1.0.0-", "1.0.0\n"]:
            with self.subTest(version=version), self.assertRaises(release.ReleaseError):
                release.version_info(version)


class PublicationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="komodo release tests ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "checkout with spaces"
        self.root.mkdir()
        self.remote = Path(self.temp.name) / "origin.git"
        subprocess.run(["git", "init", "--bare", "--quiet", str(self.remote)], check=True)
        subprocess.run(["git", "init", "--quiet", "--initial-branch=main", str(self.root)], check=True)
        self.git = release.Git(self.root)
        self.git.run("config", "user.name", "Release Test")
        self.git.run("config", "user.email", "release@example.invalid")
        for path, value in [("Cargo.toml", MANIFEST), ("Cargo.lock", LOCK), ("CHANGELOG.md", CHANGELOG),
                            (".gitignore", "target/\n")]:
            (self.root / path).write_text(value)
        self.git.run("add", "Cargo.toml", "Cargo.lock", "CHANGELOG.md", ".gitignore")
        self.git.run("commit", "-m", "Initial source")
        self.git.run("remote", "add", "origin", str(self.remote))
        self.git.run("push", "origin", "main")
        self.commit = self.git.run("rev-parse", "HEAD")
        self.spec = expected()
        self.github = FakeGitHub()

    def publish(self, **kwargs):
        return release.reconcile(self.git, self.github, self.spec, self.commit,
                                 publish=True, sleep=lambda _: None, **kwargs)

    def push_tag(self):
        self.git.create_tag("v0.1.0", self.commit)
        self.git.push_tag("v0.1.0")

    def test_new_publication_and_matching_rerun_are_verified_noop(self):
        result = self.publish()
        self.assertEqual(result["commit"], self.commit)
        self.assertEqual(self.git.run("cat-file", "-t", result["tag_object"]), "tag")
        before = self.git.run("ls-remote", "origin", "refs/tags/*")
        again = self.publish()
        self.assertEqual(result["tag_object"], again["tag_object"])
        self.assertEqual(before, self.git.run("ls-remote", "origin", "refs/tags/*"))
        self.assertEqual(len(self.github.writes), 1)
        release.reconcile(self.git, self.github, self.spec, self.commit)
        self.assertEqual(len(self.github.writes), 1)

    def test_existing_local_tag_is_reused(self):
        oid = self.git.create_tag("v0.1.0", self.commit)
        self.assertEqual(self.publish()["tag_object"], oid)

    def test_remote_tag_without_local_tag_resumes_missing_release(self):
        self.push_tag()
        oid = self.git.local_tag("v0.1.0", self.commit)
        self.git.run("tag", "-d", "v0.1.0")  # Disposable local test ref only.
        self.assertEqual(self.publish()["tag_object"], oid)
        self.assertIsNone(self.git.local_tag("v0.1.0", self.commit))

    def test_matching_draft_is_published_without_replacement(self):
        self.push_tag()
        self.github.items = [api_release(draft=True)]
        result = self.publish()
        self.assertEqual(result["release_id"], 1)
        self.assertEqual(self.github.writes, [("publish_draft", "v0.1.0", True)])

    def test_older_release_recovery_preserves_latest(self):
        self.github = FakeGitHub([api_release(expected("0.2.0"), identity=2)])
        self.publish()
        self.assertEqual(self.github.latest_id, 2)
        self.assertEqual(self.github.writes, [("create", "v0.1.0", False)])

    def test_prerelease_is_never_latest(self):
        self.spec = expected("0.1.0-rc.1")
        self.publish()
        self.assertTrue(self.github.items[0]["prerelease"])
        self.assertEqual(self.github.writes[0], ("create", "v0.1.0-rc.1", False))

    def test_lightweight_local_or_remote_tag_is_conflict(self):
        self.git.run("tag", "v0.1.0", self.commit)
        with self.assertRaisesRegex(release.ReleaseError, "lightweight"):
            self.publish()
        self.git.push_tag("v0.1.0")
        self.git.run("tag", "-d", "v0.1.0")
        with self.assertRaisesRegex(release.ReleaseError, "expected annotated"):
            self.publish()
        self.assertEqual(self.github.writes, [])

    def test_wrong_commit_or_annotation_stops_without_writes(self):
        self.git.run("commit", "--allow-empty", "-m", "Later source")
        other = self.git.run("rev-parse", "HEAD")
        self.git.create_tag("v0.1.0", other)
        with self.assertRaisesRegex(release.ReleaseError, "expected commit"):
            self.publish()
        self.git.run("tag", "-d", "v0.1.0")
        self.git.run("tag", "--no-sign", "-a", "v0.1.0", self.commit, "-m", "wrong annotation")
        with self.assertRaisesRegex(release.ReleaseError, "annotation"):
            self.publish()
        self.assertEqual(self.github.writes, [])

    def test_published_release_without_tag_is_conflict(self):
        self.github.items = [api_release()]
        with self.assertRaisesRegex(release.ReleaseError, "without expected remote tag"):
            self.publish()
        self.assertIsNone(self.git.local_tag("v0.1.0", self.commit))

    def test_release_title_notes_and_classification_conflicts(self):
        self.push_tag()
        for field, value in [("name", "Wrong title"), ("body", "Other notes"), ("prerelease", True)]:
            self.github.items = [api_release()]
            self.github.items[0][field] = value
            with self.subTest(field=field), self.assertRaisesRegex(release.ReleaseError, "release conflict"):
                self.publish()
        self.assertEqual(self.github.writes, [])

    def test_lost_create_response_reads_back_without_retrying_write(self):
        self.github.lost_create_response = True
        self.publish()
        self.assertEqual(len(self.github.writes), 1)

    def test_failed_release_resumes_after_rechecking_original_remote_tag(self):
        self.github.failed_create = True
        with self.assertRaisesRegex(release.ReleaseError, "bounded read-back"):
            self.publish()
        oid = self.git.remote_tag("v0.1.0", self.commit)
        self.github.failed_create = False
        self.assertEqual(self.publish()["tag_object"], oid)
        self.assertEqual(len(self.github.writes), 2)

    def test_lost_push_response_reads_back_before_any_release_write(self):
        push = self.git.push_tag
        def lost_response(tag):
            push(tag)
            raise release.CommandError("push response lost")
        with patch.object(self.git, "push_tag", side_effect=lost_response) as mocked:
            self.publish()
            mocked.assert_called_once()
        self.assertEqual(len(self.github.writes), 1)

    def test_failed_lookup_is_never_absence(self):
        for error in ("authentication failed", "HTTP 403", "HTTP 429", "HTTP 500"):
            with patch.object(self.github, "releases", side_effect=release.CommandError(error)):
                with self.assertRaisesRegex(release.ReleaseError, error):
                    self.publish()
        self.assertIsNone(self.git.local_tag("v0.1.0", self.commit))
        self.assertEqual(self.github.writes, [])

    def test_identity_conflict_is_not_a_retryable_read(self):
        with patch.object(self.github, "releases", side_effect=release.ReleaseError("conflict")) as mocked:
            with self.assertRaisesRegex(release.ReleaseError, "conflict"):
                release.read_back(self.github.releases, lambda _: None)
            mocked.assert_called_once()

    def test_recovery_uses_original_commit_when_main_advances(self):
        self.publish()
        self.git.run("commit", "--allow-empty", "-m", "New main commit")
        self.git.run("push", "origin", "main")
        self.assertEqual(self.publish()["commit"], self.commit)

    def test_unmerged_commit_is_rejected(self):
        self.commit = self.git.run("commit-tree", "HEAD^{tree}", "-m", "Unmerged source")
        with self.assertRaises(release.ReleaseError):
            self.publish()
        self.assertEqual(self.github.writes, [])

    def test_verify_only_path_never_creates_tags_or_releases(self):
        with self.assertRaisesRegex(release.ReleaseError, "incomplete"):
            release.reconcile(self.git, self.github, self.spec, self.commit)
        self.assertIsNone(self.git.local_tag("v0.1.0", self.commit))
        self.assertEqual(self.github.writes, [])

    def test_validation_only_path_runs_checks_and_version_probe_without_publishing(self):
        target = self.root / "target/release"
        target.mkdir(parents=True)
        (target / "komodo-mcp").write_bytes(b"synthetic executable identity")
        real_run = subprocess.run
        commands = []
        runtime_version = "0.1.0"
        artifact_executable = target / "komodo-mcp"
        def run(command, **kwargs):
            if command[0] == "git":
                return real_run(command, **kwargs)
            commands.append(command)
            if command == release.BUILD_COMMAND:
                artifact = {"reason": "compiler-artifact", "target": {"name": "komodo-mcp", "kind": ["bin"]},
                            "executable": str(artifact_executable)}
                return subprocess.CompletedProcess(command, 0, stdout=json.dumps(artifact))
            runtime = {"observed_version": runtime_version, "child_exit_success": True,
                       "forced_kill": False, "mock_stopped": True}
            return subprocess.CompletedProcess(command, 0, stdout=json.dumps(runtime))
        output = self.root / "target/evidence.json"
        with patch.object(release.subprocess, "run", side_effect=run), patch.object(release, "GitHub") as api:
            release.validation(self.git, "0.1.0", self.commit, output)
            api.assert_not_called()
        self.assertEqual(commands, release.CHECKS + [release.VERSION_COMMAND + ["0.1.0"]])
        release.require_evidence(self.git, self.spec, self.commit, output)
        evidence = json.loads(output.read_text())
        evidence["runtime"]["observed_version"] = "0.0.9"
        output.write_text(json.dumps(evidence))
        with self.assertRaisesRegex(release.ReleaseError, "evidence does not match"):
            release.require_evidence(self.git, self.spec, self.commit, output)
        output.unlink()
        runtime_version = "0.0.9"
        with patch.object(release.subprocess, "run", side_effect=run):
            with self.assertRaisesRegex(release.ReleaseError, "runtime verification failed"):
                release.validation(self.git, "0.1.0", self.commit, output)
        self.assertFalse(output.exists())
        artifact_executable = target / "different-target/komodo-mcp"
        with patch.object(release.subprocess, "run", side_effect=run):
            with self.assertRaisesRegex(release.ReleaseError, "Cargo built a different executable"):
                release.validation(self.git, "0.1.0", self.commit, output)
        self.assertFalse(output.exists())
        self.assertIsNone(self.git.local_tag("v0.1.0", self.commit))


class GitHubCommandTests(unittest.TestCase):
    def test_release_create_requires_tag_and_explicit_classification(self):
        github = release.GitHub()
        for version in ("0.1.0", "0.2.0-rc.1"):
            with patch.object(github, "call", return_value="") as call:
                github.create(expected(version), latest=False)
                args = call.call_args.args
                self.assertIn("--verify-tag", args)
                self.assertIn("--notes-file", args)
                self.assertIn("--latest=false", args)
                self.assertEqual("--prerelease" in args, "-" in version)

    def test_workflow_pagination_and_exact_main_commit_checks(self):
        github = release.GitHub()
        commit = "a" * 40
        runs = [{"id": i, "name": name, "head_branch": "main", "head_sha": commit,
                 "status": "completed", "conclusion": "success", "html_url": f"run/{i}"}
                for i, name in enumerate(("CI", "Inventory benchmark"))]
        with patch.object(github, "call", return_value=json.dumps([{"workflow_runs": runs}])):
            self.assertEqual(len(github.main_checks(commit)), 2)
        runs[0]["conclusion"] = "failure"
        with patch.object(github, "call", return_value=json.dumps([{"workflow_runs": runs}])):
            with self.assertRaisesRegex(release.ReleaseError, "not green"):
                github.main_checks(commit)


if __name__ == "__main__":
    unittest.main()
