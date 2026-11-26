import argparse
import contextlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import Mock, patch

import dockerhub_mirror as mirror
import mirror_registry as registry
import test_container_release as fixture

REVISION = "e" * 40
RAW = b"fixture index"
DIGEST = registry.describe(RAW)
OTHER = registry.describe(b"other")
IDENTITY = {"index_digest": DIGEST, "referrers": {DIGEST: []}, "platforms": {}, "blob_count": 2}


class Tags:
    def __init__(self):
        self.tags = {}

    def manifest(self, tag):
        if tag not in self.tags:
            raise mirror.Missing(tag)
        return self.tags[tag]


class MirrorPolicy(unittest.TestCase):
    def setUp(self):
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.args = argparse.Namespace(tag="v1.0.0", revision=REVISION, digest=DIGEST, tag_object=REVISION)
        self.m = mirror.Mirror(self.args, self.root)
        self.m.hub = Tags()
        self.m.source = Tags()
        self.m.source_graph = Mock(return_value=IDENTITY)
        self.m.verify_evidence = Mock()
        self.m.login = Mock()
        self.m.release_state = Mock(return_value=(True, []))
        self.m.copy = Mock(side_effect=lambda: self.m.hub.tags.update({"1.0.0": RAW}))
        self.m.put_latest = Mock(side_effect=lambda raw: self.m.hub.tags.update(latest=raw))
        graph = patch.object(mirror, "graph", return_value=IDENTITY)
        self.graph = graph.start()
        self.addCleanup(graph.stop)

    def run_mirror(self, write):
        with contextlib.redirect_stdout(io.StringIO()):
            return mirror.reconcile(self.m, write)

    def test_preview_then_first_copy_and_matching_retry(self):
        plan = self.run_mirror(False)
        self.assertTrue(plan["copy_or_repair_exact"] and plan["advance_latest"])
        self.m.login.assert_not_called()
        self.m.copy.assert_not_called()
        self.m.put_latest.assert_not_called()
        self.run_mirror(True)
        self.assertEqual(self.m.hub.tags, {"1.0.0": RAW, "latest": RAW})
        self.run_mirror(True)
        self.m.copy.assert_called_once()
        self.m.put_latest.assert_called_once_with(RAW)
        self.m.login.assert_called_once()

    def test_foreign_version_or_unknown_latest_stops_before_write(self):
        for tag in ("1.0.0", "latest"):
            self.m.hub.tags = {tag: b"other"}
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                self.run_mirror(True)
        self.m.copy.assert_not_called()

    def test_older_retry_and_prerelease_skip_latest(self):
        for version in ("1.0.0", "1.2.0-rc.1"):
            self.m.version = self.m.hub_tag = version
            self.m.release_state.return_value = (False, [])
            self.m.hub.tags = {version: RAW, "latest": b"newer stable"}
            self.run_mirror(True)
            self.assertEqual(self.m.hub.tags["latest"], b"newer stable")
        self.m.put_latest.assert_not_called()
        self.m.copy.assert_not_called()

    def test_latest_requires_verified_previous_pin(self):
        self.m.version = self.m.hub_tag = "1.1.0"
        self.m.release_state.return_value = (True, [{"tag_name": "v1.0.0"}])
        self.m.source.tags = {"v1.0.0": b"older"}
        self.m.hub.tags = {"1.0.0": b"older", "1.1.0": RAW, "latest": b"older"}
        self.run_mirror(True)
        self.m.put_latest.assert_called_once_with(RAW)
        self.m.copy.assert_not_called()

    def test_lost_write_response_reads_back_without_repeating_copy(self):
        def lost():
            self.m.hub.tags["1.0.0"] = RAW
            raise subprocess.TimeoutExpired("regctl", 1)
        self.m.copy.side_effect = lost
        self.run_mirror(True)
        self.m.copy.assert_called_once()
        self.m.put_latest.assert_called_once()

    def test_unconfirmed_write_only_retries_reads(self):
        operation = Mock(side_effect=OSError)
        read = Mock(side_effect=mirror.Missing)
        with self.assertRaises(mirror.Missing):
            mirror.readback(operation, read, sleep=lambda _: None)
        operation.assert_called_once()
        self.assertEqual(read.call_count, 4)

    def test_older_and_prerelease_classification_from_published_releases(self):
        m = mirror.Mirror(self.args, self.root)
        m.git = Mock()
        m.git.remote_tag.return_value = REVISION
        m.github = Mock()
        def release(version, number, pre):
            return {"id": number, "name": "v" + version, "tag_name": "v" + version, "body": "notes",
                    "draft": False, "prerelease": pre, "published_at": "today"}
        for v, pre in [("1.0.0", False), ("1.2.0-rc.1", True)]:
            m.version = v
            m.git.metadata.return_value = {"tag": "v" + v, "version": v, "prerelease": pre,
                                           "notes": "notes\n", "notes_sha256": "..."}
            m.github.releases.return_value = [release(v, 1, pre), release("1.1.0", 2, False)]
            m.github.latest.return_value = 2
            self.assertFalse(m.release_state()[0])

    def test_partial_referrers_repair_and_unexpected_referrer_rejection(self):
        self.m.hub.tags = {"1.0.0": RAW, "latest": RAW}
        source = dict(IDENTITY, referrers={DIGEST: [OTHER], OTHER: []})
        self.m.source_graph.return_value = source
        self.graph.side_effect = [IDENTITY, IDENTITY, source]
        self.run_mirror(True)
        self.m.copy.assert_called_once()
        self.m.put_latest.assert_not_called()
        with self.assertRaisesRegex(ValueError, "unexpected destination"):
            mirror.equivalent(IDENTITY, source)

    def test_ambiguous_remote_error_stops_before_copy(self):
        self.m.hub.tags = {"1.0.0": RAW}
        self.graph.side_effect = RuntimeError("HTTP 429")
        with self.assertRaisesRegex(RuntimeError, "429"):
            self.run_mirror(True)
        self.m.login.assert_not_called()

    def test_workflow_serializes_all_writer_tags(self):
        workflow = (mirror.ROOT / ".github/workflows/docker-hub-mirror.yml").read_text()
        preview, writer = workflow.split("\n  mirror:\n")
        self.assertNotIn("secrets.", preview)
        self.assertIn("environment: dockerhub", writer)
        self.assertIn("group: dockerhub-komodo-mcp-writer\n", writer)
        self.assertIn("cancel-in-progress: false", writer)
        self.assertIn("needs: preview", writer)
        self.assertIn("github.ref == 'refs/heads/main'", writer)


class LayoutReader(registry.Reader):
    """Run the normal anonymous graph verifier over real, registry-free OCI layouts."""

    def __init__(self, path, env):
        self.path = path
        self.image = "ocidir://" + str(path)
        self.env = env

    def open(self, kind, ref):
        return (self.path / "blobs" / "sha256" / ref[7:]).open("rb")

    def manifest(self, ref):
        return (self.path / "blobs" / "sha256" / ref[7:]).read_bytes()


@unittest.skipUnless(shutil.which("regctl"), "pinned regctl exercised by Docker Hub mirror fixtures job")
class RecursiveCopy(unittest.TestCase):
    def test_copy_two_platforms_and_attached_referrers_then_detect_corrupted_blob(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            args = argparse.Namespace(tag="v1.0.0", revision=fixture.REVISION, digest=DIGEST, tag_object=None)
            m = mirror.Mirror(args, root)
            with patch.object(fixture, "VERSION", m.version):
                receipt, _ = fixture.build_fixture(root)
            m.identifier = receipt["index_digest"]
            source, hub = (LayoutReader(root / role, m.env["anonymous"]) for role in ("source", "hub"))
            with tarfile.open(root / fixture.oci.ARCHIVE) as archive:
                archive.extractall(source.path, filter="data")
            for subject in [m.identifier, receipt["platforms"]["linux/arm64"]["manifest_digest"]]:
                subprocess.run(["regctl", "artifact", "put", "--artifact-type", "application/spdx+json",
                                "--subject", source.image + "@" + subject], env=source.env, check=True,
                               input=b'{"fixture":"attestation"}', capture_output=True)
            with patch.object(mirror, "SOURCE", source.image), patch.object(mirror, "DESTINATION", hub.image):
                m.copy()
            before = registry.graph(source, m.identifier, m.version, m.revision)
            after = registry.graph(hub, m.identifier, m.version, m.revision)
            mirror.equivalent(before, after)
            self.assertEqual(set(after["platforms"]), {"linux/amd64", "linux/arm64"})
            self.assertEqual(sum(map(len, after["referrers"].values())), 2)
            config = receipt["platforms"]["linux/amd64"]["config_digest"]
            (hub.path / "blobs" / "sha256" / config[7:]).write_bytes(b"corrupt")
            with self.assertRaisesRegex(ValueError, "blob identity"):
                registry.graph(hub, m.identifier, m.version, m.revision)
