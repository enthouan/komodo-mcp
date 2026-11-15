#!/usr/bin/env python3
"""Opt-in ResourceSync acceptance plus the full retained automation/Core smoke.

Only a new, owned, local Docker project is accepted. Source provisioning and
deliberate drift are fixture operations; MCP never commits/pushes or writes host
files. Run again from the actual final main commit for release-readiness proof.
"""
import asyncio
import hashlib
import json
import secrets
import time
import tomllib

import disposable_automation_smoke as a

s = a.s
CONTEXT = {}
OWNED = []
PREFIX = 'mcp-sync-'
RESOURCE_FILE = 'resources.toml'


def customize(config):
    a.customize(config)
    config['services']['core']['environment']['KOMODO_SYNC_DIRECTORY'] = '/syncs'


def api(credentials):
    return s.Api(CONTEXT['address'], {'x-api-key': credentials['key'],
                                     'x-api-secret': credentials['secret']})


def private_value():
    value = 'owned-private-' + secrets.token_hex(16)
    s.SECRETS.append(value)
    return value


def safe(value):
    encoded = json.dumps(value)
    s.require(not any(secret and secret in encoded for secret in s.SECRETS),
              'private ResourceSync material escaped the MCP projection')
    return value


class AuditedMcp:
    """Explicit MCP mutations, distinct from Core's internal refresh/effects."""
    def __init__(self, mcp, proxy):
        self.mcp, self.proxy = mcp, proxy

    async def tool(self, name, arguments):
        before = self.proxy.write_attempts, self.proxy.execute_attempts
        try:
            return safe(await self.mcp.tool(name, arguments))
        finally:
            writes = self.proxy.write_attempts - before[0]
            executes = self.proxy.execute_attempts - before[1]
            validate_counts(name, writes, executes)
            audit = s.REPORT['resource_sync'].setdefault('per_call_mutation_audit', [])
            s.require(len(audit) < 1000, 'ResourceSync mutation audit exceeded bound')
            audit.append({'tool': name, 'explicit_refresh_or_definition_writes': writes,
                          'run_sync_requests': executes})


def validate_counts(name, writes, executes):
    s.require(0 <= writes <= 1 and 0 <= executes <= 1,
              'an explicit tool call repeated a mutation')
    if name == 'apply_resource_sync':
        return  # One separately authorized refresh and at most one RunSync.
    if name.startswith(('get_', 'list_', 'preview_', 'review_', 'wait_', 'reconcile_', 'export_')) or name == 'fleet_health':
        s.require(writes == executes == 0, 'read-only observation mutated Core')
    else:
        s.require(writes + executes <= 1, 'non-sync tool submitted two mutations')


async def raw_error(mcp, name, arguments, kind=None, proxy=None):
    before = None if proxy is None else (proxy.write_attempts, proxy.execute_attempts,
                                         sum(proxy.read_operations.values()))
    value = await mcp.rpc('tools/call', {'name': name, 'arguments': arguments}, True)
    safe(value)
    if kind is None:
        s.require('protocol_error' in value, 'disabled direct invocation was accepted')
    else:
        s.require(value.get('isError') and value.get('structuredContent', {}).get('kind') == kind,
                  'unexpected typed rejection for ' + name)
    if proxy is not None:
        s.require((proxy.write_attempts, proxy.execute_attempts, sum(proxy.read_operations.values())) == before,
                  'disabled or consumed-reference invocation contacted Core')
        s.REPORT['resource_sync'].setdefault('per_call_mutation_audit', []).append({
            'tool': name, 'rejected_before_core_io': True,
            'explicit_refresh_or_definition_writes': 0, 'run_sync_requests': 0})
    return value


def grant(kind, resource_id):
    for user in CONTEXT['users'][:3]:
        CONTEXT['provisioner'].write('UpdatePermissionOnTarget', {
            'user_target': {'type': 'User', 'id': user['id']},
            'resource_target': {'type': kind, 'id': resource_id},
            'permission': {'level': user['workload_permission'],
                'specific': ['Attach'] if kind == 'Repo' and user['workload_permission'] == 'Write' else []}})


def remember(kind, resource_id, name):
    s.require(name.startswith(PREFIX), 'resource outside ResourceSync ownership prefix')
    if (kind, resource_id, name) not in OWNED:
        OWNED.append((kind, resource_id, name))
        grant(kind, resource_id)
        s.REPORT['resource_sync']['resources'].append({'kind': kind, 'id': resource_id, 'name': name})


def forget(kind, resource_id):
    OWNED[:] = [x for x in OWNED if x[:2] != (kind, resource_id)]


async def save(mcp, name, config=None, resource_id=None, patch=None):
    create = resource_id is None
    value = {'name': name, 'config': config} if create else {'target': {'id': resource_id}, 'patch': patch}
    op = 'resource_sync_' + ('create' if create else 'update')
    preview = await mcp.tool('preview_' + op, value)
    result = await mcp.tool('apply_' + op, dict(value, preview_id=preview['preview_id']))
    if create and result['resource'].get('id'):
        remember('ResourceSync', result['resource']['id'], name)
    s.require(result['saved_configuration_verified'] and result['outcome'] in ['saved', 'no_op'],
              'ResourceSync definition was not authoritatively verified')
    s.require(result['writes_submitted'] <= 1, 'definition write repeated')
    s.REPORT['resource_sync'].setdefault('definition_results', []).append({
        'operation': op, 'id': result['resource']['id'], 'outcome': result['outcome'],
        'writes': result['writes_submitted'], 'saved_configuration_verified': True})
    return result['resource']['id']


def config(source, **extra):
    return {'source': source, 'managed': False, 'delete': False,
            'include_resources': True, 'include_variables': False,
            'include_user_groups': False, 'match_tags': [], 'webhook_enabled': False,
            **extra}


def server_doc(name, *, prune=False, tag=None):
    value = '[[server]]\nname=' + json.dumps(name) + '\n'
    if tag:
        value += 'tags=[' + json.dumps(tag) + ']\n'
    value += '[server.config]\nenabled=false\n'
    if prune is not None:
        value += 'auto_prune=' + str(prune).lower() + '\n'
    return value


def publish_git(files):
    """Fixture provisioner only; never called from a product MCP tool."""
    work, bare = s.ROOT / 'workload', s.ROOT / 'workload.git'
    for filename, contents in files.items():
        s.require(filename.startswith('resources-') and filename.endswith('.toml') and '/' not in filename,
                  'fixture source path outside owned Git files')
        (work / filename).write_text(contents)
    s.run(['git', '-C', str(work), 'add', '--', *files])
    s.run(['git', '-C', str(work), '-c', 'user.name=Disposable fixture', '-c',
           'user.email=fixture@example.invalid', 'commit', '-m', 'change owned ResourceSync fixture'])
    s.run(['git', '--git-dir', str(bare), 'fetch', str(work), 'main:refs/heads/main'])
    s.run(['git', '--git-dir', str(bare), 'update-server-info'])
    container = s.run([*s.COMPOSE, 'ps', '-q', 'source'])
    s.run([*s.DOCKER, 'cp', str(bare) + '/.', container + ':/www/fixture/workload.git'])
    revision = s.run(['git', '-C', str(work), 'rev-parse', 'HEAD'])
    s.require(git_revision() == revision, 'owned Git server differs from provisioned revision')
    return revision


def git_revision():
    rows = s.run([*s.COMPOSE, 'exec', '-T', 'core', 'git', 'ls-remote',
                  'https://source/fixture/workload.git', 'refs/heads/main']).splitlines()
    s.require(len(rows) == 1 and rows[0].endswith('refs/heads/main'), 'owned Git ref unavailable')
    return rows[0].split()[0]


def host_path(name):
    s.require(name.startswith(PREFIX) and '/' not in name, 'host source outside owned sync directory')
    return '/syncs/' + name + '/' + RESOURCE_FILE


def write_host(name, contents):
    path = host_path(name)
    local = s.ROOT / 'owned-host-source.toml'
    local.write_text(contents)
    s.run([*s.COMPOSE, 'exec', '-T', 'core', 'mkdir', '-p', '/syncs/' + name])
    container = s.run([*s.COMPOSE, 'ps', '-q', 'core'])
    s.run([*s.DOCKER, 'cp', str(local), container + ':' + path])


def host_digest(name):
    local = s.ROOT / 'owned-host-observation.toml'
    container = s.run([*s.COMPOSE, 'ps', '-q', 'core'])
    s.run([*s.DOCKER, 'cp', container + ':' + host_path(name), str(local)])
    return hashlib.sha256(local.read_bytes()).hexdigest()


def external_evidence(host_name):
    return {'git_ref': git_revision(), 'host_sha256': host_digest(host_name)}


def unchanged(before, host_name):
    after = external_evidence(host_name)
    s.require(before == after, 'MCP modified an authoritative Git/host fixture source')
    return after


async def review(mcp, sync_id, selection):
    value = await mcp.tool('review_resource_sync_pending', {
        'target': {'id': sync_id}, 'selection': selection})
    s.require(value['review_reference'] and value['plan']['apply_coverage_complete'],
              'review did not establish complete private coverage')
    return value


async def apply(mcp, reviewed, expected='succeeded'):
    result = await mcp.tool('apply_resource_sync', {
        'review_reference': reviewed['review_reference'], 'binding': reviewed['apply_binding'],
        'wait_timeout_seconds': 3})
    s.REPORT['resource_sync'].setdefault('apply_attempts', []).append({
        'sync_id': reviewed['apply_binding']['sync_id'], 'selection': reviewed['apply_binding']['selection'],
        'outcome': result['outcome'], 'preflight': result['preflight'],
        'run_sync_requests': result['run_sync_requests_submitted'],
        'known_update_id': (result.get('receipt') or {}).get('update_id')})
    s.persist()
    s.require(result['preflight']['refresh_requests_submitted'] == 1
              and result['run_sync_requests_submitted'] == 1 and result['outcome'] == 'accepted',
              'reviewed apply did not make one refresh and one accepted RunSync')
    s.require(result['preflight']['git_pull_cache_wait_ms'] ==
              (5100 if reviewed['plan']['source']['kind'] in ['git', 'linked_repo'] else 0),
              'Git pull-cache settling protocol was not recorded')
    receipt = result['receipt']
    s.require(receipt['update_id'] and not receipt['automatic_resubmission'], 'known update ID missing/replay')
    observed = await mcp.tool('wait_for_resource_sync_update', {
        'binding': reviewed['apply_binding'], 'update_id': receipt['update_id'], 'timeout_seconds': 20})
    for _ in range(3):
        if observed['wait']['end'] == 'complete':
            break
        s.require(observed['wait']['end'] in ['deadline', 'poll_limit'], 'sync observation failed')
        observed = await mcp.tool('wait_for_resource_sync_update', {
            'binding': reviewed['apply_binding'], 'update_id': receipt['update_id'], 'timeout_seconds': 20})
    s.require(observed['wait']['end'] == 'complete' and observed['wait']['latest_update']['outcome'] == expected,
              'unexpected terminal RunSync outcome')
    s.require(not observed['selection_verified_by_core_history']
              and observed['caller_attribution'] == 'not_verified_by_history'
              and not result['workload_health_verified'], 'sync history or parent overclaimed evidence')
    entry = {'sync_id': reviewed['apply_binding']['sync_id'],
        'selection': reviewed['apply_binding']['selection'],
        'reviewed_source': reviewed['plan']['source'], 'update_id': receipt['update_id'],
        'outcome': expected, 'explicit_refresh_requests': 1, 'run_sync_requests': 1,
        'git_pull_cache_wait_ms': result['preflight']['git_pull_cache_wait_ms'],
        'parent_health_verified': False,
        'batch_reports': observed['wait']['latest_update'].get('observed_sync_result'),
        'resource_observation_incomplete': result['resource_observation_incomplete']}
    s.REPORT['resource_sync']['executions'].append(entry)
    s.persist()
    return result, observed


def whole():
    return {'kind': 'whole_sync'}


def named(kind, name=None, resource_id=None):
    return {'kind': 'resources', 'resource_type': kind,
            'targets': [{'id': resource_id} if resource_id else {'name': name}]}


async def inspect_server(mcp, name):
    actual = CONTEXT['provisioner'].read('GetServer', {'server': name})
    resource_id = s.identity(actual)
    remember('Server', resource_id, name)
    value = await mcp.tool('get_server', {'id': resource_id})
    s.require(value['id'] == resource_id and value['name'] == name
              and actual['config']['enabled'] is False, 'synced Server was not independently observed')
    return actual


async def exports(mcp):
    admin = CONTEXT['provisioner']
    for name, secret in [('MCP_SYNC_SECRET_EXPORT', True), ('MCP_SYNC_PLAIN_EXPORT', False)]:
        admin.write('CreateVariable', {'name': name, 'value': private_value(), 'is_secret': secret})
    result = await mcp.tool('export_resources_toml', {
        'targets': [{'kind': 'server', 'target': {'id': CONTEXT['server_id']}}],
        'include_variables': True})
    parsed = tomllib.loads(result['toml'])
    selected = admin.read('GetServer', {'server': CONTEXT['server_id']})['name']
    s.require([x['name'] for x in parsed['server']] == [selected]
              and all('value' not in x for x in parsed.get('variable', []))
              and not result['authoritative_apply_input'] and result['omitted_fields'] > 0,
              'scoped export leaked values, included unrelated resources or hid reuse limits')
    s.REPORT['resource_sync']['export'] = {'selected_server_id': CONTEXT['server_id'],
        'server_count': 1, 'all_variable_values_omitted': True,
        'authoritative_apply_input': False, 'selection_complete': result['selection_complete'],
        'omitted_fields': result['omitted_fields']}


async def local_profiles(sync_id):
    report = s.REPORT['resource_sync']['permissions']
    for operate, edit in [(False, False), (False, True), (True, False), (True, True)]:
        async with s.LossProxy(CONTEXT['address'], CONTEXT['admin_test'], False) as proxy:
            async with await s.Mcp.start(proxy.address, CONTEXT['admin_test'], operate, edit) as raw:
                names = {x['name'] for x in (await raw.rpc('tools/list', {}))['tools']}
                s.require(len(names) == 52 + 17 * operate + 46 * edit + 3 * (operate and edit),
                          'ResourceSync discovery capability count changed')
                before = proxy.write_attempts, proxy.execute_attempts
                before_reads = sum(proxy.read_operations.values())
                for name, enabled in [('apply_resource_sync', operate and edit),
                    ('refresh_resource_sync_pending', operate), ('apply_resource_sync_update', edit)]:
                    s.require((name in names) == bool(enabled), 'capability discovery disagrees')
                    if not enabled:
                        await raw_error(raw, name, {}, proxy=proxy)
                s.require((proxy.write_attempts, proxy.execute_attempts) == before,
                          'disabled direct invocation mutated Core')
                s.require(sum(proxy.read_operations.values()) == before_reads,
                          'disabled direct invocation read Core')
                mcp = AuditedMcp(raw, proxy)
                cached = await mcp.tool('get_resource_sync_pending', {'id': sync_id})
                if edit and not operate:
                    await save(mcp, None, resource_id=sync_id, patch={'pending_alert': False})
                if operate:
                    refreshed = await mcp.tool('refresh_resource_sync_pending', {'id': sync_id})
                    s.require(refreshed['outcome'] == 'refreshed', 'authorized standalone refresh failed')
                report.append({'local_operate': operate, 'local_edit': edit,
                    'cached_same_sync': cached['sync']['id'] == sync_id,
                    'disabled_direct_mutations': 0, 'disabled_direct_core_reads': 0})


async def upstream_permissions(sync_id, source):
    async with s.LossProxy(CONTEXT['address'], CONTEXT['reader'], False) as proxy:
        async with await s.Mcp.start(proxy.address, CONTEXT['reader'], True, True) as raw:
            mcp = AuditedMcp(raw, proxy)
            cached = await mcp.tool('get_resource_sync_pending', {'id': sync_id})
            denied = await mcp.tool('refresh_resource_sync_pending', {'id': sync_id})
            s.require(denied['outcome'] == 'denied' and denied['error']['kind'] == 'permission_denied',
                      'Core Read identity refreshed without Execute')
            reviewed = await review(mcp, sync_id, whole())
            denied = await mcp.tool('apply_resource_sync', {
                'review_reference': reviewed['review_reference'], 'binding': reviewed['apply_binding']})
            s.require(denied['outcome'] == 'not_submitted'
                and denied['preflight']['error']['kind'] == 'permission_denied'
                and denied['run_sync_requests_submitted'] == 0 and proxy.execute_attempts == 0,
                'Core Execute denial was bypassed or retried')
            s.REPORT['resource_sync']['permissions'].append({'identity': 'Core Read only',
                'same_sync': cached['sync']['id'] == sync_id, 'local_both': True,
                'standalone_and_apply_refresh_denied': True, 'refresh_attempts': proxy.write_attempts,
                'run_sync_requests': 0, 'credential_fallback': False})
    # A separate non-admin identity cannot create a ResourceSync even with both local grants.
    async with s.LossProxy(CONTEXT['address'], CONTEXT['operator'], False) as proxy:
        async with await s.Mcp.start(proxy.address, CONTEXT['operator'], True, True) as raw:
            value = {'name': PREFIX + 'denied-admin-create', 'config': config(source)}
            proposed = await raw.tool('preview_resource_sync_create', value)
            denied = safe(await raw.tool('apply_resource_sync_create', dict(value, preview_id=proposed['preview_id'])))
            s.require(not denied['saved_configuration_verified'] and denied['writes_submitted'] == 1
                      and proxy.write_attempts == 1 and proxy.execute_attempts == 0,
                      'Core admin-only sync creation changed')
            found = CONTEXT['provisioner'].read('ListResourceSyncs', {
                'query': {'names': [value['name']]}, 'page': 0, 'limit': 10})
            s.require(not found, 'non-admin created a sync or creation was retried')
            s.REPORT['resource_sync']['permissions'].append({'identity': 'non-admin operator',
                'local_both': True, 'admin_only_creation_denied': True,
                'write_attempts': 1, 'run_sync_requests': 0, 'credential_fallback': False})


async def source_workflow(creator, sources):
    host_name = sources['host']['name']
    before = external_evidence(host_name)
    for index, (kind, case) in enumerate(sources.items()):
        sync_id, name = case['id'], case['resource_name']
        raw_before = CONTEXT['provisioner'].read('GetResourceSync', {'sync': sync_id})['config']
        await save(creator, None, resource_id=sync_id, patch={'pending_alert': False})
        raw_after = CONTEXT['provisioner'].read('GetResourceSync', {'sync': sync_id})['config']
        s.require({k: v for k, v in raw_before.items() if k != 'pending_alert'} ==
                  {k: v for k, v in raw_after.items() if k != 'pending_alert'},
                  'one-field definition patch did not preserve omitted source/settings/secrets')
        async with s.LossProxy(CONTEXT['address'], CONTEXT['operator'], False) as proxy:
            async with await s.Mcp.start(proxy.address, CONTEXT['operator'], True, True) as raw:
                mcp = AuditedMcp(raw, proxy)
                cached = await mcp.tool('get_resource_sync_pending', {'id': sync_id})
                fresh = await mcp.tool('refresh_resource_sync_pending', {'id': sync_id})
                s.require(fresh['outcome'] == 'refreshed', 'source refresh failed')
                scope = whole() if index == 0 else {'kind': 'resource_type', 'resource_type': 'server'} if index == 1 else named('server', name)
                reviewed = await review(mcp, sync_id, scope)
                s.require((reviewed['plan']['source']['pending_git_revision'] is None) == (kind in ['inline', 'host']),
                          'source evidence invented/lost a Git revision')
                result, observed = await apply(mcp, reviewed)
                actual = await inspect_server(mcp, name)
                s.require(actual['config']['auto_prune'] is False, 'initial desired setting not applied')
                await raw_error(raw, 'apply_resource_sync', {
                    'review_reference': reviewed['review_reference'], 'binding': reviewed['apply_binding']}, 'invalid_preview', proxy)
                async with await s.Mcp.start(proxy.address, CONTEXT['operator'], False, False) as reader:
                    resumed = await reader.tool('wait_for_resource_sync_update', {
                        'binding': reviewed['apply_binding'], 'update_id': result['receipt']['update_id'], 'timeout_seconds': 10})
                    safe(resumed)
                    s.require(resumed['wait']['end'] == 'complete' and not resumed['automatic_resubmission'],
                              'known update failed read-only process restart')
                s.REPORT['resource_sync']['sources'].append({'kind': kind, 'sync_id': sync_id,
                    'definition_patch_omission_preserved': True, 'cached_observation': cached['observation'],
                    'scope': reviewed['apply_binding']['selection'], 'update_id': result['receipt']['update_id'],
                    'source_evidence': reviewed['plan']['source'], 'known_id_resumed_read_only': True,
                    'actual_resource_id': s.identity(actual), 'desired_configuration_verified_by_fixture': True,
                    'separate_explicit_refreshes': 1, 'apply_preflight_refreshes': 1, 'run_sync_requests': 1})
    unchanged(before, host_name)


async def resets_and_drift(creator, sources):
    admin, host_name = CONTEXT['provisioner'], sources['host']['name']
    inline = sources['inline']
    omitted = server_doc(inline['resource_name'], prune=None)
    await save(creator, None, resource_id=inline['id'], patch={'source': {'kind': 'inline', 'contents': omitted}})
    reviewed = await review(creator, inline['id'], whole())
    await apply(creator, reviewed)
    s.require(admin.read('GetServer', {'server': inline['resource_name']})['config']['auto_prune'] is True,
              'omitted desired-state field did not reset to Core default')
    s.REPORT['resource_sync']['omission_contracts'] = {
        'definition_patch_preserved_omitted_fields': True, 'desired_server_auto_prune_false_to_omitted_default_true': True}
    for kind in ['inline', 'git', 'host']:
        case = sources[kind]
        await creator.tool('refresh_resource_sync_pending', {'id': case['id']})
        old = await review(creator, case['id'], whole())
        changed = server_doc(case['resource_name'], prune=True) + '# owned source drift\n'
        if kind == 'inline':
            await save(creator, None, resource_id=case['id'], patch={'source': {'kind': 'inline', 'contents': changed}})
        elif kind == 'git':
            publish_git({case['filename']: changed})
        else:
            write_host(case['name'], changed)
        baseline = external_evidence(host_name)
        result = await creator.tool('apply_resource_sync', {
            'review_reference': old['review_reference'], 'binding': old['apply_binding']})
        s.REPORT['resource_sync'].setdefault('drift_attempts', []).append({
            'source': kind, 'outcome': result['outcome'], 'preflight': result['preflight'],
            'run_sync_requests': result['run_sync_requests_submitted'],
            'known_update_id': (result.get('receipt') or {}).get('update_id')})
        s.persist()
        s.require(result['outcome'] == 'not_submitted' and result['run_sync_requests_submitted'] == 0
                  and result['review_consumed'], 'changed source silently applied a new plan')
        if result['preflight']['refresh_requests_submitted']:
            s.require('no write' not in result['rejection']['message'].lower(),
                      'stale rejection denied its already submitted preflight refresh')
        await creator.tool('refresh_resource_sync_pending', {'id': case['id']})
        await apply(creator, await review(creator, case['id'], whole()))
        unchanged(baseline, host_name)
        s.REPORT['resource_sync'].setdefault('drift', []).append({'source': kind,
            'old_review_rejected': True, 'new_review_explicitly_required': True,
            'git_pull_cache_wait_ms': result['preflight']['git_pull_cache_wait_ms'],
            'rejected_apply_refreshes': result['preflight']['refresh_requests_submitted'], 'rejected_run_sync_requests': 0})
    linked = sources['linked_repo']
    await creator.tool('refresh_resource_sync_pending', {'id': linked['id']})
    old = await review(creator, linked['id'], whole())
    repo_id = a.CONTEXT['repo_id']
    original = admin.read('GetRepo', {'repo': repo_id})['config']['webhook_secret']
    if original:
        s.SECRETS.append(original)
    admin.write('UpdateRepo', {'id': repo_id, 'config': {'webhook_secret': private_value()}})
    result = await creator.tool('apply_resource_sync', {
        'review_reference': old['review_reference'], 'binding': old['apply_binding']})
    s.require(result['outcome'] == 'not_submitted' and result['run_sync_requests_submitted'] == 0
              and result['preflight']['refresh_requests_submitted'] == 0,
              'secret-only linked Repo baseline change did not invalidate review')
    admin.write('UpdateRepo', {'id': repo_id, 'config': {'webhook_secret': original}})
    s.REPORT['resource_sync']['drift'].append({'source': 'linked_repo_secret_reference',
        'old_review_rejected': True, 'rejected_apply_refreshes': 0, 'rejected_run_sync_requests': 0})


async def variable_group_scope(creator):
    admin = CONTEXT['provisioner']
    name, variable, group = PREFIX + 'vars-server', 'MCP_SYNC_DESIRED_VARIABLE', PREFIX + 'desired-group'
    secret = private_value()
    doc = server_doc(name) + ('[[variable]]\nname=' + json.dumps(variable) + '\nvalue=' + json.dumps(secret)
        + '\nis_secret=true\n[[user_group]]\nname=' + json.dumps(group) + '\neveryone=false\nusers=[]\n')
    sync_id = await save(creator, PREFIX + 'variables', config({'kind': 'inline', 'contents': doc},
        include_variables=True, include_user_groups=True))
    filtered = await review(creator, sync_id, named('server', name))
    s.require(not filtered['plan']['variables_in_review_scope']
              and not filtered['plan']['user_groups_in_review_scope'], 'filtered plan included variables/groups')
    await apply(creator, filtered)
    s.require(not any(x['name'] == variable for x in admin.read('ListVariables'))
              and not any(x['name'] == group for x in admin.read('ListUserGroups')),
              'filtered RunSync applied variables/groups')
    actual = await inspect_server(creator, name)
    unfiltered = await review(creator, sync_id, whole())
    s.require(unfiltered['plan']['variables_in_review_scope'] and unfiltered['plan']['user_groups_in_review_scope'],
              'whole-sync inclusion was not disclosed')
    await apply(creator, unfiltered)
    values = [x for x in admin.read('ListVariables') if x['name'] == variable]
    groups = [x for x in admin.read('ListUserGroups') if x['name'] == group]
    s.require(len(values) == len(groups) == 1 and values[0]['value'] == secret and values[0]['is_secret'],
              'whole-sync variable/group desired state not observed')
    exported = await creator.tool('export_resources_toml', {'targets': [{'kind': 'server', 'target': {'id': s.identity(actual)}}],
        'user_groups': [{'id': s.identity(groups[0])}], 'include_variables': True})
    parsed = tomllib.loads(exported['toml'])
    s.require([x['name'] for x in parsed.get('user_group', [])] == [group]
              and all('value' not in x for x in parsed.get('variable', [])), 'group selection or variable sanitization failed')
    s.REPORT['resource_sync']['variable_group_scope'] = {'filtered_applied_neither': True,
        'whole_sync_applied_both': True, 'secret_value_never_returned': True,
        'explicit_group_export_count': 1, 'resource_tags_do_not_isolate_global_scopes': True}


async def managed_delete(creator):
    admin, name, tag = CONTEXT['provisioner'], PREFIX + 'managed-target', PREFIX + 'managed'
    doc = server_doc(name, tag=tag)
    sync_id = await save(creator, PREFIX + 'managed', config({'kind': 'inline', 'contents': doc},
        managed=True, delete=False, match_tags=[tag]))
    await apply(creator, await review(creator, sync_id, named('server', name)))
    actual = await inspect_server(creator, name)
    resource_id = s.identity(actual)
    before_ids = {x['id'] for x in admin.read('ListServers', {'page': 0, 'limit': 100})}
    preview = await creator.tool('preview_resource_sync_update', {
        'target': {'id': sync_id}, 'patch': {'match_tags': []}})
    s.require(preview['definition']['details']['after']['resource_tag_filter_unrestricted'],
              'empty resource-tag restriction was hidden')
    await save(creator, None, resource_id=sync_id, patch={'source': {'kind': 'inline', 'contents': '# owned empty desired state\n'}})
    reviewed = await review(creator, sync_id, named('server', resource_id=resource_id))
    s.require(reviewed['plan']['settings']['effective_delete'] and not reviewed['plan']['settings']['delete']
              and reviewed['plan']['resource_changes']['items'][0]['action'] == 'delete',
              'managed-or-delete behavior was not represented')
    await apply(creator, reviewed)
    after_ids = {x['id'] for x in admin.read('ListServers', {'page': 0, 'limit': 100})}
    s.require(before_ids - after_ids == {resource_id}, 'managed tagged deletion affected unrelated resources')
    forget('Server', resource_id)
    s.REPORT['resource_sync']['managed_deletion'] = {'managed': True, 'delete_flag': False,
        'tag_filter': [tag], 'deleted_only_owned_id': resource_id,
        'empty_tag_preview_unrestricted': True, 'unrestricted_unmanaged_applies_exercised_by_four_sources': True}


async def partial_and_health(creator):
    admin = CONTEXT['provisioner']
    server_name = admin.read('GetServer', {'server': CONTEXT['server_id']})['name']
    good, bad = PREFIX + 'partial-good', PREFIX + 'partial-bad'
    invalid_ports = private_value()
    source = server_doc(good) + ('[[deployment]]\nname=' + json.dumps(bad)
        + '\n[deployment.config]\nserver_id=' + json.dumps(server_name) + '\nports=' + json.dumps(invalid_ports) + '\n')
    sync_id = await save(creator, PREFIX + 'partial', config({'kind': 'inline', 'contents': source}))
    result, observed = await apply(creator, await review(creator, sync_id, whole()), 'failed')
    await inspect_server(creator, good)
    s.require(observed['wait']['latest_update']['observed_sync_result']['resource_batches_failed'] >= 1,
              'partial resource batch failure was not reported')
    s.REPORT['resource_sync']['partial_failure'] = {'sync_id': sync_id,
        'update_id': result['receipt']['update_id'], 'parent_outcome': 'failed',
        'independent_successful_server_observed': True, 'raw_errors_omitted': True}
    # A child deployment can fail while the pinned parent reports success.
    workload = PREFIX + 'health-deployment'
    def deployment_doc(image, command=''):
        return ('[[deployment]]\nname=' + json.dumps(workload) + '\ndeploy=true\n[deployment.config]\nserver_id='
            + json.dumps(server_name) + '\nnetwork="bridge"\ncommand=' + json.dumps(command)
            + '\n[deployment.config.image]\ntype="Image"\n[deployment.config.image.params]\nimage=' + json.dumps(image) + '\n')
    source = deployment_doc(a.REGISTRY + '/owned-sync/missing:never')
    sync_id = await save(creator, PREFIX + 'health', config({'kind': 'inline', 'contents': source}))
    reviewed = await review(creator, sync_id, whole())
    s.require(reviewed['plan']['pending_deployments']['reported_total'] >= 1, 'pending deployment intent missing')
    result, observed = await apply(creator, reviewed)
    actual = admin.read('GetDeployment', {'deployment': workload})
    deployment_id = s.identity(actual)
    remember('Deployment', deployment_id, workload)
    children = admin.read('ListUpdates', {'query': {'target.type': 'Deployment', 'target.id': deployment_id}, 'page': 0})['updates']
    failed = [x for x in children if x['operation'] == 'Deploy' and x['status'] == 'Complete' and not x['success']]
    s.require(failed and not observed['wait']['latest_update']['observed_sync_result']['child_deployment_success_verified'],
              'parent success incorrectly established child deployment success')
    first = {'parent_update_id': result['receipt']['update_id'], 'parent_outcome': 'succeeded',
        'independently_observed_failed_child_id': failed[0]['id'],
        'child_link_evidence': 'isolated owned fixture observation; Core parent provides no reliable child association'}
    await save(creator, None, resource_id=sync_id, patch={'source': {'kind': 'inline',
        'contents': deployment_doc('busybox:1.37.0', 'sh -c "exit 19"')}})
    result, observed = await apply(creator, await review(creator, sync_id, whole()))
    s.poll(lambda: s.run([*s.COMPOSE, 'exec', '-T', 'docker', 'docker', 'inspect', workload,
        '--format', '{{.State.Status}}']) == 'exited', 'owned synced workload exit', 30)
    unhealthy = None
    for _ in range(30):
        fleet = await creator.tool('fleet_health', {})
        unhealthy = next((x for x in fleet['deployments'] if x['id'] == deployment_id), None)
        if unhealthy and unhealthy['health'] == 'unhealthy':
            break
        await asyncio.sleep(1)
    s.require(unhealthy and unhealthy['health'] == 'unhealthy'
              and not result['workload_health_verified'], 'sync success was conflated with actual workload health')
    s.REPORT['resource_sync']['deployment_health'] = {'failed_child_parent_success': first,
        'successful_parent_update_id': result['receipt']['update_id'], 'docker_state': 'exited',
        'independent_mcp_health': unhealthy, 'parent_health_verified': False}
    s.persist()
    # Retained Server-detachment acceptance requires all owned workloads running.
    # Restore this deliberately failed fixture through a new explicit sync, while
    # retaining the independent unhealthy observation and its original update ID.
    await save(creator, None, resource_id=sync_id, patch={'source': {'kind': 'inline',
        'contents': deployment_doc('busybox:1.37.0', 'sh -c "sleep 600"')}})
    restored, _ = await apply(creator, await review(creator, sync_id, whole()))
    s.poll(lambda: s.run([*s.COMPOSE, 'exec', '-T', 'docker', 'docker', 'inspect', workload,
        '--format', '{{.State.Status}}']) == 'running', 'owned synced workload restoration', 30)
    healthy = None
    for _ in range(30):
        fleet = await creator.tool('fleet_health', {})
        healthy = next((x for x in fleet['deployments'] if x['id'] == deployment_id), None)
        if healthy and healthy['health'] == 'healthy':
            break
        await asyncio.sleep(1)
    s.require(healthy and healthy['health'] == 'healthy', 'owned sync workload not restored before retained smoke')
    s.REPORT['resource_sync']['deployment_health']['restored_before_retained_core_smoke'] = {
        'new_explicit_parent_update_id': restored['receipt']['update_id'],
        'docker_state': 'running', 'independent_mcp_health': healthy}


async def pending_errors(creator):
    name = PREFIX + 'pending-error'
    sync_id = await save(creator, name, config({'kind': 'host', 'resource_paths': [RESOURCE_FILE]}))
    value = await creator.tool('refresh_resource_sync_pending', {'id': sync_id})
    s.require(value['pending']['cached_status'] == 'errors', 'missing owned host source error not observed')
    reviewed = await creator.tool('review_resource_sync_pending', {'target': {'id': sync_id}, 'selection': whole()})
    s.require(reviewed['review_reference'] is None and not reviewed['plan']['apply_coverage_complete'],
              'pending source errors authorized execution')
    await creator.tool('list_alerts', {'target': {'kind': 'resource_sync', 'id': sync_id}})
    s.REPORT['resource_sync']['pending_errors'] = {'sync_id': sync_id, 'reference_withheld': True,
        'run_sync_requests': 0, 'raw_error_details_omitted': True}


async def lost_ack_and_concurrency(creator):
    name, desired = PREFIX + 'uncertain', PREFIX + 'uncertain-target'
    sync_id = await save(creator, name, config({'kind': 'inline', 'contents': server_doc(desired)}))
    credentials = CONTEXT['operator']
    async with s.LossProxy(CONTEXT['address'], credentials, True) as lost, \
            s.LossProxy(CONTEXT['address'], credentials, False) as other:
        async with await s.Mcp.start(lost.address, credentials, True, True) as raw_a, \
                await s.Mcp.start(other.address, credentials, True, True) as raw_b:
            first, second = AuditedMcp(raw_a, lost), AuditedMcp(raw_b, other)
            reviewed_a = await review(first, sync_id, whole())
            reviewed_b = await review(second, sync_id, whole())
            request_a = {'review_reference': reviewed_a['review_reference'], 'binding': reviewed_a['apply_binding'], 'wait_timeout_seconds': 1}
            request_b = {'review_reference': reviewed_b['review_reference'], 'binding': reviewed_b['apply_binding'], 'wait_timeout_seconds': 1}
            async with asyncio.TaskGroup() as tasks:
                pending = tasks.create_task(first.tool('apply_resource_sync', request_a))
                # Overlap a second explicit caller with the first caller's lost
                # reply, while deterministically ensuring the first submitted.
                # This is fixture instrumentation, never gateway attribution.
                for _ in range(100):
                    if lost.ids:
                        break
                    await asyncio.sleep(.05)
                s.require(lost.ids, 'fixture did not observe the first Core acknowledgement')
                two = await second.tool('apply_resource_sync', request_b)
                one = await pending
            s.require(one['outcome'] == 'submission_unknown' and one['receipt']['update_id'] is None
                and lost.execute_attempts == 1 and other.execute_attempts <= 1
                and not one['automatic_resubmission'] and not two['automatic_resubmission'],
                'lost/concurrent execution was attributed or replayed')
            await raw_error(raw_a, 'apply_resource_sync', request_a, 'invalid_preview', lost)
            context = {'target': {'kind': 'resource_sync', 'id': sync_id},
                'requested_scope': {'kind': 'resource_sync', 'selection': reviewed_a['apply_binding']['selection']},
                'operation': 'run', 'submission_window': one['receipt']['submission_window']}
            s.REPORT['resource_sync']['lost_ack'] = {'sync_id': sync_id, 'original_submission': 'unknown',
                'known_update_id': None, 'instrumented_fixture_ack_ids': list(lost.ids),
                'instrumentation_is_not_gateway_attribution': True, 'first_run_sync_requests': 1,
                'concurrent_explicit_apply_run_sync_requests': other.execute_attempts,
                'second_outcome': two['outcome'], 'automatic_replay': False}
    async with s.LossProxy(CONTEXT['address'], credentials, False) as observer:
        async with await s.Mcp.start(observer.address, credentials, False, False) as raw:
            recovered = safe(await raw.tool('reconcile_operation', context))
            s.require(recovered['original_submission'] == 'unknown' and recovered['attribution'] == 'unconfirmed'
                and all(x['scope_evidence'] == 'unavailable' for x in recovered['candidates'])
                and not recovered['automatic_resubmission'] and observer.execute_attempts == observer.write_attempts == 0,
                'read-only restart reconciliation chose a candidate or replayed')
            s.REPORT['resource_sync']['lost_ack']['recovery_candidates'] = [x['update']['id'] for x in recovered['candidates']]
            s.REPORT['resource_sync']['lost_ack']['restart_read_only_reconciled'] = True


async def exercise():
    sources = {}
    documents = {kind: server_doc(PREFIX + kind + '-target') for kind in ['inline', 'git', 'linked_repo', 'host']}
    revision = publish_git({'resources-git.toml': documents['git'], 'resources-linked.toml': documents['linked_repo']})
    async with s.LossProxy(CONTEXT['address'], CONTEXT['admin_test'], False) as proxy:
        async with await s.Mcp.start(proxy.address, CONTEXT['admin_test'], True, True) as raw:
            creator = AuditedMcp(raw, proxy)
            await exports(creator)
            variants = {'inline': {'kind': 'inline', 'contents': documents['inline']},
                'git': {'kind': 'git', 'repo': a.REPOSITORY, 'branch': 'main', 'provider': 'source',
                        'https': True, 'resource_paths': ['resources-git.toml']},
                'linked_repo': {'kind': 'linked_repo', 'target': {'id': a.CONTEXT['repo_id']},
                                'resource_paths': ['resources-linked.toml']},
                'host': {'kind': 'host', 'resource_paths': [RESOURCE_FILE]}}
            for kind, source in variants.items():
                name = PREFIX + kind
                sync_id = await save(creator, name, config(source))
                sources[kind] = {'id': sync_id, 'name': name, 'resource_name': PREFIX + kind + '-target',
                                 'filename': 'resources-git.toml' if kind == 'git' else 'resources-linked.toml'}
            write_host(sources['host']['name'], documents['host'])
            await local_profiles(sources['inline']['id'])
            await upstream_permissions(sources['inline']['id'], variants['inline'])
            await source_workflow(creator, sources)
            await resets_and_drift(creator, sources)
            external = external_evidence(sources['host']['name'])
            await variable_group_scope(creator)
            await managed_delete(creator)
            await partial_and_health(creator)
            await pending_errors(creator)
            await lost_ack_and_concurrency(creator)
            final_external = unchanged(external, sources['host']['name'])
            s.REPORT['resource_sync']['external_sources'] = {'initial_fixture_git_revision': revision,
                'deliberate_fixture_drift_exercised': True, 'after_last_fixture_change': external,
                'after_mcp_workflow': final_external, 'product_external_write_back': False}
            s.REPORT['resource_sync']['mutation_summary'] = {'writes': proxy.write_attempts,
                'RunSync': proxy.execute_attempts, 'read_only_replay_requests': 0,
                'per_explicit_apply_max_refresh': 1, 'per_explicit_apply_max_RunSync': 1}
    s.REPORT['resource_sync']['passed'] = True
    s.persist()
    print('Verified integrated four-source ResourceSync workflow, omissions, scopes, deletion, drift, recovery and independent health.', flush=True)


def provision(context):
    # Reuse the entire automation extension, not only its initial infrastructure.
    a.provision(context)
    CONTEXT.update(context, admin_test=a.CONTEXT['admin_test'])
    s.REPORT['resource_sync'] = {'resources': [], 'executions': [], 'sources': [], 'permissions': [],
        'provisioner_credentials_passed_to_mcp': False,
        'retained_full_automation_extension': True,
        'evidence_limits': [
            'Core rereads sources without an expected revision; no frozen plan/CAS/exactly-once/rollback/cancellation guarantee.',
            'MCP counts its explicit refresh/RunSync requests, not internal Core refreshes or configured automatic effects.',
            'Core history cannot verify RunSync selectors or invocation ownership. Unknown candidates are never selected/replayed.',
            'Parent batch success does not verify child deployment success or workload health; independent evidence is separate.',
            'Expiry, malformed-wire and some timing/permission races are deterministic fixture coverage; live faults are not exhaustive.']}
    asyncio.run(exercise())


def disable_automatic_execution():
    failures = []
    disabled = []
    if CONTEXT:
        admin = CONTEXT['provisioner']
        for kind, resource_id, name in OWNED:
            try:
                field = 'sync' if kind == 'ResourceSync' else kind.lower()
                current = admin.read('Get' + kind, {field: resource_id})
                s.require(s.identity(current) == resource_id and current['name'] == name and name.startswith(PREFIX),
                          'owned ResourceSync fixture identity changed before cleanup')
                patch = {'webhook_enabled': False} if kind == 'ResourceSync' else {
                    'auto_update': False, 'poll_for_updates': False} if kind == 'Deployment' else None
                if patch is not None:
                    admin.write('Update' + kind, {'id': resource_id, 'config': patch})
                    checked = admin.read('Get' + kind, {field: resource_id})['config']
                    s.require(all(checked[k] == v for k, v in patch.items()), 'owned automatic execution disable not verified')
                    disabled.append({'kind': kind, 'id': resource_id, 'verified_disabled': True})
            except BaseException as error:
                failures.append(error)
    # Always disable retained automation too, even when one sync check fails.
    try:
        a.disable_automatic_execution()
    except BaseException as error:
        failures.append(error)
    s.REPORT['resource_sync_automatic_execution_cleanup'] = {'owners': disabled,
        'verified': not failures, 'before_compose_down': True}
    if failures:
        raise s.Failure('owned ResourceSync/automation trigger cleanup could not be verified')


def entry(argv=None):
    CONTEXT.clear()
    OWNED.clear()
    a.CONTEXT.clear()
    a.OWNED.clear()
    a.ACTIVE.clear()
    return s.entry(argv, fixture_customizer=customize, exercise_extension=provision,
                   before_cleanup=disable_automatic_execution)


if __name__ == '__main__':
    raise SystemExit(entry())
