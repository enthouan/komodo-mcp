"""Measure a provisioned disposable Core using the same packaged candidate and inventory oracle."""
import asyncio
import http.client
import ipaddress
import json
from pathlib import Path
import re
import tempfile
import time
from urllib.parse import urlsplit

import bench
import packaged
from core import encoded
from stdio import Client, child_env, project, verify

FORMAT = "komodo-live-inventory-benchmark/v1"


def direct(connection, endpoint, headers, expected, page_size):
    start = time.perf_counter_ns()
    items, body_bytes = [], 0
    for page in range(len(expected) // page_size + 2):
        params = {"page": page, "limit": page_size, "sort_by": "Name", "sort_desc": False}
        connection.request("POST", endpoint, encoded({"type": "ListStacks", "params": params}), headers)
        response = connection.getresponse()
        body = response.read()
        if response.status != 200:
            raise ValueError(f"disposable Core inventory returned status {response.status}")
        rows = json.loads(body)
        if not isinstance(rows, list) or len(rows) > page_size:
            raise ValueError("invalid live Core inventory page")
        items.extend(project([{"id": row["id"], "name": row["name"], "state": row["info"]["state"].lower()}
                              for row in rows]))
        body_bytes += len(body)
        if len(rows) < page_size:
            break
    else:
        raise ValueError("live Core inventory exceeded bounded pagination")
    elapsed = (time.perf_counter_ns() - start) / 1e6
    return {"elapsed_ms": elapsed, "core_requests": page + 1, "response_bytes": body_bytes,
            "equivalence": verify(items, expected)}


async def measure(artifacts, url, credentials, expected, controls, report):
    parsed = urlsplit(url)
    if (parsed.scheme != "http" or not parsed.hostname or not ipaddress.ip_address(parsed.hostname).is_loopback
            or parsed.username or parsed.password or parsed.query or parsed.fragment):
        raise ValueError("expected an owned disposable Core endpoint on loopback")
    with tempfile.TemporaryDirectory(prefix="komodo-live-benchmark-") as directory:
        home = Path(directory)
        (home / "config.toml").write_text("")
        env = child_env(home, url)
        env.update(KOMODO_API_KEY=credentials["key"], KOMODO_API_SECRET=credentials["secret"])
        headers = {"Content-Type": "application/json", "X-Api-Key": credentials["key"],
                   "X-Api-Secret": credentials["secret"]}
        for batch in range(controls["repetitions"]):
            for name in packaged.order(artifacts, batch) + ["direct_core"]:
                connection = client = None
                failed = True
                try:
                    if name == "direct_core":
                        connection = http.client.HTTPConnection(parsed.hostname, parsed.port, timeout=controls["timeout"])
                    else:
                        client = Client(artifacts[name]["argv"], env, home, controls["timeout"])
                        identity = await client.initialize()
                        if name == "rust" and identity["server_info"]["version"] != artifacts["rust"]["candidate"]["distribution"]["version"]:
                            raise ValueError("packaged MCP identity differs from shared receipt")
                    for sample in range(-controls["warmup"], controls["samples"]):
                        if client is not None:
                            data = await client.inventory(name, controls["page_size"], expected)
                        else:
                            data = await asyncio.to_thread(direct, connection, parsed.path.rstrip("/") + "/read", headers,
                                                           expected, controls["page_size"])
                        if sample >= 0:
                            report["implementations"][name].append({"batch": batch, "sample": sample, **data})
                    failed = False
                except Exception as error:
                    # Discard raw MCP/HTTP error payloads before the provisioning runner sees them.
                    raise ValueError(f"live inventory failed for {name} (batch {batch}, {type(error).__name__})") from None
                finally:
                    if client:
                        await client.close(failed)
                    if connection:
                        connection.close()


def validate(report):
    bench.require(report["format"] == FORMAT and report["status"] == "complete", "incomplete live comparison")
    packaged.validate_artifacts(report["artifacts"], report["host"])
    controls = report["controls"]
    for key in ("page_size", "samples", "repetitions", "timeout"):
        bench.require(bench.number(controls[key], True), "invalid live controls")
    bench.require(controls["concurrency"] == 1 and controls["warmup"] >= 0, "invalid live warmup/concurrency")
    for key in ("core_version", "periphery_version"):
        bench.require(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", report[key]), "missing observed live version")
    expected = report["oracle"]
    bench.require(expected["verified"] is True and re.fullmatch("[0-9a-f]{64}", expected["sha256"])
                  and expected["count"] == controls["fixture_size"], "invalid live inventory oracle")
    bench.require(set(report["implementations"]) == {"rust", "reference", "rust_reference", "direct_core"},
                  "incomplete live participants")
    indices = {(b, s) for b in range(controls["repetitions"]) for s in range(controls["samples"])}
    for name, rows in report["implementations"].items():
        bench.require(len(rows) == len(indices) and {(r["batch"], r["sample"]) for r in rows} == indices,
                      "incomplete live samples")
        for row in rows:
            calls = row.get("core_requests" if name == "direct_core" else "mcp_calls")
            bench.require(row["equivalence"] == expected and bench.number(row["elapsed_ms"], True)
                          and type(calls) is int and calls > 0 and type(row["response_bytes"]) is int
                          and row["response_bytes"] > 0, "invalid live sample")
        bench.require(report["summary"][name] == bench.stats([r["elapsed_ms"] for r in rows]), "live summary mismatch")
    return report


def run(artifacts_path, core_url, credentials, expected, output_path, *, core_version, periphery_version,
        page_size=3, warmup=2, samples=10, repetitions=2, timeout=30):
    if any(not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", v) for v in (core_version, periphery_version)):
        raise ValueError("observed Core and Periphery semvers are required")
    artifacts = bench.preflight(Path(artifacts_path))
    packaged.validate_artifacts(artifacts)
    expected = project(expected)
    oracle = verify(expected, expected)
    controls = {"page_size": page_size, "warmup": warmup, "samples": samples, "repetitions": repetitions,
                "timeout": timeout, "concurrency": 1, "fixture_size": len(expected)}
    if not 1 <= page_size <= 100 or warmup < 0 or min(samples, repetitions, timeout) <= 0:
        raise ValueError("invalid live inventory controls")
    report = {"format": FORMAT, "status": "running", "timestamp": bench.now(), "artifacts": artifacts,
              "host": bench.host_info("Disposable runner with separate benchmark-owned Core"),
              "core_version": core_version, "periphery_version": periphery_version, "oracle": oracle,
              "controls": controls, "implementations": {k: [] for k in packaged.participants(artifacts) + ["direct_core"]}}
    try:
        asyncio.run(measure(artifacts, core_url, credentials, expected, controls, report))
        report.update(status="complete", summary={k: bench.stats([r["elapsed_ms"] for r in v])
                                                  for k, v in report["implementations"].items()})
        validate(report)
    except BaseException:
        report["status"] = "failed"
        raise
    finally:
        report["finished_at"] = bench.now()
        bench.write_json(output_path, report)
    return report
