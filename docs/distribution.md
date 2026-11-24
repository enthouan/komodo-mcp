# Native binaries and matching source

Each distribution contains one archive per exercised native target, a matching source archive, `SHA256SUMS`, `LICENSE` and `THIRD-PARTY-NOTICES`. The native filename is `komodo-mcp-v<VERSION>-<TARGET>.tar.gz`. The source filename is `komodo-mcp-v<VERSION>-source.tar.gz`. Each archive extracts into a directory with the same name as the archive minus `.tar.gz`.

| OS | Processor | Rust target | System requirements |
| --- | --- | --- | --- |
| Linux | x86-64 | `x86_64-unknown-linux-musl` | A 64-bit Linux kernel. The binary is statically linked. |
| Linux | ARM64 | `aarch64-unknown-linux-musl` | A 64-bit Linux kernel. The binary is statically linked. |
| macOS | Intel | `x86_64-apple-darwin` | macOS 15 or later. |
| macOS | Apple silicon | `aarch64-apple-darwin` | macOS 15 or later. |

On Linux, HTTPS connections to Core use the machine's trust configuration. Supply a trusted CA bundle with `SSL_CERT_FILE` when your Core uses a private CA. macOS uses its platform trust configuration.

## Verify and install

For a GitHub Release that lists binary assets, obtain the executable archive and `SHA256SUMS` from that release. For a candidate, take both files from the same Native distribution workflow artifact. Earlier source-only releases provide the tagged project source. Use a separate, empty directory for these downloads. Replace the example version and target with the release version and the exact target for your machine.

```sh
version=0.8.0
target=x86_64-unknown-linux-musl
archive="komodo-mcp-v$version-$target.tar.gz"
(
  set -eu
  awk -v f="$archive" '$2 == f {print; n++} END {if (n != 1) exit 1}' SHA256SUMS > "$archive.sha256"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c "$archive.sha256"
  else
    shasum -a 256 -c "$archive.sha256"
  fi
  tar -xzf "$archive"
)
```

Configure the MCP client to launch the extracted `komodo-mcp` executable by absolute path. Its private environment needs `KOMODO_ADDRESS`, `KOMODO_API_KEY` and `KOMODO_API_SECRET`. Receiving an MCP `initialize` response reports the binary's version in `serverInfo.version`.

## Rebuild from the source archive

The source includes the exact Git tree, `Cargo.lock`, every locked registry dependency, applicable patches, build and install instructions, and the pinned sources of bundled runtime components. OpenSSL, AWS-LC and other native crate sources ship inside `vendor/`. `third-party-sources/` contains toolchain runtime sources, patches, the matching CA materials and supplemental crate license records. `DISTRIBUTION-INFO.json` identifies the source revision. The archive-root `.cargo/config.toml` directs Cargo to the included dependencies.

Install Python 3.11+, the Rust version in `rust-toolchain.toml` (currently 1.99.1), and the matching Rust target before going offline. Building requires a C/C++ toolchain, Perl 5, CMake and Make. Linux additionally needs native `musl-gcc`, `readelf` and the native development tools (on Ubuntu 24.04, `build-essential musl-tools binutils cmake ninja-build perl`). On macOS, install Xcode Command Line Tools and CMake. Allow the compiler to execute programs and the smoke test to bind loopback on the build host.

Verify the source archive against its own entry in the same `SHA256SUMS`, using the procedure above with `archive="komodo-mcp-v$version-source.tar.gz"`. Then extract it to a fresh directory and run the following with the target and version set as above.

```sh
rustup target add --toolchain 1.99.1 "$target"
cd "komodo-mcp-v$version-source"
python3 scripts/distribution_build.py --target "$target" --output ../rebuilt
```

The build script checks the native OS and CPU, compiles with Cargo's frozen offline mode and a new, empty Cargo home, and statically builds the vendored OpenSSL version. On Linux it checks the ELF machine, absence of an interpreter and absence of dynamic library requirements. On macOS it inspects Mach-O load commands for the deployment target and permits only system libraries and frameworks. It packages the executable, extracts it again, and launches it over MCP with a disposable loopback Core fixture. The check verifies the reported MCP version, reads fixture inventory, and closes both processes. The rebuilt archive and its native execution receipt appear in `../rebuilt`. Different compiler tools or SDK revisions can produce different binary hashes.

## Candidate and release identities

The read-only **Native distribution** workflow builds the exact pull-request head or main commit. Its full run uses ubuntu-24.04, ubuntu-24.04-arm, macos-15-intel and macos-15. The container build consumes the verified Linux executables from the complete, same-commit native distribution. Each run first creates a single matching source archive. Its native jobs independently rebuild directly from that archive on their respective processors. Only successful extracted-binary checks feed the assembled candidate.

The Actions artifact name is `komodo-mcp-distribution-<full-commit>`. Its internal `distribution-manifest.json` records repository, source commit, Cargo version, Actions run ID and attempt, filename, byte count, SHA-256 and native executable SHA-256. Darwin entries also record the minimum OS version actually embedded in the executable. Obtain the artifact from the expected workflow, commit and run. A pre-release candidate carries the Cargo version that was built and is distinguished by that commit and run. Candidates are available to authorized workflow readers during retention. Published binaries, notices, checksums and Corresponding Source are release assets retained with the applicable version. Manual publication and recovery use the repository's release helper.
