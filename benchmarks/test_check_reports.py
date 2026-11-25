"""Regression coverage for raw reports staged outside the benchmark directory."""

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from bench import write_json


CHECK = Path(__file__).with_name("check_reports.py").resolve()


class PublicationTests(unittest.TestCase):
    def test_recognizes_staged_reports_by_content(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(["git", "init", "-q", directory], check=True)
            reports = root / "elsewhere"
            reports.mkdir()
            cases = [
                ("inventory.txt", "inventory.txt", "komodo-inventory-benchmark/v1"),
                ("evidence.gz", "evidence.gz", "komodo-incident-benchmark/v1"),
                ("live.json", "renamed-live", "komodo-live-inventory-benchmark/v1"),
                ("manifest.gz", "renamed", "komodo-benchmark-artifacts/v1"),
            ]
            for name, staged_name, schema in cases:
                with self.subTest(name=staged_name):
                    path = reports / name
                    staged = reports / staged_name
                    # Put the format key across a read boundary.
                    write_json(path, {"padding": "x" * 65512, "format": schema})
                    path.rename(staged)
                    subprocess.run(["git", "add", "--", staged.name], cwd=reports, check=True)
                    staged.write_text("different unstaged contents")
                    result = subprocess.run([sys.executable, CHECK], cwd=reports, capture_output=True)
                    self.assertEqual(result.returncode, 1, result.stderr)
                    self.assertIn(b"raw benchmark report", result.stderr)
                    subprocess.run(["git", "rm", "--cached", "-qf", "--", staged.name], cwd=reports, check=True)

            # The source declaring a report format is safe to stage.
            source = root / "example.py"
            source.write_text('report = {"format": "komodo-inventory-benchmark/v1"}\n')
            subprocess.run(["git", "add", "--", source.name], cwd=root, check=True)
            subprocess.run([sys.executable, CHECK], cwd=reports, check=True)
