"""Exercise the same Linux filesystem, network and process boundary as packaged stdio."""
import errno
import os
from pathlib import Path
import select
import socket
import subprocess
import sys
import tempfile
import unittest

from candidate_clients import native_probe_command


AVAILABLE = sys.platform == "linux" and Path("/usr/bin/bwrap").is_file() and os.geteuid() != 0


@unittest.skipUnless(AVAILABLE, "requires unprivileged Linux with Bubblewrap")
class NativeSandboxTests(unittest.TestCase):
    def test_verified_executable_has_private_filesystem_and_dies_with_its_wrapper(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            marker = root / "verifier-sentinel"
            marker.write_bytes(b"fixture")
            binary = root / "probe"
            source = r'''
#include <arpa/inet.h>
#include <fcntl.h>
#include <stdio.h>
#include <sys/prctl.h>
#include <unistd.h>
int main(int argc, char **argv) {
    if (argc != 2 || getuid() == 0 || prctl(PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) != 1) return 1;
    if (access(argv[1], F_OK) == 0 || access("/proc/1/root", F_OK) == 0
        || access("/var/run/docker.sock", F_OK) == 0 || access("/candidate", W_OK) == 0
        || access("/etc/ssl/certs/ca-certificates.crt", R_OK) != 0) return 2;
    if (open("/tmp/private-output", O_CREAT | O_EXCL | O_WRONLY, 0600) < 0) return 3;
    int sock = socket(AF_INET, SOCK_STREAM, 0);
    struct sockaddr_in addr = {.sin_family=AF_INET, .sin_addr.s_addr=htonl(INADDR_LOOPBACK)};
    socklen_t length = sizeof(addr);
    if (bind(sock, (void *)&addr, length) || getsockname(sock, (void *)&addr, &length)) return 4;
    printf("%u\n", ntohs(addr.sin_port)); fflush(stdout);
    for (;;) pause();
}
'''
            subprocess.run(["cc", "-static", "-x", "c", "-", "-o", binary], input=source, text=True, check=True)
            with subprocess.Popen([*native_probe_command(binary), marker], stdin=subprocess.DEVNULL,
                                  stdout=subprocess.PIPE, text=True) as process:
                try:
                    self.assertTrue(select.select([process.stdout], [], [], 10)[0], "sandbox preflight deadline")
                    port = int(process.stdout.readline())
                    with socket.socket() as conflict, self.assertRaises(OSError) as busy:
                        conflict.bind(("127.0.0.1", port))
                    self.assertEqual(busy.exception.errno, errno.EADDRINUSE)
                finally:
                    process.kill()
                    process.communicate(timeout=10)
            with socket.socket() as available:
                available.bind(("127.0.0.1", port))
            self.assertEqual(marker.read_bytes(), b"fixture")


if __name__ == "__main__":
    if "--required" in sys.argv:
        sys.argv.remove("--required")
        if not AVAILABLE:
            sys.exit("Linux Bubblewrap preflight is required on this runner")
    unittest.main()
