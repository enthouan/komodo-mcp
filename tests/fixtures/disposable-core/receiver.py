"""Owned HTTP receipt proof: complete bounded bodies, never payloads or URL logs."""
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import time


class ReceiverServer(ThreadingHTTPServer):
    daemon_threads = True

    def handle_error(self, *_):
        pass  # Request paths contain the fixture nonce.


def handler(nonce, receipts):
    class Receiver(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def finish_response(self, status):
            self.send_response(status)
            self.send_header('Content-Length', '0')
            self.send_header('Connection', 'close')
            self.end_headers()
            self.close_connection = True

        def do_GET(self):
            self.finish_response(204 if self.path == '/health' else 404)

        def do_POST(self):
            mode = ('ok' if self.path == '/ok/' + nonce
                    else 'fail' if self.path == '/fail/' + nonce else None)
            lengths = self.headers.get_all('Content-Length', [])
            if (mode is None or self.headers.get('Transfer-Encoding')
                    or len(lengths) != 1 or not lengths[0].isdigit()):
                self.finish_response(404 if mode is None else 400)
                return
            length = int(lengths[0])
            if not 0 < length <= 65536:
                self.finish_response(413)
                return
            self.connection.settimeout(10)
            try:
                body = self.rfile.read(length)
            except OSError:
                self.finish_response(408)
                return
            if len(body) != length:
                self.finish_response(400)
                return
            status = 204 if mode == 'ok' else 503
            receipt = {'time': time.time(), 'mode': mode, 'status': status,
                       'method': 'POST', 'bytes': length, 'complete': True,
                       'sha256': hashlib.sha256(body).hexdigest()}
            descriptor = os.open(receipts, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
            with os.fdopen(descriptor, 'a') as output:
                output.write(json.dumps(receipt) + '\n')
            self.finish_response(status)

    return Receiver


if __name__ == '__main__':
    ReceiverServer(('0.0.0.0', 9121), handler(
        os.environ['RECEIVER_NONCE'], Path('/state/receipts.jsonl'))).serve_forever()
