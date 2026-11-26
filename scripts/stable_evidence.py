"""Generate versioned build inventories and verify detached stable release provenance."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import tarfile
import tomllib

from distribution import MANIFEST, REPOSITORY, VERSION, require, sha256, unique_object
from distribution_inputs import CA_INPUTS, RUNTIME_INPUTS

ROOT = Path(__file__).resolve().parents[1]
REPO_URL = "https://github.com/" + REPOSITORY
IMAGE = "ghcr.io/" + REPOSITORY
WORKFLOWS = {"native": "native-distribution", "oci": "container-distribution"}
PREDICATE = "https://slsa.dev/provenance/v1"
MAIN = "refs/heads/main"


def required(version):
    return int(version.split(".", 1)[0]) >= 1


def names(scope):
    return f"{scope}.sbom.cdx.json", f"{scope}.provenance.sigstore.json"


def read_json(path):
    return json.loads(path.read_bytes(), object_pairs_hook=unique_object)


def subjects(scope, record):
    if scope == "native":
        return {entry["name"]: entry["sha256"] for entry in record["files"]}
    digests = {IMAGE: record["index_digest"]}
    for platform in ("linux/amd64", "linux/arm64"):
        digests[f"{IMAGE}#{platform}"] = record["platforms"][platform]["manifest_digest"]
    require(all(re.fullmatch(r"sha256:[0-9a-f]{64}", d) for d in digests.values()), "invalid OCI subject digest")
    return {key: value[7:] for key, value in digests.items()}


def component(name, identifier, digest, kind="file", **fields):
    return {"type": kind, "bom-ref": identifier, "name": name,
            "hashes": [{"alg": "SHA-256", "content": digest}], **fields}


def native_inputs(archive):
    member = archive.name.removesuffix(".tar.gz") + "/BUILD-INPUTS.json"
    with tarfile.open(archive, "r:gz") as tar:
        info = tar.getmember(member)
        require(info.isfile() and 0 < info.size < 1024 * 1024, "invalid compiled package inventory")
        packages = json.load(tar.extractfile(info))
    require(isinstance(packages, list) and packages and all(isinstance(p, str) for p in packages)
            and packages == sorted(set(packages)), "invalid compiled package list")
    return packages


def sbom(scope, record, native_directory):
    version, revision = record["version"], record["revision"]
    crates = {}
    for p in tomllib.loads((ROOT / "Cargo.lock").read_text())["package"]:
        if "source" in p:
            require(p["source"] == "registry+https://github.com/rust-lang/crates.io-index",
                    "review new package source before generating the SBOM")
            purl = f"pkg:cargo/{p['name']}@{p['version']}"
            crates[f"{p['name']}-{p['version']}"] = component(p["name"], purl, p["checksum"], "library",
                                                              version=p["version"], purl=purl)
        else:
            require(p["name"] == "komodo-mcp" and p["version"] == version, "unexpected source package")
    inputs = {p.name: component(p.name, p.url, p.sha256) for p in (*RUNTIME_INPUTS, *CA_INPUTS)}
    native = read_json(native_directory / MANIFEST)
    components, dependencies = {}, []
    targets = {entry["name"]: entry for entry in native["files"]}
    for name, digest in subjects(scope, record).items():
        entry = targets.get(name, {})
        ref = "subject:" + name
        components[ref] = component(name, ref, digest, "container" if scope == "oci" else "file", version=version)
        deps = []
        if scope == "oci" and name == IMAGE:
            deps = ["subject:" + IMAGE + "#" + p for p in sorted(record["platforms"])]
        elif entry.get("kind") == "source":
            deps = [v["bom-ref"] for v in [*crates.values(), *inputs.values()]]
        elif entry.get("kind") == "native" or scope == "oci":
            if scope == "oci":
                target = record["platforms"][name.split("#")[1]]["target"]
                entry = next(f for f in targets.values() if f.get("target") == target)
            deps = [crates[k]["bom-ref"] for k in native_inputs(native_directory / entry["name"])]
            deps.append(RUNTIME_INPUTS[0].url)
            if entry["target"].endswith("-musl"):
                deps.append(RUNTIME_INPUTS[1].url)
            if scope == "oci":
                deps.append(CA_INPUTS[0].url)
        dependencies.append({"ref": ref, "dependsOn": sorted(deps)})
    used = {ref for relation in dependencies for ref in relation["dependsOn"]}
    for item in [*crates.values(), *inputs.values()]:
        if item["bom-ref"] in used:
            components[item["bom-ref"]] = item
    return {"bomFormat": "CycloneDX", "specVersion": "1.6", "version": 1,
            "metadata": {"component": {"type": "application", "name": "komodo-mcp", "version": version,
                                       "externalReferences": [{"type": "vcs", "url": f"{REPO_URL}/tree/{revision}"}]}},
            "components": list(components.values()), "dependencies": dependencies}


def statement(scope, version, revision, digests, run_id, attempt):
    require(VERSION.fullmatch(version) and re.fullmatch("[a-f0-9]{40}", revision), "invalid build identity")
    require(all(type(n) is int and n > 0 for n in (run_id, attempt)), "invalid run identity")
    path = f".github/workflows/{WORKFLOWS[scope]}.yml"
    workflow = f"{REPO_URL}/{path}@{MAIN}"
    materials = [{"uri": f"git+{REPO_URL}@{MAIN}", "digest": {"gitCommit": revision}}]
    return {
        "_type": "https://in-toto.io/Statement/v1", "predicateType": PREDICATE,
        "subject": [{"name": name, "digest": {"sha256": d}} for name, d in sorted(digests.items())],
        "predicate": {"buildDefinition": {"buildType": f"{REPO_URL}/blob/{revision}/{path}",
                                           "externalParameters": {"version": version},
                                           "resolvedDependencies": materials},
                      "runDetails": {"builder": {"id": workflow},
                                     "metadata": {"invocationId": f"{REPO_URL}/actions/runs/{run_id}/attempts/{attempt}"}}},
    }


def prepare(scope, native, container, out):
    record = read_json(native / MANIFEST if scope == "native" else container / "oci-receipt.json")
    out.mkdir()
    sbom_name, _ = names(scope)
    path = out / sbom_name
    path.write_text(json.dumps(sbom(scope, record, native), indent=2) + "\n")
    digests = subjects(scope, record) | {sbom_name: sha256(path)}
    value = statement(scope, record["version"], record["revision"], digests, record["run_id"], record["run_attempt"])
    (out / "statement.json").write_text(json.dumps(value, indent=2) + "\n")


def verify(scope, directory, version, revision, digests, *, run=None):
    sbom_name, provenance_name = names(scope)
    sbom_path = directory / sbom_name
    identity = f"{REPO_URL}/.github/workflows/{WORKFLOWS[scope]}.yml@{MAIN}"
    command = ["gh", "attestation", "verify", str(sbom_path), "--bundle", str(directory / provenance_name),
               "--repo", REPOSITORY, "--cert-identity", identity, "--source-ref", MAIN, "--source-digest", revision,
               "--signer-digest", revision, "--deny-self-hosted-runners", "--predicate-type", PREDICATE,
               "--format", "json"]
    verified = json.loads(subprocess.check_output(command, timeout=120))
    require(len(verified) == 1, "expected one independently verified attestation")
    value = verified[0]["verificationResult"]["statement"]
    invocation = value["predicate"]["runDetails"]["metadata"]["invocationId"]
    match = re.fullmatch(re.escape(REPO_URL) + r"/actions/runs/([1-9][0-9]*)/attempts/([1-9][0-9]*)", invocation)
    require(match, "invalid build invocation")
    run_id, attempt = map(int, match.groups())
    require(run is None or (run_id, attempt) == (run["run_id"], run["run_attempt"]),
            "provenance belongs to another workflow run or attempt")
    expected = statement(scope, version, revision, digests | {sbom_name: sha256(sbom_path)}, run_id, attempt)
    require(value == expected, "signed subjects, source revision or build identity differ from the selected release")
    bom = read_json(sbom_path)
    require((bom["bomFormat"], bom["specVersion"]) == ("CycloneDX", "1.6")
            and bom["metadata"]["component"]["version"] == version, "invalid SBOM identity")
    observed = [item for item in bom["components"] if item["bom-ref"].startswith("subject:")]
    require(len(observed) == len(digests) and all(item["bom-ref"] == "subject:" + item["name"]
            and item["hashes"] == [{"alg": "SHA-256", "content": digests.get(item["name"])}] for item in observed)
            and {item["name"] for item in observed} == digests.keys(), "SBOM subjects differ from selected artifacts")
    return {"workflow": identity, "revision": revision, "invocation": invocation, "subjects": digests}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    generate = sub.add_parser("prepare")
    generate.add_argument("--scope", choices=WORKFLOWS, required=True)
    generate.add_argument("--native", type=Path, required=True)
    generate.add_argument("--container", type=Path)
    generate.add_argument("--output", type=Path, required=True)
    inspect = sub.add_parser("verify-oci")
    inspect.add_argument("--directory", type=Path, required=True)
    for key in ("revision", "version", "index-digest", "amd64", "arm64"):
        inspect.add_argument("--" + key, required=True)
    args = parser.parse_args()
    if args.action == "prepare":
        prepare(args.scope, args.native, args.container, args.output)
    else:
        record = {"index_digest": args.index_digest, "platforms": {
            "linux/amd64": {"manifest_digest": args.amd64}, "linux/arm64": {"manifest_digest": args.arm64}}}
        print(json.dumps(verify("oci", args.directory, args.version, args.revision, subjects("oci", record)), indent=2))


if __name__ == "__main__":
    main()
