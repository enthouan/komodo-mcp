"""Bind a benchmark to the same native and OCI candidate as disposable regression."""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib

from core import ROOT

sys.path.insert(0, str(ROOT / "scripts"))
from release_artifacts import Packages

RUST_REPOSITORY = "https://github.com/Kalaghni/komodo-mcp.git"
RUST_REVISION = "2efbe2232d98ac0c7eec90135fd683b5b39c1cca"
RUST_LOCK = "1e0fd7673968cddd83cbb2d56bf5e0600c501791387c2fc261cd539569b3410c"


def check(condition, message):
    if not condition:
        raise ValueError(message)


def sha(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def commit_file(revision, name):
    return subprocess.check_output(["git", "show", f"{revision}:{name}"], cwd=ROOT)


def packaged_entry(record, target):
    manifest = record["distribution"]
    revision, version = manifest["revision"], manifest["version"]
    check(re.fullmatch("[a-f0-9]{40}", revision), "invalid candidate revision")
    check(re.fullmatch("[a-f0-9]{40}", record["contract"]["baseline_commit"]), "missing contract baseline")
    Packages.from_receipt(record, revision, version)
    entries = [row for row in manifest["files"] if row.get("target") == target and row["kind"] == "native"]
    check(len(entries) == 1, "packaged target missing from candidate")
    return entries[0]


def candidate(args):
    binary = args.candidate_binary.resolve()
    record_path = args.candidate_record.resolve()
    record = json.loads(record_path.read_bytes())
    revision, version = (record["distribution"][k] for k in ("revision", "version"))
    entry = packaged_entry(record, args.candidate_target)
    check(os.access(binary, os.X_OK) and sha(binary) == entry["executable_sha256"],
          "extracted executable differs from selected packaged target")
    source = next(row for row in record["distribution"]["files"] if row["kind"] == "source")
    channel = tomllib.loads(commit_file(revision, "rust-toolchain.toml").decode())["toolchain"]["channel"]
    package = tomllib.loads(commit_file(revision, "Cargo.toml").decode())["package"]
    check(package["name"] == "komodo-mcp" and package["version"] == version, "Cargo package identity mismatch")
    return {"argv": [str(binary)], "sha256": entry["executable_sha256"], "bytes": binary.stat().st_size,
            "repository": {"revision": revision, "dirty": False, "status": []},
            "source_sha256": source["sha256"], "source_boundary": "matching Corresponding Source archive",
            "cargo_lock_sha256": hashlib.sha256(commit_file(revision, "Cargo.lock")).hexdigest(),
            "rustc": f"distribution toolchain {channel}", "cargo": f"distribution toolchain {channel}",
            "distribution_toolchain": channel,
            "target": args.candidate_target, "profile": "frozen release, distribution feature, stripped symbols",
            "build_environment": {"source": "scripts/distribution_build.py at packaged revision"},
            "candidate": record, "candidate_record_path": str(record_path),
            "candidate_record_sha256": sha(record_path)}


def build_reference(directory, target, cargo, rustc, execute, command, git_state):
    directory = directory.resolve()
    if not directory.exists():
        execute(["git", "clone", "--no-checkout", RUST_REPOSITORY, str(directory)], ROOT)
        execute(["git", "checkout", "--detach", RUST_REVISION], directory)
    state = git_state(directory)
    check(state["revision"] == RUST_REVISION and not state["dirty"]
          and sha(directory / "Cargo.lock") == RUST_LOCK, "Rust reference revision or lock mismatch")
    builds = ROOT / "target" / "benchmark" / "rust-reference-build"
    # Place the build outside the candidate and reference source trees.
    execute([cargo, "build", "--locked", "--release", "--bin", "komodo-mcp", "--target", target,
             "--target-dir", str(builds)], directory, {"RUSTC": rustc})
    check(git_state(directory) == state, "Rust reference build changed its pinned source")
    binary = builds / target / "release" / "komodo-mcp"
    return {"repository": RUST_REPOSITORY, "revision": RUST_REVISION, "version": "0.1.0",
            "cargo_lock_sha256": RUST_LOCK, "directory": str(directory), "target": target,
            "profile": "release (thin LTO, codegen-units=1, stripped)", "rustc": command([rustc, "-vV"]),
            "cargo": command([cargo, "--version"]), "argv": [str(binary)], "sha256": sha(binary),
            "bytes": binary.stat().st_size,
            "build_environment": {k: os.environ[k] for k in os.environ if k.startswith("CARGO_PROFILE_RELEASE_")
                                  or k in {"RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTC_WRAPPER",
                                           "RUSTC_WORKSPACE_WRAPPER"}}}


def validate_artifacts(artifacts):
    candidate = artifacts["rust"]
    receipt = candidate["candidate"]
    entry = packaged_entry(receipt, candidate["target"])
    check(candidate["sha256"] == entry["executable_sha256"]
          and candidate["repository"]["revision"] == receipt["distribution"]["revision"],
          "candidate provenance differs from receipt")
    check(type(candidate["bytes"]) is int and candidate["bytes"] > 0, "invalid candidate executable size")
    ts = artifacts["reference"]
    from bench import REFERENCE_REV, REFERENCE_URL
    check(ts["revision"] == REFERENCE_REV and ts["repository"] == REFERENCE_URL and ts["release"] == "1.5.0",
          "incorrect TypeScript reference identity")
    ref = artifacts["rust_reference"]
    check(ref["repository"] == RUST_REPOSITORY and ref["revision"] == RUST_REVISION
          and ref["cargo_lock_sha256"] == RUST_LOCK and re.fullmatch(r"[a-f0-9]{64}", ref["sha256"])
          and ref["bytes"] > 0, "incorrect independent Rust reference identity")


def check_versions(report):
    expected = report["artifacts"]["rust"]["candidate"]["distribution"]["version"]
    data = report["implementations"]["rust"]
    for session in data["warm_sessions"] + data.get("cold", []):
        check(session["server_info"]["version"] == expected, "initialized candidate version differs from receipt")


def preflight(artifacts, git_state):
    validate_artifacts(artifacts)
    candidate = artifacts["rust"]
    path = Path(candidate["candidate_record_path"])
    check(sha(path) == candidate["candidate_record_sha256"]
          and json.loads(path.read_bytes()) == candidate["candidate"], "shared candidate record changed")
    ref = artifacts["rust_reference"]
    directory = Path(ref["directory"])
    state = git_state(directory)
    binary = Path(ref["argv"][0])
    check(state["revision"] == RUST_REVISION and not state["dirty"]
          and sha(directory / "Cargo.lock") == RUST_LOCK and len(ref["argv"]) == 1
          and os.access(binary, os.X_OK) and sha(binary) == ref["sha256"],
          "Rust reference checkout or executable changed")


def participants(artifacts):
    if "rust_reference" in artifacts:
        return ["rust", "reference", "rust_reference"]
    return ["rust", "reference"]


def order(artifacts, batch):
    names = participants(artifacts)
    offset = batch % len(names)
    return names[offset:] + names[:offset]
