"""Exercise packaged publication with in-memory Git/GitHub/OCI and synthetic files."""
import copy
import hashlib
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import Mock, patch
import zipfile

import release
import release_artifacts as artifacts
from test_release import FakeGitHub, api_release, expected

REVISION = "a" * 40
TAG_OID = "b" * 40


class Repository:
    def __init__(self, writes):
        self.writes = writes
        self.local = self.remote = None
        self.lose_push = False

    def accepted_main(self, revision):
        assert revision == REVISION

    def local_tag(self, tag, revision):
        return self.local

    def remote_tag(self, tag, revision):
        return self.remote

    def create_tag(self, tag, revision):
        self.writes.append("tag")
        self.local = TAG_OID
        return self.local

    def push_tag(self, tag):
        self.writes.append("push")
        self.remote = self.local
        if self.lose_push:
            raise release.CommandError("lost push response")


class Hub(FakeGitHub):
    def __init__(self, files, writes):
        super().__init__()
        self.files = files
        self.assets_by_name = {}
        self.writes = writes
        self.next_asset_id = 100
        self.lose = set()
        self.reject = set()

    def preflight(self):
        return "public"

    def create(self, spec, latest, *, draft=False):
        assert draft and latest is False
        self.writes.append("draft")
        if "draft" not in self.reject:
            self.items.append(api_release(spec, draft=True, identity=42))
        if "draft" in self.lose or "draft" in self.reject:
            raise release.CommandError("lost draft response")

    def assets(self, release_id):
        assert release_id == 42
        return list(copy.deepcopy(self.assets_by_name).values())

    def upload(self, tag, path):
        name = path.name
        self.writes.append(name)
        if name not in self.reject:
            self.next_asset_id += 1
            self.assets_by_name[name] = {"id": self.next_asset_id, "name": name,
                "size": self.files[name]["bytes"], "state": "uploaded", "digest": "sha256:"+self.files[name]["sha256"],
                "browser_download_url": artifacts.download_url(tag[1:], name)}
        if name in self.lose or name in self.reject:
            raise release.CommandError("lost upload response")

    def publish_draft(self, spec, latest):
        self.writes.append("publish")
        if "publish" not in self.reject:
            item = next(r for r in self.items if r["tag_name"] == spec["tag"])
            item.update(draft=False, immutable=False, published_at="2026-11-11T00:00:00Z")
            if latest:
                self.latest_id = item["id"]
        if "publish" in self.lose or "publish" in self.reject:
            raise release.CommandError("lost publish response")


class PackageTests(unittest.TestCase):
    def setUp(self):
        self.setup_release("0.8.0")

    def setup_release(self, version):
        self.spec = expected(version)
        self.writes = []
        self.git = Repository(self.writes)
        files = {name: {"name": name, "kind": kind, "sha256": hashlib.sha256(name.encode()).hexdigest(),
                        "bytes": len(name)} for name, kind in artifacts.asset_names(version).items()}
        for target in artifacts.TARGETS:
            name = artifacts.stem(version, target) + ".tar.gz"
            files[name].update(target=target, executable_sha256=files[name]["sha256"])
        self.hub = Hub(files, self.writes)
        self.registry = Mock()
        self.image = None
        self.public = True
        self.source = f"komodo-mcp-v{version}-source.tar.gz"
        self.registry.read_published.side_effect = self.read_image
        self.registry.publish_candidate.side_effect = self.publish_image
        ids = {"schema_version": 1, "repository": release.REPO, "revision": REVISION, "version": version,
               "run_id": 71, "run_attempt": 1}
        self.identity = {"distribution": {**ids, "files": list(files.values())},
            "container": {**ids, "image": f"ghcr.io/{release.REPO}:v{version.replace('+', '_')}",
                          "source_url": artifacts.download_url(version, self.source)},
            "native_run": {"run_id": 71, "run_attempt": 1, "artifact_id": 11, "artifact_digest": "sha256:"+"a"*64,
                           "workflow": ".github/workflows/native-distribution.yml"},
            "oci_run": {"run_id": 71, "run_attempt": 1, "artifact_id": 12, "artifact_digest": "sha256:"+"b"*64,
                        "workflow": ".github/workflows/container-distribution.yml"}}
        self.identity["container"]["platforms"] = {
            platform: {"target": target, "executable_sha256": files[artifacts.stem(version, target)+".tar.gz"]["sha256"]}
            for platform, target in [("linux/amd64", "x86_64-unknown-linux-musl"),
                                     ("linux/arm64", "aarch64-unknown-linux-musl")]}
        self.registry.validate_receipt.side_effect = lambda doc, **kw: doc
        self.packages = artifacts.Packages(self.identity, self.registry, Path("/native"), Path("/oci"))
        self.evidence = None
        self.source_read = False

    def read_image(self, *, receipt, anonymous=False):
        self.assertEqual(receipt, self.identity["container"])
        if anonymous and not self.public:
            raise release.CommandError("registry denied anonymous reads")
        return self.image

    def publish_image(self, root, *, receipt):
        self.assertTrue(self.source_read)
        self.assertTrue(self.hub.items and not self.hub.items[-1]["draft"])
        self.assertEqual(self.hub.assets_by_name.keys(), self.hub.files.keys())
        self.writes.append("image")
        self.image = {"index_digest": "sha256:" + "c" * 64}
        return self.image

    def check_source(self, asset):
        self.assertEqual(asset["sha256"], self.hub.files[self.source]["sha256"])
        self.source_read = True

    def save(self, state):
        self.evidence = copy.deepcopy(state)

    def run_helper(self, publish=True, previous=None):
        with patch.object(release, "verify_public_source", wraps=self.check_source):
            return release.reconcile_packages(self.git, self.hub, self.spec, REVISION, self.packages, previous,
                                              publish=publish, save=self.save, sleep=lambda _: None)

    def test_publication_order_and_idempotent_same_receipt(self):
        result = self.run_helper()
        written = ["tag", "push", "draft", *sorted(self.hub.files), "publish", "image"]
        self.assertEqual(self.writes, written)
        self.assertEqual((result["release_id"], result["latest_release_id"], result["tag_object"]), (42, 42, TAG_OID))
        self.assertEqual(len(result["assets"]), 8)
        for publish in [True, False]:
            self.assertEqual(self.run_helper(publish, result)["published_image"], self.image)
        self.assertEqual(self.writes, written)

    def test_every_uncertain_write_is_observed_only_once(self):
        self.git.lose_push = True
        self.hub.lose = {"draft", "publish", *self.hub.files}
        self.run_helper()
        self.assertEqual(len(self.writes), len(set(self.writes)))

    def test_incomplete_draft_reuses_exact_ids_and_missing_assets(self):
        self.hub.reject = {self.source}
        with self.assertRaisesRegex(release.ReleaseError, "bounded read-back"):
            self.run_helper()
        partial = self.evidence
        self.assertTrue(partial["assets"])
        self.assertTrue(self.hub.items[0]["draft"])
        self.assertNotIn("image", self.writes)
        self.hub.reject.clear()
        self.writes.clear()
        final = self.run_helper(previous=partial)
        self.assertEqual(final["release_id"], partial["release_id"])
        self.assertTrue(all(final["assets"][name] == val for name, val in partial["assets"].items()))
        self.assertEqual(set(self.writes), (set(self.hub.files) - partial["assets"].keys()) | {"publish", "image"})

    def test_uncertain_publish_stops_before_registry_and_recovers_same_draft(self):
        self.hub.reject.add("publish")
        with self.assertRaisesRegex(release.ReleaseError, "bounded read-back"):
            self.run_helper()
        self.assertEqual(self.writes.count("publish"), 1)
        self.assertNotIn("image", self.writes)
        self.hub.reject.clear()
        self.writes.clear()
        self.run_helper(previous=self.evidence)
        self.assertEqual(self.writes, ["publish", "image"])

    def test_inaccessible_registry_or_existing_image_without_source_blocks_all_writes(self):
        self.registry.read_published.side_effect = release.CommandError("registry unavailable")
        with self.assertRaisesRegex(release.CommandError, "unavailable"):
            self.run_helper()
        self.registry.read_published.side_effect = self.read_image
        self.image = {"existing": True}
        with self.assertRaisesRegex(release.ReleaseError, "without its published"):
            self.run_helper()
        self.assertEqual(self.writes, [])

    def test_existing_published_mutable_release_can_append_missing_asset(self):
        result = self.run_helper()
        del self.hub.assets_by_name["LICENSE"]
        self.writes.clear()
        with self.assertRaisesRegex(release.ReleaseError, "disappeared"):
            self.run_helper(previous=result)
        self.hub.items[0]["immutable"] = True
        with self.assertRaisesRegex(release.ReleaseError, "immutable"):
            self.run_helper()
        self.assertEqual(self.writes, [])
        self.hub.items[0]["immutable"] = False
        self.run_helper()
        self.assertEqual(self.writes, ["LICENSE"])

    def test_existing_public_image_needs_public_matching_source(self):
        self.run_helper()
        del self.hub.assets_by_name[self.source]
        self.writes.clear()
        with self.assertRaisesRegex(release.ReleaseError, "without its published"):
            self.run_helper()
        self.assertEqual(self.writes, [])

    def test_private_image_requires_positive_anonymous_verification(self):
        self.public = False
        with self.assertRaisesRegex(release.ReleaseError, "anonymous"):
            self.run_helper()
        self.public = True
        self.writes.clear()
        self.run_helper(False, self.evidence)
        self.assertEqual(self.writes, [])

    def test_source_visibility_failure_blocks_image_promotion(self):
        def unavailable(asset):
            raise release.CommandError("source unavailable")
        self.check_source = unavailable
        with self.assertRaisesRegex(release.CommandError, "source unavailable"):
            self.run_helper()
        self.assertNotIn("image", self.writes)

    def test_changed_asset_size_hash_id_duplicate_unknown_or_starter_stops(self):
        result = self.run_helper()
        original = self.hub.assets(42)
        for change, value in [("digest", "sha256:"+"0"*64), ("id", 901), ("size", 0),
                              ("name", "unscheduled.zip"), ("state", "starter"),
                              ("browser_download_url", "https://example.invalid/wrong")]:
            assets = copy.deepcopy(original)
            assets[0][change] = value
            with self.subTest(change=change), patch.object(self.hub, "assets", return_value=assets):
                self.writes.clear()
                with self.assertRaises(release.ReleaseError):
                    self.run_helper(previous=result)
                self.assertEqual(self.writes, [])
        with patch.object(self.hub, "assets", return_value=original + original[:1]):
            with self.assertRaisesRegex(release.ReleaseError, "duplicate"):
                self.run_helper()
        assets = copy.deepcopy(original)
        assets[0].pop("digest")
        with patch.object(self.hub, "assets", return_value=assets), patch.object(
                self.hub, "asset_hash", create=True, return_value=self.hub.files[assets[0]["name"]]["sha256"]) as by_id:
            self.run_helper(False, result)
            by_id.assert_called_with(assets[0]["id"])

    def test_original_receipt_rejects_new_tag_release_candidate_or_registry_identity(self):
        result = self.run_helper()
        self.writes.clear()
        for key, value in [("release_id", 999), ("tag_object", "e"*40), ("package_identity", {}),
                           ("published_image", {"other": True})]:
            with self.subTest(key=key), self.assertRaises(release.ReleaseError):
                self.run_helper(previous={**result, key: value})
        self.assertEqual(self.writes, [])

    def test_older_stable_and_prerelease_preserve_latest(self):
        for version in ("0.8.0", "0.8.0-rc.1", "0.8.0+build.4"):
            with self.subTest(version=version):
                self.setup_release(version)
                self.hub.items.append(api_release(expected("0.9.0"), identity=100))
                self.hub.latest_id = 100
                self.run_helper()
                self.assertEqual(self.hub.latest_id, 100)

    def test_verify_or_missing_container_adapter_cannot_write(self):
        with self.assertRaisesRegex(release.ReleaseError, "incomplete"):
            self.run_helper(False)
        with patch.object(artifacts.importlib, "import_module", side_effect=ModuleNotFoundError(name="container_release")):
            with self.assertRaisesRegex(release.ReleaseError, "container distribution adapter"):
                artifacts.Packages.download(self.hub, Path("/unused"), REVISION, "0.8.0", 10, 10)
        self.assertEqual(self.writes, [])

    def test_manifest_and_oci_run_substitution_or_incomplete_expected_assets_rejected(self):
        artifacts.Packages.check_identity(self.identity, REVISION, "0.8.0")
        for key, field, value in [("native_run", "run_attempt", 2), ("container", "revision", "c"*40),
                                  ("oci_run", "run_id", 99), ("container", "image", "other"),
                                  ("container", "source_url", "https://example.invalid/source"),
                                  ("distribution", "files", [])]:
            changed = copy.deepcopy(self.identity)
            changed[key][field] = value
            with self.subTest(key=key, field=field), self.assertRaises(release.ReleaseError):
                artifacts.Packages.check_identity(changed, REVISION, "0.8.0")

    def test_known_registry_conflict_blocks_unrelated_github_asset_repair(self):
        self.run_helper()
        del self.hub.assets_by_name["LICENSE"]
        self.registry.read_published.side_effect = release.ReleaseError("registry digest conflict")
        self.writes.clear()
        with self.assertRaisesRegex(release.ReleaseError, "registry digest conflict"):
            self.run_helper()
        self.assertEqual(self.writes, [])

    def test_incorrect_latest_blocks_tag_and_asset_writes_for_older_release(self):
        self.hub.items.append(api_release(expected("0.9.0"), identity=7))
        self.hub.latest_id = 99
        with self.assertRaisesRegex(release.ReleaseError, "Latest conflict"):
            self.run_helper()
        self.assertEqual(self.writes, [])

    def test_draft_asset_url_can_change_at_publication_without_an_identity_change(self):
        original = self.hub.assets
        def observed(release_id):
            assets = original(release_id)
            if self.hub.items[0]["draft"]:
                for asset in assets:
                    asset["browser_download_url"] = "https://github.com/temporary-draft-url"
            return assets
        with patch.object(self.hub, "assets", side_effect=observed):
            self.assertEqual(len(self.run_helper()["assets"]), 8)

    def test_archived_receipt_verifies_without_actions_or_local_candidate_files(self):
        evidence = self.run_helper()
        self.writes.clear()
        with tempfile.TemporaryDirectory() as scratch:
            path = Path(scratch) / "receipt.json"
            output = Path(scratch) / "verified.json"
            release.write_json(path, evidence)
            git = Mock(wraps=self.git)
            git.metadata = Mock(return_value=self.spec)
            git.origin = Mock()
            with patch.object(artifacts, "container_api", return_value=self.registry), \
                    patch.object(artifacts.Packages, "download") as download, \
                    patch.object(release, "Git", return_value=git), \
                    patch.object(release, "GitHub", return_value=self.hub), \
                    patch.object(release, "verify_public_source", wraps=self.check_source), \
                    patch("builtins.print"):
                release.main(["verify", "--version", self.spec["version"], "--commit", REVISION,
                              "--receipt", str(path), "--output", str(output)])
                git.exact_checkout = Mock()
                with patch.object(release, "require_evidence"), patch.object(self.hub, "main_checks", create=True,
                                                                           return_value=[{"conclusion": "success"}]):
                    release.main(["publish", "--version", self.spec["version"], "--commit", REVISION,
                                  "--evidence", "validated.json", "--authorized", "--output", str(path)])
            self.registry.validate_receipt.assert_called_with(self.identity["container"],
                                                                   revision=REVISION, version=self.spec["version"])
            download.assert_not_called()
            self.assertEqual(json.loads(output.read_bytes())["assets"], evidence["assets"])
            self.assertEqual(self.writes, [])


class AcquisitionTests(unittest.TestCase):
    def test_authenticate_run_and_exact_immutable_zip_before_unpacking(self):
        api = Mock()
        run = {"id": 32, "path": ".github/workflows/native-distribution.yml",
               "head_sha": REVISION, "head_branch": "main", "event": "push", "run_attempt": 2,
               "repository": {"full_name": release.REPO}, "head_repository": {"full_name": release.REPO},
               "status": "completed", "conclusion": "success", "html_url": "https://github.com/example/run"}
        with tempfile.TemporaryDirectory() as folder:
            base = Path(folder)
            buffer = io.BytesIO()
            with zipfile.ZipFile(buffer, "w") as zipfile_out:
                zipfile_out.writestr("file", b"tested")
            contents = buffer.getvalue()
            artifact = {"name": "bundle", "id": 74, "expired": False,
                        "digest": "sha256:"+hashlib.sha256(contents).hexdigest()}
            def fetch(endpoint, path):
                self.assertEqual(endpoint, "actions/artifacts/74/zip")
                path.write_bytes(contents)
            with patch.object(artifacts, "api_file", side_effect=fetch):
                api.api.side_effect = [run, [artifact]]
                info = artifacts.acquire(api, base/"ok", REVISION, 32, "native-distribution", "bundle", {"file"})
                self.assertEqual(info["artifact_id"], 74)
                self.assertEqual((base/"ok/file").read_bytes(), b"tested")
                for field, change in [("head_sha", "b"*40), ("head_branch", "pr"), ("event", "pull_request"),
                                      ("conclusion", "failure"), ("path", ".github/workflows/other.yml")]:
                    api.api.side_effect = [{**run, field: change}]
                    with self.subTest(field=field), self.assertRaises(release.ReleaseError):
                        artifacts.acquire(api, base/"bad", REVISION, 32, "native-distribution", "bundle", {"file"})
                api.api.side_effect = [run, [{**artifact, "digest": "sha256:"+"0"*64}]]
                with self.assertRaisesRegex(release.ReleaseError, "digest conflict"):
                    artifacts.acquire(api, base/"bad-digest", REVISION, 32, "native-distribution", "bundle", {"file"})
                api.api.side_effect = [run, [artifact]]
                with self.assertRaisesRegex(release.ReleaseError, "unexpected"):
                    artifacts.acquire(api, base/"wrong-files", REVISION, 32, "native-distribution", "bundle", {"other"})
                for items in ([], [artifact, artifact], [{**artifact, "expired": True}]):
                    api.api.side_effect = [run, items]
                    with self.assertRaisesRegex(release.ReleaseError, "missing, expired or duplicate"):
                        artifacts.acquire(api, base/"absent", REVISION, 32, "native-distribution", "bundle", {"file"})

    def test_validate_authenticated_bundles_and_reject_wrong_oci_attempt(self):
        revision, version = REVISION, "0.8.0"
        shared = {"schema_version": 1, "repository": release.REPO, "revision": revision, "version": version,
                  "run_id": 25, "run_attempt": 2}
        oci_doc = {**shared, "run_id": 26, "image": f"ghcr.io/{release.REPO}:v{version}",
                   "source_url": artifacts.download_url(version, f"komodo-mcp-v{version}-source.tar.gz")}
        def stage(github, root, revision, run_id, workflow, name, names):
            root.mkdir()
            if workflow == "native-distribution":
                files = []
                for filename, kind in artifacts.asset_names(version).items():
                    if kind == "checksums":
                        continue
                    value = filename.encode()
                    digest = hashlib.sha256(value).hexdigest()
                    entry = {"kind": kind, "name": filename, "bytes": len(value), "sha256": digest}
                    if kind == "native":
                        target = next(t for t in artifacts.TARGETS if t in filename)
                        entry.update(target=target, executable_sha256=digest)
                        if target.endswith("darwin"):
                            entry["minimum_macos"] = "14.0"
                    files.append(entry)
                    (root / filename).write_bytes(value)
                sums = "".join(f"{f['sha256']}  {f['name']}\n" for f in sorted(files, key=lambda f: f["name"]))
                (root / "SHA256SUMS").write_text(sums)
                files.append({"kind": "checksums", "name": "SHA256SUMS", "bytes": len(sums),
                              "sha256": hashlib.sha256(sums.encode()).hexdigest()})
                (root / "distribution-manifest.json").write_text(json.dumps({**shared, "files": files}))
            return {"run_id": run_id, "run_attempt": 2, "artifact_id": 10, "workflow": f".github/workflows/{workflow}.yml",
                    "artifact_digest": "sha256:"+"d"*64}

        adapter = Mock()
        adapter.load_candidate.return_value = oci_doc
        with patch.object(artifacts, "container_api", return_value=adapter), patch.object(artifacts, "acquire", stage):
            with tempfile.TemporaryDirectory() as scratch:
                packages = artifacts.Packages.download(Mock(), Path(scratch), revision, version, 25, 26)
                self.assertEqual(packages.identity["distribution"]["run_attempt"], 2)
                self.assertEqual(len(packages.identity["distribution"]["files"]), 8)
            adapter.load_candidate.return_value = {**oci_doc, "run_attempt": 1}
            with tempfile.TemporaryDirectory() as scratch, self.assertRaisesRegex(release.ReleaseError, "run attempt"):
                artifacts.Packages.download(Mock(), Path(scratch), revision, version, 25, 26)

    def test_public_source_download_matches_actual_bytes_and_carries_no_auth(self):
        value = b"synthetic corresponding source"
        spec = {"url": "https://github.com/enthouan/komodo-mcp/releases/download/v0.8.0/source.tar.gz",
                "bytes": len(value), "sha256": hashlib.sha256(value).hexdigest()}
        for data in [value, b"short", b"z"*len(value), value+b"more"]:
            def serve(request, *, timeout):
                self.assertNotIn("Authorization", dict(request.header_items()))
                self.assertEqual(request.full_url, spec["url"])
                return io.BytesIO(data)
            with self.subTest(data=data), patch.object(artifacts.urllib.request, "urlopen", side_effect=serve):
                if data == value:
                    artifacts.verify_public_source(spec)
                else:
                    with self.assertRaises(release.ReleaseError):
                        artifacts.verify_public_source(spec)
