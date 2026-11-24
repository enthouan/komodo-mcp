"""Offline checks for candidate archive identities, trust boundaries, and portable extraction."""
import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

from distribution import DistributionError, MANIFEST, TARGETS, load_manifest, sha256, stem
from distribution_archive import archive, extract
from distribution_prepare import assemble


class DistributionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.version, self.revision = "0.7.0", "a" * 40
        self.source, self.native, self.bundle = (self.root/name for name in ("source","native","bundle"))
        self.source.mkdir()
        self.native.mkdir()
        for name in (stem(self.version,"source")+".tar.gz", "LICENSE", "THIRD-PARTY-NOTICES"):
            (self.source/name).write_bytes(("fixture "+name).encode())
        for target in TARGETS:
            base = stem(self.version,target)
            directory = self.root/base
            directory.mkdir()
            for name in ("LICENSE","THIRD-PARTY-NOTICES"):
                (directory/name).write_bytes((self.source/name).read_bytes())
            binary = directory/"komodo-mcp"
            binary.write_bytes(target.encode())
            binary.chmod(0o755)
            (directory/"README.md").write_text(self.revision)
            output = self.native/(base+".tar.gz")
            archive(directory,output)
            file = {"kind":"native","name":output.name,"target":target,"bytes":output.stat().st_size,
                    "sha256":sha256(output),"executable_sha256":sha256(binary)}
            if target.endswith("-darwin"):
                file["minimum_macos"] = "15.0"
            (self.native/(target+".json")).write_text(json.dumps({"revision":self.revision,"version":self.version,
                       "native":True,"mcp_version":self.version,"file":file}))
        assemble(self.source,self.native,self.bundle,self.revision,self.version,800,2,TARGETS)

    def validate(self, **changes):
        args = dict(revision=self.revision, version=self.version, targets=TARGETS, run_id=800, run_attempt=2)
        return load_manifest(self.bundle, **(args | changes))

    def test_full_bundle_and_oci_consumer_subset(self):
        data = self.validate(targets=TARGETS[:2])
        self.assertEqual(len(data["files"]),8)
        self.assertEqual({line.split("  ")[1] for line in (self.bundle/"SHA256SUMS").read_text().splitlines()},
                         {e["name"] for e in data["files"] if e["kind"] != "checksums"})

    def test_wrong_revision_version_or_run_rejected(self):
        for change in ({"revision":"b"*40},{"version":"0.8.0"},{"run_id":801},{"run_attempt":1}):
            with self.subTest(change=change), self.assertRaises(DistributionError):
                self.validate(**change)

    def test_rejects_substituted_assets_and_unsafe_names(self):
        manifest = self.bundle/MANIFEST
        original = manifest.read_text()
        for field,value in (("sha256","0"*64),("bytes",0),("name","../source.tar.gz")):
            data = json.loads(original)
            data["files"][0][field] = value
            manifest.write_text(json.dumps(data))
            with self.subTest(field=field), self.assertRaises(DistributionError):
                self.validate()
        manifest.write_text(original)
        (self.bundle/"extra").symlink_to(self.bundle/"LICENSE")
        with self.assertRaises(DistributionError):
            self.validate()

    def test_checksums_need_complete_license_and_source_coverage(self):
        sums = self.bundle/"SHA256SUMS"
        sums.write_text("".join(line for line in sums.read_text().splitlines(True) if not line.endswith("  LICENSE\n")))
        data=json.loads((self.bundle/MANIFEST).read_text())
        entry=next(e for e in data["files"] if e["kind"]=="checksums")
        entry.update(sha256=sha256(sums),bytes=sums.stat().st_size)
        (self.bundle/MANIFEST).write_text(json.dumps(data))
        with self.assertRaisesRegex(DistributionError,"coverage"):
            self.validate()

    def test_archive_is_deterministic_and_preserves_executable_mode(self):
        base=stem(self.version,TARGETS[0])
        copied=self.root/"again.tar.gz"
        archive(self.root/base,copied)
        self.assertEqual(sha256(copied),sha256(self.native/(base+".tar.gz")))
        result=extract(copied,self.root/"extracted",base)
        self.assertEqual((result/"komodo-mcp").stat().st_mode & 0o777,0o755)

    def test_source_extract_rejects_traversal_and_links(self):
        for i,(name,kind) in enumerate((("source/../escape",tarfile.REGTYPE),("source/link",tarfile.SYMTYPE))):
            output=self.root/f"unsafe-{i}.tar"
            with tarfile.open(output,"w") as stream:
                info=tarfile.TarInfo(name)
                info.type,info.size=kind,0
                stream.addfile(info, io.BytesIO(b""))
            with self.assertRaises(DistributionError):
                extract(output,self.root/f"test-{i}","source")


if __name__ == "__main__":
    unittest.main()
