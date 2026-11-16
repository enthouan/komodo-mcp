import hashlib
import http.client
import importlib.util
import json
from pathlib import Path
import socket
import tempfile
import threading
import unittest
from unittest.mock import patch

import disposable_core_smoke as s
import disposable_swarm_admin_smoke as integration
import swarm_admin_fixture as fixture

spec = importlib.util.spec_from_file_location('owned_receiver',
    s.WORKSPACE / 'tests/fixtures/disposable-core/receiver.py')
receiver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(receiver)


class ReceiverProof(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.receipts = Path(self.directory.name) / 'receipts.jsonl'
        self.nonce = 'private-fixture-receiver-nonce'
        self.server = receiver.ReceiverServer(('127.0.0.1', 0), receiver.handler(self.nonce, self.receipts))
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(2)
        self.directory.cleanup()
    def rows(self):
        return [json.loads(row) for row in self.receipts.read_text().splitlines()] if self.receipts.exists() else []
    def request(self, mode, body):
        connection = http.client.HTTPConnection('127.0.0.1', self.server.server_port, timeout=2)
        try:
            connection.request('POST', '/' + mode + '/' + self.nonce, body)
            response = connection.getresponse()
            response.read()
            return response.status
        finally: connection.close()
    def test_success_and_failed_provider_both_require_complete_receipts_without_content(self):
        body = b'private-notification-body'
        self.assertEqual(self.request('ok', body), 204)
        self.assertEqual(self.request('fail', body), 503)
        rows = self.rows()
        self.assertEqual([r['status'] for r in rows], [204, 503])
        self.assertTrue(all(r['complete'] and r['bytes'] == len(body)
                            and r['sha256'] == hashlib.sha256(body).hexdigest() for r in rows))
        text = self.receipts.read_text()
        self.assertNotIn(body.decode(), text)
        self.assertNotIn(self.nonce, text)
        self.assertEqual(self.receipts.stat().st_mode & 0o777, 0o600)
    def test_incomplete_body_cannot_be_counted_as_a_receipt(self):
        with socket.create_connection(('127.0.0.1', self.server.server_port), timeout=2) as stream:
            stream.sendall(('POST /ok/' + self.nonce + ' HTTP/1.1\r\nHost: localhost\r\nContent-Length: 20\r\n\r\npartial').encode())
            stream.shutdown(socket.SHUT_WR)
            self.assertIn(b'400', stream.recv(4096))
        self.assertEqual(self.rows(), [])
    def test_duplicate_length_chunked_and_unknown_paths_never_record(self):
        for path, headers in [('/ok/' + self.nonce, 'Content-Length: 1\r\nContent-Length: 1'),
                              ('/ok/' + self.nonce, 'Transfer-Encoding: chunked'),
                              ('/unowned', 'Content-Length: 1')]:
            with socket.create_connection(('127.0.0.1', self.server.server_port), timeout=2) as stream:
                stream.sendall(('POST ' + path + ' HTTP/1.1\r\nHost: localhost\r\n' + headers + '\r\n\r\nx').encode())
                stream.shutdown(socket.SHUT_WR)
                stream.recv(4096)
        self.assertEqual(self.rows(), [])
    def test_oversized_body_rejected_before_receipt(self):
        self.assertEqual(self.request('ok', b'x'*65537), 413)
        self.assertEqual(self.rows(), [])


class FinalSourceProof(unittest.TestCase):
    def test_main_acceptance_cannot_skip_retained_workflows(self):
        with self.assertRaises(s.Failure), patch.object(s, 'run') as run:
            integration.validate_source('a'*40, True, 'features')
        run.assert_not_called()
    def test_changed_or_dirty_source_refused_before_infrastructure(self):
        for responses in [['b'*40], ['a'*40, ' M src/main.rs']]:
            with patch.object(s, 'run', side_effect=responses), self.assertRaises(s.Failure):
                integration.validate_source('a'*40, False, 'all')
    def test_branch_candidate_is_never_mislabeled_as_main(self):
        with patch.object(s, 'run', side_effect=['a'*40, '', 'b'*40]), self.assertRaises(s.Failure):
            integration.validate_source('a'*40, True, 'all')
    def test_permission_scalar_never_manufactures_specific_grants(self):
        self.assertEqual(fixture.permission('Execute'), {'level': 'Execute', 'specific': []})
        with self.assertRaises(s.Failure): fixture.permission('Terminal')


class AutomaticPolicies(unittest.TestCase):
    def test_preparation_preserves_unrelated_private_server_configuration(self):
        import copy
        fields = ['auto_prune', 'send_unreachable_alerts', 'send_cpu_alerts', 'send_mem_alerts',
                  'send_disk_alerts', 'send_version_mismatch_alerts']
        raw = {'id': 'a'*24, 'name': 'owned', 'config': {**{k: True for k in fields},
               'private_field': {'reference': 'preserved'}}}
        class Admin:
            def __init__(self): self.writes = []
            def read(self, *_): return copy.deepcopy(raw)
            def write(self, operation, params):
                self.writes.append((operation, params))
                raw['config'].update(params['config'])
        admin = Admin()
        result = s.disable_server_automatic_effects(admin, 'a'*24, 'owned')
        self.assertEqual(len(admin.writes), 1)
        self.assertEqual(raw['config']['private_field'], {'reference': 'preserved'})
        self.assertTrue(result['automatic_image_prune_disabled'])
        s.disable_server_automatic_effects(admin, 'a'*24, 'owned')
        self.assertEqual(len(admin.writes), 1)
    def test_wrong_server_or_missing_policy_refuses_mutation(self):
        class Admin:
            def read(self, *_): return {'id': 'a'*24, 'name': 'foreign', 'config': {}}
            def write(self, *_): raise AssertionError('must not write')
        with self.assertRaises(s.Failure): s.disable_server_automatic_effects(Admin(), 'a'*24, 'owned')


if __name__ == '__main__': unittest.main()
