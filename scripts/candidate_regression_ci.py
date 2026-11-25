"""Acquire immutable Actions inputs and export a bounded, public regression record."""
import argparse
import asyncio
import json
import os
from pathlib import Path
import re
import socket
import stat
import subprocess
import sys
import tomllib

from candidate_clients import CLIENT_INFO, PROTOCOL, NativeMcp
import candidate_source_sandbox
from disposable_candidate_regression import (CLEANUP_STEPS, DENIAL_TOOLS, REDACTION_SURFACES, WORKLOAD_RESOURCES,
                                            native_host_platform, verify_contract_identity)
from disposable_core_smoke import CORE_VERSION
from distribution import sha256, stem
from distribution_archive import extract
from release import GitHub
from release_artifacts import Packages

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "benchmarks"))
from core import Core, KEY, SECRET

TARGET = "x86_64-unknown-linux-musl"
PLATFORM = "linux/amd64"
DEFAULT_RECORD = "docs/releases/v0.9.0/candidate-record.json"
PACKAGE_FIELDS = ("native_run", "oci_run", "distribution", "container")


def require(condition, message):
    if not condition:
        raise ValueError(message)


def record(path):
    require(not path.is_absolute(), "select a record inside the checkout")
    actual = (ROOT / path).resolve(strict=True)
    require(actual.is_relative_to(ROOT) and actual.is_file() and actual.stat().st_size <= 256 * 1024,
            "invalid candidate record location or size")
    tracked = subprocess.run(["git", "-C", ROOT, "ls-files", "--error-unmatch", "--",
                              actual.relative_to(ROOT)], capture_output=True)
    require(tracked.returncode == 0, "candidate record must be tracked")
    data = json.loads(actual.read_bytes())
    revision, version = data["distribution"]["revision"], data["distribution"]["version"]
    Packages.from_receipt(data, revision, version)
    verify_contract_identity(data, ROOT)
    return actual, data


def entry_for(data, kind, target=None):
    matches = [row for row in data["distribution"]["files"] if row["kind"] == kind
               and (target is None or row.get("target") == target)]
    require(len(matches) == 1, "missing or ambiguous packaged input")
    return matches[0]


def acquire(data, work, case):
    work.mkdir(mode=0o700, exist_ok=False)
    revision, version = data["distribution"]["revision"], data["distribution"]["version"]
    downloaded = Packages.download(GitHub(), work, revision, version,
                                   data["native_run"]["run_id"], data["oci_run"]["run_id"])
    expected = {key:data[key] for key in PACKAGE_FIELDS}
    require(downloaded.identity == expected, "downloaded candidate identity differs from the shared record")
    if case == "http":
        return
    obj = entry_for(data, "source") if case == "source" else entry_for(data, "native", TARGET)
    name = stem(version, "source" if case == "source" else TARGET)
    archive = downloaded.native_dir / obj["name"]
    require(sha256(archive) == obj["sha256"], "input archive checksum differs")
    if case == "stdio":
        extracted = extract(archive, work / "extracted", name)
        require(sha256(extracted / "komodo-mcp") == obj["executable_sha256"], "extracted executable differs")


def run_live(path, data, work, case, project, context):
    version = data["distribution"]["version"]
    args = [sys.executable, ROOT / "scripts/disposable_candidate_regression.py", "--candidate-record", path,
            "--transport", case, "--evidence", work / "private-result.json",
            "--project", project, "--docker-context", context]
    if case == "stdio":
        binary = work / "extracted" / stem(version, TARGET) / "komodo-mcp"
        args += ["--binary", binary]
    else:
        args += ["--oci-archive", work / "oci/candidate.oci.tar", "--platform", PLATFORM]
    subprocess.run(args, check=True)


async def probe_rebuilt(binary, digest, version):
    # This second probe records the Core traffic and exact result from the
    # executable re-extracted from the freshly rebuilt native archive.
    with Core(3) as core:
        async with await NativeMcp.start(binary=binary, executable_sha256=digest, version=version,
                address=core.address, credentials={"key":KEY,"secret":SECRET}, environment={}) as client:
            page = await client.tool("list_stacks", {"page":0, "page_size":5})
            rows = [{key:row[key] for key in ("id","name","state")} for row in page["items"]]
            require(page["has_more"] is False and page["next_page"] is None and rows == core.expected,
                    "rebuilt executable returned an incorrect fixture inventory")
        core.mark("rebuilt", None, "finished")
        require(all(e["status"] == 200 and e["operation"] in ("GetVersion","ListStacks") for e in core.events),
                "rebuilt probe issued an unexpected Core request")
        reads = sum(e["operation"] == "ListStacks" for e in core.events)
        require(reads > 0, "rebuilt inventory call did not reach Core")
        return {"mcp_version":client.version, "client":client.client_info, "native_probe_sandbox":"bubblewrap",
                "fixture_probe":{"tool":"list_stacks", "returned_stacks":len(rows), "core_inventory_reads":reads}}


def rebuild(data, work):
    # Netlink reports interfaces in this process's network namespace even
    # when the host's sysfs mount was inherited.
    interfaces = sorted(name for _, name in socket.if_nameindex())
    require(interfaces == ["lo"], "offline rebuild requires loopback-only networking")
    require(os.geteuid() != 0 and os.environ.get("CARGO_NET_OFFLINE") == "true",
            "offline rebuild requires an unprivileged user and Cargo offline mode")
    evidence = work / "private-result.json"
    result = {"passed":False,"phase":"verify_source","network_interfaces":interfaces,"cargo_offline":True}
    evidence.write_text(json.dumps(result))
    revision, version = data["distribution"]["revision"], data["distribution"]["version"]
    source_obj = entry_for(data, "source")
    original = work / "native" / source_obj["name"]
    require(sha256(original) == source_obj["sha256"], "offline source archive differs")
    source = extract(original, work / "offline-source", stem(version, "source"))
    with (source / "rust-toolchain.toml").open("rb") as file:
        toolchain = tomllib.load(file)["toolchain"]["channel"]
    rustc = subprocess.check_output(["rustc","--version"], cwd=source, text=True).split()
    require(rustc[:2] == ["rustc", toolchain], "installed Rust compiler differs from the packaged toolchain")
    result.update(phase="rebuild", source_archive_sha256=source_obj["sha256"], rust_toolchain=toolchain)
    evidence.write_text(json.dumps(result))
    # Execute the documented procedure shipped in the candidate. Its fresh Cargo
    # home and target directory, --frozen build and extracted-binary rmcp probe
    # are the same ones used by Native distribution.
    sysroot = Path(subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip()).resolve(strict=True)
    candidate_source_sandbox.run(source, original, work / "rebuilt", sysroot, revision, TARGET)
    receipt_path = work / "rebuilt" / (TARGET + ".json")
    require(stat.S_ISREG(receipt_path.lstat().st_mode) and receipt_path.stat().st_size <= 1024 * 1024,
            "invalid isolated build receipt")
    receipt = json.loads(receipt_path.read_bytes())
    built = receipt["file"]
    require(receipt["native"] is True and receipt["source_sha256"] == source_obj["sha256"]
            and receipt["revision"] == revision and receipt["mcp_version"] == receipt["version"] == version
            and built["target"] == TARGET and re.fullmatch(r"[a-f0-9]{64}", built["executable_sha256"]),
            "rebuilt MCP identity differs from its source")
    packaged_digest = entry_for(data, "native", TARGET)["executable_sha256"]
    require(built["executable_sha256"] == packaged_digest, "rebuilt executable differs from the packaged executable")
    rebuilt_name = stem(version, TARGET)
    archive = work / "rebuilt" / (rebuilt_name + ".tar.gz")
    require(stat.S_ISREG(archive.lstat().st_mode) and built["name"] == archive.name
            and built["sha256"] == sha256(archive), "rebuilt archive differs")
    binary = extract(archive, work / "verified-rebuild", rebuilt_name) / "komodo-mcp"
    result.update(phase="fixture_probe",isolated_source_build=True,rebuilt_executable_sha256=built["executable_sha256"],
                  target=built["target"],platform=native_host_platform())
    evidence.write_text(json.dumps(result))
    probe = asyncio.run(probe_rebuilt(binary, packaged_digest, version))
    result.update(passed=True,phase="complete",**probe)
    evidence.write_text(json.dumps(result))


# The public evidence contains defined operational facts. Failure text, raw
# replies, process environments, host paths and arbitrary strings are excluded.
def known_labels(value, allowed):
    return (isinstance(value, list) and len(value) <= len(allowed)
            and all(isinstance(label, str) and label in allowed for label in value)
            and len(value) == len(set(value)))


def other_sync_updates(value):
    operations = {"CreateResourceSync","UpdateResourceSync","RenameResourceSync","DeleteResourceSync",
                  "WriteSyncContents","CommitSync","RefreshResourceSyncPending"}
    return (isinstance(value,list) and len(value) <= 100
            and all(isinstance(row,dict) and set(row) == {"id","operation"}
                    and isinstance(row["id"],str) and re.fullmatch(r"[a-f0-9]{24}",row["id"])
                    and isinstance(row["operation"],str) and row["operation"] in operations for row in value)
            and len({row["id"] for row in value}) == len(value))


SCENARIOS = {
    "container_workflow": {"default_running":int, "all_states":int, "pages":int, "stopped_exit_code":int,
        "state":{"exited"}, "health":{"healthy","starting","unhealthy","none"}, "restart_count":int,
        "matched_docker_id":bool, "list_only_identity":{"Server Read + Attach"}, "inspect_denied":bool,
        "hidden_call_rejected":bool, "seeded_private_health_output_verified":bool},
    "configuration": {"preview_writes":int, "apply_writes":int, "field":{"Stack.auto_pull"},
                      "checked_all_other_config":bool},
    "stale_resource_sync": {"source":{"inline"}, "rejection":{"stale_preview"}, "preflight_refreshes":int,
                            "run_sync_history_delta":int, "unchanged_server_configuration":bool},
    "read_only_operation": {"resource_grant":{"Read"}, "local_operate":bool, "local_configuration_edit":bool,
        "discovered_tools":lambda v: known_labels(v,DENIAL_TOOLS) and len(v) == len(DENIAL_TOOLS),
        "visible_sync_read":bool, "preflight_write":{"RefreshResourceSyncPending"}, "refresh_attempts":int,
        "preflight_error":{"permission_denied"}, "apply_outcome":{"not_submitted"},
        "run_sync_requests_submitted":int, "core_run_sync_history_delta":int, "target_unchanged":bool,
        "other_target_updates":other_sync_updates},
    "interruption": {"accepted_update_id":r"[a-f0-9]{24}", "final_outcome":{"succeeded","failed"},
        "matched_operator":bool, "core_deploy_history_delta":int, "reconnected_read_only":bool,
        "observed_running_via_docker":bool},
}
DIGEST = r"[a-f0-9]{64}"
VERSION = r"[0-9]{1,5}\.[0-9]{1,5}\.[0-9]{1,7}"


def project_values(values, fields):
    result = {}
    for key, rule in fields.items():
        if key not in values:
            continue
        value = values[key]
        if isinstance(rule, type):
            valid = type(value) is rule
        elif callable(rule):
            valid = rule(value)
        elif isinstance(rule, set):
            valid = isinstance(value, str) and value in rule
        else:
            valid = isinstance(value, str) and re.fullmatch(rule, value)
        require(valid or value is None, "unexpected value in evidence field " + key)
        result[key] = value
    return result


def remaining(values, keys):
    return {key: len(values[key]) if isinstance(values.get(key), list) else None for key in keys}


def export(path, data, work, case, destination):
    p = work / "private-result.json"
    raw = json.loads(p.read_bytes()) if p.is_file() else {}
    version = data["distribution"]["version"]
    requested = PLATFORM if case == "http" else TARGET
    result = {"case":case, "candidate_record_sha256":sha256(path), "version":version,
        "source_commit":data["distribution"]["revision"], "contract_baseline":data["contract"]["baseline_commit"],
        "contract_path":data["contract"]["path"], "contract_sha256":data["contract"]["sha256"],
        "passed":raw.get("passed") is True, "evidence_available":bool(raw), "requested_target":requested}
    result.update(project_values(raw, {"target":{requested}, "platform":{PLATFORM}, "mcp_version":{version}}))
    if case in ("stdio", "source"):
        result.update(project_values(raw, {"native_probe_sandbox":{"bubblewrap"}}))
        if result["passed"]:
            require(result.get("native_probe_sandbox") == "bubblewrap", "successful native probe lacks sandbox evidence")
    client_fields = {"name":{CLIENT_INFO["name"]}, "version":{CLIENT_INFO["version"]}, "protocol":{PROTOCOL}, "python":VERSION}
    result["client"] = project_values(raw.get("client", {}), client_fields)
    if case == "source":
        result.update(project_values(raw, {"rebuilt_executable_sha256":DIGEST, "source_archive_sha256":DIGEST,
                                          "cargo_offline":bool, "isolated_source_build":bool, "rust_toolchain":VERSION,
                                          "phase":{"verify_source","rebuild","fixture_probe","complete"}}))
        result["loopback_only"] = raw.get("network_interfaces") == ["lo"]
        probe = project_values(raw.get("fixture_probe", {}), {"tool":{"list_stacks"}, "returned_stacks":int, "core_inventory_reads":int})
        result["fixture_probe"] = probe
        if result["passed"]:
            require(result.get("rebuilt_executable_sha256") == entry_for(data, "native", TARGET)["executable_sha256"],
                    "rebuilt executable differs from the packaged executable")
            require(result.get("phase") == "complete" and result["loopback_only"] and result.get("cargo_offline") is True
                    and result.get("source_archive_sha256") == entry_for(data, "source")["sha256"]
                    and result.get("rust_toolchain") and result.get("isolated_source_build") is True
                    and probe.get("tool") == "list_stacks" and probe.get("returned_stacks") == 3
                    and (probe.get("core_inventory_reads") or 0) > 0, "offline rebuild lacks a verified MCP fixture call")
    else:
        if raw.get("candidate_record_sha256") or result["passed"]:
            require(raw.get("candidate_record_sha256") == result["candidate_record_sha256"]
                    and raw.get("source_commit") == result["source_commit"]
                    and raw.get("package_version") == version and raw.get("transport") == case,
                    "live result belongs to a different packaged candidate")
        result.update(project_values(raw, {"core_version":VERSION, "periphery_version":VERSION,
                    "docker_version":VERSION, "dind_unix_socket_only":bool, "executable_sha256":DIGEST, "index_digest":"sha256:"+DIGEST,
                    "platform_manifest_digest":"sha256:"+DIGEST, "compose_cleanup_verified":bool,
                    "workload_cleanup_verified":bool, "workload_cleanup_status":{"not_started","failed","residue","unknown","verified"},
                    "failed_workload_cleanup_step":CLEANUP_STEPS, "workload_cleanup_http_status":lambda v: type(v) is int and 400 <= v <= 599,
                    "phase":{"preflight","provisioning","exercising","cleanup","complete","failed"},
                    "candidate_image_cleanup":{"removed","failed"}}))
        result["scenarios"] = {name: project_values(raw["scenarios"][name], fields)
                               for name, fields in SCENARIOS.items() if name in raw.get("scenarios", {})}
        scans = raw.get("redaction_checked", [])
        require(known_labels(scans, REDACTION_SURFACES), "invalid redaction evidence")
        result["redaction_checked"] = [surface for surface in REDACTION_SURFACES if surface in scans]
        if case == "http":
            result.update(project_values(raw, {"http_candidate_network_verified":bool}))
        result["fixture_image_ids"] = project_values(raw.get("fixture_image_ids", {}),
            dict.fromkeys(["core","periphery","mongo","docker"], "sha256:"+DIGEST))
        project = remaining(raw.get("cleanup_remaining", {}), ("containers","networks","volumes"))
        workloads = remaining(raw.get("owned_workload_cleanup", {}), WORKLOAD_RESOURCES)
        result["remaining_project_resources"] = project
        result["remaining_test_resources"] = workloads
        status = result.get("workload_cleanup_status", "unknown")
        if status == "verified" and any(workloads.values()):
            status = "residue"
        elif status == "verified" and None in workloads.values():
            status = "unknown"
        result["workload_cleanup_status"] = status
        result["workload_cleanup_verified"] = result.get("workload_cleanup_verified") is True and status == "verified"
        result["compose_cleanup_verified"] = result.get("compose_cleanup_verified") is True and all(
            count == 0 for count in project.values())
        result["cleanup_verified"] = (raw.get("cleanup_verified") is True
                                      and result["compose_cleanup_verified"] and result["workload_cleanup_verified"])
        if result["passed"]:
            require(result["cleanup_verified"] and result["phase"] == "complete"
                    and result.get("core_version") == result.get("periphery_version") == CORE_VERSION
                    and result.get("docker_version") and result.get("executable_sha256")
                    and all(set(result["scenarios"].get(name, {})) == set(fields)
                            for name, fields in SCENARIOS.items()),
                    "complete live evidence is missing running service versions, outcomes or verified cleanup")
            require(len(scans) == len(REDACTION_SURFACES), "missing required redaction scan")
            require(result.get("dind_unix_socket_only") is True
                    and (case != "http" or result.get("http_candidate_network_verified") is True),
                    "missing verified nested Docker socket or HTTP bridge isolation")
            stale = result["scenarios"]["stale_resource_sync"]
            require(stale["preflight_refreshes"] == stale["run_sync_history_delta"] == 0
                    and stale["unchanged_server_configuration"] is True,
                    "stale inline source requires zero preflight refreshes and zero RunSync submissions")
            denied = result["scenarios"]["read_only_operation"]
            require(all(denied[field] is True for field in
                        ("local_operate","local_configuration_edit","visible_sync_read","target_unchanged"))
                    and denied["run_sync_requests_submitted"] == denied["core_run_sync_history_delta"] == 0
                    and denied["refresh_attempts"] == 1 and denied["preflight_error"] == "permission_denied"
                    and denied["apply_outcome"] == "not_submitted" and denied["resource_grant"] == "Read"
                    and denied["preflight_write"] == "RefreshResourceSyncPending"
                    and denied["discovered_tools"] is not None and denied["other_target_updates"] is not None,
                    "missing evidence of denied preflight and zero RunSync submissions")
            if case == "http":
                require(result.get("index_digest") and result.get("platform_manifest_digest"),
                        "missing running image identity")
    if result["passed"]:
        require(result.get("target") == requested and result.get("platform") == PLATFORM
                and result.get("mcp_version") == version and all(result["client"].get(k) for k in client_fields),
                "successful evidence lacks the observed platform or MCP client handshake")
    destination.parent.mkdir(parents=True,exist_ok=True)
    with destination.open("x") as output:
        json.dump(result, output, indent=2)
        output.write("\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("acquire","exercise","export"))
    parser.add_argument("--record", type=Path, default=Path(DEFAULT_RECORD))
    parser.add_argument("--work", type=Path, required=True)
    parser.add_argument("--case", choices=("source","stdio","http"), required=True)
    parser.add_argument("--project")
    parser.add_argument("--docker-context")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    path, data = record(args.record)
    work = args.work.resolve()
    if args.action == "acquire":
        acquire(data, work, args.case)
    elif args.action == "export":
        if not args.output:
            parser.error("export requires --output")
        export(path, data, work, args.case, args.output)
    elif args.case == "source":
        rebuild(data, work)
    else:
        if not args.project or not args.docker_context:
            parser.error("live exercises require --project and --docker-context")
        run_live(path, data, work, args.case, args.project, args.docker_context)


if __name__ == "__main__":
    main()
