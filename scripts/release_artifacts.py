"""Authenticated candidate acquisition and published release-asset verification."""
from dataclasses import dataclass
import hashlib
import importlib
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import urllib.error
import urllib.parse
import urllib.request
import zipfile

from distribution import REPOSITORY as REPO, TARGETS, load_manifest, stem


class ReleaseError(RuntimeError):
    pass


class CommandError(ReleaseError):
    """Uncertain transport/process outcome. A conflict is a ReleaseError."""


def require(condition, message):
    if not condition:
        raise ReleaseError(message)


def asset_names(version):
    return {**{stem(version, target) + ".tar.gz": "native" for target in TARGETS},
            stem(version, "source") + ".tar.gz": "source", "SHA256SUMS": "checksums",
            "LICENSE": "license", "THIRD-PARTY-NOTICES": "notices"}


def download_url(version, name):
    return f"https://github.com/{REPO}/releases/download/v{version}/{name}"


def matches_download_url(url, expected):
    actual = urllib.parse.urlsplit(url)
    return (actual.scheme == "https" and actual.netloc == "github.com"
            and not actual.query and not actual.fragment
            and urllib.parse.unquote(actual.path) == urllib.parse.urlsplit(expected).path)


def hash_stream(stream):
    return hashlib.file_digest(stream, "sha256").hexdigest()


def api_file(endpoint, output, *, binary=False):
    """Stream an authenticated GitHub response to disk, including large archives."""
    args = ["gh", "api", f"repos/{REPO}/{endpoint}"]
    if binary:
        args.extend(["-H", "Accept: application/octet-stream"])
    with output.open("wb") as stream:
        try:
            subprocess.run(args, stdout=stream, stderr=subprocess.PIPE, check=True, timeout=1800)
        except subprocess.SubprocessError as error:
            raise CommandError(f"GitHub download failed for {endpoint}") from error


def candidate_run(github, run_id, revision, workflow):
    require(type(run_id) is int and run_id > 0, f"a positive Actions run ID is required for {workflow}")
    run = github.api(f"actions/runs/{run_id}")
    require(run["id"] == run_id and run["path"] == f".github/workflows/{workflow}.yml"
            and run["head_sha"] == revision and run["head_branch"] == "main"
            and run["event"] in ("push", "workflow_dispatch")
            and run["repository"]["full_name"] == REPO and run["head_repository"]["full_name"] == REPO
            and run["status"] == "completed" and run["conclusion"] == "success"
            and type(run["run_attempt"]) is int and run["run_attempt"] > 0,
            f"{workflow} candidate must be a successful run on the captured main revision")
    return run


def acquire(github, root, revision, run_id, workflow, name, filenames):
    run = candidate_run(github, run_id, revision, workflow)
    items = github.api(f"actions/runs/{run_id}/artifacts?per_page=100", pages=True, key="artifacts")
    matches = [a for a in items if a["name"] == name]
    require(len(matches) == 1 and matches[0]["expired"] is False, f"missing, expired or duplicate {name}")
    artifact = matches[0]
    require(type(artifact["id"]) is int and artifact["id"] > 0
            and re.fullmatch(r"sha256:[a-f0-9]{64}", artifact.get("digest") or ""),
            f"missing immutable ID or archive digest for {name}")
    root.mkdir()
    with tempfile.TemporaryDirectory(dir=root.parent) as scratch:
        archive = Path(scratch) / "artifact.zip"
        api_file(f"actions/artifacts/{artifact['id']}/zip", archive)
        with archive.open("rb") as stream:
            require("sha256:" + hash_stream(stream) == artifact["digest"], f"Actions digest conflict for {name}")
        with zipfile.ZipFile(archive) as bundle:
            entries = bundle.infolist()
            require(len(entries) == len(filenames) and {i.filename for i in entries} == filenames,
                    f"unexpected or duplicate files in {name}")
            for member in entries:
                # All destinations come from the fixed, flat allowlist above.
                with bundle.open(member) as source, (root / member.filename).open("xb") as output:
                    shutil.copyfileobj(source, output)
    return {"run_id": run_id, "run_attempt": run["run_attempt"], "url": run["html_url"],
            "workflow": run["path"], "artifact_id": artifact["id"], "artifact_digest": artifact["digest"]}


def container_api():
    try:
        return importlib.import_module("container_release")
    except ModuleNotFoundError as error:
        if error.name != "container_release":
            raise
        raise ReleaseError("the packaged release requires the container distribution adapter from #41") from error


@dataclass
class Packages:
    identity: dict
    registry: object
    native_dir: Path | None = None
    oci_dir: Path | None = None

    @classmethod
    def download(cls, github, root, revision, version, native_run, oci_run):
        registry = container_api()
        native = root / "native"
        oci = root / "oci"
        native_run = acquire(github, native, revision, native_run, "native-distribution",
                             f"komodo-mcp-distribution-{revision}",
                             set(asset_names(version)) | {"distribution-manifest.json"})
        oci_run = acquire(github, oci, revision, oci_run, "container-distribution",
                          f"komodo-mcp-oci-{revision}", {"candidate.oci.tar", "oci-receipt.json"})
        manifest = load_manifest(native, revision=revision, version=version, targets=TARGETS,
                                 run_id=native_run["run_id"], run_attempt=native_run["run_attempt"])
        receipt = registry.load_candidate(oci, revision=revision, version=version, distribution=manifest)
        identity = {"native_run": native_run, "oci_run": oci_run, "distribution": manifest, "container": receipt}
        cls.check_identity(identity, revision, version)
        return cls(identity, registry, native, oci)

    @classmethod
    def from_receipt(cls, identity, revision, version):
        cls.check_identity(identity, revision, version)
        registry = container_api()
        receipt = registry.validate_receipt(identity["container"], revision=revision, version=version)
        native = {f["target"]: f["executable_sha256"] for f in identity["distribution"]["files"]
                  if f["kind"] == "native"}
        for platform in receipt["platforms"].values():
            require(native.get(platform["target"]) == platform["executable_sha256"],
                    "recorded OCI executable differs from the tested native binary")
        return cls(identity, registry)

    @staticmethod
    def check_identity(identity, revision, version):
        manifest, receipt = identity["distribution"], identity["container"]
        for document, run, workflow in [(manifest, identity["native_run"], "native-distribution"),
                                         (receipt, identity["oci_run"], "container-distribution")]:
            require(document["schema_version"] == 1 and document["repository"] == REPO
                    and document["revision"] == revision and document["version"] == version
                    and document["run_id"] == run["run_id"] and document["run_attempt"] == run["run_attempt"]
                    and run["workflow"] == f".github/workflows/{workflow}.yml"
                    and all(type(run[k]) is int and run[k] > 0 for k in ("run_id", "run_attempt", "artifact_id"))
                    and re.fullmatch("sha256:[a-f0-9]{64}", run["artifact_digest"]),
                    "candidate receipt belongs to a different repository, revision, version or run attempt")
        files = manifest["files"]
        require(len(files) == len(asset_names(version))
                and {f["name"]: f["kind"] for f in files} == asset_names(version)
                and all(type(f["bytes"]) is int and f["bytes"] > 0
                        and re.fullmatch("[a-f0-9]{64}", f["sha256"]) for f in files),
                "candidate receipt does not cover the exact release assets")
        for entry in files:
            if entry["kind"] == "native":
                require(entry["target"] in TARGETS and entry["name"] == stem(version, entry["target"]) + ".tar.gz"
                        and re.fullmatch("[a-f0-9]{64}", entry["executable_sha256"]),
                        "native target or tested executable digest missing from release evidence")
        require(receipt["image"] == f"ghcr.io/{REPO}:v{version.replace('+', '_')}"
                and matches_download_url(receipt["source_url"], download_url(version, stem(version, "source")+".tar.gz")),
                "candidate image or public source URL conflicts with the release version")

    def inventory(self, github, release, version, previous):
        files = {f["name"]: f for f in self.identity["distribution"]["files"]}
        observed = {}
        ids = set()
        for asset in github.assets(release["id"]):
            name, aid = asset["name"], asset["id"]
            require(name in files and name not in observed and type(aid) is int and aid > 0 and aid not in ids,
                    f"unexpected or duplicate release asset: {name}")
            ids.add(aid)
            spec = files[name]
            if asset["state"] == "starter":
                raise CommandError(f"incomplete starter upload for {name}; inspect the original asset ID {aid}")
            url = download_url(version, name)
            # GitHub may assign temporary draft URLs. The stable release and asset IDs
            # bind draft bytes until publication exposes the canonical public URL.
            require(asset["state"] == "uploaded" and asset["size"] == spec["bytes"]
                    and (release["draft"] or matches_download_url(asset["browser_download_url"], url)),
                    f"asset state, size or URL conflict for {name}")
            remote_digest = asset.get("digest")
            if remote_digest is None:
                remote_digest = "sha256:" + github.asset_hash(aid)
            require(remote_digest == "sha256:" + spec["sha256"], f"asset digest conflict for {name}")
            observed[name] = {"id": aid, "bytes": spec["bytes"], "sha256": spec["sha256"],
                              "url": url}
        require(all(observed.get(name) == asset for name, asset in previous.items()),
                "previously observed release asset disappeared or changed identity")
        return observed


def verify_public_source(asset):
    # Anonymous GET of the canonical GitHub download, including its normal CDN redirect.
    request = urllib.request.Request(asset["url"], headers={"User-Agent": "komodo-mcp-release"})
    try:
        with urllib.request.urlopen(request, timeout=120) as response:
            hasher, size = hashlib.sha256(), 0
            while block := response.read(1024 * 1024):
                size += len(block)
                require(size <= asset["bytes"], "public source exceeds the expected size")
                hasher.update(block)
    except (OSError, urllib.error.URLError) as error:
        raise CommandError("cannot read the public Corresponding Source anonymously") from error
    require(size == asset["bytes"] and hasher.hexdigest() == asset["sha256"],
            "anonymous public Corresponding Source differs from the release manifest")
