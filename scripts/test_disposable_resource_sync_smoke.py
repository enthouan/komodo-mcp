"""Deterministic ownership, lifecycle and explicit-mutation audit checks."""
import json
import unittest
from unittest.mock import Mock, patch

import disposable_resource_sync_smoke as sync


class ResourceSyncSmokeTests(unittest.TestCase):
    def tearDown(self):
        sync.CONTEXT.clear()
        sync.OWNED.clear()
        sync.a.CONTEXT.clear()
        sync.a.OWNED.clear()
        sync.a.ACTIVE.clear()

    def test_private_services_retain_automation_scope_and_core_host_directory(self):
        config = json.loads((sync.s.WORKSPACE / 'tests/fixtures/disposable-core/compose.json').read_text())
        with patch.object(sync.s, 'OWNER', 'owned-sync-session'):
            sync.customize(config)
        self.assertEqual(config['services']['core']['environment']['KOMODO_SYNC_DIRECTORY'], '/syncs')
        self.assertEqual(config['services']['core']['environment']['KOMODO_TIMEZONE'], 'Asia/Tokyo')
        for name in ['source', 'registry']:
            service = config['services'][name]
            self.assertEqual(service['labels']['komodo-mcp.test.session'], 'owned-sync-session')
            self.assertNotIn('ports', service)
            self.assertNotIn('volumes', service)
        self.assertNotIn('/var/run/docker.sock:', json.dumps(config))

    def test_only_reviewed_sync_apply_allows_one_refresh_and_one_execution(self):
        sync.validate_counts('apply_resource_sync', 1, 1)
        for name in ['get_resource_sync_pending', 'review_resource_sync_pending',
                     'export_resources_toml', 'wait_for_resource_sync_update', 'reconcile_operation']:
            sync.validate_counts(name, 0, 0)
            for writes, executes in [(1, 0), (0, 1)]:
                with self.assertRaises(sync.s.Failure):
                    sync.validate_counts(name, writes, executes)
        for name in ['apply_resource_sync_create', 'refresh_resource_sync_pending', 'run_action']:
            with self.assertRaises(sync.s.Failure):
                sync.validate_counts(name, 1, 1)
        for counts in [(2, 0), (0, 2), (2, 1), (-1, 0)]:
            with self.assertRaises(sync.s.Failure):
                sync.validate_counts('apply_resource_sync', *counts)

    def test_full_automation_extension_precedes_resource_sync_exercise(self):
        events = []
        context = {'provisioner': object(), 'address': 'owned-loopback'}
        def automation(value):
            self.assertIs(value, context)
            sync.a.CONTEXT['admin_test'] = {'key': 'test-admin', 'secret': 'test-secret'}
            events.append('full automation')
        exercise = object()
        with patch.object(sync.s, 'REPORT', {'automation': {'full_workflow': True}}), \
                patch.object(sync.a, 'provision', side_effect=automation), \
                patch.object(sync, 'exercise', new=Mock(return_value=exercise)), \
                patch.object(sync.asyncio, 'run', side_effect=lambda value: events.append('ResourceSync') if value is exercise else self.fail('wrong coroutine')):
            sync.provision(context)
            self.assertEqual(events, ['full automation', 'ResourceSync'])
            self.assertTrue(sync.s.REPORT['resource_sync']['retained_full_automation_extension'])
            self.assertFalse(sync.s.REPORT['resource_sync']['provisioner_credentials_passed_to_mcp'])
            self.assertEqual(sync.CONTEXT['admin_test']['key'], 'test-admin')

    def test_foreign_identity_blocks_write_and_still_disables_retained_automation(self):
        admin = Mock()
        admin.read.return_value = {'_id': {'$oid': 'b' * 24}, 'name': 'foreign-resource'}
        sync.CONTEXT['provisioner'] = admin
        sync.OWNED.append(('ResourceSync', 'a' * 24, sync.PREFIX + 'owned'))
        with patch.object(sync.s, 'REPORT', {}), patch.object(sync.a, 'disable_automatic_execution') as retained:
            with self.assertRaises(sync.s.Failure):
                sync.disable_automatic_execution()
            retained.assert_called_once()
            admin.write.assert_not_called()
            self.assertFalse(sync.s.REPORT['resource_sync_automatic_execution_cleanup']['verified'])

    def test_owned_sync_trigger_disable_is_read_back_before_teardown(self):
        admin = Mock()
        resource_id, name = 'a' * 24, sync.PREFIX + 'owned'
        admin.read.side_effect = [{'_id': {'$oid': resource_id}, 'name': name},
                                  {'config': {'webhook_enabled': False}}]
        sync.CONTEXT['provisioner'] = admin
        sync.OWNED.append(('ResourceSync', resource_id, name))
        with patch.object(sync.s, 'REPORT', {}), patch.object(sync.a, 'disable_automatic_execution') as retained:
            sync.disable_automatic_execution()
            admin.write.assert_called_once_with('UpdateResourceSync', {
                'id': resource_id, 'config': {'webhook_enabled': False}})
            retained.assert_called_once()
            self.assertTrue(sync.s.REPORT['resource_sync_automatic_execution_cleanup']['verified'])
            self.assertTrue(sync.s.REPORT['resource_sync_automatic_execution_cleanup']['before_compose_down'])

    def test_retained_automation_cleanup_failure_is_not_reported_as_success(self):
        with patch.object(sync.s, 'REPORT', {}), \
                patch.object(sync.a, 'disable_automatic_execution', side_effect=sync.s.Failure('disable failed')):
            with self.assertRaises(sync.s.Failure):
                sync.disable_automatic_execution()
            self.assertFalse(sync.s.REPORT['resource_sync_automatic_execution_cleanup']['verified'])

    def test_entry_uses_scoped_base_lifecycle_and_clears_only_its_fixture_state(self):
        sync.OWNED.append(('ResourceSync', 'a' * 24, sync.PREFIX + 'old'))
        sync.a.ACTIVE.append(('action', 'b' * 24, 'c' * 24))
        arguments = ['--docker-context', 'explicit-local', '--project', 'komodo-mcp-smoke-sync-test', '--evidence', 'new.json']
        with patch.object(sync.s, 'entry', return_value=17) as base:
            self.assertEqual(sync.entry(arguments), 17)
            base.assert_called_once_with(arguments, fixture_customizer=sync.customize,
                exercise_extension=sync.provision, before_cleanup=sync.disable_automatic_execution)
        self.assertEqual(sync.OWNED, [])
        self.assertEqual(sync.a.ACTIVE, [])

    def test_unmarked_private_values_are_rejected_before_public_evidence(self):
        with patch.object(sync.s, 'SECRETS', ['unmarked-private-value']):
            with self.assertRaises(sync.s.Failure):
                sync.safe({'nested': ['unmarked-private-value']})
            self.assertEqual(sync.safe({'values_omitted': True}), {'values_omitted': True})


if __name__ == '__main__':
    unittest.main()
