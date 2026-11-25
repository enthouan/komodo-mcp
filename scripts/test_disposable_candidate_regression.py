"""Regression guardrails for packaged execution and authoritative update history."""
import asyncio
import hashlib
from http.server import BaseHTTPRequestHandler
import json
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import AsyncMock, MagicMock, Mock, patch

import disposable_candidate_regression as reg
import candidate_clients
from candidate_clients import HttpMcp, NativeMcp, PROTOCOL
from test_disposable_core_smoke import http_server


class CandidateTests(unittest.TestCase):
    def test_regression_fixture_keeps_docker_on_its_socket_and_adds_only_core_to_http_bridge(self):
        template = (reg.smoke.WORKSPACE / "tests/fixtures/disposable-core/compose.json").read_text()
        for transport in ("stdio", "http"):
            with self.subTest(transport=transport):
                config = json.loads(template)
                candidate = reg.Candidate.__new__(reg.Candidate)
                candidate.args = SimpleNamespace(transport=transport)
                candidate.configure_fixture(config)
                daemon = config["services"]["docker"]
                self.assertEqual(daemon["command"], ["dockerd", "--host=unix:///var/run/docker.sock"])
                for service in (daemon, config["services"]["periphery"]):
                    self.assertEqual(service["environment"]["DOCKER_HOST"], "unix:///var/run/docker.sock")
                    self.assertIn("dind-socket:/var/run", service["volumes"])
                attached = {name for name, service in config["services"].items() if "candidate" in service.get("networks", [])}
                if transport == "http":
                    self.assertEqual(attached, {"core"})
                    self.assertEqual(config["services"]["core"]["networks"], ["default","candidate"])
                    self.assertEqual(config["networks"]["candidate"]["labels"], config["networks"]["default"]["labels"])
                else:
                    self.assertFalse(attached)
                    self.assertNotIn("candidate", config["networks"])

    def test_docker_unix_read_requires_actual_daemon_command_and_no_tcp_listener(self):
        cmd = "dockerd\n--host=unix:///var/run/docker.sock"
        header = "sl local_address rem_address st"
        for argv, tables, version, ok in (
                (cmd, header, "27.5.1", True),
                (cmd, header+"\n0: 0100007F:0947 00000000:0000 01", "27.5.1", True),
                (cmd+"\n--host=tcp://0.0.0.0:2375", header, "27.5.1", False),
                (cmd, header+"\n0: 00000000:0947 00000000:0000 0A", "27.5.1", False),
                (cmd, header+"\n0: "+"0"*32+":0948 "+"0"*32+":0000 0A", "27.5.1", False),
                (cmd, "", "27.5.1", False), (cmd, header, "99.0.0", False)):
            with self.subTest(argv=argv, tables=tables, version=version), \
                    patch.object(reg.smoke, "run", side_effect=[argv,tables]), \
                    patch.object(reg, "nested", return_value=version) as nested, \
                    patch.dict(reg.smoke.REPORT, {"docker_version":"27.5.1"}, clear=True):
                candidate = reg.Candidate.__new__(reg.Candidate)
                if ok:
                    candidate.verify_docker_socket()
                    nested.assert_called_once_with("--host", "unix:///var/run/docker.sock", "version", "--format", "{{.Server.Version}}")
                    self.assertIs(reg.smoke.REPORT["dind_unix_socket_only"], True)
                else:
                    with self.assertRaises(reg.smoke.Failure):
                        candidate.verify_docker_socket()
                    self.assertNotIn("dind_unix_socket_only", reg.smoke.REPORT)

    def test_bridge_membership_allows_concurrent_owned_peers_and_rejects_other_endpoints_or_labels(self):
        project, owner = "komodo-mcp-smoke-test", "test-owner"
        core, first, second, foreign = [char*64 for char in "abcd"]
        name = project + "_candidate"
        labels = {"komodo-mcp.test.session":owner, "com.docker.compose.project":project, "com.docker.compose.network":"candidate"}
        candidate = reg.Candidate.__new__(reg.Candidate)
        candidate.owned = {first:owner, second:owner}
        for ids, overrides, ok in (
                ([core,first,second], {}, True), ([core,first,second,foreign], {}, False),
                ([core,first], {}, False), ([core,first,second], {"komodo-mcp.test.session":"other"}, False),
                ([core,first,second], {"com.docker.compose.project":"other"}, False),
                ([core,first,second], {"com.docker.compose.network":"default"}, False)):
            network = {"Name":name, "Driver":"bridge", "Labels":dict(labels, **overrides), "Containers":dict.fromkeys(ids)}
            with self.subTest(ids=ids, labels=overrides), patch.object(reg.smoke, "PROJECT", project), \
                    patch.object(reg.smoke, "OWNER", owner), patch.object(reg.smoke, "run", side_effect=[json.dumps(network),core]):
                if ok:
                    self.assertEqual(candidate.verified_candidate_network(), name)
                else:
                    with self.assertRaises(reg.smoke.Failure):
                        candidate.verified_candidate_network()

    def test_compose_cleanup_observes_the_labelled_candidate_bridge_until_removed(self):
        for residual in (False, True):
            removed, inspected = False, []
            def run(args, **kwargs):
                nonlocal removed
                if "inspect" in args:
                    inspected.append(args[-1])
                    return "owned"
                if "down" in args:
                    self.assertEqual(inspected, ["default-id","candidate-id"])
                    removed = True
                if "network" in args and "ls" in args:
                    self.assertEqual(args[-1], "label=com.docker.compose.project=fixture")
                    if not removed:
                        return "default-id\ncandidate-id"
                    return "candidate-id" if residual else ""
                return ""
            with self.subTest(residual=residual), tempfile.TemporaryDirectory() as tmp, \
                    patch.object(reg.smoke,"ROOT",Path(tmp)), patch.object(reg.smoke,"OWNER","owned"), \
                    patch.object(reg.smoke,"PROJECT","fixture"), patch.dict(reg.smoke.REPORT,{},clear=True), \
                    patch.object(reg.smoke,"run",side_effect=run):
                if residual:
                    with self.assertRaisesRegex(reg.smoke.Failure, "resources remain"):
                        reg.smoke.cleanup()
                else:
                    reg.smoke.cleanup()
                self.assertEqual(reg.smoke.REPORT["cleanup_verified"], not residual)
                self.assertEqual(reg.smoke.REPORT["cleanup_remaining"]["networks"], ["candidate-id"] if residual else [])

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

    def test_http_rejects_markers_in_response_headers_and_status_before_using_the_session(self):
        marker = "private-fixture-credential"
        class Handler(BaseHTTPRequestHandler):
            def do_POST(self):
                self.rfile.read(int(self.headers["Content-Length"]))
                self.send_response_only(status, reason)
                for key, value in headers:
                    self.send_header(key, value)
                self.end_headers()
                self.wfile.write(b'{"jsonrpc":"2.0","id":1,"result":{}}')
            def log_message(self, *args):
                pass

        peer = HttpMcp()
        with http_server(Handler) as root, patch.object(candidate_clients,"SECRETS",[marker]):
            peer.host, peer.port = "127.0.0.1", int(root.rsplit(":",1)[1])
            peer.bearer, peer.session = "x"*40, None
            for status, reason, headers in [
                (200,"OK",[("Mcp-Session-Id",marker)]),
                (401,"Unauthorized",[("WWW-Authenticate","Bearer "+marker)]),
                (200,marker,[]), (200,"OK",[("X-"+marker,"x")])]:
                with self.subTest(status=status,reason=reason), self.assertRaisesRegex(reg.smoke.Failure,"disclosed") as error:
                    asyncio.run(peer.exchange({"method":"initialize","id":1}))
                self.assertIsNone(peer.session)
                self.assertNotIn(marker,str(error.exception))
            status, reason, headers = 200, "OK", [("X-One","a"*34000),("X-Two","b"*34000)]
            with self.assertRaisesRegex(reg.smoke.Failure,"headers exceeded"):
                asyncio.run(peer.exchange({"method":"initialize","id":1}))

    def test_wrong_bearer_is_registered_before_the_denial_and_remains_a_diagnostic_marker(self):
        marker = "incorrect-fixture-bearer-" + "a"*32
        registered = []
        async def probe(method, frame, token):
            if token == marker:
                self.assertIn(token,registered)
                raise reg.smoke.Failure("synthetic end of denial probe")
            return 401, None, None, "Bearer", b""
        self.assertIs(candidate_clients.SECRETS, reg.smoke.SECRETS)
        with patch.object(candidate_clients, "SECRETS", registered), \
                patch.object(candidate_clients.secrets, "token_urlsafe", return_value=marker), \
                patch.object(HttpMcp, "_request", side_effect=probe):
            with self.assertRaisesRegex(reg.smoke.Failure,"synthetic end"):
                asyncio.run(HttpMcp.start(endpoint="http://127.0.0.1:23456/mcp", bearer="x"*40, version="0.8.0"))
        self.assertIn(marker,registered)

    def test_http_container_final_diagnostics_include_shutdown_and_interrupted_processes(self):
        marker, cid, owner = "late-private-marker", "c"*64, "owned-session"
        for already_stopped in (False,True):
            with self.subTest(already_stopped=already_stopped):
                candidate = reg.Candidate.__new__(reg.Candidate)
                candidate.owned = {cid:owner}
                stopped = already_stopped
                commands = []
                def run(args):
                    nonlocal stopped
                    commands.append(args[1])
                    if args[1] == "inspect":
                        if ".State.Health" in args[-2]:
                            return "[]"
                        return str(not stopped).lower() if args[-2] == "{{.State.Running}}" else owner
                    if args[1] == "stop":
                        stopped = True
                    if args[1] == "logs":
                        return marker if stopped else ""
                    return ""
                with patch.object(reg.smoke,"run",side_effect=run), patch.object(reg.smoke,"DOCKER",["docker"]), \
                        patch.object(reg.smoke,"OWNER",owner), patch.object(reg.smoke,"SECRETS",[marker]):
                    with self.assertRaises(reg.smoke.Failure) as error:
                        candidate.drop_container(cid)
                self.assertNotIn(marker,str(error.exception))
                self.assertEqual(commands[-2:], ["logs","rm"])
                self.assertEqual(commands.count("stop"), int(not already_stopped))
                self.assertEqual(candidate.owned,{})

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

    def test_paged_inventory_requires_one_exact_seeded_identity_per_page(self):
        server, running, stopped = "1"*24, "a"*64, "b"*64
        def row(cid, state, server_id=server):
            return {"identity":{"server_id":server_id,"container_id":cid}, "state":state}
        active, exited = row(running, "running"), row(stopped, "exited")
        for first, second, message in (
                ([], [active, exited], "exactly one container"),
                ([active, exited], [], "exactly one container"),
                ([row("c"*64, "running")], [exited], "differ from Docker"),
                ([row(running, "running", "2"*24)], [exited], "differ from Docker"),
                ([active], [row(stopped, "running")], "differ from Docker"),
                ([exited], [active], None)):
            with self.subTest(first=first, second=second):
                candidate = reg.Candidate.__new__(reg.Candidate)
                candidate.context = {"server_id":server, "provisioner":Mock()}
                candidate.context["provisioner"].read.return_value = [{"id":stopped, "state":"exited"}]
                candidate.outcome = {}
                peer = AsyncMock()
                peer.rpc.return_value = {"tools":[{"name":"list_containers"}, {"name":"get_container"}]}
                default = {"source":"core_cache", "data":{"items":[active], "next_page":None}}
                peer.tool.side_effect = [default, {"data":{"items":first, "next_page":1}},
                    {"data":{"items":second, "next_page":None}},
                    {"source":"fresh_inspect", "data":dict(exited, exit_code=17, health_status="unhealthy", restart_count=0)},
                    default]
                candidate.connect = MagicMock()
                candidate.connect.return_value.__aenter__.return_value = peer
                with patch.dict(reg.smoke.REPORT, {"scope":{"stack_id":"3"*24}}), patch.object(reg.sync, "raw_error", AsyncMock()):
                    run = candidate.exercise_containers(running, stopped, "unhealthy", 0)
                    if message:
                        with self.assertRaisesRegex(reg.smoke.Failure, message):
                            asyncio.run(run)
                        self.assertNotIn("container_workflow", candidate.outcome)
                    else:
                        asyncio.run(run)
                        self.assertEqual(candidate.outcome["container_workflow"]["pages"], 2)

    def test_inline_staleness_requires_rejection_before_any_preflight_refresh(self):
        for refreshes in (0, 1):
            with self.subTest(refreshes=refreshes):
                candidate = reg.Candidate.__new__(reg.Candidate)
                admin = Mock()
                candidate.context = {"provisioner":admin, "alternate_server_id":"2"*24,
                                     "users":[{"id":str(i)*24} for i in range(1,4)]}
                candidate.outcome = {}
                admin.write.return_value = {"id":"3"*24}
                before = {"config":{"auto_pull":False}}
                admin.read.side_effect = [before, before, {"config":{"auto_pull":True}}, {"config":{}}, {"config":{}}]
                peer = AsyncMock()
                peer.tool.side_effect = [{"writes_submitted":0, "preview_id":"review"},
                    {"writes_submitted":1, "outcome":"saved"}, {},
                    {"outcome":"not_submitted", "review_consumed":True, "run_sync_requests_submitted":0,
                     "rejection":{"kind":"stale_preview"}, "preflight":{"refresh_requests_submitted":refreshes}}]
                candidate.connect = MagicMock()
                candidate.connect.return_value.__aenter__.return_value = peer
                candidate.exercise_denied_apply = AsyncMock()
                review = AsyncMock(return_value={"review_reference":"review", "apply_binding":"binding"})
                with patch.dict(reg.smoke.REPORT, {"scope":{"stack_id":"1"*24}}), \
                        patch.object(reg, "history", return_value={}), patch.object(reg.sync, "review", review):
                    if refreshes:
                        with self.assertRaisesRegex(reg.smoke.Failure, "submitted a preflight refresh"):
                            asyncio.run(candidate.exercise_configuration())
                        self.assertNotIn("stale_resource_sync", candidate.outcome)
                    else:
                        asyncio.run(candidate.exercise_configuration())
                        self.assertEqual(candidate.outcome["stale_resource_sync"]["preflight_refreshes"], 0)

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

    def test_both_transports_scan_escaped_values_keys_notifications_and_errors(self):
        marker = b"private"
        notification = br'{"jsonrpc":"2.0","method":"notifications/message","params":{"\u0070rivate":"fixture"}}'
        error = br'{"jsonrpc":"2.0","id":1,"error":{"code":-1,"message":"\u0070rivate"}}'
        reply = b'{"jsonrpc":"2.0","id":1,"result":{}}'
        for body in (notification,error):
            with self.subTest(body=body):
                self.assertNotIn(marker,body)
                for media, payload in (("application/json",body),
                                       ("text/event-stream",b"data: "+reply+b"\n\ndata: "+body+b"\n\n")):
                    with self.assertRaisesRegex(reg.smoke.Failure,"disclosed"):
                        candidate_clients._http_frames(media,payload,(marker,))
                async def read_native():
                    peer = NativeMcp()
                    peer.credential_bytes = (marker,)
                    peer.send = AsyncMock()
                    peer.process = SimpleNamespace(stdout=asyncio.StreamReader())
                    peer.process.stdout.feed_data(body+b"\n"+reply+b"\n")
                    await peer.exchange({"id":1})
                with self.assertRaisesRegex(reg.smoke.Failure,"disclosed"):
                    asyncio.run(read_native())

    def test_native_shutdown_drains_and_scans_final_stdout_through_eof(self):
        async def run(payload):
            client = NativeMcp()
            client.credential_bytes = (b"fixture-private-marker",)
            client.disclosed = False
            client.process = await asyncio.create_subprocess_exec(sys.executable,"-c",
                "import sys; sys.stdin.buffer.read(); sys.stdout.buffer.write(b' '*200000 + sys.argv[1].encode())",
                payload, stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE, limit=reg.smoke.MAX_MCP_BYTES)
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
        with self.assertRaisesRegex(reg.smoke.Failure, "output disclosed"):
            asyncio.run(run(r'{"method":"notifications/message","params":{"data":"fixture-\u0070rivate-marker"}}'))


if __name__ == "__main__":
    unittest.main()
