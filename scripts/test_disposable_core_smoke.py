"""Test destructive-scope guards and observers without a live Core or Docker."""
import argparse
import asyncio
import contextlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch

import disposable_core_smoke as smoke


@contextlib.contextmanager
def http_server(handler):
    server = ThreadingHTTPServer(('127.0.0.1', 0), handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield 'http://127.0.0.1:' + str(server.server_port)
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        assert not thread.is_alive()


class ScopeTests(unittest.TestCase):
    def test_rejects_external_api_and_arbitrary_project_names(self):
        for name in ['main', 'existing-core', 'komodo-mcp-smoke-', '../smoke', 'komodo-mcp-smoke-A']:
            with self.assertRaises(argparse.ArgumentTypeError):
                smoke.project_name(name)
        for address in ['https://core.example', 'http://localhost:9120', 'http://127.0.0.1:9120/read',
                        'http://secret@127.0.0.1:9120', 'http://127.0.0.1:9120?secret=key']:
            with self.assertRaises(smoke.Failure):
                smoke.Api(address)

    def test_redirect_never_forwards_scoped_credentials(self):
        received = []

        class Destination(BaseHTTPRequestHandler):
            def do_GET(self):
                received.append(dict(self.headers))
                self.send_response(200)
                self.end_headers()

            def log_message(self, *_args):
                pass

        with http_server(Destination) as destination:
            class Redirect(BaseHTTPRequestHandler):
                def do_POST(self):
                    self.send_response(302)
                    self.send_header('Location', destination + '/stolen')
                    self.end_headers()

                def log_message(self, *_args):
                    pass

            with http_server(Redirect) as address:
                with self.assertRaisesRegex(smoke.Failure, 'HTTP 302') as error:
                    smoke.Api(address, {'x-api-key': 'synthetic-private-key'}).read('GetVersion')
                self.assertNotIn('synthetic-private-key', str(error.exception))
        self.assertEqual(received, [])

    def test_response_is_bounded_before_json_decode(self):
        class Core(BaseHTTPRequestHandler):
            def do_POST(self):
                operation = json.loads(self.rfile.read(int(self.headers['Content-Length'])))['type']
                body = b'{"version":"2.3.2"}' if operation == 'GetVersion' else b'x' * (smoke.MAX_BODY + 1)
                self.send_response(200)
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *_args):
                pass

        with http_server(Core) as address:
            api = smoke.Api(address)
            self.assertEqual(api.read('GetVersion'), {'version': '2.3.2'})
            with self.assertRaisesRegex(smoke.Failure, 'bounded Core response exceeded'):
                api.read('Oversized')

    def test_existing_project_is_never_adopted_or_mutated(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(smoke, 'ROOT', Path(directory)), \
                patch.object(smoke, 'PROJECT', 'komodo-mcp-smoke-test'), \
                patch.object(smoke, 'RESERVED', False), patch.object(smoke, 'run', return_value='foreign') as run:
            with self.assertRaisesRegex(smoke.Failure, 'already in use'):
                smoke.bootstrap()
            self.assertFalse(smoke.RESERVED)
            self.assertEqual(list(Path(directory).iterdir()), [])
            self.assertEqual(run.call_count, 1)
            self.assertNotIn('up', run.call_args.args[0])

    def test_cleanup_refuses_foreign_container_network_or_volume_before_down(self):
        for foreign in ['container', 'network', 'volume']:
            calls = []

            def run(args, **_kwargs):
                calls.append(args)
                if 'inspect' in args:
                    return 'foreign-owner'
                if 'ps' in args:
                    return 'container-id' if foreign == 'container' else ''
                if 'network' in args:
                    return 'network-id' if foreign == 'network' else ''
                if 'volume' in args:
                    return 'volume-id' if foreign == 'volume' else ''
                raise AssertionError('unexpected mutation')

            with patch.object(smoke, 'OWNER', 'this-run'), patch.object(smoke, 'run', side_effect=run):
                with self.assertRaisesRegex(smoke.Failure, 'refusing cleanup'):
                    smoke.cleanup()
                self.assertFalse(any('down' in call for call in calls))

    def test_cleanup_removes_owned_partial_project_and_private_credentials(self):
        removed = False
        inspections = []

        def run(args, **_kwargs):
            nonlocal removed
            if 'inspect' in args:
                inspections.append(args[-1])
                return 'this-run'
            if 'down' in args:
                self.assertEqual(set(inspections), {'container-id', 'network-id', 'volume-id'})
                removed = True
                return ''
            if removed:
                return ''
            return 'container-id' if 'ps' in args else 'network-id' if 'network' in args else 'volume-id'

        with tempfile.TemporaryDirectory() as directory, patch.object(smoke, 'ROOT', Path(directory)), \
                patch.object(smoke, 'OWNER', 'this-run'), patch.object(smoke, 'REPORT', {}), \
                patch.object(smoke, 'run', side_effect=run):
            for name in ['compose.json', 'provisioning.json', 'test-identities.json']:
                (Path(directory)/name).write_text('private fixture')
            smoke.cleanup()
            self.assertTrue(smoke.REPORT['cleanup_verified'])
            self.assertFalse(any(smoke.REPORT['cleanup_remaining'].values()))
            self.assertEqual(list(Path(directory).iterdir()), [])

    def test_version_drift_stops_before_resource_provisioning(self):
        class Api:
            def __init__(self, *_args):
                pass

            def request(self, _endpoint, operation, *_args):
                return {'data': {'jwt': 'synthetic-jwt'}} if operation == 'LoginLocalUser' else True

            def read(self, operation, *_args):
                if operation != 'GetVersion':
                    raise AssertionError('read beyond version gate')
                return {'version': '2.3.3'}

            def write(self, *_args):
                raise AssertionError('provisioned despite unsupported version')

        with tempfile.TemporaryDirectory() as directory, patch.object(smoke, 'ROOT', Path(directory)), \
                patch.object(smoke, 'Api', Api), patch.object(smoke, 'run', return_value='127.0.0.1:9120'), \
                patch.object(smoke, 'SECRETS', []):
            (Path(directory)/'provisioning.json').write_text(json.dumps({
                'admin_username': 'fixture-admin', 'admin_password': 'synthetic-password'}))
            with self.assertRaisesRegex(smoke.Failure, 'Core version mismatch'):
                smoke.exercise_environment()

    def test_commands_have_deadlines_output_limits_and_reap_children(self):
        started = time.monotonic()
        with self.assertRaisesRegex(smoke.Failure, 'deadline'):
            smoke.run([sys.executable, '-c', 'import time; time.sleep(30)'], timeout=.1)
        self.assertLess(time.monotonic() - started, 5)
        with patch.object(smoke, 'MAX_COMMAND_OUTPUT', 32):
            with self.assertRaisesRegex(smoke.Failure, 'output'):
                smoke.run([sys.executable, '-c', 'print("x"*1024)'])


class ObservationTests(unittest.IsolatedAsyncioTestCase):
    async def test_wait_resumes_same_id_after_poll_limit_and_stops_on_read_failure(self):
        class Observer:
            def __init__(self, failed=False):
                self.calls, self.failed = [], failed

            async def tool(self, name, arguments):
                self.calls.append((name, arguments))
                if name == 'get_update':
                    return {'id': arguments['id']}
                return {'end': 'observation_failed' if self.failed else
                        'poll_limit' if len(self.calls) == 2 else 'complete',
                        'latest_update': {'id': arguments['id'], 'outcome': 'succeeded'}}

        with patch.object(smoke, 'REPORT', {}):
            observer = Observer()
            result = await smoke.observe(observer, 'known-update')
            self.assertEqual(result['outcome'], 'succeeded')
            self.assertEqual([name for name, _ in observer.calls], ['get_update', 'wait_for_update', 'wait_for_update'])
            self.assertTrue(all(args['id'] == 'known-update' for _, args in observer.calls))
            failed = Observer(True)
            with self.assertRaisesRegex(smoke.Failure, 'unavailable'):
                await smoke.observe(failed, 'known-update')
            self.assertEqual(len(failed.calls), 2)


if __name__ == '__main__':
    unittest.main()
