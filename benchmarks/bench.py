#!/usr/bin/env python3
"""Prepare artifacts, compare full stack inventories, and validate archived evidence.

Python 3.11+ standard library only. Preparation is the ONLY stage that builds or
downloads; `run`, `validate`, and `summarize` never install or compile anything.
"""

import argparse
import asyncio
import datetime
import gzip
import hashlib
import http.client
import json
import math
import os
import platform
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path

from core import Core, KEY, ROOT, SECRET, digest, encoded
from stdio import Client, PROTOCOL, child_env, project, verify

REFERENCE_URL = "https://github.com/MP-Tool/komodo-mcp-server.git"
REFERENCE_REV = "489f7e04b16019480f84e1d2fff8b872b880e335"
FORMAT = "komodo-inventory-benchmark/v1"
RUNNER = "1"


def command(argv, cwd=None):
    return subprocess.check_output([str(x) for x in argv], cwd=cwd, text=True, timeout=30).strip()


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def tree_sha(directory):
    root = Path(directory)
    if not root.is_dir():
        raise ValueError(f"missing prepared directory {root}; run prepare first")
    entries = [(str(p.relative_to(root)), sha(p)) for p in sorted(root.rglob("*")) if p.is_file()]
    if not entries:
        raise ValueError(f"empty prepared directory: {root}")
    return digest(entries)


def git_state(root):
    return {"revision": command(["git", "rev-parse", "HEAD"], root),
            "dirty": bool(command(["git", "status", "--porcelain"], root)),
            "status": command(["git", "status", "--porcelain"], root).splitlines()}


def source_sha():
    paths = [ROOT / "Cargo.toml", ROOT / "Cargo.lock", *sorted((ROOT / "src").rglob("*.rs"))]
    return digest([(str(p.relative_to(ROOT)), sha(p)) for p in paths])


def write_json(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(path.suffix + ".tmp")
    data = (json.dumps(value, indent=2, allow_nan=False) + "\n").encode()
    temporary.write_bytes(gzip.compress(data, mtime=0) if path.suffix == ".gz" else data)
    temporary.replace(path)


def read_json(path):
    data = path.read_bytes()
    return json.loads(gzip.decompress(data) if path.suffix == ".gz" else data)


def executable(value):
    resolved = shutil.which(str(value))
    if not resolved:
        raise ValueError(f"missing executable {value!r}; provide an installed executable path")
    # Multicall executables (notably rustup) dispatch using argv[0]. Resolving
    # rustc/cargo symlinks to rustup changes which command actually runs.
    return str(Path(resolved).absolute())


def prepare(args):
    reference = args.reference_dir.resolve()
    node = command([executable(args.node), "-p", "process.execPath"])
    version = command([node, "--version"])
    if int(version.lstrip("v").split(".")[0]) < 22:
        raise ValueError("reference requires Node >=22; select --node explicitly")
    npm, cargo = [executable(x) for x in (args.npm, args.cargo)]
    # Resolve rustup/asdf shims before Cargo changes cwd into dependency sources.
    # A cwd-sensitive shim supplied as RUSTC can lose the checkout's toolchain.
    sysroot = command([executable(args.rustc), "--print", "sysroot"])
    rustc = executable(Path(sysroot) / "bin/rustc")
    commands = []

    def execute(argv, cwd, overrides=None):
        commands.append({"argv": [str(x) for x in argv], "cwd": str(cwd),
                         "environment_overrides": overrides or {}})
        environment = {**os.environ, "PATH": str(Path(node).parent) + os.pathsep + os.environ.get("PATH", ""),
                       **(overrides or {})}
        subprocess.run(argv, cwd=cwd, env=environment, check=True, timeout=900)

    if not reference.exists():
        execute(["git", "clone", "--no-checkout", REFERENCE_URL, str(reference)], ROOT)
        execute(["git", "checkout", "--detach", REFERENCE_REV], reference)
    state = git_state(reference)
    if state["revision"] != REFERENCE_REV or state["dirty"]:
        raise ValueError("reference must be a clean checkout of " + REFERENCE_REV)
    if command(["git", "rev-parse", "1.5.0^{commit}"], reference) != REFERENCE_REV:
        raise ValueError("reference release 1.5.0 does not match the required pin")
    execute([npm, "ci", "--no-audit", "--no-fund"], reference)
    execute([npm, "run", "build"], reference)
    if git_state(reference)["dirty"]:
        raise ValueError("reference preparation modified source or dependency lock")
    rust_version = command([rustc, "-vV"])
    target = next(line.removeprefix("host: ") for line in rust_version.splitlines() if line.startswith("host: "))
    # Explicit target/output location prevents an inherited CARGO_TARGET_DIR or
    # CARGO_BUILD_TARGET from making us attribute an older binary to this build.
    execute([cargo, "build", "--locked", "--release", "--bin", "komodo-mcp",
             "--target", target, "--target-dir", str(ROOT / "target")], ROOT, {"RUSTC": rustc})
    binary = ROOT / "target" / target / "release/komodo-mcp"
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise ValueError(f"missing release executable: {binary}")
    manifest = {
        "format": "komodo-benchmark-artifacts/v1", "prepared_at": now(),
        "commands": commands,
        "rust": {"argv": [str(binary)], "sha256": sha(binary), "bytes": binary.stat().st_size,
                 "repository": git_state(ROOT), "source_sha256": source_sha(),
                 "cargo_lock_sha256": sha(ROOT / "Cargo.lock"),
                 "rustc": rust_version, "cargo": command([cargo, "--version"]),
                 "target": target,
                 "profile": "release (Cargo defaults; no profile overrides in Cargo.toml)",
                 "build_environment": {k: os.environ.get(k) for k in
                                       ("RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_TARGET",
                                        "CARGO_PROFILE_RELEASE_OPT_LEVEL", "CARGO_PROFILE_RELEASE_STRIP",
                                        "CARGO_PROFILE_RELEASE_LTO", "RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER")}},
        "reference": {"repository": REFERENCE_URL, "release": "1.5.0", "revision": REFERENCE_REV,
                      "directory": str(reference), "node": node, "node_version": version,
                      "node_sha256": sha(node), "npm_version": command([npm, "--version"]),
                      "package_lock_sha256": sha(reference / "package-lock.json"),
                      "build_sha256": tree_sha(reference / "build"),
                      "dependencies_sha256": tree_sha(reference / "node_modules"),
                      "argv": [node, str(reference / "build/index.js")]},
    }
    write_json(args.artifacts, manifest)
    print(f"Prepared {args.artifacts}. Preparation is outside all timers.")


def preflight(path):
    if not path.is_file():
        raise ValueError(f"missing artifact manifest {path}; run the documented prepare command")
    artifacts = json.loads(path.read_text())
    if artifacts.get("format") != "komodo-benchmark-artifacts/v1":
        raise ValueError("unsupported artifact manifest; run prepare again")
    rust, reference = artifacts["rust"], artifacts["reference"]
    binary = Path(rust["argv"][0])
    directory = Path(reference["directory"])
    if not binary.is_file() or not os.access(binary, os.X_OK) or sha(binary) != rust["sha256"]:
        raise ValueError("release executable missing/changed; run prepare again")
    if (not (directory / "build/index.js").is_file()
            or not (directory / "node_modules").is_dir()):
        raise ValueError("reference build/dependencies missing; run prepare with Node >=22")
    if reference["revision"] != REFERENCE_REV or git_state(directory) != {
            "revision": REFERENCE_REV, "dirty": False, "status": []}:
        raise ValueError("reference revision/source changed; restore the pin and run prepare")
    if (sha(directory / "package-lock.json") != reference["package_lock_sha256"]
            or tree_sha(directory / "build") != reference["build_sha256"]
            or tree_sha(directory / "node_modules") != reference["dependencies_sha256"]
            or sha(reference["node"]) != reference["node_sha256"]
            or command([reference["node"], "--version"]) != reference["node_version"]):
        raise ValueError("reference runtime/locked artifacts changed; run prepare again")
    if rust["argv"] != [str(binary)] or reference["argv"] != [reference["node"], str(directory / "build/index.js")]:
        raise ValueError("artifact launch arguments differ from the prepared direct executables")
    return artifacts


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def percentile(values, fraction):
    if not values or not 0 < fraction <= 1:
        raise ValueError("percentile requires samples and 0 < fraction <= 1")
    ordered = sorted(values)
    return ordered[math.ceil(fraction * len(ordered)) - 1]


def stats(values):
    return {"n": len(values), "min": min(values), "p50": percentile(values, .5),
            "p95": percentile(values, .95), "max": max(values)}


def host_info(note):
    info = {"os": platform.platform(), "system": platform.system(), "architecture": platform.machine(),
            "logical_cpus": os.cpu_count(), "python": sys.version,
            "virtualization_note": note, "cpu": None, "memory_bytes": None}
    if sys.platform == "darwin":
        info.update(cpu=command(["sysctl", "-n", "machdep.cpu.brand_string"]),
                    memory_bytes=int(command(["sysctl", "-n", "hw.memsize"])))
    elif sys.platform.startswith("linux"):
        cpuinfo = Path("/proc/cpuinfo").read_text()
        info["cpu"] = next((line.split(":", 1)[1].strip() for line in cpuinfo.splitlines()
                            if line.startswith(("model name", "Hardware"))), platform.processor())
        meminfo = Path("/proc/meminfo").read_text()
        info["memory_bytes"] = int(next(line.split()[1] for line in meminfo.splitlines()
                                       if line.startswith("MemTotal:"))) * 1024
        info["container_indicators"] = {p: Path(p).exists() for p in ("/.dockerenv", "/run/.containerenv")}
        info["cgroup"] = Path("/proc/self/cgroup").read_text()
    else:
        raise ValueError("RSS collection supports Linux/macOS only; run on a supported host")
    return info


async def rss(pid):
    if sys.platform.startswith("linux"):
        try:
            text = Path(f"/proc/{pid}/status").read_text()
        except FileNotFoundError:
            return None
        return next((int(line.split()[1]) * 1024 for line in text.splitlines()
                     if line.startswith("VmRSS:")), None)
    process = await asyncio.create_subprocess_exec(
        "/bin/ps", "-o", "rss=", "-p", str(pid), stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.PIPE)
    try:
        out, _ = await asyncio.wait_for(process.communicate(), 2)
    except BaseException:
        if process.returncode is None:
            process.kill()
        await process.wait()
        raise
    return int(out.strip()) * 1024 if out.strip() else None


async def memory_session(client, args, core, implementation, batch):
    samples = []

    async def poll():
        while True:
            if client.process is not None and client.process.returncode is None:
                value = await rss(client.process.pid)
                if value is not None:
                    samples.append({"since_start_ms": (time.perf_counter_ns() - start) / 1e6,
                                    "rss_bytes": value})
            await asyncio.sleep(args.rss_interval_ms / 1000)

    start = time.perf_counter_ns()
    polling = asyncio.create_task(poll())
    try:
        initialization = await client.initialize()
        core.mark(implementation, batch, "memory_idle")
        await asyncio.sleep(args.idle_ms / 1000)
        idle = await rss(client.process.pid)
        if idle is None:
            raise RuntimeError("server RSS unavailable at idle observation")
        for phase, count in (("memory_warmup", args.warmup), ("memory_workload", args.samples)):
            for i in range(count):
                core.mark(implementation, batch, phase, i)
                await client.inventory(implementation, args.page_size, core.expected)
        final = await rss(client.process.pid)
        if final is not None:
            samples.append({"since_start_ms": (time.perf_counter_ns() - start) / 1e6,
                            "rss_bytes": final})
        if not samples:
            raise RuntimeError("no RSS samples collected")
        if polling.done():
            await polling  # Never hide a failed sampler behind idle/final readings.
        return {"idle_rss_bytes": idle, "peak_sampled_rss_bytes": max(
            [idle, *(x["rss_bytes"] for x in samples)]), "samples": samples,
                "protocol": initialization["protocol"]}
    finally:
        polling.cancel()
        await asyncio.gather(polling, return_exceptions=True)


def direct_reads(core, page_size, count, batch, warmup, version=False):
    connection = http.client.HTTPConnection("127.0.0.1", core.http.server_port, timeout=20)
    readings = []
    try:
        for i in range(warmup + count):
            phase = "direct_version" if version else ("direct_warmup" if i < warmup else "direct")
            sample = i - warmup
            core.mark("direct_core", batch, phase, sample)
            items, body_bytes, calls = [], 0, 0
            start = time.perf_counter_ns()
            # Fresh TCP on the first read of each baseline session; reused thereafter.
            for page in range(max(1, len(core.expected) // page_size + 1)):
                if core.stopped.is_set():
                    raise InterruptedError("direct baseline interrupted")
                operation = "GetVersion" if version else "ListStacks"
                params = {} if version else {"page": page, "limit": page_size,
                                            "sort_by": "Name", "sort_desc": False}
                connection.request("POST", "/read", encoded({"type": operation, "params": params}),
                                   {"x-api-key": KEY, "x-api-secret": SECRET,
                                    "Content-Type": "application/json"})
                response = connection.getresponse()
                body = response.read()
                if response.status != 200:
                    raise ValueError("direct Core HTTP failure")
                value = json.loads(body)
                body_bytes += len(body)
                calls += 1
                if version:
                    if value != {"version": "2.3.2"}:
                        raise ValueError("invalid direct Core version")
                    break
                if not isinstance(value, list):
                    raise ValueError("invalid direct Core inventory")
                items.extend(project([{**x, "state": x["info"]["state"]} for x in value]))
                if len(value) < page_size:
                    break
            elapsed = (time.perf_counter_ns() - start) / 1e6
            evidence = None if version else verify(items, core.expected)
            if i >= warmup:
                readings.append({"batch": batch, "sample": sample, "elapsed_ms": elapsed,
                                 "http_calls": calls, "response_body_bytes": body_bytes,
                                 "equivalence": evidence})
        return readings
    finally:
        connection.close()


async def compare(args, report, artifacts):
    with tempfile.TemporaryDirectory(prefix="komodo benchmark ") as directory, Core(
            args.fixture_size, args.fixture_delay_ms) as core:
        home = Path(directory)
        (home / "config.toml").write_text("")
        env = child_env(home, core.address)
        report["fixture"]["wire_sha256"] = digest(core.items)
        report["fixture"]["expected"] = core.expected
        report["fixture"]["expected_sha256"] = verify(core.expected, core.expected)["sha256"]

        async def finish(client, output, implementation, batch, phase, failed, sample=None):
            try:
                core.mark(implementation, batch, phase + "_shutdown", sample)
            finally:
                try:
                    await client.close(failed)
                finally:
                    output["processes"].append({"batch": batch, "kind": phase, "sample": sample,
                                                **client.diagnostics()})

        try:
            for batch in range(args.repetitions):
                order = ["rust", "reference"] if batch % 2 == 0 else ["reference", "rust"]
                report["execution_order"].append(order)
                for implementation in order:
                    argv = artifacts[implementation]["argv"]
                    output = report["implementations"][implementation]
                    for sample in range(args.cold_samples):
                        core.mark(implementation, batch, "startup", sample)
                        client = Client(argv, env, home, args.timeout)
                        failed = True
                        try:
                            result = await client.initialize()
                            output["cold"].append({"batch": batch, "sample": sample, **result})
                            failed = False
                        finally:
                            await finish(client, output, implementation, batch, "cold", failed, sample)
                    core.mark(implementation, batch, "warm_startup")
                    client = Client(argv, env, home, args.timeout)
                    failed = True
                    try:
                        initialization = await client.initialize()
                        output["warm_sessions"].append({"batch": batch, **initialization})
                        for phase, count in (("warmup", args.warmup), ("warm", args.samples)):
                            for sample in range(count):
                                core.mark(implementation, batch, phase, sample)
                                result = await client.inventory(implementation, args.page_size, core.expected)
                                output[phase].append({"batch": batch, "sample": sample, **result})
                        failed = False
                    finally:
                        await finish(client, output, implementation, batch, "warm", failed)
                    # Dedicated RSS session prevents sampling/ps overhead contaminating latency.
                    core.mark(implementation, batch, "memory_startup")
                    client = Client(argv, env, home, args.timeout)
                    failed = True
                    try:
                        memory = await memory_session(client, args, core, implementation, batch)
                        output["memory"].append({"batch": batch, **memory})
                        failed = False
                    finally:
                        await finish(client, output, implementation, batch, "memory", failed)
                report["direct_core"].extend(await asyncio.to_thread(
                    direct_reads, core, args.page_size, args.samples, batch, args.warmup))
                report["direct_version"].extend(await asyncio.to_thread(
                    direct_reads, core, args.page_size, args.cold_samples, batch, 0, True))
        finally:
            try:
                core.mark("driver", None, "finished")
            finally:
                report["core_requests"] = list(core.events)
            # Observed HTTP accounting; never substituted with theoretical call counts.
            for implementation, output in report["implementations"].items():
                for phase in ("cold", "warmup", "warm"):
                    for sample in output[phase]:
                        events = [x for x in core.events if x["implementation"] == implementation
                                  and x["batch"] == sample["batch"] and x["sample"] == sample["sample"]
                                  and x["phase"] == ("startup" if phase == "cold" else phase)]
                        sample["core_requests"] = len(events)
                        sample["core_operations"] = {op: sum(x["operation"] == op for x in events)
                                                     for op in sorted({x["operation"] for x in events})}


def summaries(report):
    return {**{name: {
        "cold_ms": stats([x["elapsed_ms"] for x in data["cold"]]),
        "warm_ms": stats([x["elapsed_ms"] for x in data["warm"]]),
        "warm_response_bytes": stats([x["response_bytes"] for x in data["warm"]]),
        "warm_mcp_calls": stats([x["mcp_calls"] for x in data["warm"]]),
        "warm_core_requests": stats([x["core_requests"] for x in data["warm"]]),
        "idle_rss_bytes": stats([x["idle_rss_bytes"] for x in data["memory"]]),
        "peak_sampled_rss_bytes": stats([x["peak_sampled_rss_bytes"] for x in data["memory"]]),
        "batches_warm_ms": [stats([x["elapsed_ms"] for x in data["warm"] if x["batch"] == batch])
                            for batch in range(report["controls"]["repetitions"])],
    } for name, data in report["implementations"].items()},
        "direct_core_ms": stats([x["elapsed_ms"] for x in report["direct_core"]]),
        "direct_version_ms": stats([x["elapsed_ms"] for x in report["direct_version"]]),
        "fixture_service_ms": stats([x["service_ms"] for x in report["core_requests"]])}


def require(condition, message):
    if not condition:
        raise ValueError("invalid benchmark result: " + message)


def number(value, positive=False):
    return type(value) in (int, float) and math.isfinite(value) and (value > 0 if positive else value >= 0)


def validate(report):
    if report.get("format") == "komodo-incident-benchmark/v1":
        import incident
        return incident.validate(report)
    require(report.get("format") == FORMAT and report.get("status") == "complete", "incomplete/unsupported run")
    for key in ("run_id", "timestamp", "repository", "runner", "artifacts", "host", "fixture", "workload",
                "controls", "definitions", "execution_order", "core_requests", "invocation"):
        require(bool(report.get(key)), "missing " + key)
    require(report.get("errors") == [], "run contains errors")

    def fields(value, names):
        require(isinstance(value, dict) and all(k in value for k in names.split()),
                "missing metadata fields: " + names)

    def revision(value):
        fields(value, "revision dirty status")
        require(isinstance(value["revision"], str) and bool(re.fullmatch(r"[0-9a-f]{40}", value["revision"]))
                and type(value["dirty"]) is bool and isinstance(value["status"], list), "invalid repository identity")

    revision(report["repository"])
    fields(report["runner"], "version files_sha256")
    require(all(k in report["runner"]["files_sha256"] for k in ("bench.py", "core.py", "stdio.py")), "missing runner hashes")
    fields(report["host"], "os system architecture logical_cpus python virtualization_note cpu memory_bytes")
    require(number(report["host"]["memory_bytes"], True) and number(report["host"]["logical_cpus"], True), "invalid host capacity")
    fields(report["artifacts"], "format prepared_at commands rust reference")
    rust, reference = report["artifacts"]["rust"], report["artifacts"]["reference"]
    fields(rust, "argv sha256 bytes repository source_sha256 cargo_lock_sha256 rustc cargo target profile build_environment")
    revision(rust["repository"])
    fields(reference, "repository release revision directory node node_version node_sha256 npm_version package_lock_sha256 build_sha256 dependencies_sha256 argv")
    require(reference["revision"] == REFERENCE_REV and reference["release"] == "1.5.0"
            and reference["repository"] == REFERENCE_URL, "incorrect reference pin")
    require(number(rust["bytes"], True) and all(isinstance(v, str) and re.fullmatch(r"[0-9a-f]{64}", v)
            for v in (rust["sha256"], reference["package_lock_sha256"], reference["build_sha256"],
                      reference["dependencies_sha256"], reference["node_sha256"])), "missing artifact identities")
    fields(report["definitions"], "startup warm response_bytes percentiles direct_core fixture_service rss")
    fields(report["workload"], "name protocol rust_tool reference_tool arguments concurrency logging environment")
    require(report["workload"]["protocol"] == PROTOCOL and report["workload"]["concurrency"] == 1, "unsupported workload")
    controls, fixture = report["controls"], report["fixture"]
    fields(controls, "fixture_size page_size warmup samples cold_samples repetitions idle_ms rss_interval_ms timeout")
    fields(fixture, "version visible_stacks seed_sha256 permissions delay_ms delay_scope wire_sha256 expected expected_sha256")
    require(number(fixture["delay_ms"]) and 1 <= controls["page_size"] <= 100
            and type(controls["warmup"]) is int and controls["warmup"] >= 0, "invalid fixture/workload controls")
    repetitions, samples, cold = [controls[k] for k in ("repetitions", "samples", "cold_samples")]
    require(all(type(x) is int and x > 0 for x in (repetitions, samples, cold)), "invalid sample controls")
    require(report["execution_order"] == [["rust", "reference"] if b % 2 == 0 else ["reference", "rust"]
                                         for b in range(repetitions)], "invalid execution order")
    require(set(report["implementations"]) == {"rust", "reference"}, "comparison requires both implementations")
    expected = fixture["expected"]
    require(fixture["visible_stacks"] == len(expected), "invalid fixture resource count")
    oracle = verify(project(expected), expected)
    require(oracle["sha256"] == fixture["expected_sha256"], "invalid fixture oracle digest")
    for name, data in report["implementations"].items():
        require(len(data["warm_sessions"]) == repetitions and all(x["protocol"] == PROTOCOL for x in data["warm_sessions"]),
                "missing persistent session handshake")
        require(len(data["processes"]) == (cold + 2) * repetitions and
                all(x["returncode"] in (0, -signal.SIGTERM) for x in data["processes"]), "missing/failed process lifecycle")
        for phase, count in (("cold", cold), ("warmup", controls["warmup"]), ("warm", samples)):
            require(len(data[phase]) == count * repetitions, f"{name} incomplete {phase} samples")
            require({(x["batch"], x["sample"]) for x in data[phase]} ==
                    {(b, s) for b in range(repetitions) for s in range(count)}, "duplicate/missing sample IDs")
            for sample in data[phase]:
                require(number(sample["elapsed_ms"], True), "invalid latency")
                require(type(sample["response_bytes"]) is int and sample["response_bytes"] > 0, "invalid wire bytes")
                events = [x for x in report["core_requests"] if x["implementation"] == name
                          and x["batch"] == sample["batch"] and x["sample"] == sample["sample"]
                          and x["phase"] == ("startup" if phase == "cold" else phase)]
                require(sample["core_requests"] == len(events), "Core accounting mismatch")
                require(sample["core_operations"] == {op: sum(x["operation"] == op for x in events)
                                                       for op in {x["operation"] for x in events}}, "operation count mismatch")
                if phase == "cold":
                    require(sample["protocol"] == PROTOCOL, "unexpected protocol")
                else:
                    require(sample["equivalence"] == oracle, "failed equivalence")
                    require(type(sample["mcp_calls"]) is int and sample["mcp_calls"] > 0, "invalid MCP calls")
        require(len(data["memory"]) == repetitions and {x["batch"] for x in data["memory"]} == set(range(repetitions)),
                "missing memory batches")
        for memory in data["memory"]:
            require(number(memory["idle_rss_bytes"], True) and number(memory["peak_sampled_rss_bytes"], True),
                    "RSS unavailable")
            require(bool(memory["samples"]) and all(number(x["rss_bytes"], True) for x in memory["samples"]),
                    "invalid raw RSS")
            require(memory["peak_sampled_rss_bytes"] == max([memory["idle_rss_bytes"],
                    *(x["rss_bytes"] for x in memory["samples"])]), "invalid sampled RSS maximum")
    for event in report["core_requests"]:
        fields(event, "implementation batch phase sample operation route params status response_body_bytes service_ms activity")
        require(event["status"] == 200 and number(event["service_ms"]) and
                type(event["response_body_bytes"]) is int and event["response_body_bytes"] >= 0, "failed fixture request")
    for key, count in (("direct_core", samples), ("direct_version", cold)):
        require(len(report[key]) == count * repetitions, "missing " + key)
        require({(x["batch"], x["sample"]) for x in report[key]} ==
                {(b, s) for b in range(repetitions) for s in range(count)}, "duplicate/missing direct samples")
        for sample in report[key]:
            require(number(sample["elapsed_ms"], True) and sample["http_calls"] > 0
                    and sample["response_body_bytes"] > 0, "invalid direct sample")
            if key == "direct_core":
                require(sample["equivalence"] == oracle, "direct Core equivalence failed")
            events = [x for x in report["core_requests"] if x["implementation"] == "direct_core"
                      and x["batch"] == sample["batch"] and x["sample"] == sample["sample"]
                      and x["phase"] == ("direct" if key == "direct_core" else "direct_version")]
            require(len(events) == sample["http_calls"] and sum(x["response_body_bytes"] for x in events) ==
                    sample["response_body_bytes"], "direct Core accounting mismatch")
    require(KEY not in json.dumps(report) and SECRET not in json.dumps(report), "credential value in report")
    require(report.get("summary") == summaries(report), "missing or inaccurate summary")
    return report


def run(args):
    # All preflight and host inspection precede fixture/process timing.
    artifacts = preflight(args.artifacts)
    report = {"format": FORMAT, "run_id": str(uuid.uuid4()), "timestamp": now(), "status": "running",
              "repository": git_state(ROOT), "runner": {"version": RUNNER, "files_sha256": {
                  p.name: sha(p) for p in sorted(Path(__file__).parent.glob("*.py"))}},
              "artifacts": artifacts, "host": host_info(args.host_note),
              "invocation": [sys.executable, *sys.argv],
              "fixture": {"version": "synthetic-core-2.3.2/stacks-v1", "visible_stacks": args.fixture_size,
                          "seed_sha256": sha(ROOT / "tests/fixtures/core-2.3.2/stacks.json"),
                          "permissions": "simulated non-admin Read on every generated stack and associated server; all mutations rejected",
                          "delay_ms": args.fixture_delay_ms, "delay_scope": "every version/read HTTP response"},
              "controls": {k: getattr(args, k) for k in ("fixture_size", "page_size", "warmup", "samples",
                           "cold_samples", "repetitions", "idle_ms", "rss_interval_ms", "timeout")},
              "workload": {"name": "complete-visible-stack-inventory/v1", "protocol": PROTOCOL,
                           "rust_tool": "list_stacks", "reference_tool": "komodo_stack_list",
                           "arguments": {"page_size": args.page_size}, "concurrency": 1,
                           "logging": {"RUST_LOG": "warn", "LOG_LEVEL": "warn"},
                           "environment": "allowlisted synthetic variables; empty temporary HOME/cwd/config; no inherited Komodo settings"},
              "definitions": {
                  "startup": "ms, perf_counter_ns immediately before fresh process spawn through parsed/validated initialize response; initialized notification follows outside timer; filesystem caches uncontrolled",
                  "warm": "ms, complete logical inventory including every MCP page, response JSON parsing and common projection; oracle comparison excluded; persistent process/normal pooling; no RSS sampler",
                  "response_bytes": "sum of actual matched stdout JSON-RPC response frames including UTF-8 and LF, all pages; excludes notifications/stderr; preserves both structuredContent and text",
                  "percentiles": "nearest rank: sorted[ceil(p*n)-1]; pooled samples and separate batch summaries; no filtering/retries",
                  "direct_core": "ms, authenticated ListStacks page/limit Name ASC to first short page (extra empty read on exact boundary), persistent HTTPConnection per batch incl. JSON parsing/projection; body bytes exclude HTTP headers",
                  "fixture_service": "ms from handler entry through flushed response; excludes TCP accept and client receive; captures phase at entry",
                  "rss": "bytes; Linux /proc/PID/status VmRSS or macOS ps -o rss= in 1024-byte units; dedicated fresh process; sampled maximum from spawn through initialized idle, warmup and measured-count reads, before shutdown; idle after stabilization; actual timestamps retained; direct executable launches have no wrapper children; excludes driver/mock",
              },
              "execution_order": [], "implementations": {name: {
                  "cold": [], "warm_sessions": [], "warmup": [], "warm": [], "memory": [], "processes": []}
                  for name in ("rust", "reference")},
              "direct_core": [], "direct_version": [], "core_requests": [], "errors": []}
    try:
        asyncio.run(compare(args, report, artifacts))
        report["status"] = "complete"
        report["summary"] = summaries(report)
        validate(report)
    except BaseException as error:
        report["status"] = "failed"
        report["errors"].append({"type": type(error).__name__, "message": safe_error(error)})
        raise
    finally:
        report["finished_at"] = now()
        write_json(args.output, report)
    print_summary(report)


def print_summary(report):
    if report.get("format") == "komodo-incident-benchmark/v1":
        import incident
        return incident.print_summary(report)
    validate(report)
    print(f"Run {report['run_id']} — {report['fixture']['visible_stacks']} stacks; provisional synthetic results")
    print("implementation | startup p50/p95 ms | inventory p50/p95 ms | response bytes p50 | Core calls p50 | idle/peak MiB")
    for name in ("rust", "reference"):
        data = report["summary"][name]
        print(f"{name} | {data['cold_ms']['p50']:.3f}/{data['cold_ms']['p95']:.3f} | "
              f"{data['warm_ms']['p50']:.3f}/{data['warm_ms']['p95']:.3f} | "
              f"{data['warm_response_bytes']['p50']} | {data['warm_core_requests']['p50']} | "
              f"{data['idle_rss_bytes']['p50']/2**20:.2f}/{data['peak_sampled_rss_bytes']['max']/2**20:.2f}")
    print(f"Direct Core inventory p50/p95: {report['summary']['direct_core_ms']['p50']:.3f}/"
          f"{report['summary']['direct_core_ms']['p95']:.3f} ms")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subs = parser.add_subparsers(dest="action", required=True)
    prep = subs.add_parser("prepare", help="download/install/build before measurements")
    prep.add_argument("--reference-dir", type=Path, required=True)
    prep.add_argument("--node", default="node")
    prep.add_argument("--npm", default="npm")
    prep.add_argument("--cargo", default="cargo")
    prep.add_argument("--rustc", default="rustc")
    prep.add_argument("--artifacts", type=Path, default=ROOT / "target/benchmark/artifacts.json")
    runner = subs.add_parser("run", help="measure already-prepared artifacts; no compilation/install")
    runner.add_argument("--artifacts", type=Path, default=ROOT / "target/benchmark/artifacts.json")
    runner.add_argument("--output", type=Path, default=ROOT / "target/benchmark/result.json")
    for field, default in (("fixture-size", 125), ("page-size", 25), ("warmup", 5), ("samples", 30),
                           ("cold-samples", 10), ("repetitions", 3), ("idle-ms", 200), ("rss-interval-ms", 20)):
        runner.add_argument("--" + field, type=int, default=default)
    runner.add_argument("--fixture-delay-ms", type=float, default=0)
    runner.add_argument("--timeout", type=float, default=20)
    runner.add_argument("--host-note", required=True, help="describe bare metal/VM/container and other host load")
    incident_runner = subs.add_parser("incident", help="compare bounded broken-stack triage with concurrent callers")
    incident_runner.add_argument("--artifacts", type=Path, default=ROOT / "target/benchmark/artifacts.json")
    incident_runner.add_argument("--output", type=Path, default=ROOT / "target/benchmark/incident.json")
    for field, default in (("fixture-size", 7), ("page-size", 25), ("concurrency", 1), ("warmup", 2),
                           ("samples", 10), ("repetitions", 2), ("idle-ms", 200), ("rss-interval-ms", 20)):
        incident_runner.add_argument("--" + field, type=int, default=default)
    incident_runner.add_argument("--fixture-delay-ms", type=float, default=0)
    incident_runner.add_argument("--timeout", type=float, default=30)
    incident_runner.add_argument("--host-note", required=True)
    for name in ("validate", "summarize"):
        sub = subs.add_parser(name)
        sub.add_argument("result", type=Path)
    args = parser.parse_args()
    if args.action == "prepare":
        prepare(args)
    elif args.action == "incident":
        import incident
        if (not 2 <= args.fixture_size <= 1000 or not 1 <= args.page_size <= 100
                or not 1 <= args.concurrency <= 16 or args.warmup < 0
                or not number(args.fixture_delay_ms)
                or any(not number(getattr(args, key), True) for key in
                       ("samples", "repetitions", "idle_ms", "rss_interval_ms", "timeout"))):
            parser.error("require fixture size 2..1000, page size 1..100, concurrency 1..16, positive samples/repetitions/timeouts, nonnegative warmup/delay")
        incident.run(args)
    elif args.action == "run":
        for key in ("samples", "cold_samples", "repetitions", "idle_ms", "rss_interval_ms", "timeout"):
            if not number(getattr(args, key), True):
                parser.error(key + " must be positive")
        if (not 1 <= args.page_size <= 100 or not 0 <= args.fixture_size <= 10000
                or args.warmup < 0 or not number(args.fixture_delay_ms)):
            parser.error("require page size 1..100, fixture size 0..10000, nonnegative warmup/delay")
        run(args)
    else:
        result = validate(read_json(args.result))
        if args.action == "summarize":
            print_summary(result)
        else:
            print("Valid complete comparison:", args.result)


def safe_error(error):
    return str(error).replace(KEY, "[redacted]").replace(SECRET, "[redacted]")


if __name__ == "__main__":
    # asyncio.run handles SIGINT cancellation; map SIGTERM onto the same cleanup path.
    signal.signal(signal.SIGTERM, lambda *_args: signal.raise_signal(signal.SIGINT))
    try:
        main()
    except (Exception, KeyboardInterrupt) as error:
        print(f"benchmark failed: {type(error).__name__}: {safe_error(error)}", file=sys.stderr)
        sys.exit(1)
