"""Regression guardrails for packaged execution and authoritative update history."""
import asyncio
import hashlib
import json
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import AsyncMock, MagicMock, Mock, patch

import disposable_candidate_regression as reg
from candidate_clients import HttpMcp, NativeMcp, PROTOCOL


class CandidateTests(unittest.TestCase):
    def test_contract_is_the_same_frozen_blob_on_ancestor_and_package(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            config = ["git","-C",repo,"-c","user.name=Fixture","-c","user.email=fixture@example.invalid",
                      "-c","commit.gpgsign=false"]
            def git(*args):
                return subprocess.check_output([*config,*args],text=True).strip()
            git("init","--quiet")
            target = repo/"docs/contracts/v1.json"
            target.parent.mkdir(parents=True)
            target.write_bytes(b'{"tools":[]}\n')
            git("add","docs/contracts/v1.json")
            git("commit","-qm","Store the initial fixture contract")
            baseline = git("rev-parse","HEAD")
            digest = hashlib.sha256(target.read_bytes()).hexdigest()
            git("commit","--allow-empty","-qm","Select a package with the same contract")
            candidate = git("rev-parse","HEAD")
            contract = {"baseline_commit":baseline,"path":"docs/contracts/v1.json","sha256":digest}
            def verify(revision=candidate, **changes):
                return reg.verify_contract_identity({"distribution":{"revision":revision},
                                                     "contract":dict(contract,**changes)},repo)
            self.assertEqual(verify(),baseline)
            for changes in [{"sha256":"a"*64},{"sha256":None},{"path":"other.json"},{"baseline_commit":"f"*40},
                            {"baseline_commit":candidate,"revision":baseline}]:
                with self.subTest(changes=changes), self.assertRaises(ValueError):
                    verify(**changes)
            target.write_bytes(b'{"tools":["different"]}\n')
            git("commit","-qam","Change the candidate fixture contract")
            with self.assertRaisesRegex(ValueError,"recorded bytes"):
                verify(git("rev-parse","HEAD"))

    def test_both_transports_record_the_client_and_protocol_used_in_initialize(self):
        for peer in (NativeMcp(), HttpMcp()):
            with self.subTest(transport=type(peer).__name__):
                peer.rpc = AsyncMock(return_value={"serverInfo":{"name":"komodo-mcp","version":"0.8.0"},
                                                   "protocolVersion":PROTOCOL})
                asyncio.run(peer.initialize("0.8.0"))
                method, params = peer.rpc.await_args.args
                self.assertEqual(method, "initialize")
                self.assertEqual(peer.version, "0.8.0")
                self.assertEqual(peer.client_info, {**params["clientInfo"], "protocol":params["protocolVersion"],
                                                      "python":reg.platform.python_version()})

    def test_native_candidate_rejects_bytes_unlisted_in_shared_record(self):
        candidate = reg.Candidate.__new__(reg.Candidate)
        candidate.version = "0.8.0"
        candidate.revision = "a"*40
        candidate.baseline = "b"*40
        candidate.outcome = {}
        with tempfile.TemporaryDirectory() as folder:
            binary = Path(folder)/"komodo-mcp"
            binary.write_bytes(b"wrong fixture bytes")
            candidate.args = SimpleNamespace(transport="stdio", binary=binary, candidate_record=binary)
            candidate.record = {"distribution":{"files":[{"kind":"native","executable_sha256":"c"*64}]}}
            with patch.dict(reg.smoke.REPORT,{},clear=True), patch.object(reg, "native_platform") as platform:
                with self.assertRaisesRegex(reg.smoke.Failure, "differs"):
                    candidate.prepare()
                platform.assert_not_called()

    def test_running_image_comparison_covers_platform_and_configuration(self):
        candidate = reg.Candidate.__new__(reg.Candidate)
        candidate.args = SimpleNamespace(platform="linux/arm64")
        candidate.ref = "komodo-mcp-candidate:git-"+"a"*40
        candidate.platform = {"manifest_digest":"sha256:"+"b"*64,"config_digest":"sha256:"+"c"*64}
        candidate.record = {"container":{"index_digest":"sha256:"+"d"*64}}
        image = {"Id":candidate.platform["config_digest"], "Os":"linux", "Architecture":"arm64"}
        for extra, valid in [({}, True), ({"Descriptor":{"digest":candidate.platform["manifest_digest"]}}, True),
                ({"Architecture":"amd64"}, False), ({"Id":"sha256:"+"e"*64}, False),
                ({"Descriptor":{"digest":"sha256:"+"f"*64}}, False)]:
            with self.subTest(extra=extra), patch.object(reg.smoke, "run", return_value=json.dumps(dict(image,**extra))):
                if valid:
                    candidate.verify_image()
                else:
                    with self.assertRaises(reg.smoke.Failure):
                        candidate.verify_image()

    def test_history_counts_exact_operation_target_and_full_page(self):
        client = Mock()
        item = {"id":"d"*24,"target":{"type":"Deployment","id":"a"*24},"operation":"Deploy"}
        for rows, nxt, valid in [([item], None, True), ([item,item], None, False), ([item], 1, False),
                                 ([dict(item,operation="RestartDeployment")], None, False)]:
            client.read.return_value = {"updates":rows,"next_page":nxt}
            with self.subTest(rows=len(rows), next_page=nxt):
                if valid:
                    self.assertEqual(reg.history(client, "Deployment", "a"*24, "Deploy"), {"d"*24:item})
                else:
                    with self.assertRaises(reg.smoke.Failure):
                        reg.history(client, "Deployment", "a"*24, "Deploy")

    def test_read_only_apply_requires_core_denial_and_independently_unchanged_run_history(self):
        rid, uid = "a"*24, "c"*24
        target = {"type":"ResourceSync","id":rid}
        for operation, rejection, valid in [("UpdateResourceSync","permission_denied",True),
                ("RunSync","permission_denied",False), ("UpdateResourceSync","stale_preview",False)]:
            with self.subTest(operation=operation, rejection=rejection):
                candidate = reg.Candidate.__new__(reg.Candidate)
                peer = MagicMock()
                peer.__aenter__.return_value = peer
                peer.rpc = AsyncMock(return_value={"tools":[{"name":n} for n in reg.DENIAL_TOOLS]})
                peer.tool = AsyncMock(side_effect=[{"sync":{"id":rid}},
                    {"review_reference":"ref","apply_binding":{},"plan":{"apply_coverage_complete":True}},
                    {"outcome":"not_submitted","run_sync_requests_submitted":0,
                     "preflight":{"refresh_requests_submitted":1,"error":{"kind":rejection}}}])
                candidate.connect = Mock(return_value=peer)
                candidate.outcome = {}
                admin = Mock()
                admin.read.side_effect = [{"updates":[]},{"config":{}},
                                          {"updates":[{"target":target,"id":uid,"operation":operation}]},{"config":{}}]
                candidate.context = {"provisioner":admin,"alternate_server_id":"d"*24}
                if valid:
                    asyncio.run(candidate.exercise_denied_apply(rid))
                    outcome = candidate.outcome["read_only_operation"]
                    self.assertEqual(outcome["other_target_updates"],[{"id":uid,"operation":operation}])
                    self.assertEqual(outcome["core_run_sync_history_delta"],0)
                    self.assertEqual(outcome["discovered_tools"],list(reg.DENIAL_TOOLS))
                    queries = [call for call in admin.read.call_args_list if call.args[0] == "ListUpdates"]
                    self.assertEqual(len(queries),2)
                    self.assertEqual(queries[0].args[1],{"query":{"target.type":"ResourceSync","target.id":rid},"page":0})
                else:
                    with self.assertRaises(reg.smoke.Failure):
                        asyncio.run(candidate.exercise_denied_apply(rid))
                    self.assertNotIn("read_only_operation",candidate.outcome)
                candidate.connect.assert_called_once_with("reader",KOMODO_ALLOW_OPERATE="true",
                    KOMODO_ALLOW_CONFIGURATION_EDIT="true",KOMODO_ENABLED_TOOLS=",".join(reg.DENIAL_TOOLS))

    def test_core_cleanup_failure_still_observes_unknown_and_remaining_resources(self):
        candidate = reg.Candidate.__new__(reg.Candidate)
        admin = Mock()
        candidate.context = {"provisioner":admin,"users":[]}
        def read(operation, params=None):
            if operation == "ListStacks":
                return [{"name":"mcp-smoke-stack"}]
            if operation == "ListDeployments":
                raise reg.smoke.Failure("fixture read unavailable")
            return []
        admin.read.side_effect = read
        with patch.dict(reg.smoke.REPORT,{},clear=True), patch.object(reg,"nested",return_value=""), \
             patch.object(candidate,"candidate_containers",return_value=["c"*64]) as outer, \
             patch.object(candidate,"remove_workloads",side_effect=reg.smoke.Failure("delete failed")):
            with self.assertRaisesRegex(reg.smoke.Failure, "delete failed"):
                candidate.cleanup()
            outer.assert_called_once()
            report = reg.smoke.REPORT
            self.assertEqual(report["workload_cleanup_status"], "failed")
            self.assertEqual(report["failed_workload_cleanup_step"], "start")
            self.assertFalse(report["workload_cleanup_verified"])
            self.assertEqual(report["owned_workload_cleanup"], {
                "Stack":["mcp-smoke-stack"], "Deployment":None, "ResourceSync":[], "Server":[],
                "service_users":[], "nested_fixture_containers":[], "nested_deployment_containers":[],
                "outer_candidate_containers":["c"*64]})

    def test_running_deployment_deletion_has_its_own_bounded_deadline(self):
        candidate = reg.Candidate.__new__(reg.Candidate)
        candidate.args = SimpleNamespace(transport="stdio")
        admin = Mock()
        candidate.context = {"provisioner":admin}
        scope = {"stack_id":"a"*24,"deployment_id":"b"*24}
        admin.read.side_effect = [{"id":scope["stack_id"],"name":"mcp-smoke-stack"},
                                  {"id":scope["deployment_id"],"name":"mcp-smoke-deployment"}]
        admin.request.side_effect = reg.smoke.Failure("synthetic teardown failure")
        with patch.dict(reg.smoke.REPORT,{"scope":scope},clear=True), patch.object(candidate,"observe_cleanup",return_value={}):
            with self.assertRaisesRegex(reg.smoke.Failure,"synthetic teardown"):
                candidate.cleanup()
            self.assertEqual(reg.smoke.REPORT["failed_workload_cleanup_step"], "delete Deployment")
        admin.write.assert_called_once_with("DeleteStack",{"id":scope["stack_id"]})
        admin.request.assert_called_once_with("/write","DeleteDeployment",{"id":scope["deployment_id"]},timeout=45)

    def test_native_stderr_detects_a_marker_split_across_chunks(self):
        async def run():
            client = NativeMcp()
            reader = asyncio.StreamReader()
            client.process = SimpleNamespace(stderr=reader)
            client.credential_bytes = (b"recognizable-marker",)
            client.disclosed = False
            task = asyncio.create_task(client._scan_output(reader))
            reader.feed_data(b"health: recognizable-")
            await asyncio.sleep(0)
            reader.feed_data(b"marker")
            reader.feed_eof()
            await asyncio.wait_for(task, 1)
            return client.disclosed
        self.assertTrue(asyncio.run(run()))

    def test_native_shutdown_drains_and_scans_final_stdout_through_eof(self):
        async def run(payload):
            client = NativeMcp()
            client.credential_bytes = (b"fixture-private-marker",)
            client.disclosed = False
            client.process = await asyncio.create_subprocess_exec(sys.executable,"-c",
                "import sys; sys.stdin.buffer.read(); sys.stdout.buffer.write(b' '*200000 + sys.argv[1].encode())",
                payload, stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
            client.drain = asyncio.create_task(client._scan_output(client.process.stderr))
            try:
                await asyncio.wait_for(client.close(), 4)
            finally:
                if client.process.returncode is None:
                    client.process.kill()
                    await client.process.wait()
        asyncio.run(run('{"method":"notifications/initialized"}'))
        with self.assertRaisesRegex(reg.smoke.Failure, "output disclosed"):
            asyncio.run(run('{"method":"notifications/message","params":{"data":"fixture-private-marker"}}'))


if __name__ == "__main__":
    unittest.main()
