"""Correctness tests; prepared-artifact integration cases run when available."""

import asyncio
import copy
import http.client
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

import bench
from core import Core, KEY, ROOT, SECRET, encoded, expected_inventory
from stdio import Client, PROTOCOL, child_env, project, verify

ARTIFACTS = Path(os.environ.get("BENCHMARK_ARTIFACTS", ROOT / "target/benchmark/artifacts.json"))


def valid_report():
    oracle = verify([], [])
    result = {"format": bench.FORMAT, "status": "complete", "errors": [],
              "controls": {"fixture_size": 0, "page_size": 3, "repetitions": 1, "samples": 1,
                           "cold_samples": 1, "warmup": 0, "idle_ms": 200, "rss_interval_ms": 20, "timeout": 20},
              "fixture": {"visible_stacks": 0, "expected": [], "expected_sha256": oracle["sha256"],
                          "version": "test", "seed_sha256": "a" * 64, "wire_sha256": "a" * 64,
                          "permissions": "Read", "delay_ms": 0, "delay_scope": "all responses"},
              "execution_order": [["rust", "reference"]], "implementations": {}, "core_requests": [],
              "direct_core": [{"batch": 0, "sample": 0, "elapsed_ms": 1, "http_calls": 1,
                               "response_body_bytes": 2, "equivalence": oracle}],
              "direct_version": [{"batch": 0, "sample": 0, "elapsed_ms": 1, "http_calls": 1,
                                  "response_body_bytes": 19, "equivalence": None}]}
    for key in ("run_id", "timestamp", "repository", "runner", "artifacts", "host", "workload", "definitions", "invocation"):
        result[key] = "test"
    result["repository"] = {"revision": "a" * 40, "dirty": False, "status": []}
    result["runner"] = {"version": "1", "files_sha256": {k: "a" * 64 for k in ("bench.py", "core.py", "stdio.py")}}
    result["host"] = {k: "test" for k in ("os", "system", "architecture", "python", "virtualization_note", "cpu")}
    result["host"].update(logical_cpus=1, memory_bytes=1024)
    rust = {k: "test" for k in ("argv", "rustc", "cargo", "target", "profile", "build_environment")}
    rust.update(sha256="a" * 64, bytes=1024, repository=result["repository"],
                source_sha256="a" * 64, cargo_lock_sha256="a" * 64)
    reference = {k: "test" for k in ("directory", "node", "node_version", "npm_version", "argv")}
    reference.update(repository=bench.REFERENCE_URL, release="1.5.0", revision=bench.REFERENCE_REV)
    reference.update({k: "a" * 64 for k in ("node_sha256", "package_lock_sha256", "build_sha256", "dependencies_sha256")})
    result["artifacts"] = {"format": "komodo-benchmark-artifacts/v1", "prepared_at": "test", "commands": [],
                           "rust": rust, "reference": reference}
    result["workload"] = {k: "test" for k in ("name", "rust_tool", "reference_tool", "arguments", "logging", "environment")}
    result["workload"].update(protocol=PROTOCOL, concurrency=1)
    result["definitions"] = {k: "test" for k in ("startup", "warm", "response_bytes", "percentiles", "direct_core", "fixture_service", "rss")}
    for name in ("rust", "reference"):
        result["core_requests"].append({"implementation": name, "batch": 0, "sample": 0,
                                        "phase": "warm", "operation": "ListStacks", "status": 200,
                                        "response_body_bytes": 2, "service_ms": .1, "route": "/read",
                                        "params": {}, "activity": "foreground"})
        result["implementations"][name] = {
            "cold": [{"batch": 0, "sample": 0, "elapsed_ms": 1, "protocol": PROTOCOL,
                      "response_bytes": 100, "core_requests": 0, "core_operations": {}}],
            "warmup": [],
            "warm": [{"batch": 0, "sample": 0, "elapsed_ms": 1, "response_bytes": 100,
                      "mcp_calls": 1, "core_requests": 1, "core_operations": {"ListStacks": 1},
                      "equivalence": oracle}],
            "memory": [{"batch": 0, "idle_rss_bytes": 1024, "peak_sampled_rss_bytes": 2048,
                        "samples": [{"rss_bytes": 2048, "since_start_ms": 1}]}],
            "warm_sessions": [{"batch": 0, "protocol": PROTOCOL}],
            "processes": [{"returncode": 0}] * 3,
        }
    for phase, operation, size in (("direct", "ListStacks", 2), ("direct_version", "GetVersion", 19)):
        result["core_requests"].append({"implementation": "direct_core", "batch": 0, "sample": 0, "phase": phase,
                                        "operation": operation, "status": 200, "response_body_bytes": size,
                                        "service_ms": .1, "route": "/read", "params": {}, "activity": "foreground"})
    result["summary"] = bench.summaries(result)
    return result


class PureTests(unittest.TestCase):
    def test_executable_preserves_multicall_symlink_name(self):
        with tempfile.TemporaryDirectory(prefix="toolchain paths ") as temp:
            directory = Path(temp)
            proxy = directory / "multicall"
            proxy.write_text('#!/bin/sh\nprintf "%s\\n" "${0##*/}"\n')
            proxy.chmod(0o755)
            for name in ("rustc", "cargo", "npm"):
                link = directory / name
                link.symlink_to(proxy.name)
                self.assertEqual(bench.executable(link), str(link))
                self.assertEqual(bench.command([bench.executable(link), "--version"]), name)

    def test_percentile_nearest_rank(self):
        self.assertEqual(bench.percentile([4, 1, 3, 2], .5), 2)
        self.assertEqual(bench.percentile(list(range(1, 101)), .95), 95)
        self.assertEqual(bench.percentile([7], .95), 7)
        with self.assertRaises(ValueError):
            bench.percentile([], .95)

    def test_mismatch_duplicate_and_unknown_state_are_failures(self):
        expected = expected_inventory(7)
        self.assertTrue(verify(project(expected[::-1]), expected)["verified"])
        for bad in (expected[:-1], expected + expected[:1],
                    [{**expected[0], "name": "unexpected"}, *expected[1:]]):
            with self.assertRaises(ValueError):
                verify(project(bad), expected)
        with self.assertRaises(ValueError):
            project([{**expected[0], "state": "future-state"}])
        with self.assertRaises(ValueError):
            project([{ "id": "id", "name": "name"}])

    def test_missing_reference_artifacts_are_actionable(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "artifacts.json"
            with self.assertRaisesRegex(ValueError, "prepare"):
                bench.preflight(path)
            bench.write_json(path, {"format": "komodo-benchmark-artifacts/v1",
                                   "rust": {"argv": [sys.executable], "sha256": bench.sha(sys.executable)},
                                   "reference": {"directory": temp}})
            with self.assertRaisesRegex(ValueError, "reference.*missing"):
                bench.preflight(path)

    def test_incomplete_or_invalid_archive_rejected(self):
        original = valid_report()
        bench.validate(original)
        for mutate in (
            lambda x: x.update(status="failed"),
            lambda x: x.pop("host"),
            lambda x: x["implementations"].pop("reference"),
            lambda x: x["implementations"]["rust"]["warm"].clear(),
            lambda x: x["implementations"]["rust"]["warm"][0].update(elapsed_ms=float("nan")),
            lambda x: x["implementations"]["reference"]["warm"][0].update(core_requests=2),
            lambda x: x["implementations"]["reference"]["warm"][0]["equivalence"].update(count=1),
            lambda x: x["implementations"]["rust"]["memory"][0].update(idle_rss_bytes=None),
            lambda x: x["direct_core"].clear(),
            lambda x: x["summary"]["rust"]["warm_ms"].update(p50=12),
            lambda x: x["artifacts"]["reference"].pop("package_lock_sha256"),
            lambda x: x["direct_version"][0].update(http_calls=4),
        ):
            with self.subTest(mutate=mutate):
                result = copy.deepcopy(original)
                mutate(result)
                with self.assertRaises((ValueError, KeyError)):
                    bench.validate(result)

    def test_environment_does_not_inherit_credentials_or_node_options(self):
        env = child_env(Path("/tmp/synthetic home"), "http://127.0.0.1:1")
        self.assertNotIn("NODE_OPTIONS", env)
        self.assertNotIn("KOMODO_JWT_TOKEN", env)
        self.assertEqual(env["KOMODO_API_KEY"], KEY)
        self.assertEqual(env["MCP_TRANSPORT"], "stdio")


class FixtureTests(unittest.TestCase):
    def request(self, core, operation, params=None, credentials=True):
        connection = http.client.HTTPConnection("127.0.0.1", core.http.server_port, timeout=2)
        try:
            headers = {"x-api-key": KEY, "x-api-secret": SECRET} if credentials else {}
            connection.request("POST", "/read", encoded({"type": operation, "params": params or {}}), headers)
            response = connection.getresponse()
            return response.status, json.loads(response.read())
        finally:
            connection.close()

    def test_independent_requests_filters_sort_pagination_permissions(self):
        with Core(6) as core:
            all_items = self.request(core, "ListStacks", {"limit": 0})[1]
            second = self.request(core, "ListStacks", {"page": 1, "limit": 3})[1]
            self.assertEqual(second, all_items[3:])
            self.assertEqual(self.request(core, "ListStacks", {"limit": 3, "page": 2})[1], [])
            self.assertEqual(self.request(core, "ListStacks", {"limit": 0, "sort_desc": True})[1], all_items[::-1])
            self.assertEqual(self.request(core, "ListStacks", {"query": {"names": [all_items[0]["id"]]}})[1], all_items[:1])
            self.assertEqual(self.request(core, "ListStacks", {"query": {"terms": ["STACK-000005"]}})[1], all_items[5:])
            self.assertEqual(self.request(core, "ListStacks", {"query": {"tags": ["absent"]}})[1], [])
            self.assertEqual(self.request(core, "DeleteStack")[0], 403)
            self.assertEqual(self.request(core, "ListStacks", credentials=False)[0], 401)
            self.assertEqual(self.request(core, "ListStacks", {"query": {"specific": {"bogus": "value"}}})[0], 400)

    def test_accounting_captures_entry_phase_including_inflight_request(self):
        from concurrent.futures import ThreadPoolExecutor
        with Core(0, delay_ms=50) as core, ThreadPoolExecutor() as pool:
            core.mark("rust", 0, "startup", 0)
            pending = pool.submit(self.request, core, "GetVersion")
            with core.changed:
                self.assertTrue(core.changed.wait_for(lambda: core.active > 0, timeout=2))
            core.mark("rust", 0, "warm", 0)
            self.assertEqual(pending.result()[0], 200)
            self.request(core, "GetVersion")  # Background health must remain visible.
            self.request(core, "ListStacks", {"limit": 0})
            core.mark("driver", None, "finished")
            self.assertEqual([x["phase"] for x in core.events], ["startup", "warm", "warm"])
            self.assertEqual(core.events[1]["activity"], "background_health")
            self.assertTrue(all(x["service_ms"] >= 40 for x in core.events))
            self.assertEqual(core.events[-1]["response_body_bytes"], 2)


class ClientTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.directory = tempfile.TemporaryDirectory(prefix="benchmark test spaces ")
        self.home = Path(self.directory.name)
        (self.home / "config.toml").write_text("")

    async def asyncTearDown(self):
        self.directory.cleanup()

    def scripted(self, code, timeout=.3):
        script = self.home / "fake server with spaces.py"
        script.write_text(code)
        return Client([sys.executable, str(script)], child_env(self.home, "http://127.0.0.1:1"),
                      self.home, timeout)

    async def test_actual_response_bytes_and_concurrent_stderr_drain(self):
        code = '''import sys, json
request = json.loads(sys.stdin.readline())
sys.stderr.write("x" * 200000)
sys.stderr.flush()
print('{"jsonrpc":"2.0","method":"notifications/message","params":{}}', flush=True)
print(json.dumps({"jsonrpc":"2.0", "id":request["id"], "result":{"text":"é"}}, ensure_ascii=False), flush=True)
sys.stdin.read()
'''
        client = self.scripted(code, timeout=3)
        try:
            await client.spawn()
            value, size = await client.request("test", {})
            expected = json.dumps({"jsonrpc": "2.0", "id": "1", "result": {"text": "é"}}, ensure_ascii=False).encode() + b"\n"
            self.assertEqual(value, {"text": "é"})
            self.assertEqual(size, len(expected))
        finally:
            await client.close()
        self.assertEqual(client.stderr_bytes, 200000)
        self.assertEqual(len(client.notifications), 1)
        self.assertEqual(client.process.returncode, 0)

    async def test_startup_failure_and_timeout_reap_process(self):
        for code, exception in (("import sys; sys.exit(42)", EOFError),
                                ("import time; time.sleep(30)", TimeoutError),
                                ('print("not-json", flush=True)', ValueError)):
            with self.subTest(code=code):
                client = self.scripted(code)
                try:
                    with self.assertRaises(exception):
                        await client.initialize()
                finally:
                    await client.close(failed=True)
                self.assertIsNotNone(client.process.returncode)
                with self.assertRaises(ProcessLookupError):
                    os.kill(client.process.pid, 0)

    async def test_request_timeout_and_cancellation_cleanup(self):
        code = '''import sys, json, time
request = json.loads(sys.stdin.readline())
print(json.dumps({"jsonrpc":"2.0", "id":request["id"], "result":{
"protocolVersion":"2025-03-26", "serverInfo":{"name":"fake", "version":"1"}, "capabilities":{"tools":{}}}}), flush=True)
time.sleep(30)
'''
        for cancel in (False, True):
            client = self.scripted(code)
            try:
                await client.initialize()
                task = asyncio.create_task(client.request("tools/call", {}))
                if cancel:
                    await asyncio.sleep(.02)
                    task.cancel()
                with self.assertRaises(asyncio.CancelledError if cancel else TimeoutError):
                    await task
            finally:
                await client.close(failed=True)
            self.assertIsNotNone(client.process.returncode)

    async def test_adapter_rejects_mismatch_errors_and_nonterminating_pages(self):
        expected = expected_inventory(1)
        for implementation in ("rust", "reference"):
            for failure in ("mismatch", "tool_error", "pagination"):
                client = Client([], {}, self.home)
                item = {**expected[0], "name": "wrong"} if failure == "mismatch" else expected[0]
                payload = {"items": [item], "page": 0, "page_size": 1, "has_more": False, "next_page": None}
                if implementation == "reference":
                    payload["page"] = {"total": 1}
                if failure == "pagination":
                    if implementation == "rust":
                        payload.update(has_more=True, next_page=1)
                    else:
                        payload["page"]["next_cursor"] = "repeating-cursor"

                async def request(*_args):
                    return {"structuredContent": payload, "isError": failure == "tool_error"}, 100

                client.request = request
                with self.subTest(implementation=implementation, failure=failure), self.assertRaises(ValueError):
                    await client.inventory(implementation, 1, expected)


@unittest.skipUnless(ARTIFACTS.is_file(), "prepare artifacts to run real Rust/reference integration cases")
class PreparedIntegrationTests(unittest.IsolatedAsyncioTestCase):
    async def test_empty_exact_boundary_and_multiple_pages(self):
        artifacts = await asyncio.to_thread(bench.preflight, ARTIFACTS)
        for count in (0, 6, 7):
            with self.subTest(count=count), tempfile.TemporaryDirectory(prefix="inventory spaces ") as temp, Core(count) as core:
                home = Path(temp)
                (home / "config.toml").write_text("")
                for name in bench.packaged.participants(artifacts):
                    core.mark(name, 0, "startup")
                    client = Client(artifacts[name]["argv"], child_env(home, core.address), home)
                    try:
                        await client.initialize()
                        core.mark(name, 0, "warm", 0)
                        sample = await client.inventory(name, 3, core.expected)
                        self.assertEqual(sample["equivalence"]["count"], count)
                        # Raw ListStacks has no next cursor. A full last page needs one final read.
                        pages = count // 3 + 1 if name == "rust_reference" else max(1, (count + 2) // 3)
                        self.assertEqual(sample["mcp_calls"], pages)
                        self.assertGreater(sample["response_bytes"], 0)
                    finally:
                        await client.close()
                direct = await asyncio.to_thread(bench.direct_reads, core, 3, 1, 0, 0)
                self.assertEqual(direct[0]["equivalence"]["count"], count)


if __name__ == "__main__":
    unittest.main()
