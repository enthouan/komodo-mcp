"""Synthetic, read-only Core 2.3.2 wire fixture; no deployed authorization code."""

import copy
import hashlib
import json
import re
import socket
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
KEY = "benchmark-synthetic-key"
SECRET = "benchmark-synthetic-secret"
STATES = (
    "deploying", "running", "paused", "stopped", "created", "restarting",
    "dead", "removing", "unhealthy", "down", "unknown",
)


def encoded(value):
    return json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode()


def digest(value):
    return hashlib.sha256(encoded(value)).hexdigest()


def expected_inventory(count):
    # The oracle is defined independently of either implementation's output.
    return [
        {"id": f"{i + 2:024x}", "name": f"benchmark-stack-{i:06d}",
         "state": STATES[i % len(STATES)]}
        for i in range(count)
    ]


class Core:
    def __init__(self, count, delay_ms=0):
        self.expected = expected_inventory(count)
        self.delay_ms = delay_ms
        seed = json.loads((ROOT / "tests/fixtures/core-2.3.2/stacks.json").read_text())[0]
        self.items = []
        for item in self.expected:
            wire = copy.deepcopy(seed)
            wire.update(id=item["id"], name=item["name"])
            wire["info"]["state"] = item["state"]
            self.items.append(wire)
        self.events = []
        self.context = {"implementation": "driver", "batch": None,
                        "phase": "setup", "sample": None}
        self.changed = threading.Condition()
        self.active = 0
        self.ready = threading.Event()
        self.stopped = threading.Event()
        core = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def setup(self):
                super().setup()
                self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

            def log_message(self, *_args):
                pass

            def do_GET(self):
                self.handle_request()

            def do_POST(self):
                self.handle_request()

            def handle_request(self):
                start = time.perf_counter_ns()
                with core.changed:
                    context = dict(core.context)
                    core.active += 1
                    core.changed.notify_all()
                operation, params, status = self.command + " " + self.path, {}, 400
                body = encoded({"error": "unsupported fixture request"})
                content_type = "application/json"
                try:
                    self.connection.settimeout(5)
                    if self.command == "GET" and self.path == "/version":
                        operation, status, body = "GET /version", 200, b"2.3.2"
                        content_type = "text/plain"
                    elif self.command == "POST" and (self.path == "/read" or self.path.startswith("/read/")):
                        size = int(self.headers.get("Content-Length", "0"))
                        if not 0 < size <= 1_000_000:
                            raise ValueError("invalid body length")
                        request = json.loads(self.rfile.read(size))
                        if self.path == "/read":
                            operation = request["type"]
                            params = request.get("params", {})
                        else:
                            # The pinned TypeScript SDK uses Core's per-operation
                            # route and sends params directly, rather than an envelope.
                            operation = self.path.removeprefix("/read/")
                            params = request
                        if (self.headers.get("x-api-key") != KEY
                                or self.headers.get("x-api-secret") != SECRET):
                            status, body = 401, encoded({"error": "fixture authentication rejected"})
                        elif operation == "GetVersion" and params == {}:
                            status, body = 200, encoded({"version": "2.3.2"})
                        elif operation == "ListStacks":
                            status, body = 200, encoded(core.list_stacks(params))
                        else:
                            status = 403  # Including every write/execute operation.
                    if core.delay_ms:
                        time.sleep(core.delay_ms / 1000)
                    self.send_response(status)
                    self.send_header("Content-Type", content_type)
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                    self.wfile.flush()
                except (ValueError, KeyError, TypeError) as error:
                    status, body = 400, encoded({"error": str(error)})
                    self.send_response(status)
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                finally:
                    with core.changed:
                        core.events.append({**context, "operation": operation,
                                            "route": self.path, "params": params,
                                            "status": status, "response_body_bytes": len(body),
                                            "service_ms": (time.perf_counter_ns() - start) / 1e6,
                                            "activity": "background_health" if operation in
                                            ("GetVersion", "GET /version") and context["phase"]
                                            not in ("startup", "warm_startup", "memory_startup",
                                                    "direct_version") else "foreground"})
                        core.active -= 1
                        core.changed.notify_all()

        self.http = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.http.daemon_threads = True
        self.address = f"http://127.0.0.1:{self.http.server_port}"
        self.thread = threading.Thread(target=self.serve, daemon=True)

    def list_stacks(self, params):
        if set(params) - {"query", "page", "limit", "sort_by", "sort_desc"}:
            raise ValueError("unsupported ListStacks params")
        query = params.get("query", {})
        if set(query) - {"names", "terms", "tags", "tag_behavior", "templates", "specific"}:
            raise ValueError("unsupported query")
        specific = query.get("specific", {})
        # Only these default specific filters are exercised by the pinned clients.
        if any(value not in (None, [], "", False) for value in specific.values()):
            raise ValueError("unsupported non-default specific filter")
        names, terms, tags = [query.get(k, []) for k in ("names", "terms", "tags")]
        if not all(isinstance(v, list) and all(isinstance(x, str) for x in v)
                   for v in (names, terms, tags)):
            raise ValueError("filters must be string arrays")
        items = [x for x in self.items if
                 (not names or any((x["id"].lower() == n.lower() if
                                   re.fullmatch(r"[0-9a-fA-F]{24}", n) else x["name"] == n)
                                  for n in names))
                 and all(t.lower() in x["name"].lower() for t in terms)
                 and not tags]  # All synthetic resources have no tags.
        templates = query.get("templates", "Include")
        if templates not in ("Include", "Exclude", "Only"):
            raise ValueError("unsupported templates filter")
        if templates == "Only":
            items = []
        if params.get("sort_by", "Name") != "Name":
            raise ValueError("fixture supports Name sorting only")
        items = sorted(items, key=lambda x: x["name"], reverse=params.get("sort_desc", False))
        page, limit = params.get("page", 0), params.get("limit", 30)
        if limit is None:
            limit = 30
        if any(type(n) is not int or n < 0 for n in (page, limit)):
            raise ValueError("page/limit must be nonnegative integers")
        return items if limit == 0 else items[page * limit:(page + 1) * limit]

    def serve(self):
        self.ready.set()
        self.http.serve_forever(poll_interval=0.05)

    def __enter__(self):
        self.thread.start()
        if not self.ready.wait(5):
            raise RuntimeError("fixture failed to signal readiness")
        return self

    def mark(self, implementation, batch, phase, sample=None):
        with self.changed:
            if not self.changed.wait_for(lambda: self.active == 0, timeout=10):
                raise TimeoutError("fixture still servicing previous phase")
            self.context = dict(implementation=implementation, batch=batch,
                                phase=phase, sample=sample)

    def __exit__(self, *_args):
        self.stopped.set()
        self.http.shutdown()
        self.http.server_close()
        self.thread.join(5)
