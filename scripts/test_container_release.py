import gzip
import hashlib
import io
import json
import re
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import container_release as oci
import container_registry as registry

VERSION = "0.7.0"
REVISION = "a" * 40
TEST_CA = b"fixture CA"


def tar_bytes(files, modes=None):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w") as archive:
        for name, value in files.items():
            info = tarfile.TarInfo(name)
            info.size = len(value)
            info.mode = 0o755 if name == oci.EXE.lstrip("/") else 0o644
            if modes and name in modes:
                info.mode = modes[name]
            archive.addfile(info, io.BytesIO(value))
    return output.getvalue()


def build_fixture(root, *, ca=TEST_CA, missing=None, modes=None):
    blobs = {}

    def descriptor(value, media):
        raw = value if isinstance(value, bytes) else json.dumps(value).encode()
        digest = oci.describe(raw)
        blobs["blobs/sha256/" + digest[7:]] = raw
        return {"digest": digest, "mediaType": media, "size": len(raw)}

    platforms = {}
    children = []
    native = []
    labels = {
        "org.opencontainers.image.revision": REVISION,
        "org.opencontainers.image.version": VERSION,
        "org.opencontainers.image.licenses": "GPL-3.0-or-later",
        "org.opencontainers.image.source": "https://github.com/" + oci.REPOSITORY,
        "org.opencontainers.image.description": oci.source_url(VERSION),
    }
    for platform, target in oci.TARGETS.items():
        arch = platform.split("/")[1]
        exe = (
            b"\x7fELF" + bytes([2, 1]) + b"\0" * 12 + (62 if arch == "amd64" else 183).to_bytes(2, "little")
        )
        files = {oci.EXE: exe, oci.CA: ca}
        files.update(
            {oci.DOCS + "/" + name: name.encode() for name in [*oci.LEGAL, "ca/LICENSE", "ca/NOTICE"]}
        )
        files[oci.DOCS + "/SOURCE.txt"] = oci.source_url(VERSION).encode()
        if missing:
            files.pop(missing)
        layer = tar_bytes({name[1:]: data for name, data in files.items()}, modes)
        config = descriptor(
            {
                "os": "linux",
                "architecture": arch,
                "rootfs": {"type": "layers", "diff_ids": [oci.describe(layer)]},
                "config": {
                    "Labels": labels,
                    "User": "65532:65532",
                    "Entrypoint": [oci.EXE],
                    "Env": ["SSL_CERT_FILE=" + oci.CA],
                },
            },
            oci.CONFIG,
        )
        child = descriptor(
            {
                "schemaVersion": 2,
                "mediaType": oci.MANIFEST,
                "config": config,
                "layers": [descriptor(gzip.compress(layer, mtime=0), oci.LAYER + "+gzip")],
            },
            oci.MANIFEST,
        )
        children.append(dict(child, platform={"os": "linux", "architecture": arch}))
        platforms[platform] = {
            "target": target,
            "manifest_digest": child["digest"],
            "config_digest": config["digest"],
            "executable_sha256": hashlib.sha256(exe).hexdigest(),
        }
        native.append(
            {
                "kind": "native",
                "target": target,
                "executable_sha256": platforms[platform]["executable_sha256"],
            }
        )
    index = descriptor(
        {
            "schemaVersion": 2,
            "mediaType": oci.INDEX,
            "manifests": children,
            "annotations": {"org.opencontainers.image.description": oci.source_url(VERSION)},
        },
        oci.INDEX,
    )
    blobs["index.json"] = json.dumps({"schemaVersion": 2, "manifests": [index]}).encode()
    blobs["oci-layout"] = b'{"imageLayoutVersion":"1.0.0"}'
    archive = root / oci.ARCHIVE
    archive.write_bytes(tar_bytes(blobs))
    receipt = {
        "schema_version": 1,
        "repository": oci.REPOSITORY,
        "revision": REVISION,
        "version": VERSION,
        "native_run": {
            "run_id": 15,
            "run_attempt": 3,
            "workflow": ".github/workflows/native-distribution.yml",
            "artifact_id": 34,
            "artifact_digest": "sha256:" + "1" * 64,
        },
        "run_id": 17,
        "run_attempt": 2,
        "image": oci.public_ref(VERSION),
        "source_url": oci.source_url(VERSION),
        "archive": {"name": oci.ARCHIVE, "bytes": archive.stat().st_size, "sha256": oci.sha_file(archive)},
        "index_digest": index["digest"],
        "platforms": platforms,
    }
    (root / oci.RECEIPT).write_text(json.dumps(receipt))
    legal = [
        {"kind": kind, "name": name, "sha256": hashlib.sha256(name.encode()).hexdigest()}
        for kind, name in [("license", "LICENSE"), ("notices", "THIRD-PARTY-NOTICES")]
    ]
    return receipt, {
        "repository": oci.REPOSITORY,
        "version": VERSION,
        "revision": REVISION,
        "run_id": 15,
        "run_attempt": 3,
        "files": native + legal,
    }


class CandidateChecks(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        ca = patch.object(oci, "CA_SHA", hashlib.sha256(TEST_CA).hexdigest())
        ca.start()
        self.addCleanup(ca.stop)
        self.receipt, self.distribution = build_fixture(self.root)

    def load(self):
        return oci.load_candidate(
            self.root, version=VERSION, revision=REVISION, distribution=self.distribution
        )

    def test_verified_archive_matches_both_native_release_files(self):
        self.assertEqual(self.load(), self.receipt)
        self.distribution["files"][1]["executable_sha256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "executable bytes"):
            self.load()

    def test_scratch_payload_is_accessible_to_the_numeric_runtime_user(self):
        for name, mode, error in [
            (oci.CA, 0o600, "unreadable"),
            (oci.DOCS + "/LICENSE", 0o600, "unreadable"),
            (oci.EXE, 0o744, "Linux ELF"),
        ]:
            build_fixture(self.root, modes={name[1:]: mode})
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, error):
                self.load()

    def test_legal_and_ca_payload_are_verified(self):
        self.distribution["files"][-1]["sha256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "legal"):
            self.load()
        for missing in (oci.CA, oci.DOCS + "/SOURCE.txt", oci.DOCS + "/ca/LICENSE"):
            build_fixture(self.root, missing=missing)
            with self.assertRaisesRegex(ValueError, "payload"):
                self.load()

    def test_receipt_tag_and_tar_checks(self):
        self.assertEqual(oci.public_ref("1.0.0-rc.1+meta.12"), oci.IMAGE + ":v1.0.0-rc.1_meta.12")
        for invalid in ("0.7.0-01", "1.0", "0.7.0+ci_bad", "0.7.0+" + "a" * 123):
            with self.assertRaises(ValueError):
                oci.public_ref(invalid)
        with (self.root / oci.ARCHIVE).open("ab") as stream:
            stream.write(b"changed")
        with self.assertRaisesRegex(ValueError, "differs from receipt"):
            self.load()
        self.receipt["image"] = "ghcr.io/other/image:v0.7.0"
        with self.assertRaisesRegex(ValueError, "destination"):
            oci.validate_receipt(self.receipt, version=VERSION, revision=REVISION)

    def test_registry_missing_requires_authenticated_conclusive_not_found(self):
        for anonymous, status, body, missing in [
            (False, 404, b'{"errors":[{"code":"MANIFEST_UNKNOWN"}]}', True),
            (False, 404, b'{"errors":[{"code":"NAME_UNKNOWN"}]}', True),
            (True, 404, b'{"errors":[{"code":"NAME_UNKNOWN"}]}', False),
            (False, 401, b"{}", False),
            (False, 404, b"{}", False),
            (False, 429, b"", False),
        ]:
            with (
                self.subTest(anonymous=anonymous, status=status, body=body),
                patch.object(registry, "access"),
                patch.object(registry, "exchange", return_value=(status, {}, body)),
            ):
                if missing:
                    self.assertIsNone(oci.read_published(receipt=self.receipt))
                else:
                    with self.assertRaisesRegex(ValueError, "unavailable"):
                        oci.read_published(receipt=self.receipt, anonymous=anonymous)

    def test_reads_index_and_chain_and_rejects_remote_conflict(self):
        with tarfile.open(self.root / oci.ARCHIVE) as archive:

            def get(method, url, **kwargs):
                ref = url.rsplit("/", 1)[-1]
                identifier = ref if ref.startswith("sha256:") else self.receipt["index_digest"]
                data = archive.extractfile("blobs/sha256/" + identifier[7:]).read()
                headers = {"Docker-Content-Digest": identifier, "Content-Length": str(len(data))}
                return 200, headers, b"" if method == "HEAD" else data

            with patch.object(registry, "access"), patch.object(registry, "exchange", side_effect=get):
                result = oci.read_published(receipt=self.receipt, anonymous=True)
                self.assertEqual(result["platforms"], self.receipt["platforms"])
                self.receipt["platforms"]["linux/amd64"]["config_digest"] = "sha256:" + "0" * 64
                with self.assertRaisesRegex(ValueError, "config differs"):
                    oci.read_published(receipt=self.receipt)

    def test_publication_recovers_lost_reply_without_overwriting(self):
        seen = {"image": self.receipt["image"]}
        for states, writes, stages in [([seen], 0, 0), ([None, seen], 0, 1), ([None, None, seen], 1, 1)]:
            with (
                self.subTest(states=states),
                patch.object(registry, "access"),
                patch.object(registry, "state", side_effect=states),
                patch.object(registry, "stage") as stage,
                patch.object(registry, "put_manifest", side_effect=RuntimeError("lost reply")) as put,
            ):
                self.assertEqual(oci.publish_candidate(self.root, receipt=self.receipt), seen)
                self.assertEqual(put.call_count, writes)
                self.assertEqual(stage.call_count, stages)
        with (
            patch.object(registry, "access"),
            patch.object(registry, "state", side_effect=[None, ValueError("conflict")]),
            patch.object(registry, "stage"),
            patch.object(registry, "put_manifest") as put,
        ):
            with self.assertRaisesRegex(ValueError, "conflict"):
                oci.publish_candidate(self.root, receipt=self.receipt)
            put.assert_not_called()

    def test_redirected_blob_upload_is_rejected_before_credentials_leave_ghcr(self):
        with patch.object(
            registry,
            "exchange",
            side_effect=[
                (404, {}, b""),
                (202, {"Location": "https://unrelated.invalid/upload"}, b""),
            ],
        ) as network:
            with self.assertRaisesRegex(ValueError, "location"):
                registry.upload_blob("fixture", oci.describe(b"fixture"), b"fixture")
            self.assertEqual(network.call_count, 2)

    def test_independent_wire_readback_after_digest_staging(self):
        blobs, manifests, writes = {}, {}, []
        missing = b'{"errors":[{"code":"MANIFEST_UNKNOWN"}]}'
        next_upload = 0

        def wire(method, url, *, authorization, headers=None, body=None):
            nonlocal next_upload
            self.assertEqual(authorization, "registry-fixture")
            path = registry.parse.urlsplit(url).path.removeprefix("/v2/" + oci.REPOSITORY)
            if path.startswith("/manifests/"):
                reference = path.rsplit("/", 1)[1]
                if method == "GET":
                    if reference not in manifests:
                        return 404, {}, missing
                    data = manifests[reference]
                    return 200, {"Docker-Content-Digest": oci.describe(data)}, data
                self.assertEqual(method, "PUT")
                model = json.loads(body)
                if model["mediaType"] == oci.MANIFEST:
                    self.assertIn(model["config"]["digest"], blobs)
                    for layer in model["layers"]:
                        self.assertIn(layer["digest"], blobs)
                    self.assertEqual(reference, oci.describe(body))
                else:
                    self.assertTrue(all(child["digest"] in manifests for child in model["manifests"]))
                manifests[reference] = body
                writes.append(reference)
                return 201, {}, b""
            if method == "HEAD":
                identifier = path.rsplit("/", 1)[1]
                if identifier not in blobs:
                    return 404, {}, b""
                return (
                    200,
                    {"Docker-Content-Digest": identifier, "Content-Length": str(len(blobs[identifier]))},
                    b"",
                )
            if method == "POST":
                self.assertEqual(path, "/blobs/uploads/")
                next_upload += 1
                return 202, {"Location": registry.BASE + f"/blobs/uploads/{next_upload}?opaque=a%2Fb"}, b""
            self.assertEqual(method, "PUT")
            self.assertIn("?opaque=a%2Fb&digest=", url)
            claimed = registry.parse.parse_qs(registry.parse.urlsplit(url).query)["digest"][0]
            self.assertEqual(claimed, oci.describe(body))
            blobs[claimed] = body
            return 201, {"Docker-Content-Digest": claimed}, b""

        with (
            patch.object(registry, "access", return_value="registry-fixture"),
            patch.object(registry, "exchange", side_effect=wire),
        ):
            first = oci.publish_candidate(self.root, receipt=self.receipt)
            second = oci.publish_candidate(self.root, receipt=self.receipt)
            self.assertEqual(first, second)
            self.assertEqual(first, oci.read_published(receipt=self.receipt, anonymous=True))
            blobs.clear()
            with self.assertRaisesRegex(ValueError, "blob unavailable"):
                oci.read_published(receipt=self.receipt, anonymous=True)
            self.assertEqual(
                writes, [p["manifest_digest"] for p in self.receipt["platforms"].values()] + ["v" + VERSION]
            )


class NativeInputChecks(unittest.TestCase):
    def setUp(self):
        from container_native import Source, WORKFLOW

        self.pr = Source(REVISION, "topic", oci.REPOSITORY, "pull_request", 17)
        self.run = {
            "id": 100,
            "run_attempt": 2,
            "status": "completed",
            "conclusion": "success",
            "path": WORKFLOW,
            "repository": {"full_name": oci.REPOSITORY},
            "head_repository": {"full_name": oci.REPOSITORY},
            "head_sha": REVISION,
            "head_branch": "topic",
            "event": "pull_request",
            "pull_requests": [{"number": 17}],
        }

    def test_wait_covers_native_dependencies_and_reserves_container_build_time(self):
        from container_native import NATIVE_WAIT_MINUTES

        def job_minutes(workflow, job):
            text = (oci.ROOT / ".github" / "workflows" / workflow).read_text()
            block = text.split("\n  " + job + ":\n", 1)[1]
            block = re.split(r"\n  \S", block, maxsplit=1)[0]
            return int(re.search(r"(?m)^    timeout-minutes: ([0-9]+)$", block)[1])

        native_path = sum(
            job_minutes("native-distribution.yml", job) for job in ("source", "native", "assemble")
        )
        self.assertGreaterEqual(NATIVE_WAIT_MINUTES, native_path)
        # The shared artifact download and Buildx command allow up to 30 and 10 minutes.
        self.assertGreaterEqual(
            job_minutes("container-distribution.yml", "assemble"), NATIVE_WAIT_MINUTES + 40
        )

    def test_pr_requires_the_exact_source_workflow_and_head(self):
        from container_native import select
        from unittest.mock import Mock

        api = Mock()
        for change in [
            {"head_branch": "main"},
            {"head_repository": {"full_name": "another/repository"}},
            {"head_sha": "f" * 40},
            {"event": "push"},
            {"path": ".github/workflows/ci.yml"},
            {"pull_requests": [{"number": 18}]},
        ]:
            api.api.return_value = [self.run | change]
            self.assertIsNone(select(api, self.pr))
        api.api.return_value = [self.run | {"status": "in_progress"}]
        self.assertIsNone(select(api, self.pr))
        api.api.return_value = [self.run | {"conclusion": "failure"}]
        with self.assertRaisesRegex(ValueError, "failed"):
            select(api, self.pr)
        api.api.return_value = [self.run]
        self.assertEqual(select(api, self.pr), self.run)

    def test_dispatch_requires_an_explicit_successful_main_run_and_attempt(self):
        from container_native import Source, select
        from unittest.mock import Mock

        api = Mock()
        main = Source(REVISION, "main", oci.REPOSITORY, "workflow_dispatch")
        api.api.return_value = self.run | {"event": "push", "head_branch": "main"}
        self.assertEqual(select(api, main, run_id=100, attempt=2)["id"], 100)
        for run_id, attempt in [(None, None), (101, 2), (100, 1)]:
            with self.assertRaises(ValueError):
                select(api, main, run_id=run_id, attempt=attempt)
        api.api.return_value = self.run
        with self.assertRaisesRegex(ValueError, "wrong source"):
            select(api, main, run_id=100, attempt=2)
