"""Exercise a registry-free candidate on its native Linux Docker host against a disposable TLS Core."""

import argparse
import collections
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import ipaddress
import json
import os
from pathlib import Path
import platform
import secrets
import ssl
import subprocess
import tempfile
import threading
import tomllib

from container_release import ARCHIVE, CA, RECEIPT, ROOT, check, sha_file, validate_receipt

PROTOCOL = "2025-11-25"
TOOLS = {"list_stacks", "get_stack", "stack_diagnose"}


def tls_fixture(directory, gateway, key, secret):
    cert, private = directory / "ca.pem", directory / "ca-key.pem"
    leaf, leaf_key, request = directory / "leaf.pem", directory / "leaf-key.pem", directory / "leaf.csr"
    extensions = directory / "extensions"
    extensions.write_text(
        "basicConstraints=critical,CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\n"
        "extendedKeyUsage=serverAuth\nsubjectAltName=DNS:fixture-core\n"
    )

    def openssl(*args):
        subprocess.run(["openssl", *args], check=True, capture_output=True, timeout=20)

    openssl(
        "req",
        "-x509",
        "-newkey",
        "rsa:2048",
        "-sha256",
        "-nodes",
        "-days",
        "1",
        "-subj",
        "/CN=Ephemeral fixture CA",
        "-addext",
        "basicConstraints=critical,CA:TRUE",
        "-addext",
        "keyUsage=critical,keyCertSign,cRLSign",
        "-keyout",
        private,
        "-out",
        cert,
    )
    openssl(
        "req",
        "-new",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-subj",
        "/CN=fixture-core",
        "-keyout",
        leaf_key,
        "-out",
        request,
    )
    openssl(
        "x509",
        "-req",
        "-days",
        "1",
        "-in",
        request,
        "-CA",
        cert,
        "-CAkey",
        private,
        "-CAcreateserial",
        "-extfile",
        extensions,
        "-out",
        leaf,
    )
    cert.chmod(0o644)
    observed = collections.Counter()

    class Core(BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_POST(self):
            supplied = self.headers.get("x-api-key"), self.headers.get("x-api-secret")
            length = int(self.headers.get("Content-Length", "0"))
            if self.path != "/read" or supplied != (key, secret) or not 0 < length <= 65536:
                self.send_error(403)
                return
            name = json.loads(self.rfile.read(length)).get("type")
            replies = {"GetVersion": {"version": "2.3.2"}, "ListStacks": []}
            if name not in replies:
                self.send_error(404)
                return
            observed[name] += 1
            data = json.dumps(replies[name]).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

    server = ThreadingHTTPServer((gateway, 0), Core)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(leaf, leaf_key)
    server.socket = context.wrap_socket(server.socket, server_side=True)
    return server, cert, observed


def mcp(port, token, version):
    session = None

    def send(method, frame=None, *, authenticated=True):
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
        headers = {"Accept": "application/json, text/event-stream", "Mcp-Protocol-Version": PROTOCOL}
        if authenticated:
            headers["Authorization"] = "Bearer " + token
        if session:
            headers["Mcp-Session-Id"] = session
        if frame:
            headers["Content-Type"] = "application/json"
        try:
            conn.request(method, "/mcp", json.dumps(frame) if frame else None, headers)
            with conn.getresponse() as response:
                data = response.read(2**20 + 1)
                check(
                    len(data) <= 2**20 and token.encode() not in data,
                    "invalid MCP response size or privacy",
                )
                return response.status, response.headers, data
        finally:
            conn.close()

    def rpc(identifier, method, params):
        status, headers, body = send(
            "POST", {"id": identifier, "jsonrpc": "2.0", "method": method, "params": params}
        )
        check(status == 200, "authenticated MCP request failed")
        if headers.get_content_type() == "application/json":
            frames = [json.loads(body)]
        else:
            check(headers.get_content_type() == "text/event-stream", "unexpected MCP response media type")
            frames = [
                json.loads(line[5:])
                for line in body.splitlines()
                if line.startswith(b"data:") and line[5:].strip()
            ]
        results = [f["result"] for f in frames if f.get("id") == identifier and "result" in f]
        check(len(results) == 1, "missing or duplicate MCP response")
        return results[0], headers

    status, headers, _ = send("GET", authenticated=False)
    check(
        status == 401 and headers.get("WWW-Authenticate") == "Bearer",
        "MCP listener failed unauthenticated check",
    )
    initialized, headers = rpc(
        1,
        "initialize",
        {
            "protocolVersion": PROTOCOL,
            "capabilities": {},
            "clientInfo": {"name": "container-fixture", "version": "1"},
        },
    )
    identity = initialized["serverInfo"]
    check(
        initialized["protocolVersion"] == PROTOCOL
        and identity["name"] == "komodo-mcp"
        and identity["version"] == version,
        "image reported unexpected MCP identity/version",
    )
    session = headers.get("Mcp-Session-Id")
    check(session, "missing MCP session")
    check(
        send("POST", {"jsonrpc": "2.0", "method": "notifications/initialized"})[0] == 202,
        "initialization failed",
    )
    listing, _ = rpc(2, "tools/list", {})
    check({tool["name"] for tool in listing["tools"]} == TOOLS, "unexpected tool selection")
    read, _ = rpc(3, "tools/call", {"name": "list_stacks", "arguments": {}})
    check(
        not read.get("isError") and read["structuredContent"]["items"] == [],
        "authenticated Core read failed",
    )
    check(send("DELETE")[0] in (200, 202, 204), "MCP session cleanup failed")


def run(candidate, expected_platform, runner_temp):
    native = {"x86_64": "linux/amd64", "aarch64": "linux/arm64"}
    check(
        platform.system() == "Linux" and native.get(platform.machine()) == expected_platform,
        "candidate runtime checks require the matching native Linux runner",
    )

    def docker(*args, env=None, timeout=120, must_succeed=True):
        result = subprocess.run(["docker", *args], env=env, capture_output=True, timeout=timeout)
        if must_succeed:
            check(result.returncode == 0, "Docker command failed: " + str(args[0]))
        return result

    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    revision = subprocess.check_output(["git", "-C", ROOT, "rev-parse", "HEAD"], text=True).strip()
    receipt = validate_receipt(
        json.loads((candidate / RECEIPT).read_bytes()), revision=revision, version=version
    )
    archive = candidate / ARCHIVE
    check(sha_file(archive) == receipt["archive"]["sha256"], "downloaded candidate archive differs")
    docker("image", "load", "--input", archive)
    ref = "komodo-mcp-candidate:git-" + revision
    selected = receipt["platforms"][expected_platform]
    image = json.loads(docker("image", "inspect", "--platform", expected_platform, ref).stdout)[0]
    descriptor = image.get("Descriptor", {}).get("digest")
    check(
        descriptor == selected["manifest_digest"]
        if descriptor
        else image["Id"] == selected["config_digest"],
        "loaded Docker platform differs from the verified candidate",
    )
    # Use the runner's Docker bridge, keeping the temporary authenticated fixture off its public interfaces.
    bridge = json.loads(docker("network", "inspect", "bridge").stdout)[0]
    gateway = next(
        item["Gateway"]
        for item in bridge["IPAM"]["Config"]
        if ipaddress.ip_address(item["Gateway"]).version == 4
    )
    env = {k: v for k, v in os.environ.items() if not k.startswith(("KOMODO_", "COMPOSE_"))}
    key, secret, bearer = (secrets.token_urlsafe(32) for _ in range(3))
    env.update(
        KOMODO_API_KEY=key,
        KOMODO_API_SECRET=secret,
        KOMODO_HTTP_TOKENS=bearer,
        KOMODO_MCP_IMAGE=ref,
        KOMODO_ENABLED_TOOLS=",".join(sorted(TOOLS)),
    )
    with tempfile.TemporaryDirectory(dir=runner_temp) as tmp:
        directory = Path(tmp)
        server, ca, observed = tls_fixture(directory, gateway, key, secret)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        port = server.server_address[1]
        env["KOMODO_ADDRESS"] = f"https://fixture-core:{port}"
        hosts = ["fixture-core:host-gateway", "wrong-core:host-gateway"]
        overlay = directory / "fixture.json"
        overlay.write_text(json.dumps({"services": {"komodo-mcp": {"extra_hosts": hosts}}}))
        project = "komodo-candidate-" + secrets.token_hex(6)
        compose = [
            "compose",
            "--project-name",
            project,
            "--env-file",
            os.devnull,
            "-f",
            ROOT / "compose.yaml",
            "-f",
            overlay,
        ]
        try:
            model = json.loads(docker(*compose, "config", "--format", "json", env=env).stdout)["services"][
                "komodo-mcp"
            ]
            check(
                len(model["ports"]) == 1
                and model["ports"][0]["host_ip"] == "127.0.0.1"
                and str(model["ports"][0]["published"]) == "3000",
                "default port is not loopback-only",
            )
            env["KOMODO_HTTP_PORT"] = "0"
            result = docker(
                *compose,
                "run",
                "--rm",
                "--no-deps",
                "-T",
                "komodo-mcp",
                env=env,
                must_succeed=False,
                timeout=30,
            )
            check(result.returncode != 0 and not observed, "private CA was accepted by default")
            overlay.write_text(
                json.dumps(
                    {"services": {"komodo-mcp": {"extra_hosts": hosts, "volumes": [f"{ca}:{CA}:ro"]}}}
                )
            )
            result = docker(
                *compose,
                "run",
                "--rm",
                "--no-deps",
                "-T",
                "-e",
                f"KOMODO_ADDRESS=https://wrong-core:{port}",
                "komodo-mcp",
                env=env,
                must_succeed=False,
                timeout=30,
            )
            check(
                result.returncode != 0 and not observed, "fixture with incorrect TLS hostname was accepted"
            )
            docker(
                *compose,
                "up",
                "--pull",
                "never",
                "-d",
                "--wait",
                "--wait-timeout",
                "60",
                "komodo-mcp",
                env=env,
            )
            cid = docker(*compose, "ps", "--quiet", "komodo-mcp", env=env).stdout.decode().strip()
            running = json.loads(docker("inspect", cid).stdout)[0]
            runtime = running.get("ImageManifestDescriptor", {}).get("digest")
            check(
                runtime == selected["manifest_digest"]
                if runtime
                else running["Image"] in {image["Id"], selected["config_digest"]},
                "running container image differs",
            )
            check(
                running["Config"]["User"] == "65532:65532"
                and running["HostConfig"]["ReadonlyRootfs"]
                and running["State"]["Health"]["Status"] == "healthy",
                "container runtime/health mismatch",
            )
            exposed = docker(*compose, "port", "komodo-mcp", "3000", env=env).stdout.decode().strip()
            host, host_port = exposed.rsplit(":", 1)
            check(host == "127.0.0.1", "unexpected Compose port")
            mcp(int(host_port), bearer, version)
            check(
                observed["GetVersion"] >= 1 and observed["ListStacks"] >= 1,
                "Core did not observe authenticated reads",
            )
        finally:
            try:
                docker(*compose, "down", "--remove-orphans", env=env)
            finally:
                server.shutdown()
                thread.join(timeout=5)
                server.server_close()
    print(
        json.dumps(
            {
                "platform": expected_platform,
                "execution": "native",
                "image": receipt["image"],
                "version": version,
                "revision": revision,
                "index": receipt["index_digest"],
                "manifest": selected["manifest_digest"],
                "config": selected["config_digest"],
                "executable_sha256": selected["executable_sha256"],
                "compose_health": "healthy",
                "authenticated_read": "list_stacks",
                "tls": "verified",
            }
        )
    )


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--platform", required=True)
    args = parser.parse_args()
    run(args.candidate, args.platform, os.environ.get("RUNNER_TEMP"))
