"""Assemble versioned source, notices and the flat, verified candidate bundle (no publishing)."""
import argparse
from collections import defaultdict
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time
import tomllib
import urllib.error
import urllib.request

from distribution import MANIFEST, REPOSITORY, TARGETS, load_manifest, require, sha256, stem
from distribution_archive import archive, extract


def regular_license(path):
    return re.fullmatch(r"(?:licen[cs]es?|copying|copyright|notice)s?(?:[-_.].*)?", path.name, re.I)


def legal_files(crate):
    manifest = tomllib.loads((crate/"Cargo.toml").read_text())["package"]
    declared = manifest.get("license-file")
    if declared:
        path = (crate/declared).resolve()
        require(path.is_relative_to(crate) and path.is_file(), "missing declared license: " + crate.name)
    return sorted(p for p in crate.rglob("*") if p.is_file() and (p == (crate/declared if declared else None)
              or regular_license(p) or any(parent.lower() in ("license","licenses","licences") for parent in
              p.relative_to(crate).parts[:-1])))


def fetch_notice(url, expected):
    for attempt in range(4):
        try:
            with urllib.request.urlopen(url, timeout=30) as stream:
                blob = stream.read(2_000_001)
            break
        except (TimeoutError, urllib.error.URLError):
            if attempt == 3:
                raise
            time.sleep(2 ** attempt)
    require(len(blob) <= 2_000_000 and hashlib.sha256(blob).hexdigest() == expected, "upstream notice hash mismatch")
    return blob


def crate_notices(source):
    vendor, supplemental = source/"vendor", source/"third-party-sources"/"crate-licenses"
    supplemental.mkdir(parents=True)
    fallback = json.loads((source/"scripts/distribution-licenses.json").read_text())
    mapped = {name:group for group in fallback for name in group["crates"]}
    locked = {p["name"]+"-"+p["version"]:p for p in tomllib.loads((source/"Cargo.lock").read_text())["package"]
              if "source" in p}
    require(set(locked) == {path.name for path in vendor.iterdir()}, "vendor directory differs from Cargo.lock")
    notes, texts, labels = [], {}, defaultdict(set)
    for name, record in sorted(locked.items()):
        crate = vendor/name
        require(record["source"] == "registry+https://github.com/rust-lang/crates.io-index"
                and json.loads((crate/".cargo-checksum.json").read_text())["package"] == record["checksum"],
                "unverified registry package: "+name)
        meta = tomllib.loads((crate/"Cargo.toml").read_text())["package"]
        expr = meta.get("license") or meta.get("license-file")
        require(expr, "missing license declaration: "+name)
        notes.append(f"{name} | {expr} | {meta.get('repository') or 'https://crates.io/crates/'+record['name']}")
        if meta.get("authors"):
            notes.append("  " + ", ".join(meta["authors"]))
        files = legal_files(crate)
        # Cargo workspace subpackages sometimes omit the root license. Cross-check the published
        # crate's VCS record before adding the root notice from the same immutable upstream tree.
        if name in mapped:
            group = mapped[name]
            vcs = json.loads((crate/".cargo_vcs_info.json").read_text())["git"]["sha1"]
            require(vcs == group["revision"], "license map uses a different upstream revision: "+name)
            for item in group["files"]:
                dest = supplemental/group["repository"]/vcs/item["path"]
                if not dest.exists():
                    url = f"https://raw.githubusercontent.com/{group['repository']}/{vcs}/{item['path']}"
                    dest.parent.mkdir(parents=True, exist_ok=True)
                    dest.write_bytes(fetch_notice(url,item["sha256"]))
                files.append(dest)
            if not group["files"]:
                notes.append("  Upstream revision "+vcs+" supplies the license declaration in Cargo.toml.")
        require(files or name in mapped, "review new package without a license file: "+name)
        for path in files:
            blob = path.read_bytes()
            require(len(blob) < 4_000_000 and b"\0" not in blob, "unexpected binary or oversize license file")
            key = hashlib.sha256(blob).hexdigest()
            texts[key] = blob.decode("utf-8",errors="replace").strip()
            labels[key].add(path.relative_to(source).as_posix())
    notes += ["","Upstream notices and license texts",""]
    for key in sorted(texts):
        notes.extend([*sorted(labels[key]),"",texts[key],""])
    return "\n".join(notes)


def prepare(root, output, revision):
    from distribution_inputs import bundle_toolchain
    root, output = root.resolve(), output.resolve()
    require(subprocess.check_output(["git","rev-parse","HEAD"],cwd=root,text=True).strip() == revision
            and re.fullmatch(r"[0-9a-f]{40}",revision), "exact source checkout required")
    version = tomllib.loads((root/"Cargo.toml").read_text())["package"]["version"]
    output.mkdir(parents=True,exist_ok=False)
    with tempfile.TemporaryDirectory(prefix="komodo-source-") as td:
        tmp = Path(td)
        base = stem(version,"source")
        project = tmp/"tracked.tar"
        subprocess.run(["git","archive","--format=tar","--prefix="+base+"/","--output",str(project),revision],
                       cwd=root,check=True)
        source = extract(project,tmp/"tracked",base)
        subprocess.run(["cargo","vendor","--locked","--versioned-dirs","vendor"],cwd=source,check=True,
                       stdout=subprocess.DEVNULL)
        cargo_config = source/".cargo"/"config.toml"
        cargo_config.parent.mkdir(exist_ok=True)
        require(not cargo_config.exists(), "review existing source Cargo configuration before vendoring")
        cargo_config.write_text('[source.crates-io]\nreplace-with = "vendored-sources"\n\n'
                                '[source.vendored-sources]\ndirectory = "vendor"\n\n[net]\noffline = true\n')
        toolchain_notices = bundle_toolchain(source/"third-party-sources")
        notices = ("komodo-mcp third-party notices\n\n"
                   f"Project: https://github.com/{REPOSITORY}/tree/{revision}\nVersion: {version}\n"
                   f"Corresponding Source: {base}.tar.gz from this distribution.\n"
                   "The source includes all locked registry packages, including optional and target-specific packages.\n\n"
                   + crate_notices(source) + "\n" + toolchain_notices.strip() + "\n")
        for local in (str(root), str(tmp), str(Path.home())):
            require(local not in notices and local not in cargo_config.read_text(), "local path in distribution metadata")
        (source/"THIRD-PARTY-NOTICES").write_text(notices)
        (source/"DISTRIBUTION-INFO.json").write_text(json.dumps({"repository":REPOSITORY,"revision":revision,
                                                                 "version":version},indent=2)+"\n")
        for name in ("LICENSE","THIRD-PARTY-NOTICES"):
            shutil.copyfile(source/name,output/name)
        archive(source,output/(base+".tar.gz"))


def assemble(source, native, output, revision, version, run_id, run_attempt):
    output.mkdir(parents=True, exist_ok=False)
    files = []
    for kind,name in (("source",stem(version,"source")+".tar.gz"),("license","LICENSE"),("notices","THIRD-PARTY-NOTICES")):
        file = output/name
        shutil.copyfile(source/name,file)
        files.append({"kind":kind,"name":name,"bytes":file.stat().st_size,"sha256":sha256(file)})
    for target in TARGETS:
        receipt = json.loads((native/(target+".json")).read_text())
        require(receipt["version"] == version and receipt["revision"] == revision and receipt["native"] is True
                and receipt["mcp_version"] == version and receipt.get("source_sha256") == files[0]["sha256"],
                "native receipt does not match this source archive, revision and MCP version")
        file = receipt["file"]
        name = stem(version,target)+".tar.gz"
        require(file["name"] == name and file["target"] == target, "native archive identity mismatch")
        shutil.copyfile(native/name,output/name)
        files.append(file)
    sums = output/"SHA256SUMS"
    sums.write_text("".join(f"{entry['sha256']}  {entry['name']}\n" for entry in sorted(files,key=lambda e:e["name"])))
    files.append({"kind":"checksums","name":sums.name,"bytes":sums.stat().st_size,"sha256":sha256(sums)})
    (output/MANIFEST).write_text(json.dumps({"schema_version":1,"repository":REPOSITORY,"revision":revision,
        "version":version,"run_id":run_id,"run_attempt":run_attempt,"files":files},indent=2)+"\n")
    load_manifest(output, revision=revision,version=version,targets=TARGETS,run_id=run_id,run_attempt=run_attempt)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action",required=True)
    source = sub.add_parser("source")
    source.add_argument("--revision",required=True)
    source.add_argument("--output",type=Path,required=True)
    unpack = sub.add_parser("extract-source")
    unpack.add_argument("--archive",type=Path,required=True)
    unpack.add_argument("--destination",type=Path,required=True)
    unpack.add_argument("--version",required=True)
    collect = sub.add_parser("assemble")
    for name in ("source","native","output"):
        collect.add_argument("--"+name,required=True,type=Path)
    for name in ("revision","version"):
        collect.add_argument("--"+name,required=True)
    for name in ("run-id","run-attempt"):
        collect.add_argument("--"+name,required=True,type=int)
    args = parser.parse_args()
    if args.action == "source":
        prepare(Path.cwd(),args.output,args.revision)
    elif args.action == "extract-source":
        extract(args.archive,args.destination,stem(args.version,"source"))
    else:
        assemble(args.source,args.native,args.output,args.revision,args.version,args.run_id,args.run_attempt)


if __name__ == "__main__":
    main()
