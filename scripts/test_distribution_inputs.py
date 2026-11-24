import hashlib
import io
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
from urllib.error import URLError

import distribution_inputs as source


def write_archive(path, members, mode="w:gz"):
    with tarfile.open(path, mode) as archive:
        for name, text in members.items():
            contents = text.encode()
            info = tarfile.TarInfo(name)
            info.size = len(contents)
            archive.addfile(info, io.BytesIO(contents))


class SourceTests(unittest.TestCase):
    def test_download_is_bounded_and_digest_checked(self):
        payload = b"matching source"
        spec = source.Source("nested/file", "https://example.test/source", len(payload),
                             hashlib.sha256(payload).hexdigest())
        with tempfile.TemporaryDirectory() as td:
            for body in (payload[:-1], payload + b"!", b"X" * len(payload)):
                with self.subTest(body=body), patch.object(source, "urlopen", return_value=io.BytesIO(body)):
                    with self.assertRaises(source.DistributionError):
                        source.fetch(spec, Path(td))
            with patch.object(source, "urlopen", return_value=io.BytesIO(payload)):
                self.assertEqual(source.fetch(spec, Path(td)).read_bytes(), payload)

    def test_github_fallback_uses_same_revision_and_digest(self):
        spec = source.Source("notice", source.RAW + "/example/project/abc/dir/license", 1,
                             hashlib.sha256(b"A").hexdigest())
        with tempfile.TemporaryDirectory() as td:
            with patch.object(source, "urlopen", side_effect=[URLError("unavailable"), io.BytesIO(b"A")]) as open_url:
                self.assertEqual(source.fetch(spec, Path(td)).read_bytes(), b"A")
                fallback = open_url.call_args_list[1].args[0]
                self.assertEqual(fallback.full_url, "https://api.github.com/repos/example/project/contents/dir/license?ref=abc")
                self.assertEqual(fallback.get_header("Accept"), "application/vnd.github.raw+json")
            with patch.object(source, "urlopen", return_value=io.BytesIO(b"B")) as open_url:
                with self.assertRaises(source.DistributionError):
                    source.fetch(spec, Path(td))
                self.assertEqual(open_url.call_count, 1)

    def test_partial_input_failure_leaves_no_distribution_and_preserves_existing_files(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            destination = root / "third-party-sources"
            with patch.object(source, "fetch", side_effect=URLError("interrupted")):
                with self.assertRaises(URLError):
                    source.bundle_toolchain(destination)
            self.assertEqual(list(root.iterdir()), [])
            destination.mkdir()
            kept = destination / "existing"
            kept.write_text("keep")
            with self.assertRaises(source.DistributionError):
                source.bundle_toolchain(destination)
            self.assertEqual(kept.read_text(), "keep")

    def test_bundle_contains_matching_licenses_sources_and_ca_receipt(self):
        def fixture(spec, root):
            path = root / spec.name
            path.parent.mkdir(parents=True, exist_ok=True)
            if spec == source.RUST_NOTICES:
                prefix = f"rust-src-{source.RUST}/"
                files = {prefix + name: name for name in ["COPYRIGHT", "LICENSE-MIT", "LICENSE-APACHE",
                         "rust-src/lib/rustlib/src/rust/library/compiler-builtins/LICENSE.txt",
                         "rust-src/lib/rustlib/src/rust/src/llvm-project/libunwind/LICENSE.TXT"]}
                files[prefix + "rust-src/lib/rustlib/src/rust/library/vendor/a-1.0/COPYING"] = "upstream attribution"
                files[prefix + "git-commit-hash"] = source.RUST_COMMIT
                write_archive(path, files, "w:xz")
            elif spec.name == "musl-1.2.5.tar.gz":
                write_archive(path, {"musl-1.2.5/COPYRIGHT": "musl authors"})
            elif spec.name == "musl-cross-make.tar.gz":
                write_archive(path, {f"musl-cross-make-{source.CROSS}/{key}": key for key in ("COPYRIGHT", "LICENSE")})
            else:
                path.write_text(spec.name)
            return path

        with tempfile.TemporaryDirectory() as td, patch.object(source, "fetch", side_effect=fixture):
            root = Path(td) / "third-party-sources"
            notices = source.bundle_toolchain(root)
            for part in ("musl authors", "LLVM compiler-rt", "LLVM libunwind", "compiler-builtins", "Mozilla Public License 2.0", "upstream attribution"):
                self.assertIn(part, notices)
            self.assertTrue((root / "ca/LICENSE").is_file())
            self.assertIn("perl mk-ca-bundle.pl -n", (root / "ca/NOTICE").read_text())
            inventory = (root / "SOURCES").read_text()
            for item in (*source.RUNTIME_INPUTS, *source.CA_INPUTS):
                self.assertTrue((root / item.name).is_file())
                self.assertIn(f"{item.sha256}  {item.name}  {item.url}", inventory)
            self.assertFalse((root / source.RUST_NOTICES.name).exists())
