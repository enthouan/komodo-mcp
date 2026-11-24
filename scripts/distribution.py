"""Validate the flat bundle produced by the read-only native distribution workflow."""
import hashlib
import json
from pathlib import Path
import re

REPOSITORY = "enthouan/komodo-mcp"
TARGETS = ("x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl",
           "x86_64-apple-darwin", "aarch64-apple-darwin")
MANIFEST = "distribution-manifest.json"
VERSION = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?")
HEX = re.compile(r"[a-f0-9]{64}")


class DistributionError(ValueError):
    pass


def require(condition, message):
    if not condition:
        raise DistributionError(message)


def sha256(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def stem(version, target):
    return f"komodo-mcp-v{version}-{target}"


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, f"duplicate JSON field {key}")
        result[key] = value
    return result


def load_manifest(root: Path, *, revision: str, version: str, targets, run_id=None, run_attempt=None):
    """Verify files against an independently selected revision, version, workflow run and target subset."""
    root = Path(root)
    require(isinstance(revision, str) and re.fullmatch(r"[a-f0-9]{40}", revision), "invalid expected revision")
    require(isinstance(version, str) and VERSION.fullmatch(version), "invalid Cargo release version")
    required = set(targets)
    require(required <= set(TARGETS), "unknown expected native target")
    path = root / MANIFEST
    require(root.is_dir() and not root.is_symlink() and path.is_file() and not path.is_symlink()
            and path.stat().st_size < 128_000, "invalid distribution manifest")
    data = json.loads(path.read_bytes(), object_pairs_hook=unique_object)
    require(isinstance(data, dict) and set(data) == {"schema_version", "repository", "revision", "version",
            "run_id", "run_attempt", "files"}, "unexpected distribution manifest fields")
    require(type(data["schema_version"]) is int and data["schema_version"] == 1
            and data["repository"] == REPOSITORY and data["revision"] == revision and data["version"] == version,
            "distribution manifest identity mismatch")
    for key, expected in (("run_id", run_id), ("run_attempt", run_attempt)):
        value = data[key]
        require(type(value) is int and value > 0 and
                (expected is None or type(expected) is int and value == expected), f"wrong workflow {key}")
    entries = data["files"]
    require(isinstance(entries, list) and 4 < len(entries) <= 4 + len(TARGETS), "invalid artifact count")
    singletons = {"source":stem(version, "source")+".tar.gz", "checksums":"SHA256SUMS",
                  "license":"LICENSE", "notices":"THIRD-PARTY-NOTICES"}
    registered, found_targets, sums = set(), set(), {}
    for entry in entries:
        require(isinstance(entry, dict), "invalid distribution file entry")
        kind = entry.get("kind")
        keys = {"name","kind","bytes","sha256"}
        if kind == "native":
            target = entry.get("target")
            require(target in TARGETS and target not in found_targets, "unexpected or duplicate native target")
            found_targets.add(target)
            expected = stem(version, target)+".tar.gz"
            keys |= {"target", "executable_sha256"}
            require(isinstance(entry.get("executable_sha256"), str) and HEX.fullmatch(entry["executable_sha256"]),
                    "invalid executable digest")
            if target.endswith("-darwin"):
                keys.add("minimum_macos")
                require(isinstance(entry.get("minimum_macos"), str)
                        and re.fullmatch(r"[0-9]+\.[0-9]+(?:\.[0-9]+)?", entry["minimum_macos"]),
                        "missing Mach-O deployment target")
        else:
            require(kind in singletons, "unexpected artifact kind")
            expected = singletons[kind]
        require(set(entry) == keys and entry["name"] == expected and expected not in registered,
                "unexpected artifact name, duplicate, or fields")
        registered.add(expected)
        file = root / expected
        require(file.is_file() and not file.is_symlink() and type(entry["bytes"]) is int
                and 0 < entry["bytes"] == file.stat().st_size, f"size mismatch for {expected}")
        require(isinstance(entry["sha256"], str) and HEX.fullmatch(entry["sha256"])
                and sha256(file) == entry["sha256"], f"SHA-256 mismatch for {expected}")
        if kind != "checksums":
            sums[expected] = entry["sha256"]
    require(set(singletons.values()) <= registered and required <= found_targets, "missing distribution files")
    require({p.name for p in root.iterdir()} == registered | {MANIFEST}, "unexpected files in distribution bundle")
    contents = "".join(f"{value}  {name}\n" for name, value in sorted(sums.items()))
    require((root / "SHA256SUMS").read_bytes() == contents.encode("ascii"), "incorrect manifest checksum coverage")
    return data
