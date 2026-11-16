"""Live variable, permission-engine and owned HTTP Alerter acceptance."""
import asyncio
import datetime
import hashlib
import json
import secrets

import disposable_core_smoke as s
import swarm_admin_fixture as f


async def save(mcp, family, args, create=False):
    operation = family + ('_create' if create else '_update') if family == 'alerter' else family + '_change'
    preview = await mcp.tool('preview_' + operation, args)
    result = await mcp.tool('apply_' + operation, dict(args, preview_id=preview['preview_id']))
    verified = result.get('saved_configuration_verified', result.get('saved_state_matches'))
    s.require(verified is True, 'authoritative ' + family + ' save not verified')
    return result


async def variables():
    fixture = f.F
    name = 'MCP_V07_' + s.OWNER[:10].upper()
    secret_name = name + '_CREDENTIAL'
    private = 'owned-private-' + secrets.token_hex(20)
    s.SECRETS.append(private)
    fixture.admin.write('CreateVariable', {'name': secret_name, 'value': private,
                                          'description': 'owned metadata', 'is_secret': True})
    report = fixture.report['variables'] = {'ordinary': name, 'secret': secret_name, 'cases': []}
    async with f.Proxy('ordinary-admin') as proxy:
        async with await f.Mcp.start(proxy.address, fixture.roles['ordinary-admin']['credentials'],
                                    configuration_edit=True) as process:
            mcp = f.Audited(process, proxy)
            await mcp.tool('list_variables', {})
            await mcp.tool('list_core_secret_names', {})
            secret = await mcp.tool('get_variable', {'name': secret_name})
            s.require(secret['is_secret'] and secret.get('value') is None, 'administrator secret value exposed')
            changes = [
                {'kind': 'create_ordinary', 'name': name, 'value': 'blue', 'description': 'owned mode'},
                {'kind': 'set_ordinary_value', 'name': name, 'value': 'green'},
                {'kind': 'set_description', 'name': name, 'description': 'changed metadata'},
                {'kind': 'set_description', 'name': secret_name, 'description': 'metadata preserved value'},
            ]
            for change in changes:
                result = await save(mcp, 'variable', {'change': change})
                report['cases'].append({'kind': change['kind'], 'name': change['name'],
                    'outcome': result['outcome'], 'writes_submitted': result['writes_submitted']})
            noop = await save(mcp, 'variable', {'change': changes[1]})
            s.require(noop['writes_submitted'] == 0, 'variable no-op wrote')
            # Change only an undisplayed secret value between preview and apply.
            args = {'change': {'kind': 'set_description', 'name': secret_name, 'description': 'stale proposed'}}
            review = await mcp.tool('preview_variable_change', args)
            changed = 'owned-private-' + secrets.token_hex(20)
            s.SECRETS.append(changed)
            fixture.admin.write('UpdateVariableValue', {'name': secret_name, 'value': changed})
            before = len(proxy.mutations())
            rejected = await f.raw(process, 'apply_variable_change', dict(args, preview_id=review['preview_id']))
            s.require(rejected['structuredContent']['kind'] == 'stale_preview'
                      and len(proxy.mutations()) == before, 'secret-only drift failed closed')
            report['secret_only_drift_rejected_without_write'] = True
            # An acknowledged write is deliberately hidden by this fixture.
            args = {'change': {'kind': 'set_ordinary_value', 'name': name, 'value': 'amber'}}
            review = await mcp.tool('preview_variable_change', args)
            proxy.lose = 'UpdateVariableValue'
            result = await mcp.tool('apply_variable_change', dict(args, preview_id=review['preview_id']))
            s.require(result['outcome'] == 'submission_unknown' and result['writes_submitted'] == 1,
                      'lost variable acknowledgement claimed certainty or replayed')
            report['lost_ack'] = {'injected_after_acceptance': True, 'outcome': result['outcome'],
                                  'writes_submitted': result['writes_submitted']}
            await mcp.tool('export_resources_toml', {'targets': [], 'include_variables': True})
        async with await f.Mcp.start(proxy.address, fixture.roles['ordinary-admin']['credentials'],
                                    operate=False) as observer:
            mcp = f.Audited(observer, proxy)
            await mcp.tool('get_variable', {'name': secret_name})
            await mcp.tool('get_variable', {'name': name})
            history = fixture.admin.read('ListUpdates', {'query': {'operation': {'$in':
                ['CreateVariable', 'UpdateVariableValue', 'DeleteVariable']}}, 'page': 0})
            ids = [s.identity(row) for row in history['updates']]
            s.require(ids and len(ids) <= 100, 'variable history proof unavailable')
            for uid in ids: await mcp.tool('get_update', {'id': uid})
            await mcp.tool('export_resources_toml', {'targets': [], 'include_variables': True})
            report['restart_history_ids'] = ids
        async with await f.Mcp.start(proxy.address, fixture.roles['ordinary-admin']['credentials'],
                                    configuration_edit=True) as process:
            await save(f.Audited(process, proxy), 'variable',
                {'change': {'kind': 'delete_ordinary', 'name': name}})
        report['proxy'] = proxy.summary()
    async with f.Proxy('reader') as proxy:
        async with await f.Mcp.start(proxy.address, fixture.roles['reader']['credentials'],
                                    configuration_edit=True) as process:
            denied = await f.raw(process, 'preview_variable_change', {'change': {
                'kind': 'create_ordinary', 'name': name, 'value': 'blue'}})
            s.require(denied['structuredContent']['kind'] == 'permission_denied' and not proxy.mutations(),
                      'ordinary Core caller bypassed variable admin boundary')
    actual = next(v for v in fixture.admin.read('ListVariables') if v['name'] == secret_name)
    s.require(actual['is_secret'] and actual['value'] == changed
              and actual['description'] == 'metadata preserved value', 'secret metadata edit changed private value')
    report.update(private_value_preserved=True, mutation_history_export_restart_sanitized=True,
                  no_implicit_execution=True, passed=True)
    fixture.admin.write('DeleteVariable', {'name': secret_name})
    s.persist()


async def access():
    fixture = f.F
    affected, unrelated = fixture.roles['affected']['id'], fixture.roles['reader']['id']
    group_name = fixture.prefix + 'operators'
    resource = {'kind': 'resource', 'resource_kind': 'server', 'target': {'id': fixture.sid}}
    report = fixture.report['access'] = {'affected_id': affected, 'cases': []}
    async with f.Proxy('ordinary-admin') as proxy:
        async with await f.Mcp.start(proxy.address, fixture.roles['ordinary-admin']['credentials'],
                                    configuration_edit=True) as process:
            mcp = f.Audited(process, proxy)
            created = await save(mcp, 'access', {'change': {'kind': 'create_group', 'name': group_name}})
            groups = fixture.admin.read('ListUserGroups')
            group = next(v for v in groups if v['name'] == group_name)
            gid = s.identity(group)
            fixture.remember('UserGroup', gid, group_name)
            subject = {'kind': 'group', 'target': {'id': gid}}
            await save(mcp, 'access', {'change': {'kind': 'change_permission', 'subject': subject,
                'scope': resource, 'patch': {'level': 'execute', 'add_specific': ['inspect', 'logs']}}})
            await save(mcp, 'access', {'change': {'kind': 'set_group_member', 'group': {'id': gid},
                'user': {'id': unrelated}, 'present': True}})
            for present in [True, False]:
                await save(mcp, 'access', {'change': {'kind': 'set_group_member', 'group': {'id': gid},
                    'user': {'id': affected}, 'present': present}})
                actual = fixture.admin.read('GetUserGroup', {'user_group': gid})
                s.require(unrelated in actual['users'] and (affected in actual['users']) == present,
                          'group change lost unrelated membership')
                async with f.Proxy('affected') as affected_proxy:
                    async with await f.Mcp.start(affected_proxy.address, fixture.roles['affected']['credentials'],
                                                operate=False) as observer:
                        own = await f.Audited(observer, affected_proxy).tool('get_effective_permission', {
                            'resource_kind': 'server', 'target': {'id': fixture.sid}})
                        effective = f.permission(fixture.api('affected').read('GetPermission', {
                            'target': {'type': 'Server', 'id': fixture.sid}}))
                        s.require(effective['level'] == ('Execute' if present else 'Read'),
                                  'real group inheritance/revocation not observed')
                        report['cases'].append({'group_member': present, 'effective': effective,
                                                'mcp_evidence': own})
                await mcp.tool('list_direct_permissions', {'subject': {'kind': 'selected',
                    'subject': {'kind': 'user', 'target': {'id': affected}}}})
            # Broad changes require explicit typed scopes, then restore them.
            for enabled in [True, False]:
                result = await save(mcp, 'access', {'change': {'kind': 'set_group_everyone',
                    'target': {'id': gid}, 'everyone': enabled}})
                s.require(result['broad_scope'], 'everyone change omitted broad effect')
            result = await save(mcp, 'access', {'change': {'kind': 'change_permission', 'subject': subject,
                'scope': {'kind': 'resource_type', 'resource_kind': 'server'},
                'patch': {'level': 'read', 'add_specific': ['terminal']}}})
            s.require(result['broad_scope'], 'type-wide change omitted broad effect')
            # Ordinary admin cannot promote; no alternate credential is used.
            denied = await f.raw(process, 'preview_access_change', {'change': {
                'kind': 'set_user_admin', 'target': {'id': affected}, 'admin': True}})
            s.require(denied['structuredContent']['kind'] == 'permission_denied', 'ordinary admin crossed super-admin boundary')
            for change in [
                {'kind': 'delete_user', 'target': {'id': fixture.roles['ordinary-admin']['id']}},
                {'kind': 'set_user_base', 'target': {'id': fixture.roles['ordinary-admin']['id']}, 'enabled': False},
                {'kind': 'delete_user', 'target': {'id': fixture.roles['super-admin']['id']}},
            ]:
                before = len(proxy.mutations())
                denied = await f.raw(process, 'preview_access_change', {'change': change})
                s.require(denied.get('isError') and len(proxy.mutations()) == before, 'protected target mutation allowed')
            # Preserve an existing specific when adding a different flag.
            user_subject = {'kind': 'user', 'target': {'id': affected}}
            for flag in ['logs', 'inspect']:
                await save(mcp, 'access', {'change': {'kind': 'change_permission', 'subject': user_subject,
                    'scope': resource, 'patch': {'add_specific': [flag]}}})
            actual = f.permission(fixture.api('affected').read('GetPermission', {'target': {'type': 'Server', 'id': fixture.sid}}))
            s.require(set(actual['specific']) == {'Logs', 'Inspect'}, 'permission specifics not preserved')
            await save(mcp, 'access', {'change': {'kind': 'change_permission', 'subject': user_subject,
                'scope': resource, 'patch': {'remove_specific': ['logs', 'inspect']}}})
            await save(mcp, 'access', {'change': {'kind': 'delete_group', 'target': {'id': gid}}})
            managed_name = fixture.prefix + 'managed-service-user'
            await save(mcp, 'access', {'change': {'kind': 'create_service_user',
                'username': managed_name, 'description': 'owned service metadata'}})
            managed = [u for u in fixture.admin.read('ListUsers') if u['username'] == managed_name]
            s.require(len(managed) == 1, 'exact created service user not observed')
            managed_id = s.identity(managed[0])
            await save(mcp, 'access', {'change': {'kind': 'set_service_user_description',
                'target': {'id': managed_id}, 'description': 'updated owned metadata'}})
            for enabled in [False, True]:
                await save(mcp, 'access', {'change': {'kind': 'set_user_base',
                    'target': {'id': managed_id}, 'enabled': enabled}})
            s.require(fixture.admin.read('ListApiKeysForServiceUser', {'user': managed_id}) == [],
                      'product service user unexpectedly received credentials')
            await save(mcp, 'access', {'change': {'kind': 'delete_user', 'target': {'id': managed_id}}})
            report['managed_service_user'] = {'id': managed_id, 'lifecycle_verified': True,
                                             'api_keys_issued': 0}
        report['ordinary_admin_proxy'] = proxy.summary()
    async with f.Proxy('super-admin') as proxy:
        async with await f.Mcp.start(proxy.address, fixture.roles['super-admin']['credentials'],
                                    configuration_edit=True) as process:
            for admin in [True, False]:
                await save(f.Audited(process, proxy), 'access', {'change': {
                    'kind': 'set_user_admin', 'target': {'id': affected}, 'admin': admin}})
                s.require(fixture.api('affected').request('/user')['admin'] == admin,
                          'designated super-admin change not observed')
        report['super_admin_proxy'] = proxy.summary()
    report.update(group_only_inheritance_and_revocation=True, unrelated_members_preserved=True,
                  direct_records_not_treated_as_effective=True, self_and_super_protection=True, passed=True)
    s.persist()


async def alerters():
    fixture = f.F
    endpoint_variable = 'MCP_V07_RECEIVER_' + s.OWNER[:10].upper()
    ok_url = 'http://receiver:9121/ok/' + fixture.receiver_nonce
    fail_url = 'http://receiver:9121/fail/' + fixture.receiver_nonce
    s.SECRETS.extend([ok_url, fail_url])
    fixture.admin.write('CreateVariable', {'name': endpoint_variable, 'value': ok_url, 'is_secret': True})
    report = fixture.report['alerters'] = {'cases': []}
    async with f.Proxy('ordinary-admin') as proxy:
        async with await f.Mcp.start(proxy.address, fixture.roles['ordinary-admin']['credentials'],
                                    configuration_edit=True) as process:
            mcp = f.Audited(process, proxy)
            args = {'name': fixture.prefix + 'local-receiver', 'config': {
                'endpoint': {'kind': 'custom', 'url': '[[' + endpoint_variable + ']]'},
                'enabled': False, 'alert_types': ['ServerUnreachable']}}
            before = len(fixture.receipts())
            created = await save(mcp, 'alerter', args, True)
            aid = created['resource']['id']
            fixture.remember('Alerter', aid, args['name'])
            s.require(len(fixture.receipts()) == before, 'saving implicitly sent a notification')
            # Stored endpoint/reference-only drift remains private but must
            # invalidate a preview before any test or configuration submission.
            stale_args = {'target': {'id': aid}, 'patch': {'enabled': True}}
            review = await mcp.tool('preview_alerter_update', stale_args)
            fixture.admin.write('UpdateAlerter', {'id': aid, 'config': {'endpoint': {
                'type': 'Custom', 'params': {'url': ok_url}}}})
            before_mutations = len(proxy.mutations())
            rejected = await f.raw(process, 'apply_alerter_update', dict(stale_args,
                preview_id=review['preview_id']))
            s.require(rejected['structuredContent']['kind'] == 'stale_preview'
                      and len(proxy.mutations()) == before_mutations,
                      'private endpoint drift was not rejected without mutation')
            await save(mcp, 'alerter', {'target': {'id': aid}, 'patch': {
                'endpoint': args['config']['endpoint']}})
            report['private_endpoint_reference_drift_rejected'] = True
            async def test(label, expected_success, expected_receipts):
                previous = len(fixture.receipts())
                result = await mcp.tool('test_alerter', {'target': {'id': aid}, 'acknowledge_delivery': True})
                uid = result['receipt']['update_id']
                update = await s.observe(mcp, uid)
                actual = fixture.receipts()
                s.require((update['outcome'] == 'succeeded') == expected_success
                          and len(actual) - previous == expected_receipts, 'Core/provider/receiver observations disagreed')
                report['cases'].append({'case': label, 'update_id': uid, 'core_outcome': update['outcome'],
                    'independent_complete_http_receipts': actual[previous:],
                    'provider_attempt_not_inferred_from_core_success': True})
            await test('disabled', False, 0)
            await save(mcp, 'alerter', {'target': {'id': aid}, 'patch': {'enabled': True}})
            raw = fixture.admin.read('GetAlerter', {'alerter': aid})
            s.require(raw['config']['endpoint']['params']['url'] == '[[' + endpoint_variable + ']]',
                      'unrelated patch changed endpoint secret reference')
            await test('enabled-filter-bypassed', True, 1)
            fixture.admin.write('UpdateVariableValue', {'name': endpoint_variable, 'value': fail_url})
            await test('provider-503-after-complete-body', False, 1)
            fixture.admin.write('UpdateVariableValue', {'name': endpoint_variable, 'value': ok_url})
            now = datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(minutes=1)
            await save(mcp, 'alerter', {'target': {'id': aid}, 'patch': {'maintenance_windows': [{
                'name': 'owned suppression', 'schedule': {'kind': 'daily'}, 'hour': now.hour,
                'minute': now.minute, 'duration_minutes': 5, 'timezone': 'UTC', 'enabled': True}]}})
            await test('maintenance-suppressed-success', True, 0)
            await save(mcp, 'alerter', {'target': {'id': aid}, 'patch': {'enabled': False}})
        report['proxy'] = proxy.summary()
    fixture.admin.write('DeleteAlerter', {'id': aid})
    fixture.admin.write('DeleteVariable', {'name': endpoint_variable})
    report.update(saved_provider_and_receipt_separate=True, secret_reference_preserved=True, passed=True)
    s.persist()
