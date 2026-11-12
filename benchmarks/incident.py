"""Incident workflow on the same prepared stdio artifacts and HTTP instrumentation.

Concurrent callers share one persistent MCP process. HTTP activity is attributed
to their whole group, since neither server propagates a caller ID to Core.
"""

import asyncio
import copy
import json
import re
import signal
import sys
import tempfile
import time
import uuid
from pathlib import Path

from core import Core, KEY, ROOT, SECRET, digest
from stdio import Client, PROTOCOL, child_env

FORMAT = "komodo-incident-benchmark/v1"
FIXTURE = ROOT / "tests/fixtures/core-2.3.2/incident.json"
WIRE = ROOT / "tests/fixtures/core-2.3.2/incident-wire.json"
READS = frozenset(("GetVersion", "GET /version", "ListServers", "ListStacks", "ListDeployments",
                  "GetServerState", "GetSystemStats", "ListAllStackServices", "GetStack",
                  "ListUpdates", "ListAlerts", "GetUpdate", "GetStackLog", "GetContainerLog",
                  "ListContainers", "ListDockerContainers", "InspectContainer", "InspectDockerContainer",
                  "GetPermission"))


def expected_evidence(count):
    fixture = json.loads(FIXTURE.read_text())
    discovery = [{"id": fixture["stack"]["id"], "name": fixture["stack"]["name"], "state": "unhealthy"},
                 {"id": fixture["healthy_stack"]["id"], "name": fixture["healthy_stack"]["name"], "state": "running"}]
    discovery.extend({"id": f"{0x4000 + i:024x}", "name": f"benchmark-stack-{i:06d}", "state": "running"}
                     for i in range(count - 2))
    return {"stack_id": fixture["stack"]["id"], "stack_name": fixture["stack"]["name"],
            "service": fixture["affected_service"], "update_id": fixture["update"]["id"],
            "failed_stage": fixture["update"]["failed_stage"],
            "log_excerpt": fixture["expected_log_excerpt"], "visible_stacks": count,
            "discovery_sha256": digest(sorted(discovery, key=lambda s: s["id"]))}


class IncidentCore(Core):
    def __init__(self, count, delay_ms=0):
        if type(count) is not int or not 2 <= count <= 1000:
            raise ValueError("incident fixture requires 2..1000 stacks")
        super().__init__(0, delay_ms)
        self.fixture = json.loads(FIXTURE.read_text())
        self.wire = json.loads(WIRE.read_text())
        self.items = copy.deepcopy(self.wire["stacks"])
        self.servers = copy.deepcopy(self.wire["servers"])
        for index in range(count - 2):
            server = copy.deepcopy(self.servers[1])
            server.update(id=f"{0x3000 + index:024x}", name=f"benchmark-server-{index:06d}")
            stack = copy.deepcopy(self.items[1])
            stack.update(id=f"{0x4000 + index:024x}", name=f"benchmark-stack-{index:06d}")
            stack["info"].update(server_id=server["id"], server_name=server["name"])
            self.servers.append(server)
            self.items.append(stack)
        self.expected = expected_evidence(count)

    def read(self, operation, params):
        f, w = self.fixture, self.wire
        if operation == "ListServers":
            return 200, self.list_resources(self.servers, params)
        if operation == "ListDeployments":
            return 200, self.list_resources(w["deployments"], params)
        if operation in ("GetServerState", "GetSystemStats"):
            server = params.get("server")
            if server not in {s["id"] for s in self.servers}:
                return 404, {"error": "unknown fixture server"}
            if server == f["unavailable_server"]["id"]:
                return 503, {"error": "synthetic unavailable server"}
            if operation == "GetServerState" and server == f["unknown_server"]["id"]:
                return 200, {"status": "Disabled"}
            return 200, w["server_state" if operation == "GetServerState" else "stats"]
        if operation == "ListAllStackServices":
            if (not isinstance(params.get("stacks"), list) or len(params["stacks"]) != 1
                    or not 1 <= params.get("limit", 0) <= 101):
                raise ValueError("incident service query must be bounded and target one stack")
            stack = next((s for s in self.items if s["id"] == params["stacks"][0]), None)
            if stack is None:
                return 200, []
            services = copy.deepcopy(w["services"])
            for service in services:
                service.update(stack_id=stack["id"], stack_name=stack["name"])
                if stack["id"] != f["stack"]["id"]:
                    service["state"] = "Running"
            start = params.get("page", 0) * params["limit"]
            return 200, services[start:start + params["limit"]]
        if operation == "GetStack":
            stack = next((s for s in self.items if params.get("stack") in (s["id"], s["name"])), None)
            if stack is None:
                return 404, {"error": "unknown fixture stack"}
            detail = copy.deepcopy(w["stack_detail"])
            detail["_id"]["$oid"] = stack["id"]
            detail["name"] = stack["name"]
            detail["config"]["server_id"] = stack["info"]["server_id"]
            return 200, detail
        if operation in ("ListUpdates", "ListAlerts"):
            field = "updates" if operation == "ListUpdates" else "alerts"
            query = params.get("query") or {}
            if set(query) - {"target.id", "target.type", "operation"}:
                raise ValueError("unsupported history filter")
            items = w[field] if params.get("page", 0) == 0 else []
            items = [item for item in items if
                     (not query.get("target.id") or item["target"]["id"] == query["target.id"])
                     and (not query.get("target.type") or item["target"]["type"] == query["target.type"])
                     and (not query.get("operation") or item.get("operation") == query["operation"])]
            return 200, {field: items, "next_page": None}
        if operation == "GetUpdate" and params.get("id") == f["update"]["id"]:
            return 200, w["update"]
        if operation == "GetStackLog" and params.get("stack") == f["stack"]["id"]:
            return 200, self.log(params, f["affected_service"] in params.get("services", [])
                                 or not params.get("services"))
        if operation in ("ListContainers", "ListDockerContainers") and params.get("server") == f["server"]["id"]:
            return 200, [{"name": name, "state": "running", "image": "fixture:latest",
                          "status": "Up 1 minute (unhealthy)" if service == "web" else "Up 1 minute (healthy)"}
                         for service, name in (("web", f["container"]), ("db", "alpha-web-db-1"))]
        if operation in ("InspectContainer", "InspectDockerContainer") and params.get("server") == f["server"]["id"]:
            if params.get("container") not in (f["container"], "alpha-web-db-1"):
                return 404, {"error": "unknown fixture container"}
            affected = params["container"] == f["container"]
            return 200, {"Name": params["container"], "State": {"Status": "running", "Health": {
                "Status": "unhealthy" if affected else "healthy"}}, "Config": {"Image": "fixture:latest", "Labels": {
                    "com.docker.compose.project": w["stack_detail"]["info"]["deployed_project_name"],
                    "com.docker.compose.service": f["affected_service"] if affected else f["healthy_service"]}}}
        if operation == "GetContainerLog" and params.get("server") == f["server"]["id"]:
            if params.get("container") not in (f["container"], "alpha-web-db-1"):
                return 404, {"error": "unknown fixture container"}
            return 200, self.log(params, params["container"] == f["container"])
        if operation == "GetPermission":
            return 200, {"level": "Read", "specific": []}
        return super().read(operation, params)

    def log(self, params, affected):
        tail = params.get("tail")
        if type(tail) is not int or not 1 <= tail <= 500:
            raise ValueError("incident logs require bounded positive tail")
        log = copy.deepcopy(self.wire["log"])
        if not affected:
            log["stdout"] = "db | ready\n"
        log["stdout"] = "".join(log["stdout"].splitlines(keepends=True)[-tail:])
        return log


def require(condition, message):
    if not condition:
        raise ValueError("invalid incident benchmark: " + message)


async def workflow(client, implementation, page_size, expected, verify=True):
    """Read equivalent target/service/failed-stage/log evidence through stdio."""
    calls, response_bytes = 0, 0

    async def call(name, arguments):
        nonlocal calls, response_bytes
        result, size = await client.request("tools/call", {"name": name, "arguments": arguments})
        calls += 1
        response_bytes += size
        require(result.get("isError", False) is False, f"{name} tool failed")
        payload = result.get("structuredContent")
        require(isinstance(payload, dict), f"{name} missing structured content")
        return payload

    start = time.perf_counter_ns()
    async with asyncio.timeout(client.timeout):
        stacks, page, cursor, cursors = [], 0, None, set()
        page_bound = (expected["visible_stacks"] + 2 + page_size - 1) // page_size + 1
        for _ in range(page_bound):
            if implementation == "rust":
                payload = await call("fleet_health", {"page": page, "page_size": page_size})
                chunk = payload.get("stacks")
                require(isinstance(chunk, list) and len(chunk) <= page_size, "invalid fleet page")
                require(payload.get("page") == page and payload.get("page_size") == page_size,
                        "invalid fleet page identity")
                more = payload.get("has_more")
                require(type(more) is bool and payload.get("next_page") == (page + 1 if more else None),
                        "invalid fleet continuation")
                stacks.extend(chunk)
                if not more:
                    break
                page += 1
            else:
                arguments = {"page_size": page_size, **({"cursor": cursor} if cursor is not None else {})}
                payload = await call("komodo_stack_list", arguments)
                chunk = payload.get("items")
                require(isinstance(chunk, list) and len(chunk) <= page_size, "invalid reference stack page")
                stacks.extend(chunk)
                pagination = payload.get("page", {})
                require(pagination.get("total") == expected["visible_stacks"], "invalid reference total")
                cursor = pagination.get("next_cursor")
                if cursor is None:
                    break
                require(isinstance(cursor, str) and cursor and cursor not in cursors and len(chunk) == page_size,
                        "invalid reference cursor")
                cursors.add(cursor)
        else:
            raise ValueError("incident discovery pagination exceeded bound")
        ids = [s.get("id") for s in stacks]
        require(len(ids) == len(set(ids)) and len(ids) == expected["visible_stacks"], "incomplete/duplicate fleet")
        broken = [s for s in stacks if s.get("health" if implementation == "rust" else "state") == "unhealthy"]
        require(len(broken) == 1, "expected exactly one unhealthy stack")
        stack = broken[0]
        if implementation == "rust":
            diagnosis = await call("stack_diagnose", {"id": stack["id"]})
            require(diagnosis.get("outcome") == "unhealthy" and diagnosis.get("stack_id") == stack["id"],
                    "diagnosis did not identify stack failure")
            affected = diagnosis.get("affected_services")
            require(isinstance(affected, list) and len(affected) == 1, "missing affected service")
            service = affected[0]
            failed = diagnosis.get("failed_update")
            require(isinstance(failed, dict) and failed.get("outcome") == "failed", "missing failed update")
            update = await call("get_update", {"id": failed["id"]})
            require(update.get("id") == failed["id"] and update.get("outcome") == "failed"
                    and update.get("target") == {"kind": "stack", "id": stack["id"]}, "update evidence mismatch")
            stages = [s for s in update.get("stages", []) if s.get("success") is False]
            require(len(stages) == 1, "missing failed update stage")
            log = await call("get_stack_logs", {"id": stack["id"], "services": [service],
                                                  "tail": 40, "max_bytes": 4096, "timestamps": False})
            require(diagnosis.get("logs", {}).get("stdout") and log.get("target", {}).get("services") == [service],
                    "missing diagnosis/service log evidence")
            extra = {"alert_ids": [a["id"] for a in diagnosis.get("recent_alerts", [])],
                     "diagnosis_gaps": [g["source"] for g in diagnosis.get("gaps", [])]}
            update_id, failed_stage = update["id"], stages[0]["stage"]
        else:
            detail = await call("komodo_stack_info", {"stack": stack["id"], "inline_full": True})
            info = detail.get("info")
            require(isinstance(info, dict), "reference stack info must be inline")
            server = info["config"]["server_id"]
            project = info.get("info", {}).get("deployed_project_name") or info["config"].get("project_name") or info["name"]
            containers = await call("komodo_container_list", {"server": server, "page_size": 100})
            require(containers.get("page", {}).get("next_cursor") is None, "container fixture exceeded bounded page")
            affected = []
            for container in containers.get("items", []):
                inspection = await call("komodo_container_inspect", {"server": server, "container": container["name"], "inline_full": True})
                inspect = inspection.get("inspect", {})
                labels = inspect.get("Config", {}).get("Labels", {})
                if (labels.get("com.docker.compose.project") == project
                        and inspect.get("State", {}).get("Health", {}).get("Status") == "unhealthy"):
                    affected.append((container["name"], labels.get("com.docker.compose.service")))
            require(len(affected) == 1 and isinstance(affected[0][1], str), "reference did not identify service")
            container, service = affected[0]
            updates = await call("komodo_update_list", {"target_type": "Stack", "target_id": stack["id"], "page_size": 100})
            failed = [u for u in updates.get("items", []) if u.get("status") == "Complete" and u.get("success") is False]
            require(len(failed) == 1, "reference did not identify failed update")
            update = await call("komodo_update_info", {"id": failed[0]["id"], "inline_full": True})
            info = update.get("info", {})
            require(info.get("target") == {"type": "Stack", "id": stack["id"]}
                    and info.get("success") is False and info.get("status") == "Complete", "reference update mismatch")
            stages = [s for s in info.get("logs", []) if s.get("success") is False]
            require(len(stages) == 1, "reference missing failed stage")
            update_id, failed_stage = info["_id"]["$oid"], stages[0]["stage"]
            log = await call("komodo_container_logs", {"server": server, "container": container,
                                                         "tail": 40, "timestamps": False, "inline_full": True})
            extra = {"limits": "Reference has no fleet health, stack diagnosis, alert or stack service reader; targeted Docker inspect supplies service health."}
        excerpts = [line for text in (log.get("stdout", ""), log.get("stderr", ""))
                    for line in text.splitlines() if "ERROR:" in line]
        require(len(excerpts) == 1, "missing or ambiguous error log excerpt")
        observed = {"stack_id": stack["id"], "stack_name": stack["name"], "service": service,
                    "update_id": update_id, "failed_stage": failed_stage,
                    "log_excerpt": excerpts[0], "visible_stacks": len(stacks),
                    "discovery_sha256": digest(sorted(({k: s.get(k) for k in ("id", "name", "state")} for s in stacks),
                                                      key=lambda s: s["id"]))}
    elapsed = (time.perf_counter_ns() - start) / 1e6
    if verify:
        require(observed == expected, "workflow differs from independent incident oracle")
    return {"elapsed_ms": elapsed, "mcp_calls": calls, "response_bytes": response_bytes,
            "observed": observed, "equivalence": {"sha256": digest(observed), "verified": True} if verify else None,
            "additional_evidence": extra}


async def group(client, implementation, args, expected):
    start = time.perf_counter_ns()
    tasks = [asyncio.create_task(workflow(client, implementation, args.page_size, expected, verify=False))
             for _ in range(args.concurrency)]
    try:
        results = await asyncio.gather(*tasks)
    except BaseException:
        for task in tasks:
            task.cancel()
        await asyncio.gather(*tasks, return_exceptions=True)
        raise
    elapsed = (time.perf_counter_ns() - start) / 1e6
    for result in results:
        require(result["observed"] == expected, "workflow differs from independent incident oracle")
        result["equivalence"] = {"sha256": digest(result["observed"]), "verified": True}
    return {"elapsed_ms": elapsed,
            "callers": results, "mcp_calls": sum(x["mcp_calls"] for x in results),
            "response_bytes": sum(x["response_bytes"] for x in results)}


def events_for(report, name, batch, phase, sample):
    return [e for e in report["core_requests"] if e["implementation"] == name and e["batch"] == batch
            and e["phase"] == phase and e["sample"] == sample]


async def compare(args, report, artifacts):
    import bench
    with tempfile.TemporaryDirectory(prefix="komodo incident benchmark ") as temp, IncidentCore(
            args.fixture_size, args.fixture_delay_ms) as core:
        home = Path(temp)
        (home / "config.toml").write_text("")
        env = child_env(home, core.address)
        report["fixture"]["wire_sha256"] = digest({"stacks": core.items, "servers": core.servers, "wire": core.wire})
        report["fixture"]["expected"] = core.expected
        report["fixture"]["expected_sha256"] = digest(core.expected)

        async def close(client, output, name, batch, kind, failed):
            try:
                core.mark(name, batch, kind + "_shutdown")
            finally:
                try:
                    await client.close(failed)
                finally:
                    output["processes"].append({"batch": batch, "kind": kind, **client.diagnostics()})

        try:
            for batch in range(args.repetitions):
                order = ["rust", "reference"] if batch % 2 == 0 else ["reference", "rust"]
                report["execution_order"].append(order)
                for name in order:
                    output = report["implementations"][name]
                    client = Client(artifacts[name]["argv"], env, home, args.timeout)
                    failed = True
                    core.mark(name, batch, "warm_startup")
                    try:
                        output["warm_sessions"].append({"batch": batch, **await client.initialize()})
                        for phase, count in (("warmup", args.warmup), ("warm", args.samples)):
                            for sample in range(count):
                                core.mark(name, batch, phase, sample)
                                output[phase].append({"batch": batch, "sample": sample,
                                                      **await group(client, name, args, core.expected)})
                        failed = False
                    finally:
                        await close(client, output, name, batch, "warm", failed)
                    # RSS gets its own fresh process so ps/proc sampling never
                    # contaminates the recorded warm latency distributions.
                    client = Client(artifacts[name]["argv"], env, home, args.timeout)
                    samples = []
                    started = time.perf_counter_ns()

                    async def poll():
                        while True:
                            if client.process is not None and client.process.returncode is None:
                                value = await bench.rss(client.process.pid)
                                if value is not None:
                                    samples.append({"since_start_ms": (time.perf_counter_ns() - started) / 1e6,
                                                    "rss_bytes": value})
                            await asyncio.sleep(args.rss_interval_ms / 1000)

                    polling = asyncio.create_task(poll())
                    failed = True
                    core.mark(name, batch, "memory_startup")
                    try:
                        initialization = await client.initialize()
                        core.mark(name, batch, "memory_idle")
                        await asyncio.sleep(args.idle_ms / 1000)
                        idle = await bench.rss(client.process.pid)
                        require(idle is not None, "idle RSS unavailable")
                        for phase, count in (("memory_warmup", args.warmup), ("memory_workload", args.samples)):
                            for sample in range(count):
                                core.mark(name, batch, phase, sample)
                                await group(client, name, args, core.expected)
                        final = await bench.rss(client.process.pid)
                        if final is not None:
                            samples.append({"since_start_ms": (time.perf_counter_ns() - started) / 1e6, "rss_bytes": final})
                        if polling.done():
                            await polling
                        require(bool(samples), "no RSS observations")
                        output["memory"].append({"batch": batch, "protocol": initialization["protocol"],
                                                 "idle_rss_bytes": idle, "peak_sampled_rss_bytes": max(
                                                     [idle, *(x["rss_bytes"] for x in samples)]), "samples": samples})
                        failed = False
                    finally:
                        polling.cancel()
                        await asyncio.gather(polling, return_exceptions=True)
                        await close(client, output, name, batch, "memory", failed)
        finally:
            try:
                core.mark("driver", None, "finished")
            finally:
                report["core_requests"] = list(core.events)
            for name, output in report["implementations"].items():
                for phase in ("warmup", "warm"):
                    for sample in output[phase]:
                        events = events_for(report, name, sample["batch"], phase, sample["sample"])
                        sample["core_requests"] = len(events)
                        sample["core_response_bytes"] = sum(e["response_body_bytes"] for e in events)
                        sample["core_operations"] = {op: sum(e["operation"] == op for e in events)
                                                     for op in sorted({e["operation"] for e in events})}
                        sample["peak_observed_core_concurrency"] = max((e["concurrency_at_entry"] for e in events), default=0)


def summaries(report):
    import bench
    return {name: {
        "warm_group_ms": bench.stats([x["elapsed_ms"] for x in data["warm"]]),
        "warm_caller_ms": bench.stats([c["elapsed_ms"] for x in data["warm"] for c in x["callers"]]),
        "warm_response_bytes": bench.stats([x["response_bytes"] for x in data["warm"]]),
        "warm_mcp_calls": bench.stats([x["mcp_calls"] for x in data["warm"]]),
        "warm_core_requests": bench.stats([x["core_requests"] for x in data["warm"]]),
        "warm_core_response_bytes": bench.stats([x["core_response_bytes"] for x in data["warm"]]),
        "peak_observed_core_concurrency": max(x["peak_observed_core_concurrency"] for x in data["warm"]),
        "idle_rss_bytes": bench.stats([x["idle_rss_bytes"] for x in data["memory"]]),
        "peak_sampled_rss_bytes": bench.stats([x["peak_sampled_rss_bytes"] for x in data["memory"]]),
        "batches_warm_group_ms": [bench.stats([x["elapsed_ms"] for x in data["warm"] if x["batch"] == batch])
                                  for batch in range(report["controls"]["repetitions"])],
    } for name, data in report["implementations"].items()}


def validate(report):
    import bench
    require(report.get("format") == FORMAT and report.get("status") == "complete", "incomplete/unsupported run")
    require(report.get("errors") == [], "run contains errors")
    for key in ("run_id", "timestamp", "finished_at", "repository", "runner", "artifacts", "host", "fixture",
                "workload", "controls", "definitions", "execution_order", "core_requests", "invocation"):
        require(bool(report.get(key)), "missing " + key)
    def fields(value, names):
        require(isinstance(value, dict) and all(k in value for k in names.split()), "missing metadata fields: " + names)

    def revision(value):
        fields(value, "revision dirty status")
        require(isinstance(value["revision"], str) and bool(re.fullmatch(r"[0-9a-f]{40}", value["revision"]))
                and type(value["dirty"]) is bool and isinstance(value["status"], list), "invalid repository identity")

    def hash_value(value):
        return isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value)

    revision(report["repository"])
    fields(report["runner"], "version files_sha256")
    require(all(hash_value(report["runner"]["files_sha256"].get(k)) for k in ("bench.py", "core.py", "stdio.py", "incident.py")),
            "missing runner hashes")
    fields(report["host"], "os system architecture logical_cpus python virtualization_note cpu memory_bytes")
    require(bench.number(report["host"]["logical_cpus"], True) and bench.number(report["host"]["memory_bytes"], True), "invalid host capacity")
    fields(report["artifacts"], "format prepared_at commands rust reference")
    require(report["artifacts"]["format"] == "komodo-benchmark-artifacts/v1", "unsupported artifact manifest")
    rust, reference = report["artifacts"]["rust"], report["artifacts"]["reference"]
    fields(rust, "argv sha256 bytes repository source_sha256 cargo_lock_sha256 rustc cargo target profile build_environment")
    revision(rust["repository"])
    fields(reference, "repository release revision directory node node_version node_sha256 npm_version package_lock_sha256 build_sha256 dependencies_sha256 argv")
    require(reference["revision"] == bench.REFERENCE_REV and reference["release"] == "1.5.0"
            and reference["repository"] == bench.REFERENCE_URL, "incorrect reference pin")
    require(bench.number(rust["bytes"], True) and all(hash_value(v) for v in
            (rust["sha256"], rust["source_sha256"], rust["cargo_lock_sha256"], reference["package_lock_sha256"],
             reference["node_sha256"], reference["build_sha256"], reference["dependencies_sha256"])), "missing artifact hashes")
    fields(report["definitions"], "warm concurrency response_bytes core_requests rss percentiles limits")
    fields(report["workload"], "name protocol concurrency rust reference equivalence environment")
    controls = report["controls"]
    for key in ("samples", "repetitions", "concurrency", "fixture_size", "page_size", "idle_ms", "rss_interval_ms"):
        require(type(controls.get(key)) is int and controls[key] > 0, "invalid " + key)
    require(2 <= controls["fixture_size"] <= 1000 and 1 <= controls["page_size"] <= 100
            and 1 <= controls["concurrency"] <= 16, "invalid workload bounds")
    require(type(controls.get("warmup")) is int and controls["warmup"] >= 0 and bench.number(controls.get("timeout"), True),
            "invalid warmup/timeout")
    require(report["workload"]["protocol"] == PROTOCOL and report["workload"]["concurrency"] == controls["concurrency"],
            "workload/control mismatch")
    expected = expected_evidence(controls["fixture_size"])
    require(report["fixture"]["expected"] == expected and report["fixture"]["expected_sha256"] == digest(expected),
            "incorrect incident oracle")
    require(report["fixture"]["visible_stacks"] == controls["fixture_size"], "fixture count mismatch")
    require(bench.number(report["fixture"].get("delay_ms")) and bool(report["fixture"].get("permissions")), "invalid fixture controls")
    for key in ("wire_sha256", "seed_sha256", "wire_seed_sha256", "expected_sha256"):
        require(hash_value(report["fixture"].get(key)), "missing fixture hash")
    require(report["artifacts"]["reference"]["revision"] == bench.REFERENCE_REV
            and report["artifacts"]["reference"]["repository"] == bench.REFERENCE_URL, "incorrect reference pin")
    require(set(report["implementations"]) == {"rust", "reference"}, "comparison requires both implementations")
    repetitions = controls["repetitions"]
    require(report["execution_order"] == [["rust", "reference"] if b % 2 == 0 else ["reference", "rust"]
                                         for b in range(repetitions)], "incorrect execution order")
    for name, data in report["implementations"].items():
        require(len(data["warm_sessions"]) == repetitions and {x["batch"] for x in data["warm_sessions"]} == set(range(repetitions))
                and all(x["protocol"] == PROTOCOL for x in data["warm_sessions"]), "missing initialized sessions")
        require(len(data["processes"]) == 2 * repetitions
                and all(x["returncode"] in (0, -signal.SIGTERM) for x in data["processes"]), "missing/failed process lifecycle")
        for phase, count in (("warmup", controls["warmup"]), ("warm", controls["samples"])):
            require(len(data[phase]) == count * repetitions and {(x["batch"], x["sample"]) for x in data[phase]} ==
                    {(b, s) for b in range(repetitions) for s in range(count)}, "missing/duplicate samples")
            for sample in data[phase]:
                require(bench.number(sample["elapsed_ms"], True) and len(sample["callers"]) == controls["concurrency"],
                        "invalid group latency/concurrency")
                require(sample["mcp_calls"] == sum(x["mcp_calls"] for x in sample["callers"])
                        and sample["response_bytes"] == sum(x["response_bytes"] for x in sample["callers"]),
                        "group MCP accounting mismatch")
                for caller in sample["callers"]:
                    require(bench.number(caller["elapsed_ms"], True) and type(caller["mcp_calls"]) is int
                            and caller["mcp_calls"] > 0 and type(caller["response_bytes"]) is int and caller["response_bytes"] > 0,
                            "invalid caller measurement")
                    require(caller["observed"] == expected and caller["equivalence"] == {"sha256": digest(expected), "verified": True},
                            "failed incident equivalence")
                events = events_for(report, name, sample["batch"], phase, sample["sample"])
                require(sample["core_requests"] == len(events) and sample["core_response_bytes"] == sum(e["response_body_bytes"] for e in events),
                        "group Core accounting mismatch")
                require(sample["core_operations"] == {op: sum(e["operation"] == op for e in events)
                                                       for op in {e["operation"] for e in events}}, "operation count mismatch")
                require(sample["peak_observed_core_concurrency"] == max((e["concurrency_at_entry"] for e in events), default=0),
                        "invalid observed concurrency")
        require(len(data["memory"]) == repetitions and {x["batch"] for x in data["memory"]} == set(range(repetitions)), "missing memory session")
        for memory in data["memory"]:
            require(memory["protocol"] == PROTOCOL and bench.number(memory["idle_rss_bytes"], True) and bool(memory["samples"])
                    and all(bench.number(x["rss_bytes"], True) and bench.number(x["since_start_ms"]) for x in memory["samples"]),
                    "RSS unavailable")
            require(memory["peak_sampled_rss_bytes"] == max([memory["idle_rss_bytes"], *(x["rss_bytes"] for x in memory["samples"])]),
                    "invalid sampled RSS maximum")
    fixture_contract = json.loads(FIXTURE.read_text())
    for event in report["core_requests"]:
        require(event["operation"] in READS and bench.number(event["service_ms"])
                and type(event["response_body_bytes"]) is int and event["response_body_bytes"] >= 0
                and type(event["concurrency_at_entry"]) is int and event["concurrency_at_entry"] > 0, "invalid/non-read Core event")
        expected_failure = (event["operation"] in ("GetServerState", "GetSystemStats")
                            and event["params"].get("server") == fixture_contract["unavailable_server"]["id"])
        require(event["status"] == (503 if expected_failure else 200), "unexpected fixture failure")
    encoded = json.dumps(report)
    require(KEY not in encoded and SECRET not in encoded and fixture_contract["credential_marker"] not in encoded,
            "credential in report")
    require(report.get("summary") == summaries(report), "missing/inaccurate summary")
    return report


def run(args):
    import bench
    artifacts = bench.preflight(args.artifacts)
    report = {"format": FORMAT, "run_id": str(uuid.uuid4()), "timestamp": bench.now(), "status": "running",
              "repository": bench.git_state(ROOT), "runner": {"version": "incident-1", "files_sha256": {
                  p.name: bench.sha(p) for p in sorted(Path(__file__).parent.glob("*.py"))}},
              "artifacts": artifacts, "host": bench.host_info(args.host_note), "invocation": [sys.executable, *sys.argv],
              "fixture": {"version": "synthetic-core-2.3.2/incident-v1", "visible_stacks": args.fixture_size,
                          "seed_sha256": bench.sha(FIXTURE), "wire_seed_sha256": bench.sha(WIRE),
                          "permissions": "simulated non-admin Read; every mutation rejected; unavailable server intentionally returns 503",
                          "delay_ms": args.fixture_delay_ms, "delay_scope": "every HTTP response"},
              "controls": {k: getattr(args, k) for k in ("fixture_size", "page_size", "concurrency", "warmup", "samples",
                                                        "repetitions", "idle_ms", "rss_interval_ms", "timeout")},
              "workload": {"name": "broken-stack-triage/v1", "protocol": PROTOCOL, "concurrency": args.concurrency,
                           "rust": ["fleet_health (all pages)", "stack_diagnose", "get_update", "get_stack_logs"],
                           "reference": ["komodo_stack_list (all pages)", "komodo_stack_info", "komodo_container_list",
                                         "komodo_container_inspect (each target container)", "komodo_update_list", "komodo_update_info", "komodo_container_logs"],
                           "equivalence": "Complete stack discovery plus target ID, affected Compose service, failed update ID/stage, and error log excerpt. Rust additionally reads server metrics and alerts; reference lacks fleet/diagnosis/alerts tools.",
                           "environment": "same allowlisted synthetic child environment and empty HOME/config as inventory workload"},
              "definitions": {
                  "warm": "persistent initialized stdio process; group wall time and individual logical workflow times in ms incl. MCP JSON parsing/shape checks; equality comparison/reporting excluded",
                  "concurrency": "N independently progressing workflows in one asyncio.gather on the same persistent MCP process; no phase changes until all N finish; Core attributed to whole group only",
                  "response_bytes": "sum of actual matched MCP stdout JSON-RPC frames incl. LF and UTF-8 for every workflow call; notifications/stderr separate; full reference payload stays in size count",
                  "core_requests": "observed HTTP handler events grouped by implementation/batch/phase/sample; includes background health; observed Core body bytes exclude HTTP headers",
                  "rss": "separate fresh process; same concurrent workflow; idle after stabilization and peak sampled through startup/workload before shutdown; server PID only; actual observation timestamps retained",
                  "percentiles": "nearest rank, retained unfiltered samples; pooled group/caller distributions plus per-batch group summaries; no retries",
                  "limits": "Synthetic loopback only; simulated permission checks; no Core database, Docker, real rollout, production performance, or full functional equivalence claim. Small samples prove correctness, not stable performance."},
              "execution_order": [], "implementations": {n: {"warm_sessions": [], "warmup": [], "warm": [], "memory": [], "processes": []}
                                                            for n in ("rust", "reference")}, "core_requests": [], "errors": []}
    try:
        asyncio.run(compare(args, report, artifacts))
        report["status"] = "complete"
        report["finished_at"] = bench.now()
        report["summary"] = summaries(report)
        validate(report)
    except BaseException as error:
        report["status"] = "failed"
        report["errors"].append({"type": type(error).__name__, "message": bench.safe_error(error)})
        raise
    finally:
        report["finished_at"] = bench.now()
        bench.write_json(args.output, report)
    print_summary(report)


def print_summary(report):
    validate(report)
    print(f"Incident {report['run_id']} — {report['fixture']['visible_stacks']} stacks; {report['controls']['concurrency']} concurrent callers; provisional synthetic results")
    print("implementation | group p50/p95 ms | caller p50/p95 ms | MCP bytes/calls p50 | Core bytes/calls p50 | peak RSS MiB")
    for name, result in report["summary"].items():
        print(f"{name} | {result['warm_group_ms']['p50']:.3f}/{result['warm_group_ms']['p95']:.3f} | "
              f"{result['warm_caller_ms']['p50']:.3f}/{result['warm_caller_ms']['p95']:.3f} | "
              f"{result['warm_response_bytes']['p50']}/{result['warm_mcp_calls']['p50']} | "
              f"{result['warm_core_response_bytes']['p50']}/{result['warm_core_requests']['p50']} | {result['peak_sampled_rss_bytes']['max']/2**20:.2f}")
