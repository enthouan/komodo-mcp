"""Bounded stdio and authenticated HTTP peers for packaged-candidate regression."""
import asyncio
import hashlib
import hmac
import http.client
import json
import os
from pathlib import Path
import platform
import re
import secrets
import socket
import stat
import threading

from disposable_core_smoke import Failure, MAX_MCP_BYTES, Mcp, SECRETS, require


PROTOCOL = "2025-11-25"
CLIENT_INFO = {"name": "komodo-mcp-disposable-candidate", "version": "1"}
RPC_TIMEOUT = 35

def _loopback(value, path):
    """Only canonical literal loopback URLs with an explicit port and exact path."""
    match = re.fullmatch(r"http://(127\.0\.0\.1|\[::1\]):([1-9][0-9]{0,4})" + re.escape(path), value)
    require(match is not None and int(match[2]) <= 65535, "invalid fixture loopback endpoint")
    return match[1].strip("[]"), int(match[2])


def _validate_version(version):
    require(isinstance(version, str) and re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.+-]+)?", version),
            "expected artifact version is missing or invalid")


def _artifact(binary, digest):
    require(isinstance(binary, Path) and binary.is_absolute() and not binary.is_symlink()
            and isinstance(digest, str) and re.fullmatch("[0-9a-f]{64}", digest),
            "invalid verified executable reference")
    try:
        with binary.open("rb") as executable:
            require(stat.S_ISREG(os.fstat(executable.fileno()).st_mode) and os.access(binary, os.X_OK),
                    "fixture artifact is not executable")
            require(hmac.compare_digest(hashlib.file_digest(executable, "sha256").hexdigest(), digest),
                    "fixture executable digest mismatch")
    except OSError:
        raise Failure("cannot read the verified executable") from None


def _unpack(frame, expected, allow_error):
    require(isinstance(frame, dict) and frame.get("jsonrpc") == "2.0"
            and type(frame.get("id")) is int and frame["id"] == expected,
            "MCP response identity mismatch")
    require(("error" in frame) != ("result" in frame), "invalid MCP reply")
    if "error" in frame:
        error = frame["error"]
        require(isinstance(error, dict) and type(error.get("code")) is int, "invalid MCP protocol error")
        require(allow_error, "MCP protocol error")
        return {"protocol_error": error["code"]}
    require(isinstance(frame["result"], dict), "invalid MCP result")
    return frame["result"]


def _identity(result, version):
    require(isinstance(result, dict) and result.get("protocolVersion") == PROTOCOL
            and isinstance(info := result.get("serverInfo"), dict)
            and info.get("name") == "komodo-mcp" and info.get("version") == version,
            "installed MCP identity, version, or protocol mismatch")
    return info["version"]


def _init_params():
    return {"protocolVersion": PROTOCOL, "capabilities": {},
            "clientInfo": dict(CLIENT_INFO)}


class _Client(Mcp):
    """Same rpc/tool/context contract as the existing disposable smoke client."""

    def __init__(self):
        self.next_id = 0
        self.lock = asyncio.Lock()
        self.closed = False

    async def initialize(self, version):
        request = _init_params()
        reply = await self.rpc("initialize", request)
        self.version = _identity(reply, version)
        self.client_info = {**request["clientInfo"], "protocol": reply["protocolVersion"],
                            "python": platform.python_version()}

    async def rpc(self, method, params, allow_error=False):
        async with self.lock:
            require(not self.closed, "MCP client is closed")
            self.next_id += 1
            frame = {"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params}
            return _unpack(await self.exchange(frame), self.next_id, allow_error)

    async def tool(self, name, arguments):
        result = await self.rpc("tools/call", {"name": name, "arguments": arguments})
        require(not result.get("isError") and isinstance(result.get("structuredContent"), dict),
                "MCP tool failed or lacked structured output")
        return result["structuredContent"]


class NativeMcp(_Client):
    """Launch only the caller's extracted and digest-verified native executable."""

    @classmethod
    async def start(cls, *, binary, executable_sha256, version, address, credentials, environment):
        _validate_version(version)
        _loopback(address, "")
        require(set(credentials) == {"key", "secret"}
                and all(isinstance(v, str) and v and v.isascii() and v.isprintable() and not v.isspace()
                        for v in credentials.values())
                and credentials["key"] != credentials["secret"], "invalid scoped Core credentials")
        env = dict(environment, RUST_LOG="trace")
        env.update(KOMODO_ADDRESS=address, KOMODO_API_KEY=credentials["key"],
                   KOMODO_API_SECRET=credentials["secret"])
        await asyncio.to_thread(_artifact, binary, executable_sha256)
        self = cls()
        self.credential_bytes = tuple(v.encode() for v in (*credentials.values(), *SECRETS) if v)
        self.disclosed = False
        try:
            self.process = await asyncio.create_subprocess_exec(
                str(binary), env=env, stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE, limit=MAX_MCP_BYTES)
        except OSError:
            raise Failure("cannot launch verified native executable") from None
        self.drain = asyncio.create_task(self._scan_output(self.process.stderr))
        try:
            await self.initialize(version)
            await self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
            return self
        except BaseException:
            await self.close()
            raise

    async def interrupt(self):
        self.interrupted = True
        self.process.kill()
        await self.close()

    async def _scan_output(self, stream):
        overlap = max(map(len, self.credential_bytes)) - 1
        tail = b""
        while block := await stream.read(65536):
            window = tail + block
            self.disclosed |= any(secret in window for secret in self.credential_bytes)
            tail = window[-overlap:] if overlap else b""

    async def exchange(self, frame):
        try:
            async with asyncio.timeout(RPC_TIMEOUT):
                await self.send(frame)
                while True:
                    line = await self.process.stdout.readline()
                    require(line and len(line) <= MAX_MCP_BYTES, "invalid native MCP frame size or EOF")
                    require(all(secret not in line for secret in self.credential_bytes),
                            "native MCP reply disclosed a credential")
                    reply = json.loads(line)
                    if isinstance(reply, dict) and reply.get("jsonrpc") == "2.0" and "method" in reply and "id" not in reply:
                        continue
                    return reply
        except (ValueError, OSError, TimeoutError, RecursionError):
            raise Failure("native MCP framing or transport failure") from None

    async def close(self):
        if self.closed:
            return
        self.closed = True
        self.process.stdin.close()
        async def scan_remaining_stdout():
            async with self.lock:
                await self._scan_output(self.process.stdout)
        # Drain while waiting so a final notification can exceed the pipe
        # buffer without blocking shutdown or escaping the marker check.
        trailing = asyncio.create_task(scan_remaining_stdout())
        try:
            await asyncio.wait_for(self.process.wait(), 5)
        except TimeoutError:
            self.process.kill()
            await asyncio.wait_for(self.process.wait(), 5)
        finally:
            await asyncio.wait_for(asyncio.gather(self.drain, trailing), 5)
            self.credential_bytes = ()
        require(not self.disclosed, "native MCP output disclosed a credential or fixture marker")
        require(self.process.returncode == 0 or getattr(self, "interrupted", False), "native MCP failed to shut down cleanly")


def _parse_reply(content_type, payload, wanted):
    try:
        if content_type == "application/json":
            return json.loads(payload)
        require(content_type == "text/event-stream", "unexpected MCP response content type")
        blocks = payload.decode("utf-8").replace("\r\n", "\n").replace("\r", "\n").split("\n\n")
        for block in blocks:
            data = [s[5:].removeprefix(" ") for s in block.split("\n") if s.startswith("data:")]
            if not data or not "".join(data).strip():
                continue
            frame = json.loads("\n".join(data))
            require(isinstance(frame, dict), "malformed SSE MCP frame")
            if frame.get("id") == wanted:
                return frame
            require("id" not in frame and "method" in frame, "unexpected SSE response identity")
    except (ValueError, RecursionError):
        raise Failure("invalid MCP response encoding") from None
    raise Failure("missing MCP response in event stream")


class HttpMcp(_Client):
    """Connect to the verified candidate image at its loopback HTTP port."""

    @classmethod
    async def start(cls, *, endpoint, bearer, version):
        _validate_version(version)
        self = cls()
        self.host, self.port = _loopback(endpoint, "/mcp")
        require(isinstance(bearer, str) and len(bearer) <= 512 and
                re.fullmatch(r"[a-zA-Z0-9._~+/-]{32,}=*", bearer), "invalid isolated MCP bearer")
        self.bearer, self.session = bearer, None
        try:
            probe = {"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": _init_params()}
            for token in (None, secrets.token_urlsafe(32)):
                status, _, issued, challenge, _ = await self._request("POST", probe, token)
                require(status == 401 and issued is None and challenge == "Bearer",
                        "HTTP MCP did not reject the unauthenticated request")
            await self.initialize(version)
            status, _, _, _, _ = await self._request("POST", {"jsonrpc": "2.0", "method": "notifications/initialized"}, bearer)
            require(status == 202, "HTTP MCP initialization notification failed")
            return self
        except BaseException:
            await self.close()
            raise

    async def _request(self, method, frame, bearer):
        return await asyncio.to_thread(self._request_sync, method, frame, bearer)

    def _request_sync(self, method, frame, bearer):
        headers = {"Accept": "application/json, text/event-stream", "MCP-Protocol-Version": PROTOCOL}
        if bearer:
            headers["Authorization"] = "Bearer " + bearer
        if self.session:
            headers["Mcp-Session-Id"] = self.session
        body = None
        if frame is not None:
            body = json.dumps(frame, separators=(",", ":")).encode()
            require(len(body) < MAX_MCP_BYTES, "MCP request exceeded fixture size limit")
            headers["Content-Type"] = "application/json"
        connection = http.client.HTTPConnection(self.host, self.port, timeout=RPC_TIMEOUT)
        timer = None
        try:
            connection.connect()
            transport = connection.sock

            def expire():
                try:
                    transport.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass

            timer = threading.Timer(RPC_TIMEOUT, expire)
            timer.daemon = True
            timer.start()
            connection.request(method, "/mcp", body=body, headers=headers)
            with connection.getresponse() as response:
                data = response.read(MAX_MCP_BYTES + 1)
                require(len(data) <= MAX_MCP_BYTES, "HTTP MCP response exceeded fixture size limit")
                require(not any(value.encode() in data for value in SECRETS if value),
                        "HTTP MCP response disclosed a fixture credential or marker")
                return (response.status, response.headers.get_content_type(),
                        response.getheader("Mcp-Session-Id"), response.getheader("WWW-Authenticate"), data)
        except (ValueError, OSError, http.client.HTTPException):
            raise Failure("bounded loopback MCP HTTP request failed") from None
        finally:
            if timer:
                timer.cancel()
                timer.join(timeout=1)
            connection.close()

    async def exchange(self, frame):
        status, media_type, session, _, data = await self._request("POST", frame, self.bearer)
        require(status == 200, "unexpected authenticated MCP HTTP status")
        if frame["method"] == "initialize":
            require(isinstance(session, str) and re.fullmatch(r"[!-~]{1,256}", session) and "," not in session,
                    "invalid HTTP MCP session")
            self.session = session
        else:
            require(session is None or session == self.session, "MCP session identity changed")
        return _parse_reply(media_type, data, frame["id"])

    async def close(self):
        if self.closed:
            return
        self.closed = True
        try:
            if self.session:
                status, _, _, _, _ = await self._request("DELETE", None, self.bearer)
                require(status in (200, 202, 204), "HTTP MCP session deletion failed")
        finally:
            self.session = self.bearer = None
