"""Bounded newline-delimited JSON-RPC client for actual stdio child processes."""

import asyncio
import contextlib
import json
import os
import signal
import time

from core import KEY, SECRET, STATES, digest, encoded

PROTOCOL = "2025-03-26"  # Supported by both rmcp 3.8.0 and the pinned JS SDK.
STATE_MAP = {state: state for state in STATES}  # Explicit identity enum mapping.


def project(items):
    if not isinstance(items, list):
        raise ValueError("inventory items must be an array")
    result = []
    for item in items:
        if (not isinstance(item, dict) or not isinstance(item.get("id"), str)
                or not item["id"] or not isinstance(item.get("name"), str) or not item["name"]):
            raise ValueError("invalid inventory identity")
        state = item.get("state")
        if not isinstance(state, str) or state not in STATE_MAP:
            raise ValueError(f"unknown/missing stack state: {state!r}")
        result.append({"id": item["id"], "name": item["name"], "state": STATE_MAP[state]})
    return result


def verify(items, expected):
    ids = [item["id"] for item in items]
    if len(set(ids)) != len(ids):
        raise ValueError("duplicate stack IDs")
    ordered = sorted(items, key=lambda x: x["id"])
    if ordered != sorted(expected, key=lambda x: x["id"]):
        raise ValueError("inventory differs from independent fixture oracle")
    return {"count": len(items), "sha256": digest(ordered), "verified": True}


def child_env(home, address):
    # An allowlist: no inherited credentials, proxies, NODE_OPTIONS, telemetry or configs.
    return {"PATH": "/usr/bin:/bin", "HOME": str(home), "XDG_CONFIG_HOME": str(home),
            "LANG": "C", "TZ": "UTC", "KOMODO_ADDRESS": address,
            "KOMODO_URL": address, "KOMODO_API_KEY": KEY, "KOMODO_API_SECRET": SECRET,
            "MCP_TRANSPORT": "stdio", "RUST_LOG": "warn", "LOG_LEVEL": "warn",
            "MCP_CONFIG_FILE_PATH": str(home / "config.toml")}


class Client:
    def __init__(self, argv, env, cwd, timeout=20):
        self.argv, self.env, self.cwd, self.timeout = argv, env, cwd, timeout
        self.process = None
        self.tasks = []
        self.pending = {}
        self.next_id = 0
        self.notifications = {}
        self.stderr = bytearray()
        self.stderr_bytes = 0
        self.failure = None

    async def spawn(self):
        spawning = asyncio.create_task(asyncio.create_subprocess_exec(
            *self.argv, env=self.env, cwd=self.cwd, start_new_session=True,
            stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE, limit=16 * 1024 * 1024,
        ))
        try:
            self.process = await asyncio.shield(spawning)
        except BaseException:
            # A cancelled spawn may already have created the OS process. Recover
            # its handle before propagating cancellation so cleanup can reap it.
            self.process = await spawning
            await self.close(failed=True)
            raise
        self.tasks = [asyncio.create_task(self.read_stdout()),
                      asyncio.create_task(self.read_stderr())]

    async def read_stderr(self):
        while chunk := await self.process.stderr.read(65536):
            self.stderr_bytes += len(chunk)
            self.stderr.extend(chunk[:max(0, 16384 - len(self.stderr))])

    async def read_stdout(self):
        try:
            while line := await self.process.stdout.readline():
                if not line.endswith(b"\n"):
                    raise ValueError("unterminated MCP frame")
                message = json.loads(line)
                if not isinstance(message, dict) or message.get("jsonrpc") != "2.0":
                    raise ValueError("invalid JSON-RPC envelope")
                if "id" not in message and isinstance(message.get("method"), str):
                    method = message["method"]
                    if len(self.notifications) >= 100 and method not in self.notifications:
                        raise ValueError("too many distinct MCP notification methods")
                    totals = self.notifications.setdefault(method, {"count": 0, "bytes": 0})
                    totals["count"] += 1
                    totals["bytes"] += len(line)
                    continue
                future = self.pending.get(message.get("id"))
                if future is None or future.done() or "method" in message:
                    raise ValueError("unmatched or duplicate MCP response")
                if ("result" in message) == ("error" in message):
                    raise ValueError("response must contain exactly one of result/error")
                future.set_result((message, len(line)))
            raise EOFError("MCP stdout closed")
        except Exception as error:
            self.failure = error
            for future in self.pending.values():
                if not future.done():
                    future.set_exception(error)

    async def send(self, message):
        try:
            self.process.stdin.write(encoded(message) + b"\n")
            await self.process.stdin.drain()
        except (BrokenPipeError, ConnectionResetError) as error:
            if self.failure is not None:
                raise self.failure
            raise EOFError("MCP stdin closed") from error

    async def request(self, method, params):
        if self.failure:
            raise self.failure
        self.next_id += 1
        request_id = str(self.next_id)
        future = asyncio.get_running_loop().create_future()
        self.pending[request_id] = future
        try:
            async with asyncio.timeout(self.timeout):
                await self.send({"jsonrpc": "2.0", "id": request_id,
                                 "method": method, "params": params})
                message, size = await future
            if "error" in message:
                raise ValueError(f"MCP error: {message['error']}")
            if not isinstance(message["result"], dict):
                raise ValueError("MCP result must be an object")
            return message["result"], size
        finally:
            self.pending.pop(request_id, None)
            if future.done() and not future.cancelled():
                # A failed write can race the reader failing this same request.
                future.exception()

    async def initialize(self):
        # No driver/mock preparation between this timestamp and actual process spawn.
        start = time.perf_counter_ns()
        async with asyncio.timeout(self.timeout):
            await self.spawn()
            result, size = await self.request("initialize", {
                "protocolVersion": PROTOCOL, "capabilities": {},
                "clientInfo": {"name": "komodo-inventory-benchmark", "version": "1"},
            })
            if (result.get("protocolVersion") != PROTOCOL
                    or not isinstance(result.get("serverInfo", {}).get("name"), str)
                    or not isinstance(result.get("serverInfo", {}).get("version"), str)
                    or not isinstance(result.get("capabilities", {}).get("tools"), dict)):
                raise ValueError("invalid initialize response or unsupported protocol")
            elapsed = (time.perf_counter_ns() - start) / 1e6
            await self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        return {"elapsed_ms": elapsed, "protocol": result["protocolVersion"],
                "response_bytes": size, "server_info": result["serverInfo"]}

    async def inventory(self, implementation, page_size, expected):
        if implementation == "rust_reference":
            from rust_reference import Reader
            reader = Reader(self)
            start = time.perf_counter_ns()
            async with asyncio.timeout(self.timeout):
                items = await reader.inventory(page_size, len(expected))
            elapsed = (time.perf_counter_ns() - start) / 1e6
            return {"elapsed_ms": elapsed, "mcp_calls": reader.calls, "response_bytes": reader.response_bytes,
                    "equivalence": verify(items, expected)}
        page, cursor, seen_cursors, items = 0, None, set(), []
        wire_bytes, calls = 0, 0
        start = time.perf_counter_ns()
        # At most one empty terminal page beyond the fixture's page count.
        bound = max(1, (len(expected) + page_size - 1) // page_size) + 1
        async with asyncio.timeout(self.timeout):
            for _ in range(bound):
                arguments = {"page_size": page_size}
                if implementation == "rust":
                    arguments["page"] = page
                elif cursor is not None:
                    arguments["cursor"] = cursor
                result, size = await self.request("tools/call", {
                    "name": "list_stacks" if implementation == "rust" else "komodo_stack_list",
                    "arguments": arguments,
                })
                if result.get("isError", False) is not False:
                    raise ValueError("inventory tool returned an error")
                payload = result.get("structuredContent")
                if not isinstance(payload, dict):
                    raise ValueError("missing structured inventory result")
                chunk = project(payload.get("items"))
                if len(chunk) > page_size:
                    raise ValueError("page exceeds requested size")
                items.extend(chunk)
                calls += 1
                wire_bytes += size
                if implementation == "rust":
                    more = payload.get("has_more")
                    if (type(more) is not bool or payload.get("page") != page
                            or payload.get("page_size") != page_size
                            or payload.get("next_page") != (page + 1 if more else None)):
                        raise ValueError("malformed Rust pagination")
                    if not more:
                        break
                    page += 1
                else:
                    envelope = payload.get("page")
                    if not isinstance(envelope, dict) or envelope.get("total") != len(expected):
                        raise ValueError("malformed reference pagination/total")
                    cursor = envelope.get("next_cursor")
                    if cursor is None:
                        break
                    if not isinstance(cursor, str) or not cursor or cursor in seen_cursors:
                        raise ValueError("invalid/repeated reference cursor")
                    seen_cursors.add(cursor)
                if len(chunk) != page_size:
                    raise ValueError("nonterminal page is not full")
            else:
                raise ValueError("inventory pagination exceeded bound")
        elapsed = (time.perf_counter_ns() - start) / 1e6
        # Oracle sorting/comparison and report generation are outside the timer.
        return {"elapsed_ms": elapsed, "mcp_calls": calls, "response_bytes": wire_bytes,
                "equivalence": verify(items, expected)}

    async def close(self, failed=False):
        if self.process is None:
            return
        process = self.process
        process.stdin.close()
        # EOF is the normal stdio shutdown. Reference monitoring may keep Node alive;
        # fall back to SIGTERM, then SIGKILL, each with a bounded reap.
        try:
            if failed:
                self.kill_group(signal.SIGTERM)
            try:
                await asyncio.wait_for(process.wait(), 0.5 if not failed else 2)
            except TimeoutError:
                self.kill_group(signal.SIGTERM)
                try:
                    await asyncio.wait_for(process.wait(), 3)
                except TimeoutError:
                    self.kill_group(signal.SIGKILL)
                    await asyncio.wait_for(process.wait(), 3)
            await asyncio.wait_for(asyncio.gather(*self.tasks), 3)
        finally:
            self.kill_group(signal.SIGKILL)  # Also removes unexpected surviving descendants.
            for task in self.tasks:
                if not task.done():
                    task.cancel()
            await asyncio.gather(*self.tasks, return_exceptions=True)
            if process.returncode is None:
                await asyncio.wait_for(process.wait(), 3)
        if not failed and process.returncode not in (0, -signal.SIGTERM):
            raise RuntimeError(f"server exited with {process.returncode}")
        if not failed and self.failure and not isinstance(self.failure, EOFError):
            raise self.failure

    def kill_group(self, sig):
        with contextlib.suppress(ProcessLookupError):
            os.killpg(self.process.pid, sig)

    def diagnostics(self):
        text = self.stderr.decode(errors="replace").replace(KEY, "[redacted]").replace(SECRET, "[redacted]")
        return {"stderr_bytes": self.stderr_bytes, "stderr_prefix": text,
                "notifications": self.notifications,
                "returncode": self.process.returncode if self.process else None}
