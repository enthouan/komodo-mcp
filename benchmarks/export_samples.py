#!/usr/bin/env python3
"""Export field-allowlisted numeric measurements for the reviewed packaged candidate."""
import argparse
import math
from pathlib import Path
import re

import bench
import live_inventory
import packaged

PHASE_FIELDS = ("batch", "sample", "elapsed_ms", "response_bytes", "mcp_calls", "core_requests",
                "core_response_bytes", "peak_observed_core_concurrency", "http_calls", "response_body_bytes")
MEMORY = ("batch", "idle_rss_bytes", "peak_sampled_rss_bytes")
CONTROLS = ("fixture_size", "page_size", "concurrency", "warmup", "samples", "cold_samples", "repetitions",
            "idle_ms", "rss_interval_ms", "timeout")
ROLES = {"rust": "candidate", "reference": "typescript_reference", "rust_reference": "rust_reference",
         "direct_core": "direct_core"}


def nums(row, fields):
    result = {}
    for key in fields:
        if key in row:
            value = row[key]
            if type(value) not in (int, float) or not math.isfinite(value) or value < 0:
                raise ValueError(f"invalid numeric export field {key}")
            result[key] = value
    return result


def token(value, pattern):
    if not isinstance(value, str) or not re.fullmatch(pattern, value):
        raise ValueError("invalid benchmark identity for public export")
    return value


def identities(artifacts):
    packaged.validate_artifacts(artifacts)
    rust, ts, comparator = (artifacts[k] for k in ("rust", "reference", "rust_reference"))
    record = rust["candidate"]
    native = packaged.packaged_entry(record, rust["target"])
    def hashes(source, fields):
        return {k: token(source[k], r"[0-9a-f]{64}") for k in fields}

    rustc = re.search(r"^rustc (\d+\.\d+\.\d+)\b", comparator["rustc"])
    cargo = re.search(r"^cargo (\d+\.\d+\.\d+)\b", comparator["cargo"])
    if not rustc or not cargo:
        raise ValueError("missing Rust comparator toolchain")
    return {
        "candidate": {"commit": token(record["distribution"]["revision"], "[0-9a-f]{40}"),
                      "baseline_commit": token(record["contract"]["baseline_commit"], "[0-9a-f]{40}"),
                      "version": token(record["distribution"]["version"], r"[0-9]+\.[0-9]+\.[0-9]+"),
                      "target": token(rust["target"], r"(?:aarch64|x86_64)-(?:unknown-linux-musl|apple-darwin)"),
                      **hashes(rust, ("sha256", "cargo_lock_sha256", "source_sha256", "candidate_record_sha256")),
                      "archive_bytes": native["bytes"], "archive_sha256": native["sha256"],
                      "executable_bytes": nums(rust, ("bytes",))["bytes"],
                      "toolchain": token(rust["distribution_toolchain"], r"\d+\.\d+\.\d+"),
                      "profile": "release, distribution feature, stripped symbols",
                      "native": nums(record["native_run"], ("run_id", "run_attempt", "artifact_id")),
                      "oci": nums(record["oci_run"], ("run_id", "run_attempt", "artifact_id"))},
        "rust_reference": {"repository": packaged.RUST_REPOSITORY, "commit": packaged.RUST_REVISION,
                           "version": "0.1.0", "rustc": rustc[1], "cargo": cargo[1],
                           "target": token(comparator["target"], r"(?:x86_64|aarch64)-(?:unknown-linux-(?:musl|gnu)|apple-darwin)"),
                           "profile": "release, thin LTO, one codegen unit, stripped",
                           **hashes(comparator, ("sha256", "cargo_lock_sha256")),
                           "executable_bytes": nums(comparator, ("bytes",))["bytes"]},
        "typescript_reference": {"repository": bench.REFERENCE_URL, "commit": bench.REFERENCE_REV,
                                 "version": "1.5.0", **hashes(ts, ("build_sha256", "dependencies_sha256",
                                                                  "package_lock_sha256", "node_sha256")),
                                 **nums(ts, ("build_bytes", "dependencies_bytes", "node_bytes")),
                                 "node_version": token(ts["node_version"], r"v\d+\.\d+\.\d+"),
                                 "npm_version": token(ts["npm_version"], r"\d+\.\d+\.\d+")}}


def rows(data):
    result = []
    for item in data:
        export = nums(item, PHASE_FIELDS)
        if "callers" in item:
            export["callers"] = [nums(caller, ("elapsed_ms", "mcp_calls", "response_bytes"))
                                 for caller in item["callers"]]
        result.append(export)
    return result


def distribution(samples):
    return {key: bench.stats([s[key] for s in samples]) for key in PHASE_FIELDS
            if key not in ("batch", "sample") and samples and key in samples[0]}


def export(raw):
    report = bench.validate(raw)
    live = report["format"] == live_inventory.FORMAT
    incident = report["format"] == "komodo-incident-benchmark/v1"
    result = {"workload": "incident" if incident else "inventory", "environment": "live" if live else "fixture",
              "controls": nums(report["controls"], CONTROLS), "protocol": "2025-03-26",
              "client": "komodo-inventory-benchmark/1", "transport": "stdio",
              "host": nums(report["host"], ("logical_cpus", "memory_bytes")),
              "observed_cpu_load": [nums({"load": x}, ("load",))["load"] for x in report["host"]["load_average"]],
              "implementations": {}}
    if not live and not incident:
        result["controls"]["concurrency"] = report["workload"]["concurrency"]
    python = re.match(r"(\d+\.\d+\.\d+)\b", report["host"]["python"])
    if python:
        result["host"]["python"] = python[1]
    cpu = re.search(r"AMD EPYC [0-9A-Z]+|Intel\(R\) Xeon\(R\)(?: Platinum| Gold| Silver| Bronze)? [A-Z0-9-]+"
                    r"|Intel\(R\) Core\(TM\) i[3579]-[0-9]+[A-Z]*|Apple M[0-9]+(?: Max| Pro| Ultra)?",
                    report["host"].get("cpu") or "")
    if cpu:
        result["host"]["cpu_model"] = cpu[0]
    result["host"]["system"] = token(report["host"]["system"], r"Linux|Darwin")
    result["host"]["architecture"] = token(report["host"]["architecture"], r"x86_64|arm64|aarch64")
    if live:
        result.update(core_version=token(report["core_version"], r"\d+\.\d+\.\d+"),
                      periphery_version=token(report["periphery_version"], r"\d+\.\d+\.\d+"))
    else:
        result.update(fixture_wire_sha256=token(report["fixture"]["wire_sha256"], "[a-f0-9]{64}"),
                      fixture_delay_ms=nums(report["fixture"], ("delay_ms",))["delay_ms"])
        result["fixture_service_ms"] = bench.stats([nums(event, ("service_ms",))["service_ms"]
                                                   for event in report["core_requests"]])
    for name, data in report["implementations"].items():
        phases = {"warm": rows(data if live else data["warm"])}
        if not live:
            phases["memory"] = [nums(session, MEMORY) for session in data["memory"]]
            if "cold" in data:
                phases["cold"] = rows(data["cold"])
        result["implementations"][ROLES[name]] = phases
    if not live and not incident:
        result["implementations"]["direct_core"] = {"warm": rows(report["direct_core"]),
                                                    "version": rows(report["direct_version"])}
    result["summary"] = {name: {phase: distribution(samples) for phase, samples in phases.items() if phase != "memory"}
                         for name, phases in result["implementations"].items()}
    for name, phases in result["implementations"].items():
        if "memory" in phases:
            result["summary"][name]["memory"] = {key: bench.stats([m[key] for m in phases["memory"]])
                                                for key in MEMORY if key != "batch"}
        if incident:
            latencies = [c["elapsed_ms"] for group in phases["warm"] for c in group["callers"]]
            result["summary"][name]["caller_latency_ms"] = bench.stats(latencies)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("reports", nargs="+", type=Path)
    args = parser.parse_args()
    runs, identity = [], None
    for path in args.reports:
        report = bench.read_json(path)
        runs.append(export(report))
        selected = identities(report["artifacts"])
        if identity is not None and selected != identity:
            raise ValueError("inputs identify different candidates or comparator builds")
        identity = selected
    bench.write_json(args.output, {"format": "komodo-public-measurements/v1", "artifacts": identity, "runs": runs})


if __name__ == "__main__":
    main()
