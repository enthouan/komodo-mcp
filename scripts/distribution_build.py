"""Build and exercise a native distribution from the extracted, vendored source. Python 3.11+."""
import argparse
import ctypes
import errno
import json
import os
from pathlib import Path
import platform
import re
import shutil
import shlex
import subprocess
import sys
import tempfile
import tomllib

from distribution import REPOSITORY, TARGETS, require, sha256, stem
from distribution_archive import archive, extract


def sysctl_int(name):
    """Read a Darwin integer sysctl while preserving ENOENT distinctly from probe failures."""
    sysctl = ctypes.CDLL(None, use_errno=True).sysctlbyname
    sysctl.argtypes = [ctypes.c_char_p, ctypes.c_void_p, ctypes.POINTER(ctypes.c_size_t),
                       ctypes.c_void_p, ctypes.c_size_t]
    sysctl.restype = ctypes.c_int
    value = ctypes.c_int()
    size = ctypes.c_size_t(ctypes.sizeof(value))
    if sysctl(name.encode("ascii"), ctypes.byref(value), ctypes.byref(size), None, 0) != 0:
        code = ctypes.get_errno()
        raise OSError(code, os.strerror(code))
    require(size.value == ctypes.sizeof(value), "unexpected sysctl value size")
    return value.value


def native_platform(target):
    require(target in TARGETS, "unsupported distribution target")
    arch = {"arm64":"aarch64", "aarch64":"aarch64", "x86_64":"x86_64"}.get(platform.machine())
    system = "Darwin" if target.endswith("-darwin") else "Linux"
    require(platform.system() == system and arch == target.split("-")[0], "a native target host is required")
    if system == "Darwin":
        try:
            translated = sysctl_int("sysctl.proc_translated")
        except OSError as error:
            if error.errno != errno.ENOENT:
                raise
            translated = 0
        require(translated == 0, "Rosetta cannot provide native execution evidence")


def image_linkage(binary, target):
    def output(*args):
        return subprocess.check_output([*args, binary], text=True)
    if target.endswith("-musl"):
        headers, programs, dynamic = output("readelf","-hW"), output("readelf","-lW"), output("readelf","-dW")
        machine = "AArch64" if target.startswith("aarch64") else "Advanced Micro Devices X86-64"
        require(re.search(r"Machine:\s+" + re.escape(machine) + r"\s*$", headers, re.M)
                and " INTERP " not in programs and not re.search(r"^\s*INTERP\s", programs, re.M)
                and "(NEEDED)" not in dynamic, "Linux binary has the wrong architecture or dynamic dependencies")
        return {}
    arch = "arm64" if target.startswith("aarch64") else "x86_64"
    require(output("lipo","-archs").strip() == arch, "wrong Mach-O architecture")
    for line in output("otool","-L").splitlines()[1:]:
        dependency = line.strip().split(" (", 1)[0]
        require(dependency.startswith(("/usr/lib/", "/System/Library/Frameworks/")), "non-system Mach-O dependency")
    loads = output("otool","-l")
    require("cmd LC_RPATH" not in loads, "unexpected Mach-O runpath")
    match = re.search(r"cmd LC_BUILD_VERSION\b(?:(?!Load command).)*?\bminos\s+(\d+\.\d+(?:\.\d+)?)", loads, re.S)
    require(match is not None and match.group(1) in ("15.0","15.0.0"), "unexpected Mach-O deployment target")
    return {"minimum_macos":match.group(1)}


def musl_inputs(link_map, target_libdir):
    """The private linker map identifies the contributing libc and startup objects."""
    bundled = (Path(target_libdir)/"self-contained").resolve()
    startup = set()
    archives = set()
    checked_paths = set()
    builtins = unwind = False
    names = {"libc.a", "libunwind.a", "libgcc.a", "libgcc_eh.a"}
    with Path(link_map).open(errors="replace") as stream:
        for line in stream:
            builtins |= "libcompiler_builtins-" in line
            unwind |= "libunwind-" in line
            if "crt1.o" in line:
                startup.update(name for name in ("crt1.o","Scrt1.o","rcrt1.o") if str(bundled/name) in line)
            if any(name+"(" in line for name in names):
                for token in line.split():
                    lib, separator, _ = token.partition("(")
                    if separator and lib not in checked_paths and lib.rsplit("/",1)[-1] in names:
                        path = Path(lib)
                        require(path.parent.resolve() == bundled, "unaccounted C or unwind runtime in native binary")
                        checked_paths.add(lib)
                        archives.add(path.name)
    require("libc.a" in archives, "musl libc did not come from the pinned Rust target")
    require(len(startup) == 1, "missing or ambiguous Rust target startup object")
    return {"libc":"rust-target/self-contained/libc.a","startup":"rust-target/self-contained/"+startup.pop(),
            "compiler_builtins":builtins,"libunwind":"libunwind.a" in archives or unwind}


def build(source, output, target, *, revision=None, source_archive=None):
    source, output = source.resolve(), output.resolve()
    native_platform(target)
    info = json.loads((source / "DISTRIBUTION-INFO.json").read_bytes())
    version = tomllib.loads((source/"Cargo.toml").read_text())["package"]["version"]
    toolchain = tomllib.loads((source/"rust-toolchain.toml").read_text())["toolchain"]["channel"]
    require(info["repository"] == REPOSITORY and info["version"] == version
            and re.fullmatch(r"[a-f0-9]{40}",info["revision"]) and (revision is None or info["revision"] == revision),
            "matching-source identity mismatch")
    compiler = subprocess.check_output(["rustc","-V"],text=True).split()
    require(compiler[:2] == ["rustc",toolchain], "install the pinned Rust toolchain first")
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="komodo-dist-") as temporary:
        tmp = Path(temporary).resolve()
        cargo_home = tmp / "cargo-home"
        cargo_home.mkdir()
        build_dir = tmp / "build"
        sysroot = Path(subprocess.check_output(["rustc","--print","sysroot"],text=True).strip()).resolve()
        replacements = sorted([(source,"/source"), (tmp,"/build"), (sysroot,"/rust"), (Path.home(),"/home/builder")],
                              key=lambda item:len(str(item[0])))
        rustflags = [f"--remap-path-prefix={path}={name}" for path,name in replacements]
        # Cargo's encoded representation also handles workspaces whose names contain spaces.
        cflags = [f"-ffile-prefix-map={path}={name}" for path,name in replacements]
        env = os.environ.copy()
        # OpenSSL embeds its CFLAGS in its version metadata. Add path remapping inside the
        # compiler launcher so even its compiler description contains only the portable name.
        compiler_dir = tmp / "compilers"
        compiler_dir.mkdir()
        for variable, compiler in (("CC","musl-gcc" if target.endswith("-musl") else "clang"),
                                   ("CXX","c++" if target.endswith("-musl") else "clang++")):
            launcher = compiler_dir / ("distribution-" + compiler)
            executable = shutil.which(compiler)
            require(executable, f"missing native compiler {compiler}")
            launcher.write_text("#!/bin/sh\nexec " + shlex.join([executable,*cflags]) + ' "$@"\n')
            launcher.chmod(0o755)
            env[variable+"_"+target.replace("-","_")] = launcher.name
        env["PATH"] = str(compiler_dir) + os.pathsep + env["PATH"]
        env.update(CARGO_HOME=str(cargo_home), CARGO_TARGET_DIR=str(build_dir), CARGO_NET_OFFLINE="true",
                   CARGO_ENCODED_RUSTFLAGS="\x1f".join(rustflags),
                   CARGO_PROFILE_RELEASE_STRIP="symbols", CARGO_PROFILE_RELEASE_DEBUG="0",
                   OPENSSL_NO_VENDOR="0", OPENSSL_STATIC="1", OPENSSL_CONFIG_DIR="/etc/ssl",
                   SOURCE_DATE_EPOCH="0")
        if target.endswith("-musl"):
            env["CARGO_TARGET_" + target.replace("-","_").upper() + "_LINKER"] = "musl-gcc"
        else:
            env["MACOSX_DEPLOYMENT_TARGET"] = "15.0"
        flags = ["--frozen","--release","--features","distribution","--target",target]
        runtime = {}
        build_command = ["cargo", "build", *flags, "--bin", "komodo-mcp", "--message-format=json-render-diagnostics"]
        if target.endswith("-musl"):
            target_libdir = subprocess.check_output(["rustc","--print","target-libdir","--target",target],text=True).strip()
            link_map = tmp/"komodo-mcp.map"
            build_command[1] = "rustc"
            build_command.extend(["--", "-C", "link-self-contained=yes", "-C", f"link-arg=-Wl,-Map={link_map}"])
        events = subprocess.check_output(build_command, cwd=source, env=env, text=True)
        compiled = set()
        for line in events.splitlines():
            message = json.loads(line)
            if message.get("reason") == "compiler-artifact":
                package = Path(message["manifest_path"]).resolve().parent
                if package != source:
                    require(package.parent == source / "vendor", "unexpected non-vendored build input")
                    compiled.add(package.name)
        require(compiled, "missing binary build input inventory")
        if target.endswith("-musl"):
            runtime = musl_inputs(link_map,target_libdir)
        subprocess.run(["cargo","build",*flags,"--example","release_version"],cwd=source,env=env,check=True)
        require(not any((cargo_home/name).exists() for name in ("registry/index","registry/src","registry/cache","git/db")),
                "offline rebuild populated a dependency cache")
        release = build_dir/target/"release"
        bundle = tmp / stem(version,target)
        bundle.mkdir()
        (bundle/"BUILD-INPUTS.json").write_text(json.dumps(sorted(compiled))+"\n")
        binary = bundle / "komodo-mcp"
        shutil.copyfile(release/"komodo-mcp", binary)
        binary.chmod(0o755)
        data = binary.read_bytes()
        for path,_ in replacements:
            require(os.fsencode(path) not in data, "native executable contains a build-machine path")
        require(not re.search(rb"/(?:Users/[^/\x00 ]+|home/runner|private/tmp/komodo-dist-[a-z0-9_]+)(?:/|\x00)",data),
                "native executable contains a workstation or runner path")
        linkage = image_linkage(binary,target)
        for name in ("LICENSE","THIRD-PARTY-NOTICES"):
            shutil.copyfile(source/name,bundle/name)
        (bundle/"README.md").write_text(
            f"# komodo-mcp {version}\n\nTarget: {target}\nRevision: {info['revision']}\n\n"
            "Verify this archive against the SHA256SUMS from the same distribution. Extract it and configure "
            "your MCP client to launch the included komodo-mcp executable using stdio. Supply KOMODO_ADDRESS, "
            "KOMODO_API_KEY and KOMODO_API_SECRET in the client environment.\n\n"
            f"Matching source: komodo-mcp-v{version}-source.tar.gz from the same distribution. "
            f"Build and install instructions: https://github.com/{REPOSITORY}/blob/{info['revision']}/docs/distribution.md\n")
        tar = output / (bundle.name+".tar.gz")
        archive(bundle,tar)
        extracted = extract(tar,tmp/"extracted",bundle.name)/"komodo-mcp"
        require(sha256(extracted) == sha256(binary), "extracted executable digest mismatch")
        run = subprocess.run([release/"examples/release_version",extracted,version],capture_output=True,text=True,
                             timeout=120,check=True)
        result = json.loads(run.stdout)
        require(result["observed_version"] == version and result["child_exit_success"]
                and result["mock_stopped"] and not result["forced_kill"], "extracted MCP smoke failed")
        entry = {"kind":"native","name":tar.name,"target":target,"bytes":tar.stat().st_size,
                 "sha256":sha256(tar),"executable_sha256":sha256(extracted),**linkage}
        (output/(target+".json")).write_text(json.dumps({"revision":info["revision"],"version":version,
                    "source_sha256":sha256(source_archive) if source_archive else None,
                    "native":True,"mcp_version":result["observed_version"],"runtime":runtime,"file":entry},indent=2)+"\n")
        print(f"Verified extracted {target} MCP {version} at {info['revision']} ({entry['sha256']})")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=Path.cwd())
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--revision")
    parser.add_argument("--source-archive", type=Path, help="exact downloaded archive used for this candidate")
    args = parser.parse_args()
    build(args.source, args.output, args.target, revision=args.revision, source_archive=args.source_archive)
