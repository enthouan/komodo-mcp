"""Check the Git index before publishing benchmark results."""

import gzip
import io
import re
import subprocess
import sys
from itertools import chain


SUMMARY = b"benchmarks/results/README.md"
REPORT_FORMAT = re.compile(rb'"format"\s*:\s*"komodo-(?:inventory-benchmark|incident-benchmark|benchmark-artifacts)/')
LOCAL_PATH = re.compile(rb"(?:/(?:Users|home|(?:private/)?tmp|(?:private/)?var/folders)/|[A-Z]:[\\/]+Users[\\/]+)", re.I)


def contains_report(stream):
    blocks = iter(lambda: stream.read(64 * 1024), b"")
    # A report is a JSON object. Source files also declare the format strings.
    first = next((block.lstrip() for block in blocks if block.strip()), b"")
    if not first.startswith(b"{"):
        return False
    tail = b""
    for block in chain((first,), blocks):
        if REPORT_FORMAT.search(tail + block):
            return True
        # The format field can straddle two reads in a large or reordered report.
        tail = block[-128:]
    return False


def main():
    entries = subprocess.check_output([
        "git", "ls-files", "--full-name", "--stage", "-z", "--", ":/",
    ])
    for entry in entries.split(b"\0"):
        if not entry:
            continue
        metadata, path = entry.split(b"\t", 1)
        mode, oid, _stage = metadata.split()
        if path != SUMMARY and path.startswith((b"benchmarks/results/", b"target/benchmark/")):
            sys.exit("Keep raw benchmark files local. Commit findings in benchmarks/results/README.md.")
        if mode not in (b"100644", b"100755"):
            if path == SUMMARY:
                sys.exit("The benchmark summary must be a regular Markdown file.")
            continue

        contents = subprocess.check_output(["git", "cat-file", "blob", oid])
        if path == SUMMARY:
            if b"\0" in contents or LOCAL_PATH.search(contents) or REPORT_FORMAT.search(contents):
                sys.exit("Keep raw reports and workstation paths out of the benchmark summary.")
            contents.decode("utf-8")

        stream = io.BytesIO(contents)
        if contents.startswith(b"\x1f\x8b"):
            stream = gzip.GzipFile(fileobj=stream)
        with stream:
            found = contains_report(stream)
        if found:
            sys.exit("A staged file contains a raw benchmark report or preparation manifest.")


if __name__ == "__main__":
    main()
