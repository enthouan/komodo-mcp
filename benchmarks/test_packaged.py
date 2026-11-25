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
