import asyncio
import json
from pathlib import Path
from types import SimpleNamespace
import tempfile
import unittest
from unittest.mock import AsyncMock, MagicMock, patch

import candidate_regression_ci as ci
from distribution_archive import archive


def record():
    return {"distribution": {"version":"0.8.0","revision":"a"*40},
            "container":{"run_id":2}, "native_run":{"run_id":1}, "oci_run":{"run_id":2},
            "contract":{"baseline_commit":"b"*40}}


class HostedRegressionTests(unittest.TestCase):
    def test_acquisition_rejects_superseded_attempt_before_extraction(self):
        original = record()
        wrong = dict(original, native_run={"run_id":1,"run_attempt":2})
        with tempfile.TemporaryDirectory() as tmp, patch.object(
                ci.Packages,"download",return_value=SimpleNamespace(identity=wrong)) as download:
            work = Path(tmp)/"candidate"
            with self.assertRaisesRegex(ValueError, "differs"):
                ci.acquire(original, work, "stdio")
            self.assertEqual(download.call_args.args[1:], (work, "a"*40, "0.8.0", 1, 2))
            self.assertFalse((work/"extracted").exists())

    def test_partial_result_only_exports_bounded_fields_and_residue_counts(self):
        marker = "sensitive-fixture-marker"
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            candidate = root/"record.json"
            candidate.write_text(json.dumps(record()))
            private = {"passed":False,"phase":"failed","cleanup_verified":False,
                       "private_cleanup_directory":"/Users/"+marker, "failure":marker,
                       "scenarios":{"container_workflow":{"pages":2,"Env":[marker]}},
                       "owned_workload_cleanup":{"Server":[marker]}}
            (root/"private-result.json").write_text(json.dumps(private))
            output = root/"public.json"
            ci.export(candidate, record(), root, "stdio", output)
            result = json.loads(output.read_bytes())
            self.assertFalse(result["passed"])
            expected = dict.fromkeys(ci.WORKLOAD_RESOURCES)
            expected["Server"] = 1
            self.assertEqual(result["remaining_test_resources"], expected)
            self.assertEqual(result["workload_cleanup_status"], "unknown")
            self.assertEqual(result["scenarios"]["container_workflow"], {"pages":2})
            self.assertNotIn(marker, output.read_text())
            self.assertNotIn("/Users/", output.read_text())

    def test_unrecognized_evidence_field_value_and_unverified_success_fail_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = root/"record.json"
            path.write_text(json.dumps(record()))
            for unsafe in [{"docker_version":"private-hint"}, {"passed":True, "phase":"complete"}]:
                with self.subTest(fields=list(unsafe)):
                    (root/"private-result.json").write_text(json.dumps(unsafe))
                    with self.assertRaises(ValueError):
                        ci.export(path, record(), root, "http", root/"result.json")
                    self.assertFalse((root/"result.json").exists())

    def test_success_requires_zero_observed_in_every_cleanup_category(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            path = root/"record.json"
            path.write_text(json.dumps(record()))
            raw = {"phase":"complete", "passed":False, "cleanup_verified":True,
                   "candidate_record_sha256":ci.sha256(path), "source_commit":"a"*40,
                   "package_version":"0.8.0", "transport":"http",
                   "compose_cleanup_verified":True, "workload_cleanup_status":"verified",
                   "workload_cleanup_verified":True,
                   "cleanup_remaining":{"containers":[],"networks":[],"volumes":[]},
                   "owned_workload_cleanup":{k:[] for k in ci.WORKLOAD_RESOURCES}}
            result = root/"private-result.json"
            for absent in ci.WORKLOAD_RESOURCES:
                with self.subTest(missing=absent):
                    sample = dict(raw, owned_workload_cleanup={k:[] for k in ci.WORKLOAD_RESOURCES if k != absent})
                    result.write_text(json.dumps(sample))
                    output = root/(absent+".json")
                    ci.export(path, record(), root, "http", output)
                    public = json.loads(output.read_bytes())
                    self.assertFalse(public["cleanup_verified"])
                    self.assertFalse(public["workload_cleanup_verified"])
                    self.assertEqual(public["workload_cleanup_status"], "unknown")
                    self.assertTrue(public["compose_cleanup_verified"])
                    self.assertIsNone(public["remaining_test_resources"][absent])
                    sample["passed"] = True
                    result.write_text(json.dumps(sample))
                    with self.assertRaisesRegex(ValueError, "verified cleanup"):
                        ci.export(path, record(), root, "http", root/"rejected.json")

            raw["owned_workload_cleanup"]["outer_candidate_containers"].append("private-id")
            result.write_text(json.dumps(raw))
            ci.export(path, record(), root, "http", root/"residue.json")
            public = json.loads((root/"residue.json").read_bytes())
            self.assertEqual(public["workload_cleanup_status"], "residue")
            self.assertEqual(public["remaining_test_resources"]["outer_candidate_containers"], 1)
            self.assertFalse(public["workload_cleanup_verified"])
            self.assertFalse(public["cleanup_verified"])

    def test_rebuild_refuses_an_external_network_interface(self):
        with patch.object(ci.socket, "if_nameindex", return_value=[(1,"lo"),(2,"eth0")]), patch.object(ci.subprocess, "run") as run:
            with self.assertRaisesRegex(ValueError, "loopback-only"):
                ci.rebuild(record(), Path("irrelevant"))
            run.assert_not_called()

    def test_rebuild_extracts_clean_source_and_probes_binary_from_verified_output_archive(self):
        data = record()
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root/"native").mkdir()
            version = data["distribution"]["version"]
            source_name = ci.stem(version,"source")
            seed = root/source_name
            seed.mkdir()
            (seed/"rust-toolchain.toml").write_text('[toolchain]\nchannel = "1.99.1"')
            original = root/"native"/(source_name+".tar.gz")
            archive(seed,original)
            source_digest = ci.sha256(original)
            data["distribution"]["files"] = [{"kind":"source","name":original.name,"sha256":source_digest}]
            native_name = ci.stem(version,ci.TARGET)
            rebuilt = root/native_name
            rebuilt.mkdir()
            executable = rebuilt/"komodo-mcp"
            executable.write_bytes(b"unit fixture")
            executable.chmod(0o755)
            binary_digest = ci.sha256(executable)

            def build(args, *, cwd, check):
                self.assertTrue(check)
                self.assertEqual(args[:2], [ci.sys.executable, cwd/"scripts/distribution_build.py"])
                self.assertEqual(cwd, root/"offline-source"/source_name)
                self.assertEqual((cwd/"rust-toolchain.toml").read_bytes(),(seed/"rust-toolchain.toml").read_bytes())
                out = root/"rebuilt"
                out.mkdir()
                tar = out/(native_name+".tar.gz")
                archive(rebuilt,tar)
                receipt = {"native":True,"revision":"a"*40,"source_sha256":source_digest,"mcp_version":version,
                           "version":version,"file":{"name":tar.name,"sha256":ci.sha256(tar),
                                                    "target":ci.TARGET,"executable_sha256":binary_digest}}
                (out/(ci.TARGET+".json")).write_text(json.dumps(receipt))

            probe_result = {"mcp_version":version,"client":dict(ci.CLIENT_INFO,protocol=ci.PROTOCOL,python="3.12.8"),
                            "fixture_probe":{"tool":"list_stacks","returned_stacks":3,"core_inventory_reads":1}}
            with patch.object(ci.socket,"if_nameindex",return_value=[(1,"lo")]), \
                    patch.dict(ci.os.environ,CARGO_NET_OFFLINE="true"), patch.object(ci.os,"geteuid",return_value=1001), \
                    patch.object(ci,"native_host_platform",return_value=ci.PLATFORM), \
                    patch.object(ci.subprocess,"check_output",return_value="rustc 1.99.1 (unit)"), \
                    patch.object(ci.subprocess,"run",side_effect=build), \
                    patch.object(ci,"probe_rebuilt",return_value=probe_result) as probe:
                ci.rebuild(data,root)
            extracted = root/"verified-rebuild"/native_name/"komodo-mcp"
            self.assertEqual(ci.sha256(extracted),binary_digest)
            probe.assert_awaited_once_with(extracted,binary_digest,version)
            candidate = root/"candidate.json"
            candidate.write_text(json.dumps(data))
            ci.export(candidate,data,root,"source",root/"public.json")
            public = json.loads((root/"public.json").read_bytes())
            self.assertEqual(public["fixture_probe"],probe_result["fixture_probe"])
            self.assertEqual(public["rust_toolchain"],"1.99.1")
            report = root/"private-result.json"
            partial = json.loads(report.read_bytes())
            del partial["fixture_probe"]
            report.write_text(json.dumps(partial))
            with self.assertRaisesRegex(ValueError,"verified MCP fixture call"):
                ci.export(candidate,data,root,"source",root/"invalid.json")

    def test_rebuilt_fixture_requires_exact_inventory_and_an_observed_core_read(self):
        core = MagicMock()
        core.address = "http://127.0.0.1:20000"
        core.expected = [{"id":"abc","name":"fixture","state":"running"}]
        core.__enter__.return_value = core
        peer = AsyncMock()
        peer.__aenter__.return_value = peer
        peer.version = "0.8.0"
        peer.client_info = dict(ci.CLIENT_INFO, protocol=ci.PROTOCOL, python="3.12.8")
        binary = Path("/synthetic/rebuilt/komodo-mcp")
        with patch.object(ci,"Core",return_value=core), patch.object(ci.NativeMcp,"start",return_value=peer) as start:
            for rows, events, error in [
                (core.expected, [{"operation":"ListStacks","status":200}], None),
                (core.expected, [{"operation":"GetVersion","status":200}], "did not reach Core"),
                ([], [{"operation":"ListStacks","status":200}], "incorrect fixture inventory")]:
                with self.subTest(error=error):
                    peer.tool.return_value = {"items":rows,"has_more":False,"next_page":None}
                    core.events = events
                    if error:
                        with self.assertRaisesRegex(ValueError,error):
                            asyncio.run(ci.probe_rebuilt(binary,"d"*64,"0.8.0"))
                    else:
                        result = asyncio.run(ci.probe_rebuilt(binary,"d"*64,"0.8.0"))
                        self.assertEqual(result["fixture_probe"],{"tool":"list_stacks","returned_stacks":1,"core_inventory_reads":1})
                    self.assertEqual(start.await_args.kwargs["binary"], binary)
                    self.assertEqual(start.await_args.kwargs["executable_sha256"], "d"*64)
                    peer.tool.assert_awaited_with("list_stacks",{"page":0,"page_size":5})

    def test_exports_only_observed_supported_live_versions_platform_and_handshake(self):
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            path = work/"record.json"
            path.write_text(json.dumps(record()))
            for case in ("stdio","http"):
                raw = {"passed":True,"phase":"complete", "workload_cleanup_status":"verified",
                       "cleanup_verified":True, "compose_cleanup_verified":True, "workload_cleanup_verified":True,
                       "cleanup_remaining":{"containers":[],"networks":[],"volumes":[]},
                       "owned_workload_cleanup":{key:[] for key in ci.WORKLOAD_RESOURCES},
                       "core_version":ci.CORE_VERSION, "periphery_version":ci.CORE_VERSION, "docker_version":"29.1.1",
                       "source_commit":"a"*40, "package_version":"0.8.0", "transport":case,
                       "candidate_record_sha256":ci.sha256(path), "mcp_version":"0.8.0",
                       "target":ci.PLATFORM if case == "http" else ci.TARGET, "platform":ci.PLATFORM,
                       "client":dict(ci.CLIENT_INFO,protocol=ci.PROTOCOL,python="3.12.8"),
                       "index_digest":"sha256:"+"b"*64,"platform_manifest_digest":"sha256:"+"c"*64,
                       "executable_sha256":"d"*64, "redaction_checked":list(ci.REDACTION_SURFACES)}
                raw["scenarios"] = {name:{field:(True if rule is bool else 1 if rule is int
                                        else sorted(rule)[0] if isinstance(rule,set) else "f"*24)
                                    for field,rule in fields.items()} for name,fields in ci.SCENARIOS.items()}
                raw["scenarios"]["read_only_operation"].update(discovered_tools=list(ci.DENIAL_TOOLS),
                    other_target_updates=[],run_sync_requests_submitted=0,core_run_sync_history_delta=0)
                for missing in (None, "client", "platform", "target", "core_version", "periphery_version",
                                *ci.REDACTION_SURFACES):
                    with self.subTest(case=case, missing=missing):
                        sample = dict(raw)
                        if missing in ci.REDACTION_SURFACES:
                            sample["redaction_checked"] = [s for s in ci.REDACTION_SURFACES if s != missing]
                        elif missing:
                            del sample[missing]
                        (work/"private-result.json").write_text(json.dumps(sample))
                        out = work/(case+"-"+str(missing)+".json")
                        if missing:
                            with self.assertRaises(ValueError):
                                ci.export(path,record(),work,case,out)
                        else:
                            ci.export(path,record(),work,case,out)
                            actual = json.loads(out.read_bytes())
                            for key in ("client","platform","target","core_version","periphery_version","redaction_checked"):
                                self.assertEqual(actual[key],raw[key])
                            self.assertEqual(actual["scenarios"]["read_only_operation"],raw["scenarios"]["read_only_operation"])
            for invalid in (["MCP frames","MCP frames"], ["private fixture marker"], "MCP frames"):
                with self.subTest(invalid=invalid):
                    (work/"private-result.json").write_text(json.dumps({"passed":False,"redaction_checked":invalid}))
                    with self.assertRaisesRegex(ValueError,"invalid redaction evidence"):
                        ci.export(path,record(),work,"stdio",work/"invalid-scans.json")
            for update in [{"id":"a"*24,"operation":"UpdateResourceSync","raw":"private fixture marker"},
                           {"id":"b"*24,"operation":"private fixture marker"}]:
                with self.subTest(fields=list(update)):
                    private = {"scenarios":{"read_only_operation":{"other_target_updates":[update]}}}
                    (work/"private-result.json").write_text(json.dumps(private))
                    with self.assertRaisesRegex(ValueError,"other_target_updates"):
                        ci.export(path,record(),work,"http",work/"invalid-update.json")


if __name__ == "__main__":
    unittest.main()
