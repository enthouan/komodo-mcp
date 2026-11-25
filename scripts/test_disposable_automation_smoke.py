"""Automation fixture ownership, credential separation and teardown invariants."""
import contextlib
import asyncio
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import AsyncMock, Mock, patch

import disposable_automation_smoke as automation
import disposable_core_smoke as smoke


class AutomationScopeTests(unittest.TestCase):
    def tearDown(self):
        automation.CONTEXT.clear()
        automation.OWNED.clear()
        automation.ACTIVE.clear()

    def test_private_services_are_owned_and_do_not_publish_ports_or_mount_host_socket(self):
        config = json.loads((smoke.WORKSPACE / 'tests/fixtures/disposable-core/compose.json').read_text())
        with patch.object(smoke, 'OWNER', 'owned-session'):
            automation.customize(config)
        for kind in ['registry', 'source']:
            service = config['services'][kind]
            self.assertEqual(service['labels']['komodo-mcp.test.session'], 'owned-session')
            self.assertNotIn('ports', service)
            self.assertNotIn('volumes', service)
        self.assertEqual(config['services']['docker']['command'], ['--insecure-registry=registry.test:5000'])
        self.assertEqual(config['services']['registry']['networks']['default']['aliases'], ['registry.test'])
        self.assertEqual(config['services']['core']['environment']['KOMODO_TIMEZONE'], 'Asia/Tokyo')
        self.assertNotIn('/var/run/docker.sock:', json.dumps(config))

    def test_automatic_execution_is_verified_disabled_before_teardown(self):
        provisioner = Mock()
        resource_id = 'a' * 24
        name = automation.PREFIX + 'action'
        provisioner.read.side_effect = [
            {'_id': {'$oid': resource_id}, 'name': name, 'config': {}},
            {'config': {'webhook_enabled': False, 'schedule': '',
                        'schedule_enabled': False, 'run_at_startup': False}}]
        automation.CONTEXT['provisioner'] = provisioner
        automation.OWNED.append(('Action', resource_id, name))
        with patch.object(smoke, 'REPORT', {}):
            automation.disable_automatic_execution()
            self.assertTrue(smoke.REPORT['automatic_execution_cleanup']['verified'])
            self.assertTrue(smoke.REPORT['automatic_execution_cleanup']['before_compose_down'])
        provisioner.write.assert_called_once_with('UpdateAction', {'id': resource_id,
            'config': {'webhook_enabled': False, 'schedule': '',
                       'schedule_enabled': False, 'run_at_startup': False}})

    def test_definition_editor_receives_explicit_reference_attachment_permission(self):
        provisioner = Mock()
        automation.CONTEXT.update(provisioner=provisioner, users=[
            {'id': 'operator', 'workload_permission': 'Execute'},
            {'id': 'reader', 'workload_permission': 'Read'},
            {'id': 'editor', 'workload_permission': 'Write'}])
        for kind in ['Builder', 'Repo', 'Build']:
            provisioner.reset_mock()
            automation.grant(kind, 'a' * 24)
            permissions = [call.args[1]['permission'] for call in provisioner.write.call_args_list]
            self.assertEqual(permissions, [
                {'level': 'Execute', 'specific': []}, {'level': 'Read', 'specific': []},
                {'level': 'Write', 'specific': ['Attach']}])

    def test_changed_owner_identity_blocks_cleanup_write(self):
        for changed in ['id', 'name']:
            with self.subTest(changed=changed):
                provisioner = Mock()
                resource_id = 'a' * 24
                name = automation.PREFIX + 'action'
                provisioner.read.return_value = {'_id': {'$oid': 'b' * 24 if changed == 'id' else resource_id},
                    'name': 'foreign-resource' if changed == 'name' else name, 'config': {}}
                automation.CONTEXT['provisioner'] = provisioner
                automation.OWNED[:] = [('Action', resource_id, name)]
                with self.assertRaises(smoke.Failure):
                    automation.disable_automatic_execution()
                provisioner.write.assert_not_called()

    def test_failed_disable_readback_cannot_be_reported_verified(self):
        provisioner = Mock()
        resource_id, name = 'a' * 24, automation.PREFIX + 'procedure'
        provisioner.read.side_effect = [
            {'_id': {'$oid': resource_id}, 'name': name},
            {'config': {'webhook_enabled': False, 'schedule': 'every second', 'schedule_enabled': True}}]
        automation.CONTEXT['provisioner'] = provisioner
        automation.OWNED.append(('Procedure', resource_id, name))
        with patch.object(smoke, 'REPORT', {}), self.assertRaises(smoke.Failure):
            automation.disable_automatic_execution()
            self.assertNotIn('automatic_execution_cleanup', smoke.REPORT)

    def test_extension_failure_and_disable_failure_still_remove_owned_docker_resources(self):
        for fail_at in ['exercise', 'disable', None]:
            with self.subTest(fail_at=fail_at), tempfile.TemporaryDirectory() as directory, \
                    contextlib.ExitStack() as stack:
                private = Path(directory) / 'private'
                private.mkdir()
                for name, value in {'REPORT': {}, 'SECRETS': [], 'RESERVED': False, 'ROOT': None,
                                    'PROJECT': '', 'OWNER': '', 'COMPOSE': [], 'DOCKER': [],
                                    'BINARY': None, 'EVIDENCE': None}.items():
                    stack.enter_context(patch.object(smoke, name, value))
                stack.enter_context(patch.object(smoke.tempfile, 'mkdtemp', return_value=str(private)))
                stack.enter_context(patch.object(smoke, 'run', return_value='unix:///synthetic-owned'))
                stack.enter_context(patch.object(smoke, 'build_candidate', return_value=Path('fixture-binary')))
                events = []
                customizer, extension = object(), object()

                def bootstrap(value):
                    self.assertIs(value, customizer)
                    smoke.RESERVED = True
                    events.append('bootstrap')

                def exercise(value):
                    self.assertIs(value, extension)
                    events.append('exercise')
                    if fail_at == 'exercise':
                        raise smoke.Failure('synthetic exercise failure')
                    smoke.REPORT['passed'] = True

                def disable():
                    events.append('disable')
                    if fail_at == 'disable':
                        raise smoke.Failure('synthetic disable failure')

                def cleanup():
                    events.append('cleanup')
                    smoke.REPORT['cleanup_verified'] = True

                for name, action in [('bootstrap', bootstrap), ('exercise_environment', exercise), ('cleanup', cleanup)]:
                    stack.enter_context(patch.object(smoke, name, side_effect=action))
                evidence = Path(directory) / 'evidence.json'
                result = smoke.entry(['--docker-context', 'synthetic-owned', '--project',
                    'komodo-mcp-smoke-automation-test', '--evidence', str(evidence)],
                    fixture_customizer=customizer, exercise_extension=extension, before_cleanup=disable)
                self.assertEqual(result, int(fail_at is not None))
                self.assertEqual(events, ['bootstrap', 'exercise', 'disable', 'cleanup'])
                report = json.loads(evidence.read_text())
                self.assertEqual(report['passed'], fail_at is None)
                self.assertTrue(report['compose_cleanup_verified'])
                self.assertEqual(report['cleanup_verified'], fail_at != 'disable')
                self.assertEqual(private.exists(), fail_at == 'disable')

    def test_automation_entry_selects_extensions_and_clears_previous_run_state(self):
        automation.CONTEXT['old'] = 'must not be reused'
        automation.OWNED.append(('Action', 'a' * 24, 'old'))
        with patch.object(smoke, 'entry', return_value=7) as entry:
            self.assertEqual(automation.entry(['arguments']), 7)
        self.assertEqual(automation.CONTEXT, {})
        self.assertEqual(automation.OWNED, [])
        entry.assert_called_once_with(['arguments'], fixture_customizer=automation.customize,
            exercise_extension=automation.provision, before_cleanup=automation.disable_automatic_execution)

    def test_mutation_audit_rejects_replay_and_read_only_mutation_but_allows_separate_calls(self):
        for name, mutations, rejected in [
            ('run_action', 2, True), ('get_update', 1, True), ('run_action', 1, False)]:
            with self.subTest(name=name, mutations=mutations), patch.object(smoke, 'REPORT', {'automation': {}}):
                proxy = Mock(execute_attempts=0, write_attempts=0)

                async def tool(*_args):
                    proxy.execute_attempts += mutations
                    return {'safe': 'result'}

                audited = automation.AuditedMcp(Mock(tool=tool), proxy)
                if rejected:
                    with self.assertRaises(smoke.Failure):
                        asyncio.run(audited.tool(name, {}))
                else:
                    asyncio.run(audited.tool(name, {}))
                    asyncio.run(audited.tool(name, {}))
                    self.assertEqual(proxy.execute_attempts, 2)
                calls = smoke.REPORT['automation']['per_call_mutation_audit']
                self.assertEqual(calls[0]['execute_attempts'], mutations)
                self.assertNotIn('arguments', calls[0])

    def test_uncertain_created_id_remains_owned_without_create_replay(self):
        resource_id, name = 'a' * 24, automation.PREFIX + 'action'
        mcp = Mock(tool=AsyncMock(side_effect=[{'preview_id': 'one-use'}, {
            'resource': {'id': resource_id, 'name': name}, 'outcome': 'unknown',
            'writes_submitted': 1, 'saved_configuration_verified': False,
            'observation_error': {'kind': 'unavailable'}}]))
        with patch.object(smoke, 'REPORT', {'automation': {}}), patch.object(smoke, 'persist'):
            with self.assertRaises(smoke.Failure):
                asyncio.run(automation.save(mcp, 'action', {'name': name, 'config': {'source': ''}}, True))
            self.assertEqual(automation.OWNED, [('Action', resource_id, name)])
            self.assertEqual(mcp.tool.await_count, 2)
            self.assertEqual(smoke.REPORT['automation']['configuration_results'][0]['outcome'], 'unknown')


if __name__ == '__main__':
    unittest.main()
