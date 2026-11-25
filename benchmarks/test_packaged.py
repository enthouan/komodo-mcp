"""Regression cases for candidate provenance, public samples and comparator scope."""
import asyncio
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
from types import SimpleNamespace

import bench
import export_samples
import incident
import packaged
from rust_reference import Reader
import test_bench
from stdio import Client


class WireClient:
    def __init__(self, core):
        self.core = core

    async def request(self, method, request):
        assert method == "tools/call" and request["name"] == "komodo_read"
        args = request["arguments"]
        status, rows = self.core.read(args["request"], args["params"])
        self.last = args
        body = json.dumps(rows)
        return {"isError": status != 200, "content": [{"type": "text", "text": body}]}, len(body)


class PackagedTests(unittest.TestCase):
    def test_rejects_foreign_native_targets_before_preparing_artifacts(self):
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            args = SimpleNamespace(reference_dir=directory / "ts", node="node", npm="npm", cargo="cargo",
                                   rustc="rustc", candidate_binary=directory / "binary", candidate_record=directory / "record",
                                   candidate_target="x86_64-apple-darwin", rust_reference_dir=directory / "rust",
                                   artifacts=directory / "artifacts.json")
            outputs = ["node", "v24.18.0", tmp, "rustc 1.99.1\nhost: aarch64-apple-darwin", "arm64"]
            with patch.object(bench, "command", side_effect=outputs), patch.object(bench, "executable", side_effect=str), \
                    patch.object(bench.platform, "system", return_value="Darwin"), \
                    patch.object(bench.platform, "machine", return_value="arm64"), \
                    patch.object(packaged, "candidate") as prepare_candidate:
                with self.assertRaisesRegex(ValueError, "candidate target differs"):
                    bench.prepare(args)
                prepare_candidate.assert_not_called()
            self.assertFalse(args.artifacts.exists())

    def test_native_linux_libcs_and_mismatched_comparators(self):
        target = "aarch64-unknown-linux-musl"
        for libc in ("gnu", "musl"):
            packaged.check_host_targets(target, f"aarch64-unknown-linux-{libc}", "Linux", "aarch64", "arm64")
        with self.assertRaisesRegex(ValueError, "Rust comparator target differs"):
            packaged.check_host_targets(target, "x86_64-unknown-linux-gnu", "Linux", "arm64")
        with self.assertRaisesRegex(ValueError, "Node runtime architecture differs"):
            packaged.check_host_targets(target, target, "Linux", "arm64", "x64")

    def test_reference_pages_and_diagnostic_evidence(self):
        with incident.IncidentCore(7) as fixture:
            reader = Reader(WireClient(fixture))
            self.assertEqual(asyncio.run(reader.diagnose(3, 7)), incident.expected_evidence(7))
            self.assertEqual(reader.calls, 7)
            self.assertGreater(reader.response_bytes, 0)

    def test_mismatched_candidate_binary_stops_preparation(self):
        with tempfile.TemporaryDirectory() as tmp:
            binary, record = Path(tmp) / "exe", Path(tmp) / "candidate.json"
            binary.write_bytes(b"unrelated binary")
            binary.chmod(0o700)
            record.write_text(json.dumps({"distribution": {"revision": "f" * 40, "version": "0.8.0"}}))
            args = SimpleNamespace(candidate_binary=binary, candidate_record=record, candidate_target="darwin")
            with patch.object(packaged, "packaged_entry", return_value={"executable_sha256": "a" * 64}):
                with self.assertRaisesRegex(ValueError, "differs"):
                    packaged.candidate(args)

    def test_numeric_export_drops_private_fields(self):
        report = test_bench.valid_report()
        report["host"].update(load_average=[1., 0.5, 0.9], system="Linux", architecture="x86_64")
        marker = "synthetic-private-command-or-local-path"
        report["host"]["virtualization_note"] = marker
        report["artifacts"]["commands"] = [marker]
        report["implementations"]["rust"]["warm"][0]["private"] = marker
        result = export_samples.export(report)
        self.assertNotIn(marker, json.dumps(result))
        self.assertEqual(result["implementations"]["candidate"]["warm"][0]["elapsed_ms"],
                         report["implementations"]["rust"]["warm"][0]["elapsed_ms"])

    def test_closed_pipe_is_an_mcp_eof(self):
        class Closed:
            def write(self, _data):
                pass

            async def drain(self):
                raise ConnectionResetError

        client = Client([], {}, None)
        client.process = SimpleNamespace(stdin=Closed())
        with self.assertRaisesRegex(EOFError, "stdin closed"):
            asyncio.run(client.send({}))


if __name__ == "__main__":
    unittest.main()
