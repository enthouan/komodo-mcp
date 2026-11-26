"""Verify release policy independently of the signer and of the original Actions artifact."""
import copy
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import stable_evidence as evidence
from distribution import DistributionError
from release_artifacts import Packages, ReleaseError
import test_release_packages as package_fixtures

REVISION = package_fixtures.REVISION


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.directory = Path(self.tmp.name)
        self.record = {"index_digest": "sha256:" + "1"*64, "platforms": {
            "linux/" + arch: {"manifest_digest": "sha256:" + d*64} for arch, d in (("amd64", "2"), ("arm64", "3"))}}
        self.digests = evidence.subjects("oci", self.record)
        self.bom = {"bomFormat": "CycloneDX", "specVersion": "1.6", "metadata": {"component": {"version": "1.0.0"}},
                    "components": [evidence.component(name, "subject:"+name, d) for name, d in self.digests.items()]}
        self.path = self.directory / evidence.names("oci")[0]
        self.path.write_text(json.dumps(self.bom))
        self.value = evidence.statement("oci", "1.0.0", REVISION,
                                        self.digests | {self.path.name: evidence.sha256(self.path)}, 10, 2)

    def verify(self, statement, *, failure=None):
        with patch("stable_evidence.subprocess.check_output", side_effect=failure,
                   return_value=json.dumps([{"verificationResult": {"statement": statement}}])) as gh:
            result = evidence.verify("oci", self.directory, "1.0.0", REVISION, self.digests,
                                     run={"run_id": 10, "run_attempt": 2})
            command = gh.call_args.args[0]
            self.assertEqual(command[command.index("--source-digest")+1], REVISION)
            self.assertEqual(command[command.index("--cert-identity")+1],
                             evidence.REPO_URL+"/.github/workflows/container-distribution.yml@refs/heads/main")
            return result

    def test_native_and_oci_sboms_distinguish_compiled_crates_from_full_source(self):
        (self.directory / "Cargo.lock").write_text("""
[[package]]
name = "komodo-mcp"
version = "1.0.0"
[[package]]
name = "built"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
[[package]]
name = "source-only"
version = "2.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
""")
        files = [{"name": "source.tar.gz", "kind": "source", "sha256": "e"*64}]
        for arch, data in self.record["platforms"].items():
            target = {"linux/amd64": "x86_64-unknown-linux-musl", "linux/arm64": "aarch64-unknown-linux-musl"}[arch]
            data["target"] = target
            name = target + ".tar.gz"
            with tarfile.open(self.directory / name, "w:gz") as archive:
                body = b'["built-1.0.0"]'
                member = tarfile.TarInfo(target + "/BUILD-INPUTS.json")
                member.size = len(body)
                archive.addfile(member, io.BytesIO(body))
            files.append({"name": name, "kind": "native", "target": target, "sha256": evidence.sha256(self.directory / name)})
        metadata = {"version": "1.0.0", "revision": REVISION, "run_id": 7, "run_attempt": 2}
        (self.directory / evidence.MANIFEST).write_text(json.dumps(metadata | {"files": files}))
        (self.directory / "oci-receipt.json").write_text(json.dumps(metadata | self.record))
        for scope in evidence.WORKFLOWS:
            with self.subTest(scope=scope), patch.object(evidence, "ROOT", self.directory):
                output = self.directory / scope
                evidence.prepare(scope, self.directory, self.directory, output)
                name = evidence.names(scope)[0]
                bom = evidence.read_json(output / name)
                graph = {d["ref"]: d["dependsOn"] for d in bom["dependencies"]}
                source_only = "pkg:cargo/source-only@2.0.0"
                if scope == "native":
                    self.assertIn(source_only, graph["subject:source.tar.gz"])
                    self.assertNotIn(source_only, graph["subject:x86_64-unknown-linux-musl.tar.gz"])
                else:
                    self.assertNotIn(source_only, {c["bom-ref"] for c in bom["components"]})
                    self.assertIn("pkg:cargo/built@1.0.0", graph["subject:"+evidence.IMAGE+"#linux/arm64"])
                value = evidence.read_json(output / "statement.json")
                digests = {s["name"]: s["digest"]["sha256"] for s in value["subject"]}
                self.assertEqual(digests.pop(name), evidence.sha256(output / name))
                with patch("stable_evidence.subprocess.check_output",
                           return_value=json.dumps([{"verificationResult": {"statement": value}}])):
                    evidence.verify(scope, output, "1.0.0", REVISION, digests)

    def test_signed_build_policy_and_all_subject_digests(self):
        self.assertEqual(self.verify(self.value)["subjects"], self.digests)
        for field in ("subject", "source", "builder", "run", "version"):
            value = copy.deepcopy(self.value)
            predicate = value["predicate"]
            match field:
                case "subject":
                    value["subject"][0]["digest"]["sha256"] = "0"*64
                case "source":
                    predicate["buildDefinition"]["resolvedDependencies"][0]["digest"]["gitCommit"] = "b"*40
                case "builder":
                    predicate["runDetails"]["builder"]["id"] = "https://example.invalid/workflow"
                case "run":
                    predicate["runDetails"]["metadata"]["invocationId"] += "0"
                case "version":
                    predicate["buildDefinition"]["externalParameters"]["version"] = "0.9.0"
            with self.subTest(field=field), self.assertRaises(DistributionError):
                self.verify(value)
        with self.assertRaises(subprocess.CalledProcessError):
            self.verify(self.value, failure=subprocess.CalledProcessError(1, "gh"))

    def test_authenticated_sbom_still_has_to_describe_the_selected_subjects(self):
        self.bom["components"][0]["hashes"][0]["content"] = "9"*64
        self.path.write_text(json.dumps(self.bom))
        updated = evidence.statement("oci", "1.0.0", REVISION, self.digests | {self.path.name: evidence.sha256(self.path)}, 10, 2)
        with self.assertRaisesRegex(DistributionError, "SBOM subjects"):
            self.verify(updated)


class StableReleaseTests(unittest.TestCase):
    def test_release_and_readback_require_exact_new_public_assets(self):
        # Reuse the tag/draft/registry fixture used for 0.x publication.
        fixture = package_fixtures.PackageTests()
        fixture.setup_release("1.0.0")
        identity = fixture.identity
        with self.assertRaisesRegex(ReleaseError, "missing stable"):
            Packages.check_identity(identity, REVISION, "1.0.0")
        identity["stable_evidence"] = {}
        for scope in evidence.WORKFLOWS:
            files = [{"name": name, "bytes": len(name), "sha256": hashlib.sha256(name.encode()).hexdigest()}
                     for name in evidence.names(scope)]
            identity["stable_evidence"][scope] = {"run": identity[scope+"_run"] | {"artifact_id": 99}, "files": files}
            fixture.hub.files.update({f["name"]: f for f in files})
        Packages.check_identity(identity, REVISION, "1.0.0")
        def read_downloads(assets, version, revision):
            self.assertEqual(set(assets), set(fixture.hub.files))
            fixture.source_read = True
            return {"oci": "verified", "native": "verified"}
        with patch.object(fixture.packages, "verify_stable_downloads", side_effect=read_downloads) as verify:
            result = fixture.run_helper()
            self.assertEqual(result["schema"], "komodo-release-publication/v3")
            self.assertEqual(len(result["assets"]), 12)
            self.assertEqual(result["verified_provenance"]["oci"], "verified")
            self.assertEqual(verify.call_count, 1)
            writes = fixture.writes.copy()
            fixture.run_helper(False, result)
            self.assertEqual(verify.call_count, 2)
            self.assertEqual(fixture.writes, writes)
