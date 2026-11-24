"""Build and verify registry-free Linux OCI candidates. Publication is called only by release.py."""

import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = "enthouan/komodo-mcp"
IMAGE = "ghcr.io/" + REPOSITORY
INDEX = "application/vnd.oci.image.index.v1+json"
MANIFEST = "application/vnd.oci.image.manifest.v1+json"
CONFIG = "application/vnd.oci.image.config.v1+json"
LAYER = "application/vnd.oci.image.layer.v1.tar"
TARGETS = {"linux/amd64": "x86_64-unknown-linux-musl", "linux/arm64": "aarch64-unknown-linux-musl"}
CA_SHA = "427c064216761a1e72a8b30f0d954d08cb6e4435a82091fb865afd20c8497285"
EXE = "/usr/local/bin/komodo-mcp"
CA = "/etc/ssl/certs/ca-certificates.crt"
DOCS = "/usr/share/doc/komodo-mcp"
MAX_LAYER = 256 * 1024 * 1024
ARCHIVE = "candidate.oci.tar"
RECEIPT = "oci-receipt.json"
DEPS = "third-party-sources/ca/"
LEGAL = {"LICENSE", "THIRD-PARTY-NOTICES", "README.md"}
SEMVER = re.compile(
    r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-([A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*))?(?:\+[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*)?"
)


def check(condition, message):
    if not condition:
        raise ValueError(message)


def hexsha(value):
    return isinstance(value, str) and re.fullmatch("[0-9a-f]{64}", value)


def digest(value):
    return isinstance(value, str) and value.startswith("sha256:") and hexsha(value[7:])


def sha_file(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def public_ref(version):
    match = SEMVER.fullmatch(version)
    check(
        match and not any(p.isdigit() and len(p) > 1 and p[0] == "0" for p in (match[1] or "").split(".")),
        "invalid release SemVer",
    )
    tag = "v" + version.replace("+", "_")
    check(len(tag) <= 128, "version exceeds the OCI tag limit")
    return IMAGE + ":" + tag


def source_url(version):
    return (
        f"https://github.com/{REPOSITORY}/releases/download/v{version}/komodo-mcp-v{version}-source.tar.gz"
    )


def listing(version):
    page = f"https://github.com/{REPOSITORY}/releases/tag/v{version}"
    description = (
        f"GPL-3.0-or-later. [LICENSE, THIRD-PARTY-NOTICES and SHA256SUMS]({page}). "
        f"[Corresponding Source]({source_url(version)})."
    )
    if len(description) > 512:
        # GHCR caps the displayed description. All legal materials also travel in the image/source.
        return f"GPL-3.0-or-later. LICENSE, THIRD-PARTY-NOTICES and SHA256SUMS accompany [Corresponding Source]({source_url(version)})."
    return description


def validate_receipt(receipt, *, revision, version):
    check(
        isinstance(receipt, dict)
        and set(receipt)
        == {
            "schema_version",
            "repository",
            "revision",
            "version",
            "run_id",
            "run_attempt",
            "image",
            "source_url",
            "native_run",
            "archive",
            "index_digest",
            "platforms",
        },
        "invalid OCI receipt fields",
    )
    check(
        type(receipt["schema_version"]) is int
        and receipt["schema_version"] == 1
        and receipt["repository"] == REPOSITORY,
        "unexpected OCI receipt schema or repository",
    )
    check(
        re.fullmatch("[0-9a-f]{40}", revision)
        and receipt["revision"] == revision
        and receipt["version"] == version,
        "OCI receipt source identity differs",
    )
    check(
        receipt["image"] == public_ref(version) and receipt["source_url"] == source_url(version),
        "OCI destination differs",
    )
    check(
        all(type(receipt[k]) is int and receipt[k] > 0 for k in ("run_id", "run_attempt")),
        "invalid Actions identity",
    )
    native = receipt["native_run"]
    check(
        isinstance(native, dict)
        and set(native) == {"run_id", "run_attempt", "workflow", "artifact_id", "artifact_digest"}
        and all(type(native[k]) is int and native[k] > 0 for k in ("run_id", "run_attempt", "artifact_id"))
        and native["workflow"] == ".github/workflows/native-distribution.yml"
        and digest(native["artifact_digest"]),
        "invalid source Native distribution run or artifact identity",
    )
    archive = receipt["archive"]
    check(
        set(archive) == {"name", "bytes", "sha256"}
        and archive["name"] == ARCHIVE
        and type(archive["bytes"]) is int
        and 0 < archive["bytes"] <= 2**31
        and hexsha(archive["sha256"]),
        "invalid OCI archive identity",
    )
    platforms = receipt["platforms"]
    check(
        digest(receipt["index_digest"]) and isinstance(platforms, dict) and set(platforms) == set(TARGETS),
        "invalid OCI platform matrix",
    )
    for platform, target in TARGETS.items():
        p = platforms[platform]
        check(
            set(p) == {"target", "manifest_digest", "config_digest", "executable_sha256"}
            and p["target"] == target
            and digest(p["manifest_digest"])
            and digest(p["config_digest"])
            and hexsha(p["executable_sha256"]),
            "invalid OCI platform identity",
        )
    check(len({p["manifest_digest"] for p in platforms.values()}) == 2, "duplicate platform manifest")
    return receipt


def tar_members(archive):
    entries = {}
    for m in archive:
        name = m.name.removeprefix("./").rstrip("/")
        if name in ("", ".") and m.isdir():
            continue
        check(
            name
            and not name.startswith("/")
            and all(part not in ("", ".", "..") for part in name.split("/"))
            and name not in entries
            and (m.isdir() or m.isfile())
            and len(entries) < 65536,
            "unsafe or duplicate tar member",
        )
        entries[name] = m
    return entries


def content(archive, entries, name, limit):
    m = entries.get(name)
    check(m is not None and m.isfile() and 0 < m.size <= limit, "missing or oversized archive member")
    with archive.extractfile(m) as stream:
        data = stream.read(limit + 1)
    check(len(data) == m.size, "truncated archive member")
    return data


def describe(data):
    return "sha256:" + hashlib.sha256(data).hexdigest()


def validate_platforms(index):
    check(
        index.get("schemaVersion") == 2
        and index.get("mediaType") == INDEX
        and isinstance(index.get("manifests"), list)
        and len(index["manifests"]) == 2,
        "expected a two-platform OCI image index",
    )
    result = {}
    for desc in index["manifests"]:
        p = desc["platform"]
        platform = p["os"] + "/" + p["architecture"]
        check(
            platform in TARGETS
            and platform not in result
            and desc["mediaType"] == MANIFEST
            and digest(desc["digest"])
            and type(desc["size"]) is int
            and desc["size"] > 0,
            "invalid or duplicate OCI platform descriptor",
        )
        result[platform] = desc
    return result


def verify_config(config, version, revision, platform):
    expected = {
        "org.opencontainers.image.version": version,
        "org.opencontainers.image.revision": revision,
        "org.opencontainers.image.licenses": "GPL-3.0-or-later",
        "org.opencontainers.image.source": f"https://github.com/{REPOSITORY}",
    }
    c = config["config"]
    check(
        config["os"] + "/" + config["architecture"] == platform
        and c["Entrypoint"] == [EXE]
        and c["User"] == "65532:65532",
        "wrong container architecture, entrypoint or runtime user",
    )
    check(
        all(c.get("Labels", {}).get(k) == v for k, v in expected.items())
        and source_url(version) in c["Labels"].get("org.opencontainers.image.description", ""),
        "OCI labels do not identify matching source",
    )
    env = c.get("Env", [])
    check(
        "SSL_CERT_FILE=" + CA in env and all(x.startswith(("PATH=", "SSL_CERT_FILE=")) for x in env),
        "unexpected baked-in environment or CA location",
    )
    # OCI config readers may omit Docker's healthcheck extension. Compose carries the same exec-form check.
    if "Healthcheck" in c:
        check(c["Healthcheck"].get("Test") == ["CMD", EXE, "--healthcheck"], "unexpected image healthcheck")


def inspect_archive(path, *, version, revision, expected_files=None, expected_legal=None):
    """Return the verified image descriptor chain and hashes of the installed native bytes."""
    with tarfile.open(path, "r:") as archive:
        entries = tar_members(archive)

        def blob(desc, media, limit):
            check(
                desc["mediaType"] == media and digest(desc["digest"]) and type(desc["size"]) is int,
                "unexpected OCI descriptor",
            )
            data = content(archive, entries, "blobs/sha256/" + desc["digest"][7:], limit)
            check(
                len(data) == desc["size"] and describe(data) == desc["digest"],
                "OCI blob digest or size mismatch",
            )
            return data

        check(
            json.loads(content(archive, entries, "oci-layout", 4096)) == {"imageLayoutVersion": "1.0.0"},
            "invalid OCI layout",
        )
        root = json.loads(content(archive, entries, "index.json", 2**20))
        check(
            root.get("schemaVersion") == 2 and len(root.get("manifests", [])) == 1,
            "expected one named candidate",
        )
        named = root["manifests"][0]
        index = json.loads(blob(named, INDEX, 2**20))
        check(
            source_url(version)
            in index.get("annotations", {}).get("org.opencontainers.image.description", ""),
            "OCI index lacks matching source URL",
        )
        outputs = {}
        for platform, desc in validate_platforms(index).items():
            manifest = json.loads(blob(desc, MANIFEST, 2**20))
            check(
                manifest.get("schemaVersion") == 2 and manifest.get("mediaType") == MANIFEST,
                "invalid image manifest",
            )
            config = json.loads(blob(manifest["config"], CONFIG, 2**20))
            verify_config(config, version, revision, platform)
            diff_ids = []
            files = {}
            for layer in manifest["layers"]:
                check(layer["mediaType"] in (LAYER, LAYER + "+gzip"), "unsupported OCI layer")
                data = blob(layer, layer["mediaType"], MAX_LAYER)
                if layer["mediaType"].endswith("+gzip"):
                    with gzip.GzipFile(fileobj=io.BytesIO(data)) as zipped:
                        data = zipped.read(MAX_LAYER + 1)
                check(len(data) <= MAX_LAYER, "oversized unpacked OCI layer")
                diff_ids.append(describe(data))
                with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as inner:
                    members = tar_members(inner)
                    for name, m in members.items():
                        if m.isdir():
                            check(m.mode & 0o001, "scratch directory is inaccessible to the non-root user")
                            continue
                        check(m.mode & 0o004, "scratch file is unreadable by the non-root user")
                        dest = "/" + name
                        check(
                            dest not in files and (dest in {EXE, CA} or dest.startswith(DOCS + "/")),
                            "unexpected or replaced file in scratch image",
                        )
                        value = content(inner, members, name, MAX_LAYER)
                        if dest == EXE:
                            check(
                                m.mode & 0o001
                                and value.startswith(b"\x7fELF")
                                and int.from_bytes(value[18:20], "little")
                                == ({"linux/amd64": 62, "linux/arm64": 183}[platform]),
                                "image executable is not the expected Linux ELF",
                            )
                        if dest == DOCS + "/SOURCE.txt":
                            check(
                                source_url(version) in value.decode(), "missing source access instructions"
                            )
                        files[dest] = hashlib.sha256(value).hexdigest()
            required = {
                EXE,
                CA,
                *[DOCS + "/" + f for f in LEGAL],
                DOCS + "/SOURCE.txt",
                DOCS + "/ca/LICENSE",
                DOCS + "/ca/NOTICE",
            }
            check(
                set(files) == required and files[CA] == CA_SHA,
                "missing, unexpected, or incorrect image payload",
            )
            check(
                config["rootfs"]["type"] == "layers" and config["rootfs"]["diff_ids"] == diff_ids,
                "invalid root filesystem",
            )
            if expected_legal is not None:
                check(
                    all(files[DOCS + "/" + name] == sha for name, sha in expected_legal.items()),
                    "container legal files differ from verified distribution",
                )
            if expected_files is not None:
                check(
                    files == expected_files[platform],
                    "container content differs from verified source/native artifacts",
                )
            outputs[platform] = {
                "target": TARGETS[platform],
                "manifest_digest": desc["digest"],
                "config_digest": manifest["config"]["digest"],
                "executable_sha256": files[EXE],
            }
        return named["digest"], outputs


def distribution_files(distribution, revision, version):
    check(
        distribution["revision"] == revision
        and distribution["version"] == version
        and distribution["repository"] == REPOSITORY,
        "native and OCI source identities differ",
    )
    natives = {f["target"]: f for f in distribution["files"] if f["kind"] == "native"}
    check(set(TARGETS.values()) <= natives.keys(), "missing verified native archives")
    return natives


def load_candidate(root, *, revision, version, distribution):
    root = Path(root)
    receipt = validate_receipt(json.loads((root / RECEIPT).read_text()), revision=revision, version=version)
    check(
        all(receipt["native_run"][k] == distribution[k] for k in ("run_id", "run_attempt")),
        "candidate uses a different Native distribution run",
    )
    archive = root / ARCHIVE
    check(
        archive.is_file()
        and not archive.is_symlink()
        and archive.stat().st_size == receipt["archive"]["bytes"]
        and sha_file(archive) == receipt["archive"]["sha256"],
        "candidate OCI tar differs from receipt",
    )
    legal = {f["name"]: f["sha256"] for f in distribution["files"] if f["kind"] in {"license", "notices"}}
    check(set(legal) == {"LICENSE", "THIRD-PARTY-NOTICES"}, "missing verified distribution legal files")
    index, platforms = inspect_archive(archive, version=version, revision=revision, expected_legal=legal)
    check(
        index == receipt["index_digest"] and platforms == receipt["platforms"],
        "candidate contents differ from receipt",
    )
    natives = distribution_files(distribution, revision, version)
    for platform, target in TARGETS.items():
        check(
            platforms[platform]["executable_sha256"] == natives[target]["executable_sha256"],
            "container and release executable bytes differ",
        )
    return receipt


def build_context(context, directory, distribution):
    version = distribution["version"]
    natives = distribution_files(distribution, distribution["revision"], version)
    (source,) = (f for f in distribution["files"] if f["kind"] == "source")
    stem = source["name"].removesuffix(".tar.gz")
    # Corresponding Source can be large. Read only the three required, exact regular members.
    needed = {stem + "/" + DEPS + n: n for n in ("ca-certificates.crt", "LICENSE", "NOTICE")}
    ca = {}
    with tarfile.open(directory / source["name"], "r|gz") as source_tar:
        for m in source_tar:
            if m.name in needed:
                key = needed[m.name]
                check(key not in ca and m.isfile() and 0 < m.size <= 2**20, "invalid source CA member")
                ca[key] = source_tar.extractfile(m).read()
    check(set(ca) == set(needed.values()), "missing CA or its attribution in source")
    check(hashlib.sha256(ca["ca-certificates.crt"]).hexdigest() == CA_SHA, "source CA pin changed")
    expected = {}
    for platform, target in TARGETS.items():
        record = natives[target]
        stem = record["name"].removesuffix(".tar.gz")
        arch = context / platform.split("/")[1]
        docs = arch / "docs"
        (docs / "ca").mkdir(parents=True)
        with tarfile.open(directory / record["name"], "r:gz") as archive:
            files = tar_members(archive)
            binary = content(archive, files, stem + "/komodo-mcp", MAX_LAYER)
            check(hashlib.sha256(binary).hexdigest() == record["executable_sha256"], "wrong native ELF")
            (arch / "komodo-mcp").write_bytes(binary)
            (arch / "komodo-mcp").chmod(0o755)
            for name in LEGAL:
                (docs / name).write_bytes(content(archive, files, stem + "/" + name, 64 * 1024**2))
        for name in ("LICENSE", "THIRD-PARTY-NOTICES"):
            check(
                (docs / name).read_bytes() == (directory / name).read_bytes(),
                "native notices differ from distribution",
            )
        for name in ("LICENSE", "NOTICE"):
            (docs / "ca" / name).write_bytes(ca[name])
        (docs / "SOURCE.txt").write_text(
            "Matching Corresponding Source and build instructions\n" + source_url(version) + "\n"
        )
        expected[platform] = {
            DOCS + "/" + p.relative_to(docs).as_posix(): sha_file(p) for p in docs.rglob("*") if p.is_file()
        }
        expected[platform].update({EXE: record["executable_sha256"], CA: CA_SHA})
    (context / "ca-certificates.crt").write_bytes(ca["ca-certificates.crt"])
    # Docker COPY preserves source modes, including an operator's restrictive umask.
    for path in context.rglob("*"):
        path.chmod(0o755 if path.is_dir() or path.name == "komodo-mcp" else 0o644)

    shutil.copyfile(ROOT / "container" / "Dockerfile", context / "Dockerfile")
    return expected


def assemble(args):
    from distribution import load_manifest

    args.output_dir.mkdir(parents=True, exist_ok=False)
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    revision = subprocess.check_output(["git", "-C", ROOT, "rev-parse", "HEAD"], text=True).strip()
    native_run = json.loads(args.native_identity.read_bytes())
    source = load_manifest(
        args.distribution_dir,
        revision=revision,
        version=version,
        targets=TARGETS.values(),
        run_id=native_run["run_id"],
        run_attempt=native_run["run_attempt"],
    )
    output = args.output_dir.resolve()
    archive = output / ARCHIVE
    with tempfile.TemporaryDirectory() as tmp:
        context = Path(tmp)
        expected = build_context(context, args.distribution_dir, source)
        ref = "komodo-mcp-candidate:git-" + revision
        subprocess.run(
            [
                "docker",
                "buildx",
                "build",
                "--platform",
                ",".join(TARGETS),
                "--provenance=false",
                "--build-arg",
                "VERSION=" + version,
                "--build-arg",
                "REVISION=" + revision,
                "--build-arg",
                "SOURCE_URL=" + source_url(version),
                "--annotation",
                "index:org.opencontainers.image.description=" + listing(version),
                "--output",
                f"type=oci,name={ref},dest={archive}",
                str(context),
            ],
            check=True,
            timeout=600,
        )
    index, platforms = inspect_archive(archive, version=version, revision=revision, expected_files=expected)
    receipt = {
        "schema_version": 1,
        "repository": REPOSITORY,
        "revision": revision,
        "version": version,
        "run_id": args.run_id,
        "run_attempt": args.run_attempt,
        "native_run": native_run,
        "image": public_ref(version),
        "source_url": source_url(version),
        "archive": {"name": ARCHIVE, "bytes": archive.stat().st_size, "sha256": sha_file(archive)},
        "index_digest": index,
        "platforms": platforms,
    }
    validate_receipt(receipt, revision=revision, version=version)
    (output / RECEIPT).write_text(json.dumps(receipt, indent=2) + "\n")


def read_published(*, receipt, anonymous=False):
    from container_registry import remote_state

    validate_receipt(receipt, revision=receipt["revision"], version=receipt["version"])
    return remote_state(receipt, anonymous=anonymous)


def publish_candidate(root, *, receipt):
    from container_registry import promote

    # The authorized release helper supplies the validated distribution-bound receipt.
    validate_receipt(receipt, revision=receipt["revision"], version=receipt["version"])
    candidate = Path(root) / ARCHIVE
    check(
        candidate.is_file()
        and candidate.stat().st_size == receipt["archive"]["bytes"]
        and sha_file(candidate) == receipt["archive"]["sha256"],
        "candidate changed before publication",
    )
    return promote(candidate, receipt)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--distribution-dir", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--native-identity", type=Path, required=True)
    parser.add_argument("--run-id", type=int, required=True)
    parser.add_argument("--run-attempt", type=int, required=True)
    assemble(parser.parse_args())
