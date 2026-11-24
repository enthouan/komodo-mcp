"""Bundle the pinned non-Cargo sources and notices for native binaries and images."""
import hashlib
from pathlib import Path, PurePosixPath
import re
import tarfile
import tempfile
from typing import NamedTuple
from urllib.error import URLError
from urllib.request import Request, urlopen

from distribution import DistributionError

RUST = "1.99.1"
RUST_COMMIT = "58cf8f9c35b355199727627d07ae34a4e2846d67"
RUST_DIST = "https://static.rust-lang.org/dist/2026-10-15"
CROSS = "3635262e4524c991552789af6f36211a335a77b3"
LLVM = "1b9c0d5ff9bbe7634aead059efe6b11a7eeba145"
FIREFOX = "7ff62457dcf126523608e2f4b90d232d49d58185"
CURL = "a7342f165557f6889e6ba9ba193f0499b5084327"
RAW = "https://raw.githubusercontent.com"


class Source(NamedTuple):
    name: str
    url: str
    size: int
    sha256: str


# The official compiler source distribution also contains the fork of LLVM used
# by this toolchain. Its compiler-rt C/assembly sources are absent from rust-src.
RUNTIME_INPUTS = (
    Source(f"rustc-{RUST}-src.tar.xz", f"{RUST_DIST}/rustc-{RUST}-src.tar.xz", 265270392,
           "6451598a99e77f07571960f6b21d577399d3a30f33e80cbd2e3a4aae6535d265"),
    Source("musl-1.2.5.tar.gz", "https://musl.libc.org/releases/musl-1.2.5.tar.gz", 1080786,
           "a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4"),
    Source("musl-cross-make.tar.gz", f"https://codeload.github.com/richfelker/musl-cross-make/tar.gz/{CROSS}", 213688,
           "60bed670d689d5c2164020960df36b80189dc5617e9763e023672f98a99d567e"),
    Source("licenses/compiler-rt.txt", f"{RAW}/rust-lang/llvm-project/{LLVM}/compiler-rt/LICENSE.TXT", 16708,
           "1a8f1058753f1ba890de984e48f0242a3a5c29a6a8f2ed9fd813f36985387e8d"),
)

CA_INPUTS = (
    Source("ca/ca-certificates.crt", "https://curl.se/ca/cacert-2026-11-05.pem", 200116,
           "427c064216761a1e72a8b30f0d954d08cb6e4435a82091fb865afd20c8497285"),
    Source("ca/certdata.txt", f"{RAW}/mozilla-firefox/firefox/{FIREFOX}/security/nss/lib/ckfw/builtins/certdata.txt", 1356175,
           "3ec2b4d240dc12c747eaf450061db84af757c483bbda302f9e29ceb5d70be510"),
    Source("ca/LICENSE", "https://www.mozilla.org/media/MPL/2.0/index.txt", 16726,
           "3f3d9e0024b1921b067d6f7f88deb4a60cbe7a78e76c64e3f1d7fc3b779b9d04"),
    Source("ca/mk-ca-bundle.pl", f"{RAW}/curl/curl/{CURL}/scripts/mk-ca-bundle.pl", 26130,
           "a70b45f335e51177748dbc29b3041964e4921c00554ca389ae05598a6f418659"),
    Source("ca/COPYING.curl", f"{RAW}/curl/curl/{CURL}/COPYING", 1088,
           "82f2f4427d6545ee5aaac4f0b80428da6cc8ba41c2cf5da3a03680ec327b9681"),
)

# The smaller official component carries the license texts alongside the actual
# standard-library sources. Use it to build notices without unpacking the compiler.
RUST_NOTICES = Source("rust-src.tar.xz", f"{RUST_DIST}/rust-src-{RUST}.tar.xz", 5863216,
                      "e485a88c45db546b73571df7bebfcb4454ba4fb552c492e25fc5b00eeb5bf22d")

RUNTIME_NOTE = f"""Rust {RUST} ({RUST_COMMIT}), LLVM compiler-rt, LLVM libunwind and compiler-builtins
The upstream Rust source archive contains the standard library in library/, the
LLVM fork in src/llvm-project/, and the build configuration and patches in src/ci/.
LLVM revision: {LLVM}.

musl 1.2.5 and musl-cross-make ({CROSS})
The two Rust Linux musl target recipes use src/ci/docker/scripts/musl-toolchain.sh
in the included Rust source. That script applies the patches supplied by Rust on
top of the patches/musl-1.2.5/ directory of the matching musl-cross-make source.
The two upstream source archives and their copyright notices accompany the recipe.
"""

CA_NOTE = f"""Mozilla CA certificate data, converted by the curl project
The container's PEM file is the unmodified curl CA Extract dated 2026-11-05.
Mozilla distributes the certificate data under MPL 2.0. See LICENSE.
The matching Mozilla certdata.txt is from Firefox revision {FIREFOX}.
The curl conversion script is from {CURL}. See COPYING.curl.

To regenerate the exact PEM without network access, run these commands in ca/
with Python 3 and Perl's mk-ca-bundle.pl prerequisites:
  python3 -c 'import os; os.utime("certdata.txt", (1793851921, 1793851921))'
  perl mk-ca-bundle.pl -n -d release rebuilt.pem
  cmp rebuilt.pem ca-certificates.crt
The timestamp reproduces the source date embedded in the original PEM header.
"""


def fetch(source, directory):
    """Read an immutable input. The byte count bounds the download before hashing."""
    path = directory / source.name
    path.parent.mkdir(parents=True, exist_ok=True)
    urls = [source.url]
    if source.url.startswith(RAW + "/"):
        owner, repository, revision, filename = source.url.removeprefix(RAW + "/").split("/", 3)
        urls.append(f"https://api.github.com/repos/{owner}/{repository}/contents/{filename}?ref={revision}")
    for url in urls:
        headers = {"Accept": "application/vnd.github.raw+json", "User-Agent": "komodo-mcp-distribution"}
        try:
            with urlopen(Request(url, headers=headers), timeout=120) as body, path.open("wb") as out:
                digest = hashlib.sha256()
                remaining = source.size
                while chunk := body.read(min(remaining + 1, 1024 * 1024)):
                    remaining -= len(chunk)
                    if remaining < 0:
                        raise DistributionError(f"oversized source input: {source.name}")
                    digest.update(chunk)
                    out.write(chunk)
            if remaining or digest.hexdigest() != source.sha256:
                raise DistributionError(f"source input digest or size mismatch: {source.name}")
            return path
        except (URLError, TimeoutError):
            if url == urls[-1]:
                raise
    raise AssertionError("no source URL")


def archive_text(archive, member):
    info = archive.getmember(member)
    if not info.isfile() or info.size > 2 * 1024 * 1024:
        raise DistributionError(f"invalid license or revision member: {member}")
    with archive.extractfile(info) as reader:
        return reader.read().decode("utf-8")


def bundle_toolchain(destination: Path) -> str:
    """Create third-party-sources in a fresh distribution and return its notices."""
    if destination.exists() or destination.is_symlink():
        raise DistributionError("third-party source destination already exists")
    with tempfile.TemporaryDirectory(dir=destination.parent) as temporary:
        staging = Path(temporary) / "bundle"
        staging.mkdir()
        inputs = (*RUNTIME_INPUTS, *CA_INPUTS)
        for source in inputs:
            fetch(source, staging)
        rust_src = fetch(RUST_NOTICES, Path(temporary))
        licenses = {"LLVM compiler-rt": (staging / "licenses/compiler-rt.txt").read_text()}
        with tarfile.open(rust_src, "r:xz") as tar:
            base = f"rust-src-{RUST}/"
            if archive_text(tar, base + "git-commit-hash").strip() != RUST_COMMIT:
                raise DistributionError("Rust notices are from a different revision")
            runtime = base + "rust-src/lib/rustlib/src/rust/"
            for name, member in (
                ("Rust copyright", base + "COPYRIGHT"),
                ("Rust MIT license", base + "LICENSE-MIT"),
                ("Rust Apache license", base + "LICENSE-APACHE"),
                ("compiler-builtins", runtime + "library/compiler-builtins/LICENSE.txt"),
                ("LLVM libunwind", runtime + "src/llvm-project/libunwind/LICENSE.TXT"),
            ):
                licenses[name] = archive_text(tar, member)
            vendor = runtime + "library/vendor/"
            vendor_texts = {}
            for member in tar.getmembers():
                if not member.isfile() or not member.name.startswith(vendor):
                    continue
                path = PurePosixPath(member.name.removeprefix(vendor))
                if re.match(r"^(licen[cs]e|copying|copyright|notice)(?:[-._]|$)", path.name, re.I):
                    text = archive_text(tar, member.name)
                    vendor_texts.setdefault(text, []).append(str(path))
            for text, paths in vendor_texts.items():
                licenses["Rust standard library vendor: " + ", ".join(sorted(paths))] = text
        with tarfile.open(staging / "musl-1.2.5.tar.gz") as tar:
            licenses["musl"] = archive_text(tar, "musl-1.2.5/COPYRIGHT")
        with tarfile.open(staging / "musl-cross-make.tar.gz") as tar:
            for name in ("LICENSE", "COPYRIGHT"):
                licenses["musl-cross-make " + name] = archive_text(tar, f"musl-cross-make-{CROSS}/{name}")
        licenses["Mozilla Public License 2.0"] = (staging / "ca/LICENSE").read_text()
        licenses["curl"] = (staging / "ca/COPYING.curl").read_text()
        (staging / "ca/NOTICE").write_text(CA_NOTE)
        (staging / "SOURCES").write_text(RUNTIME_NOTE + "\n" + "\n".join(
            f"{source.sha256}  {source.name}  {source.url}" for source in inputs) + "\n")
        notices = RUNTIME_NOTE + "\n" + CA_NOTE + "\n" + "\n".join(
            f"== {name} ==\n{license_text}\n" for name, license_text in licenses.items())
        staging.rename(destination)
        return notices
