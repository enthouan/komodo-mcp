"""Incident equivalence, grouped concurrency/accounting and real stdio adapters."""

import asyncio
import copy
import json
import os
import tempfile
import unittest
from unittest.mock import patch
from pathlib import Path
from types import SimpleNamespace

import bench
import incident
from core import KEY, ROOT, SECRET, digest
from stdio import Client, PROTOCOL, child_env
import test_bench

ARTIFACTS = Path(os.environ.get("BENCHMARK_ARTIFACTS", ROOT / "target/benchmark/artifacts.json"))


def valid_report():
    original = test_bench.valid_report()
    expected = incident.expected_evidence(2)
    report = {**original, "format": incident.FORMAT, "finished_at": "test",
              "controls": {**original["controls"], "fixture_size": 2, "concurrency": 2},
              "fixture": {**original["fixture"], "visible_stacks": 2, "expected": expected,
                          "expected_sha256": digest(expected), "wire_seed_sha256": "a" * 64},
              "workload": {"name": "test", "protocol": PROTOCOL, "concurrency": 2,
                           "rust": ["fleet_health"], "reference": ["komodo_stack_list"],
                           "equivalence": "test", "environment": "test"},
              "definitions": {k: "test" for k in ("warm", "concurrency", "response_bytes", "core_requests", "rss", "percentiles", "limits")},
              "implementations": {}, "core_requests": []}
    report["runner"]["files_sha256"]["incident.py"] = "a" * 64
    for name in ("rust", "reference"):
        callers = [{"elapsed_ms": 1, "mcp_calls": 4, "response_bytes": 100, "observed": expected,
                    "equivalence": {"sha256": digest(expected), "verified": True}, "additional_evidence": {}} for _ in range(2)]
        report["implementations"][name] = {
            "warm_sessions": [{"batch": 0, "protocol": PROTOCOL}], "warmup": [],
            "warm": [{"batch": 0, "sample": 0, "elapsed_ms": 2, "callers": callers, "mcp_calls": 8,
                      "response_bytes": 200, "core_requests": 1, "core_response_bytes": 2,
                      "core_operations": {"ListStacks": 1}, "peak_observed_core_concurrency": 2}],
            "memory": [{**original["implementations"][name]["memory"][0], "protocol": PROTOCOL}],
            "processes": [{"returncode": 0}, {"returncode": 0}],
        }
        report["core_requests"].append({"implementation": name, "batch": 0, "phase": "warm", "sample": 0,
                                        "operation": "ListStacks", "params": {}, "route": "/read", "status": 200,
                                        "response_body_bytes": 2, "concurrency_at_entry": 2, "service_ms": .1,
                                        "activity": "foreground"})
    report["summary"] = incident.summaries(report)
    return report


class IncidentPureTests(unittest.TestCase):
    def test_report_validation_dispatch_and_recomputed_group_accounting(self):
        original = valid_report()
        self.assertIs(bench.validate(original), original)
        for mutate in (
            lambda r: r.update(status="failed"),
            lambda r: r.pop("host"),
            lambda r: r["artifacts"]["rust"].pop("source_sha256"),
            lambda r: r["artifacts"]["reference"].update(revision="b" * 40),
            lambda r: r["runner"]["files_sha256"].pop("incident.py"),
            lambda r: r["controls"].update(concurrency=0),
            lambda r: r["implementations"].pop("reference"),
            lambda r: r["implementations"]["rust"]["warm"][0].update(core_requests=9),
            lambda r: r["implementations"]["reference"]["warm"][0].update(response_bytes=9),
            lambda r: r["implementations"]["rust"]["warm"][0]["callers"].pop(),
            lambda r: r["implementations"]["rust"]["warm"][0]["callers"][0]["observed"].update(service="wrong"),
            lambda r: r["implementations"]["rust"]["memory"][0].update(peak_sampled_rss_bytes=1),
            lambda r: r["core_requests"][0].update(operation="DeployStack"),
            lambda r: r["core_requests"][0].update(status=500),
            lambda r: r["summary"]["rust"]["warm_group_ms"].update(p50=500),
        ):
            candidate = copy.deepcopy(original)
            mutate(candidate)
            with self.subTest(mutate=mutate), self.assertRaises((ValueError, KeyError)):
                bench.validate(candidate)

    def test_fixture_growth_preserves_the_seed_and_read_only_contract(self):
        request = test_bench.FixtureTests().request
        for count in (2, 7, 125):
            with self.subTest(count=count), incident.IncidentCore(count) as core:
                status, stacks = request(core, "ListStacks", {"limit": 0})
                self.assertEqual(status, 200)
                self.assertEqual(len(stacks), count)
                broken = [s for s in stacks if s["info"]["state"] == "unhealthy"]
                self.assertEqual([s["id"] for s in broken], [core.expected["stack_id"]])
                self.assertEqual(len(core.servers), count + 2)
                status, services = request(core, "ListAllStackServices", {"stacks": [core.expected["stack_id"]], "limit": 101})
                self.assertEqual(status, 200)
                self.assertEqual([s["service"] for s in services if s["state"] == "Unhealthy"], [core.expected["service"]])
                self.assertEqual(request(core, "GetServerState", {"server": core.fixture["unavailable_server"]["id"]})[0], 503)
                self.assertEqual(request(core, "GetStackLog", {"stack": core.expected["stack_id"], "services": ["db"], "tail": 40})[1]["stdout"], "db | ready\n")
                self.assertEqual(request(core, "GetStackLog", {"stack": core.expected["stack_id"], "tail": 0})[0], 400)
                self.assertEqual(request(core, "DeployStack")[0], 403)
                self.assertEqual(request(core, "GetUpdate", {"id": core.expected["update_id"]}, credentials=False)[0], 401)
                core.mark("driver", None, "finished")
                self.assertTrue(all(e["concurrency_at_entry"] > 0 for e in core.events))


class IncidentAdapterTests(unittest.IsolatedAsyncioTestCase):
    async def test_failed_caller_cancels_and_drains_other_group_workflows(self):
        started, drained = [], []

        async def fake_workflow(*_args, **_kwargs):
            caller = len(started)
            started.append(caller)
            try:
                if caller == 0:
                    await asyncio.sleep(.01)
                    raise ValueError("seeded failure")
                await asyncio.sleep(10)
            finally:
                drained.append(caller)

        with patch.object(incident, "workflow", fake_workflow), self.assertRaisesRegex(ValueError, "seeded failure"):
            await asyncio.wait_for(incident.group(None, "rust", SimpleNamespace(page_size=3, concurrency=2), {}), .5)
        self.assertEqual(started, [0, 1])
        self.assertEqual(sorted(drained), [0, 1])

    async def test_missing_evidence_and_duplicate_discovery_are_rejected(self):
        expected = incident.expected_evidence(2)
        for failure in ("duplicate", "missing_failure", "tool_error"):
            client = Client([], {}, Path("."), timeout=.2)

            async def request(method, params):
                if params["name"] == "fleet_health":
                    stacks = [{"id": expected["stack_id"], "name": expected["stack_name"], "health": "unhealthy"},
                              {"id": "000000000000000000000004", "name": "beta-other", "health": "healthy"}]
                    if failure == "duplicate":
                        stacks[1] = stacks[0]
                    return {"isError": failure == "tool_error", "structuredContent": {
                        "page": 0, "page_size": 3, "has_more": False, "next_page": None, "stacks": stacks}}, 100
                return {"structuredContent": {"outcome": "inconclusive", "stack_id": expected["stack_id"]}}, 100

            client.request = request
            with self.subTest(failure=failure), self.assertRaises(ValueError):
                await incident.workflow(client, "rust", 3, expected)

    async def test_concurrent_client_matches_response_ids_without_per_caller_core_attribution(self):
        with tempfile.TemporaryDirectory(prefix="incident client ") as temp:
            home = Path(temp)
            script = home / "echo.py"
            script.write_text('''import json,sys
requests=[json.loads(sys.stdin.readline()) for _ in range(2)]
for request in reversed(requests):
 print(json.dumps({"jsonrpc":"2.0","id":request["id"],"result":{"value":request["params"]["value"]}}),flush=True)
sys.stdin.read()
''')
            client = Client([__import__("sys").executable, str(script)], child_env(home, "http://127.0.0.1:1"), home)
            try:
                await client.spawn()
                results = await asyncio.gather(*(client.request("echo", {"value": i}) for i in range(2)))
                self.assertEqual([r[0]["value"] for r in results], [0, 1])
                self.assertTrue(all(r[1] > 0 for r in results))
            finally:
                await client.close()


@unittest.skipUnless(ARTIFACTS.is_file(), "prepare artifacts to run real incident Rust/reference stdio adapters")
class IncidentPreparedTests(unittest.IsolatedAsyncioTestCase):
    async def test_real_stdio_workflow_concurrent_callers_and_larger_fleet(self):
        artifacts = await asyncio.to_thread(bench.preflight, ARTIFACTS)
        for count in (7, 31):
            with self.subTest(count=count), tempfile.TemporaryDirectory(prefix="incident stdio ") as temp, incident.IncidentCore(count) as core:
                home = Path(temp)
                (home / "config.toml").write_text("")
                for name in bench.packaged.participants(artifacts):
                    client = Client(artifacts[name]["argv"], child_env(home, core.address), home, timeout=30)
                    failed = True
                    try:
                        core.mark(name, 0, "warm_startup")
                        await client.initialize()
                        core.mark(name, 0, "warm", 0)
                        result = await incident.group(client, name, SimpleNamespace(page_size=7, concurrency=2), core.expected)
                        self.assertEqual(len(result["callers"]), 2)
                        self.assertEqual(result["mcp_calls"], sum(x["mcp_calls"] for x in result["callers"]))
                        self.assertTrue(all(x["observed"] == core.expected for x in result["callers"]))
                        self.assertNotIn(KEY, json.dumps(result))
                        self.assertNotIn(SECRET, json.dumps(result))
                        failed = False
                    finally:
                        core.mark(name, 0, "shutdown")
                        await client.close(failed)
                self.assertTrue(all(e["operation"] in incident.READS for e in core.events))


if __name__ == "__main__":
    unittest.main()
