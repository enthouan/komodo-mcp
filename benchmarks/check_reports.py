"""Check the Git index before publishing benchmark results."""

import gzip
import re
import subprocess
import sys


SUMMARY = b"benchmarks/results/README.md"
REPORT_FORMAT = re.compile(rb'"format"\s*:\s*"komodo-(?:inventory-benchmark|incident-benchmark|benchmark-artifacts)/')
LOCAL_PATH = re.compile(rb"(?:/(?:Users|home|(?:private/)?tmp|(?:private/)?var/folders)/|[A-Z]:[\\/]+Users[\\/]+)", re.I)


def main():
    entries = subprocess.check_output([
        "git", "ls-files", "--stage", "-z", "--",
        "benchmarks/results", "target/benchmark", "*.json", "*.json.gz", "benchmarks/*.gz",
    ])
    for entry in entries.split(b"\0"):
        if not entry:
            continue
        metadata, path = entry.split(b"\t", 1)
        mode, oid, _stage = metadata.split()
        if path != SUMMARY and path.startswith((b"benchmarks/results/", b"target/benchmark/")):
            sys.exit("Keep raw benchmark files local. Commit findings in benchmarks/results/README.md.")

        contents = subprocess.check_output(["git", "cat-file", "blob", oid])
        if path == SUMMARY:
            if mode not in (b"100644", b"100755") or b"\0" in contents or LOCAL_PATH.search(contents):
                sys.exit("The benchmark summary must be Markdown text without workstation paths.")
            contents.decode("utf-8")
        elif path.endswith(b".gz"):
            contents = gzip.decompress(contents)
        if REPORT_FORMAT.search(contents):
            sys.exit("A staged file contains a raw benchmark report or preparation manifest.")


if __name__ == "__main__":
    main()
