"""Run the packaged build procedure with only its source and toolchain mounted."""
import os
from pathlib import Path
import socket
import subprocess
import sys
import tarfile
import tempfile
import uuid


def run(source, archive, output, toolchain, revision, target):
    output.mkdir(mode=0o700)
    docker = ["docker", "--host", "unix:///var/run/docker.sock"]
    image = "komodo-candidate-source:" + uuid.uuid4().hex
    with tempfile.TemporaryFile() as rootfs, tempfile.TemporaryDirectory(dir=output.parent) as temporary:
        scratch = Path(temporary)
        (scratch / "home").mkdir()
        # An empty, local image needs only Ubuntu's /usr merge symlinks.
        with tarfile.open(fileobj=rootfs, mode="w") as tar:
            for name in ("bin", "lib", "lib64", "sbin"):
                link = tarfile.TarInfo(name)
                link.type, link.linkname = tarfile.SYMTYPE, "usr/" + name
                tar.addfile(link)
        rootfs.seek(0)
        subprocess.run([*docker, "import", "-", image], stdin=rootfs, stdout=subprocess.DEVNULL, check=True)
        try:
            args = [*docker, "run", "--rm", "--pull=never", "--read-only", "--network=none",
                    "--cap-drop=ALL", "--security-opt=no-new-privileges", "--user", f"{os.getuid()}:{os.getgid()}",
                    "--workdir", "/inputs/unpacked", "--env", "HOME=/tmp/home", "--env", "TMPDIR=/tmp",
                    "--env", "PATH=/opt/pinned-toolchain/bin:/usr/bin:/bin", "--env", "CARGO_NET_OFFLINE=true",
                    "--env", "CARGO_BUILD_JOBS=2", "--env", "PYTHONDONTWRITEBYTECODE=1"]
            mounts = [(Path("/usr"), "/usr", True), (Path("/etc/alternatives"), "/etc/alternatives", True),
                      (Path("/etc/ssl/certs/ca-certificates.crt"), "/etc/ssl/certs/ca-certificates.crt", True),
                      (toolchain, "/opt/pinned-toolchain", True), (source, "/inputs/unpacked", True),
                      (archive, "/inputs/source.tar.gz", True), (Path(__file__).resolve(), "/verify.py", True),
                      (scratch, "/tmp", False), (output, "/output", False)]
            for host, guest, readonly in mounts:
                if "," in str(host):
                    raise ValueError("unsupported comma in source build mount path")
                args += ["--mount", f"type=bind,src={host},dst={guest}" + (",readonly" if readonly else "")]
            # The trusted entry point checks the actual process environment before
            # handing control to the source-provided script. Its output stays private.
            with (output.parent / "private-rebuild.log").open("xb") as log:
                build = subprocess.run([*args, image, "/usr/bin/python3", "-I", "/verify.py", revision, target,
                                        str(archive.parent), str(output.parent / "oci"), str(Path(__file__).resolve().parents[1])],
                                       stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
                if build.returncode:
                    raise RuntimeError(f"isolated source build failed with exit code {build.returncode}")
        finally:
            subprocess.run([*docker, "image", "rm", image], stdout=subprocess.DEVNULL, check=True)


def inside(revision, target, hidden_paths):
    status = dict(line.split(":", 1) for line in Path("/proc/self/status").read_text().splitlines() if ":" in line)
    if (os.geteuid() == 0 or int(status["CapEff"], 16) != 0 or int(status["NoNewPrivs"]) != 1
            or [name for _, name in socket.if_nameindex()] != ["lo"]
            or any(Path(p).exists() for p in [*hidden_paths, "/var/run/docker.sock"])
            or any(not os.statvfs(p).f_flag & os.ST_RDONLY for p in
                   ("/", "/usr", "/etc/ssl/certs/ca-certificates.crt", "/opt/pinned-toolchain",
                    "/inputs/unpacked", "/inputs/source.tar.gz", "/verify.py"))
            or any(Path("/output").iterdir())):
        raise ValueError("source build sandbox does not match the required isolation")
    os.execv("/usr/bin/python3", ["/usr/bin/python3", "/inputs/unpacked/scripts/distribution_build.py",
             "--source", "/inputs/unpacked", "--source-archive", "/inputs/source.tar.gz", "--target", target,
             "--revision", revision, "--output", "/output"])


if __name__ == "__main__":
    inside(sys.argv[1], sys.argv[2], sys.argv[3:])
