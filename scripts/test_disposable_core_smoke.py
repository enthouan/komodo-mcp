"""Test destructive-scope guards and observers without a live Core or Docker."""
import argparse
import asyncio
import contextlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import signal
import socket
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
    def test_evidence_write_failure_still_cleans_and_cleanup_error_cannot_report_pass(self):
        for failed in ['evidence', 'cleanup', 'before_cleanup']:
            with self.subTest(failed=failed), tempfile.TemporaryDirectory() as directory, \
                    contextlib.ExitStack() as stack:
                private = Path(directory)/'private'
                private.mkdir()
                for name,value in {'REPORT':{},'SECRETS':[],'RESERVED':False,'ROOT':None,
                                   'PROJECT':'','OWNER':'','COMPOSE':[],'DOCKER':[],
                                   'BINARY':None,'EVIDENCE':None}.items():
                    stack.enter_context(patch.object(smoke,name,value))
                stack.enter_context(patch.object(smoke.tempfile,'mkdtemp',return_value=str(private)))
                stack.enter_context(patch.object(smoke,'run',return_value='unix:///synthetic-local-docker'))
                stack.enter_context(patch.object(smoke,'build_candidate',return_value=Path('synthetic-binary')))
                stack.enter_context(patch.object(smoke,'bootstrap',side_effect=lambda: setattr(smoke,'RESERVED',True)))
                stack.enter_context(patch.object(smoke,'exercise_environment',side_effect=lambda: smoke.REPORT.update(passed=True)))
                original_persist, writes = smoke.persist, 0

                def persist():
                    nonlocal writes
                    writes += 1
                    if writes == 4 and failed == 'evidence':
                        raise OSError('synthetic evidence failure')
                    original_persist()

                def cleanup():
                    smoke.REPORT['cleanup_verified'] = True
                    if failed == 'cleanup':
                        raise OSError('synthetic private-file cleanup failure')

                stack.enter_context(patch.object(smoke,'persist',side_effect=persist))
                cleaned = stack.enter_context(patch.object(smoke,'cleanup',side_effect=cleanup))
                evidence = Path(directory)/'evidence.json'
                def before_cleanup():
                    if failed == 'before_cleanup':
                        raise smoke.Failure('synthetic workload cleanup failed')
                result = smoke.entry(['--docker-context','synthetic-local','--project',
                    'komodo-mcp-smoke-test','--evidence',str(evidence)], before_cleanup=before_cleanup)
                self.assertEqual(result,1)
                cleaned.assert_called_once()
                report = json.loads(evidence.read_text())
                self.assertFalse(report['passed'])
                self.assertEqual(report['cleanup_verified'],failed == 'evidence')
                self.assertEqual(report['compose_cleanup_verified'],failed != 'cleanup')
                self.assertEqual(private.exists(),failed in ('cleanup', 'before_cleanup'))

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
                    self.rfile.read(int(self.headers['Content-Length']))
                    self.send_response(302)
                    self.send_header('Location', destination + '/stolen')
                    self.send_header('Content-Length', '0')
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
                self.send_response(403 if operation == 'OversizedError' else 200)
                self.send_header('Content-Length', str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *_args):
                pass

        with http_server(Core) as address:
            api = smoke.Api(address)
            self.assertEqual(api.read('GetVersion'), {'version': '2.3.2'})
            for operation in ['Oversized','OversizedError']:
                with self.assertRaisesRegex(smoke.Failure, 'bounded Core response exceeded'):
                    api.read(operation)

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

    def test_exited_command_leader_cannot_leave_a_live_descendant(self):
        child = '''import json,os,socket,sys,time
from pathlib import Path
listener=socket.socket()
listener.bind(('127.0.0.1',0))
listener.listen()
Path(sys.argv[1]).write_text(json.dumps({'pid':os.getpid(),'group':os.getpgrp(),'port':listener.getsockname()[1]}))
time.sleep(30)
'''
        parent = '''import subprocess,sys,time
from pathlib import Path
subprocess.Popen([sys.executable,'-c',sys.argv[1],sys.argv[2]])
for _ in range(500):
    if Path(sys.argv[2]).exists(): break
    time.sleep(.01)
raise SystemExit(int(sys.argv[3]))
'''
        for status in [0, 1]:
            with self.subTest(status=status), tempfile.TemporaryDirectory() as directory:
                marker = Path(directory)/'descendant.json'
                try:
                    command = [sys.executable,'-c',parent,child,str(marker),str(status)]
                    if status:
                        with self.assertRaises(smoke.Failure):
                            smoke.run(command,timeout=10)
                    else:
                        smoke.run(command,timeout=10)
                    info = json.loads(marker.read_text())
                    end = time.monotonic() + 2
                    while time.monotonic() < end:
                        try:
                            connection = socket.create_connection(('127.0.0.1',info['port']),timeout=.1)
                            connection.close()
                        except OSError:
                            break
                        time.sleep(.02)
                    else:
                        self.fail('descendant survived its exited command leader')
                finally:
                    if marker.exists():
                        try:
                            os.killpg(json.loads(marker.read_text())['group'],signal.SIGKILL)
                        except ProcessLookupError:
                            pass

    def test_absolute_http_deadline_interrupts_trickled_headers_and_body(self):
        for phase in ['headers','body']:
            finished = threading.Event()

            class SlowCore(BaseHTTPRequestHandler):
                def do_POST(self):
                    try:
                        self.wfile.write(b'HTTP/1.1 200 OK\r\n')
                        if phase == 'body':
                            self.wfile.write(b'Content-Length: 10000\r\n\r\n')
                        for _ in range(100):
                            self.wfile.write(b'X-Trickle: yes\r\n' if phase == 'headers' else b' ')
                            self.wfile.flush()
                            time.sleep(.04)
                    except OSError:
                        pass
                    finally:
                        finished.set()

                def log_message(self, *_args):
                    pass

            with self.subTest(phase=phase), http_server(SlowCore) as address, \
                    patch.object(smoke,'HTTP_TIMEOUT',.25):
                started = time.monotonic()
                with self.assertRaisesRegex(smoke.Failure,'deadline'):
                    smoke.Api(address).read('GetVersion')
                self.assertLess(time.monotonic()-started,2)
                self.assertTrue(finished.wait(2),'deadline did not terminate the underlying connection')


class ObservationTests(unittest.IsolatedAsyncioTestCase):
    async def test_fault_proxy_preserves_bounded_core_permission_errors_without_retries_or_trace_messages(self):
        received = []
        private = 'synthetic-private-error-chain'

        class Core(BaseHTTPRequestHandler):
            def do_POST(self):
                request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                received.append((self.path, self.headers['x-api-key'], request['type']))
                status = 403 if request['type'] == 'CreateServer' else 500
                body = json.dumps({'error':'Core denied permission','trace':[private]}).encode()
                self.send_response(status)
                self.send_header('Content-Length',str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *_args):
                pass

        credentials = {'key':'synthetic-reader-key','secret':'synthetic-reader-secret'}
        with http_server(Core) as address:
            async with smoke.LossProxy(address,credentials,lose_reply=False) as proxy:
                api = smoke.Api(proxy.address,{'x-api-key':credentials['key'],'x-api-secret':credentials['secret']})
                for operation,status in [('CreateServer',403),('UpdateServer',500)]:
                    with self.assertRaises(smoke.CoreHttpFailure) as caught:
                        await asyncio.to_thread(api.write,operation,{'name':'owned','config':{}})
                    self.assertEqual(caught.exception.status,status)
                    self.assertEqual(json.loads(caught.exception.body)['trace'],[private])
                    self.assertNotIn(private,str(caught.exception))
                self.assertEqual(proxy.write_attempts,2)
                self.assertEqual(proxy.execute_attempts,0)
                self.assertEqual(proxy.failures,[])
        self.assertEqual(received,[('/write',credentials['key'],'CreateServer'),('/write',credentials['key'],'UpdateServer')])

    async def test_read_identity_proxy_measures_unexpected_execute_before_ack(self):
        received = []

        class Core(BaseHTTPRequestHandler):
            def do_POST(self):
                request = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                received.append((self.path,self.headers['x-api-key'],request['type']))
                value = {'updates':[],'next_page':None} if self.path == '/read' else {
                    '_id':{'$oid':'100000000000000000000001'},'status':'InProgress','success':False}
                body = json.dumps(value).encode()
                self.send_response(200)
                self.send_header('Content-Length',str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *_args):
                pass

        credentials = {'key':'synthetic-reader-key','secret':'synthetic-reader-secret'}
        with http_server(Core) as address:
            async with smoke.LossProxy(address,credentials,lose_reply=False) as proxy:
                api = smoke.Api(proxy.address,{'x-api-key':credentials['key'],'x-api-secret':credentials['secret']})
                await asyncio.to_thread(api.read,'ListUpdates')
                self.assertEqual(proxy.read_operations,{'ListUpdates':1})
                self.assertEqual(proxy.execute_attempts,0)
                await asyncio.to_thread(api.request,'/execute','RestartStack',{'stack':'synthetic-stack'})
                self.assertEqual(proxy.execute_attempts,1)
                self.assertEqual(proxy.ids,['100000000000000000000001'])
                await asyncio.to_thread(api.write,'UpdateStack',{'id':'synthetic-stack','config':{'environment':'private-fixture-value'}})
                self.assertEqual(proxy.write_attempts,1)
                self.assertEqual(proxy.write_operations,[{'type':'UpdateStack','fields':['environment']}])
                self.assertNotIn('private-fixture-value',json.dumps(proxy.write_operations))
        self.assertEqual(received,[('/read',credentials['key'],'ListUpdates'),
                                   ('/execute',credentials['key'],'RestartStack'),
                                   ('/write',credentials['key'],'UpdateStack')])

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
