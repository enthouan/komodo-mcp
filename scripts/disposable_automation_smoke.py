#!/usr/bin/env python3
"""Opt-in v0.5 acceptance against an owned Core/Periphery 2.3.2 environment.

Runs the existing disposable stdio acceptance first, then the automation chain.
Requires an explicit local Docker context, unused project and new evidence path.
No external Core, host Docker socket, AWS or distribution publication is used.
"""
import asyncio
import json
import secrets
import time

import disposable_core_smoke as s


CONTEXT = {}
OWNED = []
ACTIVE = []
PREFIX = 'mcp-automation-'
REPOSITORY = 'fixture/workload.git'
REGISTRY = 'registry.test:5000'


def customize(config):
    labels = {'komodo-mcp.test.session': s.OWNER,
              'komodo-mcp.test.scope': 'disposable-stdio-smoke'}
    config['services']['docker']['command'] = ['--insecure-registry=' + REGISTRY]
    config['services']['core']['environment']['KOMODO_TIMEZONE'] = 'Asia/Tokyo'
    for service in ['core', 'periphery']:
        config['services'][service]['environment']['GIT_SSL_CAINFO'] = '/tmp/automation-source-ca.crt'
    config['services']['registry'] = {'image': 'registry:2.8.3', 'labels': dict(labels),
        'networks': {'default': {'aliases': ['registry.test']}}}
    config['services']['source'] = {'image': 'nginx:1.27.5-alpine', 'labels': dict(labels),
        'command': ['sh', '-c', 'while [ ! -f /etc/nginx/automation-ready ]; do sleep 1; done; exec nginx -g "daemon off;"']}


def api(credentials):
    return s.Api(CONTEXT['address'], {'x-api-key': credentials['key'],
                                    'x-api-secret': credentials['secret']})


def grant(kind, resource_id):
    for user in CONTEXT['users'][:3]:
        CONTEXT['provisioner'].write('UpdatePermissionOnTarget', {
            'user_target': {'type': 'User', 'id': user['id']},
            'resource_target': {'type': kind, 'id': resource_id},
            'permission': {'level': user['workload_permission'],
                'specific': ['Attach'] if kind in ['Builder', 'Repo', 'Build']
                    and user['workload_permission'] == 'Write' else []}})


def remember(kind, resource_id, name):
    s.require(name.startswith(PREFIX), 'automation resource outside owned name prefix')
    OWNED.append((kind, resource_id, name))
    grant(kind, resource_id)
    s.REPORT['automation']['resources'].append({'kind': kind, 'id': resource_id, 'name': name})


class AuditedMcp:
    """Count actual mutation attempts around each serialized explicit tool call."""
    def __init__(self, mcp, proxy):
        self.mcp, self.proxy = mcp, proxy

    async def tool(self, name, arguments):
        executes, writes = self.proxy.execute_attempts, self.proxy.write_attempts
        try:
            return await self.mcp.tool(name, arguments)
        finally:
            delta = self.proxy.execute_attempts - executes
            saved = self.proxy.write_attempts - writes
            calls = s.REPORT['automation'].setdefault('per_call_mutation_audit', [])
            s.require(len(calls) < 1000, 'automation call audit exceeded bound')
            calls.append({'tool': name, 'execute_attempts': delta, 'write_attempts': saved})
            s.require(delta <= 1 and saved <= 1 and delta + saved <= 1,
                      'explicit tool call submitted more than one mutation')
            if name.startswith(('get_', 'list_', 'preview_', 'wait_', 'reconcile_')) or name == 'fleet_health':
                s.require(delta == saved == 0, 'read-only observation submitted a mutation')


async def save(mcp, family, value, create=False):
    operation = family + ('_create' if create else '_update')
    preview = await mcp.tool('preview_' + operation, value)
    result = await mcp.tool('apply_' + operation, dict(value, preview_id=preview['preview_id']))
    s.REPORT['automation'].setdefault('configuration_results', []).append({
        'operation': operation, 'resource': result['resource'], 'outcome': result['outcome'],
        'writes_submitted': result['writes_submitted'],
        'saved_configuration_verified': result['saved_configuration_verified'],
        'observation_error': result.get('observation_error')})
    # A known created ID remains owned even when verification fails. Retain it
    # for policy disabling; never retry an uncertain create automatically.
    if create and not result['saved_configuration_verified'] and result['resource'].get('id'):
        name = value['name']
        s.require(name.startswith(PREFIX), 'unverified resource outside owned name prefix')
        OWNED.append((family.title(), result['resource']['id'], name))
    s.persist()
    s.require(result['saved_configuration_verified'], 'authoritative ' + operation + ' readback unavailable')
    s.require(result['writes_submitted'] <= 1, 'configuration was submitted more than once')
    return result


async def create(mcp, family, name, config):
    result = await save(mcp, family, {'name': name, 'config': config}, True)
    resource_id = result['resource']['id']
    remember(family.title(), resource_id, name)
    return resource_id


async def patch(mcp, family, resource_id, config):
    return await save(mcp, family, {'target': {'id': resource_id}, 'patch': config})


async def arguments(mcp, family, resource_id):
    detail = await mcp.tool('get_' + family, {'id': resource_id})
    value = {'target': {'id': resource_id}}
    if family == 'repo':
        value['expected_hooks'] = {key: detail['definition'][key + '_configured']
                                   for key in ['on_clone', 'on_pull']}
    else:
        effects = detail['configured_effects'] if family == 'build' else detail['effects']
        value['expected_effects'] = effects['acknowledgement']
    return value


async def finish(mcp, receipt, expected='succeeded'):
    s.require(receipt['submission'] == 'accepted' and receipt['update_id'],
              'execution acknowledgement unavailable')
    update_id = receipt['update_id']
    ACTIVE.append((receipt['resource']['kind'], receipt['resource']['id'], update_id))
    update = await s.observe(mcp, update_id)
    ACTIVE.remove((receipt['resource']['kind'], receipt['resource']['id'], update_id))
    evidence = {'operation': receipt['core_operation'], 'resource': receipt['resource'],
        'update_id': update_id, 'submission': receipt['submission'], 'outcome': update['outcome'],
        'stages': [{'name': x['stage'], 'success': x['success'], 'stderr': x.get('stderr')}
                   for x in update['stages']]}
    for key in ['observed_revision', 'observed_build_result', 'observed_procedure_result', 'observed_action_result']:
        if update.get(key) is not None:
            evidence[key] = update[key]
    s.REPORT['automation']['executions'].append(evidence)
    s.persist()
    s.require(update['outcome'] == expected, 'unexpected terminal execution outcome for ' + receipt['core_operation'])
    print('Verified automation execution:', receipt['core_operation'], expected, flush=True)
    return update


async def run(mcp, family, resource_id, expected='succeeded', **extra):
    value = await arguments(mcp, family, resource_id)
    value.update(extra)
    tool = 'clone_repo' if family == 'repo' else 'run_' + family
    return await finish(mcp, await mcp.tool(tool, value), expected)


def source_fixture():
    # Trust a private fixture CA explicitly for HTTPS Git;
    # do not disable certificate verification or expose the source on host ports.
    cert, key = s.ROOT / 'source-ca.crt', s.ROOT / 'source.key'
    config = s.ROOT / 'source-openssl.cnf'
    config.write_text('[req]\ndistinguished_name=dn\nx509_extensions=ext\nprompt=no\n'
        '[dn]\nCN=source\n[ext]\nsubjectAltName=DNS:source\nbasicConstraints=critical,CA:TRUE\n'
        'keyUsage=critical,digitalSignature,keyEncipherment,keyCertSign\n')
    s.run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1',
        '-keyout', str(key), '-out', str(cert), '-config', str(config)])
    nginx = s.ROOT / 'source-nginx.conf'
    nginx.write_text('events {}\nhttp { server { listen 443 ssl; server_name source; '
        'ssl_certificate /etc/nginx/source-ca.crt; ssl_certificate_key /etc/nginx/source.key; '
        'root /www; } }\n')
    source_container = s.run([*s.COMPOSE, 'ps', '-q', 'source'])
    for source, destination in [(cert, '/etc/nginx/source-ca.crt'), (key, '/etc/nginx/source.key'),
                                (nginx, '/etc/nginx/nginx.conf')]:
        s.run([*s.DOCKER, 'cp', str(source), source_container + ':' + destination])
    for service in ['core', 'periphery']:
        container = s.run([*s.COMPOSE, 'ps', '-q', service])
        s.run([*s.DOCKER, 'cp', str(cert), container + ':/tmp/automation-source-ca.crt'])
    s.run([*s.COMPOSE, 'exec', '-T', 'source', 'touch', '/etc/nginx/automation-ready'])
    directory = s.ROOT / 'workload'
    directory.mkdir()
    marker = 'owned-revision-' + secrets.token_hex(12)
    (directory / 'revision.txt').write_text(marker + '\n')
    (directory / 'Dockerfile').write_text(
        'FROM busybox:1.37.0\nCOPY revision.txt /revision.txt\n'
        'HEALTHCHECK --interval=1s --timeout=1s --retries=2 CMD test -s /revision.txt\n'
        'CMD ["sh", "-c", "sleep 3600"]\n')
    s.run(['git', 'init', '--initial-branch=main', str(directory)])
    s.run(['git', '-C', str(directory), 'add', 'Dockerfile', 'revision.txt'])
    s.run(['git', '-C', str(directory), '-c', 'user.name=Disposable fixture', '-c',
           'user.email=fixture@example.invalid', 'commit', '-m', 'add owned automation workload'])
    revision = s.run(['git', '-C', str(directory), 'rev-parse', 'HEAD'])
    bare = s.ROOT / 'workload.git'
    s.run(['git', 'clone', '--bare', str(directory), str(bare)])
    s.run(['git', '--git-dir', str(bare), 'update-server-info'])
    container = s.run([*s.COMPOSE, 'ps', '-q', 'source'])
    s.run([*s.COMPOSE, 'exec', '-T', 'source', 'mkdir', '-p', '/www/fixture'])
    s.run([*s.DOCKER, 'cp', str(bare), container + ':/www/fixture/workload.git'])
    return revision, marker


def provision(context):
    CONTEXT.update(context)
    s.REPORT['automation'] = {'resources': [], 'executions': [], 'permission_checks': [],
        'source': {}, 'no_aws_resources': True,
        'evidence_limits': [
            'Core does not link asynchronous dependent redeployments to their Build update; this test deploys explicitly.',
            'A successful execution is separate from independently observed workload health.',
            'Lost acknowledgements remain unknown; instrumentation is test evidence, not caller attribution.',
            'Schedules report Core cache evidence; no atomic cache generation, scheduled invocation or rollback is claimed.',
            'Live fault coverage complements deterministic stdio races; it does not exhaust all timing interleavings.']}
    admin = CONTEXT['provisioner']
    admin.write('UpdateUserBasePermissions', {'user_id': context['users'][2]['id'],
        'enabled': True, 'create_builds': True})
    user = admin.write('CreateServiceUser', {'username': PREFIX + 'admin-test',
        'description': 'Explicit admin test identity, separate from provisioning'})
    user_id = s.identity(user)
    admin.write('UpdateUserBasePermissions', {'user_id': user_id, 'enabled': True})
    admin.write('UpdateUserAdmin', {'user_id': user_id, 'admin': True})
    keys = admin.write('CreateApiKeyForServiceUser', {'user_id': user_id,
        'name': 'owned-automation-acceptance', 'expires': int(time.time() * 1000) + 3600000})
    s.SECRETS.extend([keys['key'], keys['secret']])
    caller = api(keys).request('/user')
    s.require(caller['admin'] and not caller['super_admin'] and s.identity(caller) == user_id,
              'admin test identity was not separately established')
    CONTEXT['admin_test'] = keys
    s.REPORT['automation']['admin_test_identity'] = {'id': user_id, 'admin': True,
        'super_admin': False, 'provisioner_credentials_passed_to_mcp': False}
    token = secrets.token_hex(32)
    s.SECRETS.append(token)
    admin.write('CreateImageRegistryAccount', {'account': {'domain': REGISTRY,
        'username': 'fixture', 'token': token}})
    revision, marker = source_fixture()
    CONTEXT['revision'], CONTEXT['marker'] = revision, marker
    s.REPORT['automation']['source'] = {'revision': revision, 'provider': 'source',
        'repository': REPOSITORY, 'branch': 'main'}
    asyncio.run(exercise())


async def permission_profiles(repo_id, action_id):
    checks = s.REPORT['automation']['permission_checks']
    for operate, edit in [(False, False), (False, True), (True, False), (True, True)]:
        async with s.LossProxy(CONTEXT['address'], CONTEXT['editor'], False) as proxy:
            async with await s.Mcp.start(proxy.address, CONTEXT['editor'], operate, edit) as mcp:
                names = {x['name'] for x in (await mcp.rpc('tools/list', {}))['tools']}
                s.require(len(names) == 58 + 20 * operate + 54 * edit + 3 * (operate and edit),
                          'capability discovery changed')
                for name, allowed in [('run_action', operate), ('apply_schedule_update', edit)]:
                    if allowed:
                        continue
                    denied = await mcp.rpc('tools/call', {'name': name, 'arguments': {}}, True)
                    s.require('protocol_error' in denied, 'disabled direct tool was callable')
                if edit and not operate:
                    denied = await mcp.rpc('tools/call', {'name': 'apply_schedule_update', 'arguments': {
                        'owner_kind': 'action', 'target': {'id': action_id},
                        'patch': {'enabled': True}, 'preview_id': 'no-side-effect'}}, True)
                    s.require(denied.get('isError'), 'schedule effect gate was bypassed')
                s.require(proxy.execute_attempts == proxy.write_attempts == 0,
                          'denied capability profile reached mutation API')
                checks.append({'operate': operate, 'configuration_edit': edit,
                    'tool_count': len(names), 'denied_mutation_requests': 0})
    async with await s.Mcp.start(CONTEXT['address'], CONTEXT['reader'], True, True) as denied:
        receipt = await denied.tool('pull_repo', await arguments(denied, 'repo', repo_id))
        update = await finish(denied, receipt, 'failed')
        s.require(any('permission' in (x.get('stderr') or '').lower() for x in update['stages']),
                  'upstream Execute denial not observed')
        checks.append({'identity': 'non-admin Read', 'local_grants': 'both',
            'upstream_execute_denied': True, 'update_id': receipt['update_id']})
        result = await denied.rpc('tools/call', {'name': 'preview_action_create', 'arguments': {
            'name': PREFIX + 'denied', 'config': {'source': ''}}}, True)
        # Preview is read-only; Core's admin-only create rejection occurs at apply.
        if not result.get('isError'):
            result = await denied.rpc('tools/call', {'name': 'apply_action_create', 'arguments': {
                'name': PREFIX + 'denied', 'config': {'source': ''},
                'preview_id': result['structuredContent']['preview_id']}}, True)
        content = result.get('structuredContent', {})
        s.require((result.get('isError') or content.get('observation_error', {}).get('kind') == 'permission_denied')
                  and not content.get('saved_configuration_verified'),
                  'non-admin Action creation unexpectedly succeeded')
        checks.append({'identity': 'non-admin Read', 'admin_only_action_creation_denied': True})


async def schedule_cycle(mcp, kind, resource_id):
    raw = api(CONTEXT['editor']).read('Get' + kind.title(), {kind: resource_id})['config']
    reports = []
    for config in [
        {'expression': '0 0 9 * * *', 'format': 'cron', 'timezone': 'America/New_York',
         'enabled': True, 'alert': False},
        {'expression': 'every day at 9:00 am', 'format': 'english', 'timezone': ''},
        {'enabled': False}, {'expression': ''}]:
        await save(mcp, 'schedule', {'owner_kind': kind, 'target': {'id': resource_id}, 'patch': config})
        detail = await mcp.tool('get_schedule', {'owner_kind': kind, 'target': {'id': resource_id}})
        if config.get('enabled') is True or config.get('format') == 'english':
            reported = detail['observation']['reported']
            s.require(detail['observation']['status'] == 'reported' and reported and
                      reported['next_scheduled_run'] is not None and reported['schedule_error'] is None,
                      'Core effective next run was not observed')
        reports.append({'saved': detail['saved'], 'observation': detail['observation']})
        current = api(CONTEXT['editor']).read('Get' + kind.title(), {kind: resource_id})['config']
        ignored = {'schedule', 'schedule_format', 'schedule_timezone', 'schedule_enabled', 'schedule_alert'}
        s.require({k: v for k, v in current.items() if k not in ignored} ==
                  {k: v for k, v in raw.items() if k not in ignored},
                  'schedule changed unrelated saved automation')
    s.REPORT['automation'].setdefault('schedule_cycles', []).append({
        'owner_kind': kind, 'owner_id': resource_id, 'saved_definition_preserved': True,
        'core_default_timezone': 'Asia/Tokyo', 'observations': reports})


async def lost_ack_and_concurrency(action_id):
    async with s.LossProxy(CONTEXT['address'], CONTEXT['operator']) as first_proxy, \
            s.LossProxy(CONTEXT['address'], CONTEXT['editor']) as second_proxy:
        async def invoke(proxy, credentials):
            async with await s.Mcp.start(proxy.address, credentials) as mcp:
                value = await arguments(mcp, 'action', action_id)
                value['args'] = {'delay_ms': 1000}
                result = await mcp.tool('run_action', value)
                s.require(result['submission'] == 'unknown' and result['update_id'] is None and
                          not result['automatic_resubmission'], 'lost acknowledgement was attributed')
                return result
        receipts = await asyncio.gather(invoke(first_proxy, CONTEXT['operator']),
                                        invoke(second_proxy, CONTEXT['editor']))
        ids = first_proxy.ids + second_proxy.ids
        s.require(first_proxy.execute_attempts == second_proxy.execute_attempts == 1
                  and len(ids) == len(set(ids)) == 2,
                  'concurrent explicit calls did not each submit exactly once')
        async with s.LossProxy(CONTEXT['address'], CONTEXT['reader'], False) as reader_proxy:
            async with await s.Mcp.start(reader_proxy.address, CONTEXT['reader'], False) as restarted:
                recovery = await restarted.tool('reconcile_operation', {
                    'target': {'kind': 'action', 'id': action_id},
                    'requested_scope': {'kind': 'whole_action'}, 'operation': 'run'})
                s.require(recovery['original_submission'] == 'unknown' and
                          recovery['attribution'] == 'unconfirmed' and not recovery['automatic_resubmission'],
                          'restart reconciliation falsely attributed another caller')
                found = {x['update']['id'] for x in recovery['candidates']}
                s.require(set(ids) <= found, 'concurrent live candidates missing')
                for update_id in ids:
                    s.require((await s.observe(restarted, update_id))['outcome'] == 'succeeded',
                              'instrumented concurrent Action failed')
            s.require(reader_proxy.execute_attempts == reader_proxy.write_attempts == 0,
                      'read-only recovery submitted work')
        s.REPORT['automation']['lost_ack_concurrent_actions'] = {'explicit_calls': 2,
            'execute_attempts': first_proxy.execute_attempts + second_proxy.execute_attempts,
            'distinct_upstream_identities': 2, 'instrumented_update_ids': ids,
            'submission_windows': [x['submission_window'] for x in receipts],
            'original_submission': 'unknown', 'attribution': 'unconfirmed',
            'recovery_candidates': len(recovery['candidates']), 'recovery_mutation_requests': 0}


async def cancel_action(mcp, action_id):
    value = await arguments(mcp, 'action', action_id)
    value['args'] = {'delay_ms': 10000}
    receipt = await mcp.tool('run_action', value)
    update_id = receipt['update_id']
    s.require(receipt['submission'] == 'accepted', 'long Action not accepted')
    expired = await mcp.tool('wait_for_update', {'id': update_id, 'timeout_seconds': 1})
    s.require(expired['end'] == 'deadline' and expired['latest_update']['outcome'] == 'running',
              'expired local wait did not leave remote execution running')
    result = await mcp.tool('cancel_action', {'target': {'id': action_id}, 'expected_update_id': update_id})
    s.require(result['cancellation_receipt']['submission'] == 'accepted', 'explicit cancellation not acknowledged')
    original = await s.observe(mcp, update_id)
    s.require(original['outcome'] == 'failed', 'cancelled Action was not independently observed terminal')
    # A completed run must never cancel a newer invocation of the same Action.
    replacement = await mcp.tool('run_action', value)
    stale = await mcp.tool('cancel_action', {'target': {'id': action_id}, 'expected_update_id': update_id})
    s.require(stale['cancellation_receipt'] is None, 'completed Action cancellation targeted a replacement')
    await mcp.tool('cancel_action', {'target': {'id': action_id},
                                  'expected_update_id': replacement['update_id']})
    await s.observe(mcp, replacement['update_id'])
    s.REPORT['automation']['action_cancellation'] = {'original_update_id': update_id,
        'local_wait_end': expired['end'], 'local_wait_cancelled_remote': False,
        'cancellation_acknowledged': True, 'original_terminal_outcome': original['outcome'],
        'replacement_update_id': replacement['update_id'], 'completed_run_cancel_submissions': 0,
        'limit': 'Core token/process cancellation is not rollback or child Core execution cancellation.'}


async def wait_marker(path):
    for _ in range(80):
        try:
            await asyncio.to_thread(s.run, [*s.COMPOSE, 'exec', '-T', 'periphery', 'test', '-f', path])
            return
        except s.Failure:
            await asyncio.sleep(.25)
    raise s.Failure('owned pre-build marker deadline')


async def fault_cases(mcp, repo_id, build_id, procedure_id, action_id, stages):
    """Inject bounded faults only inside the owned source/registry/automation."""
    faults = s.REPORT['automation'].setdefault('faults', [])
    await patch(mcp, 'repo', repo_id, {'on_pull': {'path': '.', 'command': 'exit 14', 'shell_mode': True}})
    update = await finish(mcp, await mcp.tool('pull_repo', await arguments(mcp, 'repo', repo_id)), 'failed')
    s.require(any(x['stage'] == 'On Pull' and not x['success'] for x in update['stages']),
              'failed Repo hook stage was not identified')
    faults.append({'case': 'repo_hook', 'update_id': update['id'], 'failed_stage': 'On Pull'})
    await patch(mcp, 'repo', repo_id, {'on_pull': {'path': '.', 'command': '', 'shell_mode': True}})
    await patch(mcp, 'build', build_id, {'pre_build': {'path': '.', 'command': 'exit 15', 'shell_mode': True}})
    update = await run(mcp, 'build', build_id, 'failed')
    s.require(any(x['stage'] == 'Pre Build' and not x['success'] for x in update['stages']),
              'failed pre-build stage was not identified')
    faults.append({'case': 'pre_build_hook', 'update_id': update['id'], 'failed_stage': 'Pre Build'})
    await patch(mcp, 'build', build_id, {'pre_build': {'path': '.', 'command': '', 'shell_mode': True},
                                       'dockerfile_path': 'OwnedMissingDockerfile'})
    update = await run(mcp, 'build', build_id, 'failed')
    s.require(any(x['stage'] == 'Docker Build' and not x['success'] for x in update['stages']),
              'failed Docker build stage was not identified')
    faults.append({'case': 'docker_build', 'update_id': update['id'], 'failed_stage': 'Docker Build'})
    # Login precedes Pre Build in pinned Periphery. Stop the registry only after
    # the marker, proving this is the Docker build/push phase rather than login.
    marker = '/tmp/owned-automation-push-' + secrets.token_hex(8)
    await patch(mcp, 'build', build_id, {'dockerfile_path': 'Dockerfile', 'pre_build': {
        'path': '.', 'command': 'touch ' + marker + '; sleep 3', 'shell_mode': True}})

    async def interrupt_registry():
        await wait_marker(marker)
        await asyncio.to_thread(s.run, [*s.COMPOSE, 'stop', 'registry'], timeout=30)

    interrupt = asyncio.create_task(interrupt_registry())
    try:
        update = await run(mcp, 'build', build_id, 'failed')
        await interrupt
        s.require(any(x['stage'] == 'Docker Build' and not x['success'] for x in update['stages'])
                  and not any(x['stage'] == 'Docker Login' and not x['success'] for x in update['stages']),
                  'registry fault did not reach build/push after successful login')
        faults.append({'case': 'registry_push_after_login', 'update_id': update['id'],
                       'failed_stage': 'Docker Build', 'registry_interruption_instrumented': True})
    finally:
        if not interrupt.done():
            interrupt.cancel()
        await asyncio.gather(interrupt, return_exceptions=True)
        await asyncio.to_thread(s.run, [*s.COMPOSE, 'start', 'registry'], timeout=30)
    # Resource-level Build cancellation is acknowledged separately from the
    # original pre-build process's terminal update; no rollback is claimed.
    marker = '/tmp/owned-automation-cancel-' + secrets.token_hex(8)
    await patch(mcp, 'build', build_id, {'pre_build': {'path': '.',
        'command': 'touch ' + marker + '; sleep 15', 'shell_mode': True}})
    receipt = await mcp.tool('run_build', await arguments(mcp, 'build', build_id))
    await wait_marker(marker)
    cancelled = await mcp.tool('cancel_build', {'target': {'id': build_id},
                                             'expected_update_id': receipt['update_id']})
    s.require(cancelled['cancellation_receipt']['submission'] == 'accepted', 'Build cancellation not acknowledged')
    original = await s.observe(mcp, receipt['update_id'])
    s.require(original['outcome'] == 'failed', 'Build cancellation original not independently terminal')
    faults.append({'case': 'explicit_build_cancel', 'original_update_id': receipt['update_id'],
        'cancel_update_id': cancelled['cancellation_receipt']['update_id'],
        'original_terminal_outcome': original['outcome'], 'targeting_limit': cancelled['targeting_limit']})
    await patch(mcp, 'build', build_id, {'pre_build': {'path': '.', 'command': '', 'shell_mode': True}})
    # A parallel stage has one successful and one failed child. Later stages do
    # not convert partial work into parent success or deployment-health evidence.
    partial = [{'name': 'owned-partial-stage', 'enabled': True, 'executions': [
        {'enabled': True, 'execution': {'kind': 'run_action', 'resource_id': action_id, 'args': {'fail': failed}}}
        for failed in [False, True]]},
        {'name': 'must-not-run', 'enabled': True, 'executions': [
            {'enabled': True, 'execution': {'kind': 'sleep', 'duration_ms': 1}}]}]
    await patch(mcp, 'procedure', procedure_id, {'stages': partial})
    update = await run(mcp, 'procedure', procedure_id, 'failed')
    faults.append({'case': 'parallel_child_partial_failure', 'update_id': update['id'],
        'parent_outcome': update['outcome'], 'observed': update.get('observed_procedure_result'),
        'child_link_limit': 'Only explicit Core-reported child IDs, never inferred from timing.'})
    cancellable = [{'name': 'current-stage', 'enabled': True, 'executions': [
        {'enabled': True, 'execution': {'kind': 'sleep', 'duration_ms': 5000}}]},
        {'name': 'next-stage', 'enabled': True, 'executions': [
        {'enabled': True, 'execution': {'kind': 'sleep', 'duration_ms': 15000}}]}]
    await patch(mcp, 'procedure', procedure_id, {'stages': cancellable})
    receipt = await mcp.tool('run_procedure', await arguments(mcp, 'procedure', procedure_id))
    cancelled = await mcp.tool('cancel_procedure', {'target': {'id': procedure_id},
                                                 'expected_update_id': receipt['update_id']})
    s.require(cancelled['cancellation_receipt']['submission'] == 'accepted', 'Procedure cancellation not acknowledged')
    original = await s.observe(mcp, receipt['update_id'])
    s.require(original['outcome'] == 'failed', 'Procedure original did not stop before next stage')
    faults.append({'case': 'explicit_procedure_cancel', 'original_update_id': receipt['update_id'],
        'cancel_update_id': cancelled['cancellation_receipt']['update_id'],
        'original_terminal_outcome': original['outcome'], 'targeting_limit': cancelled['targeting_limit']})
    await patch(mcp, 'procedure', procedure_id, {'stages': stages})
    s.persist()


async def exercise():
    address, editor, operator = CONTEXT['address'], CONTEXT['editor'], CONTEXT['operator']
    async with await s.Mcp.start(address, CONTEXT['admin_test'], True, True) as creator:
        builder_id = await create(creator, 'builder', PREFIX + 'builder', {
            'kind': 'server', 'servers': [{'id': CONTEXT['server_id']}]})
        source = ('console.log(JSON.stringify({automation_args: ARGS}));\n'
                  'await new Promise(resolve => setTimeout(resolve, Number(ARGS.delay_ms || 0)));\n'
                  'if (ARGS.fail) throw new Error("owned child failure");\n')
        action_id = await create(creator, 'action', PREFIX + 'action', {'source': source})
    CONTEXT.update(builder_id=builder_id, action_id=action_id)
    async with s.LossProxy(address, editor, False) as mutation_proxy, \
            await s.Mcp.start(mutation_proxy.address, editor, True, True) as raw_mcp:
        mcp = AuditedMcp(raw_mcp, mutation_proxy)
        repo_id = await create(mcp, 'repo', PREFIX + 'repo', {'repo': REPOSITORY,
            'branch': 'main', 'git_provider': 'source', 'git_https': True,
            'target': {'kind': 'server', 'target': {'id': CONTEXT['server_id']}}})
        CONTEXT['repo_id'] = repo_id
        cloned = await run(mcp, 'repo', repo_id)
        pulled = await finish(mcp, await mcp.tool('pull_repo', await arguments(mcp, 'repo', repo_id)))
        for update in [cloned, pulled]:
            observed = update.get('observed_revision', {}).get('revision', '')
            s.require(len(observed) >= 7 and CONTEXT['revision'].startswith(observed),
                      'Repo observed short revision mismatch')
        build_id = await create(mcp, 'build', PREFIX + 'build', {
            'builder': {'id': builder_id}, 'source': {'kind': 'linked_repo', 'target': {'id': repo_id}},
            'image_name': 'workload', 'image_registry': [{'domain': REGISTRY,
                'account': 'fixture', 'organization': 'owned-automation'}],
            'version': '0.0.1', 'auto_increment_version': True, 'include_latest_tag': False,
            'include_version_tags': True, 'include_commit_tag': False,
            'build_path': '.', 'dockerfile_path': 'Dockerfile'})
        CONTEXT['build_id'] = build_id
        build_before = api(editor).read('GetBuild', {'build': build_id})['config']
        built = await run(mcp, 'build', build_id)
        build_after = api(editor).read('GetBuild', {'build': build_id})['config']
        s.require({k: v for k, v in build_before.items() if k != 'version'} ==
                  {k: v for k, v in build_after.items() if k != 'version'},
                  'RunBuild rewrote saved source/definition')
        observed = built.get('observed_revision', {}).get('revision', '')
        s.require(len(observed) >= 7 and CONTEXT['revision'].startswith(observed),
                  'Build observed short revision mismatch')
        version = built['observed_build_result']['version']
        image = REGISTRY + '/owned-automation/workload:' + version
        digests = json.loads(s.run([*s.COMPOSE, 'exec', '-T', 'docker', 'docker',
            'image', 'inspect', image, '--format', '{{json .RepoDigests}}']))
        s.require(digests and any(x.startswith(REGISTRY + '/owned-automation/workload@sha256:')
                                 for x in digests), 'pushed workload digest unavailable')
        deployment_id = await create(mcp, 'deployment', PREFIX + 'deployment', {
            'attachment': {'kind': 'server', 'target': {'id': CONTEXT['server_id']}},
            'image': {'kind': 'build', 'target': {'id': build_id}, 'version': version},
            'network': 'bridge', 'command': ''})
        CONTEXT['deployment_id'] = deployment_id
        deployed_receipt = await mcp.tool('deploy_deployment', {'id': deployment_id})
        await finish(mcp, deployed_receipt)
        observed = s.poll(lambda: s.run([*s.COMPOSE, 'exec', '-T', 'docker', 'docker',
            'inspect', PREFIX + 'deployment', '--format', '{{.State.Health.Status}}']) == 'healthy',
            'independent workload Docker health', 30)
        revision_file = s.run([*s.COMPOSE, 'exec', '-T', 'docker', 'docker', 'exec',
                              PREFIX + 'deployment', 'cat', '/revision.txt'])
        s.require(observed and revision_file == CONTEXT['marker'], 'deployed source content mismatch')
        fleet = await mcp.tool('fleet_health', {})
        health = next(x for x in fleet['deployments'] if x['id'] == deployment_id)
        s.REPORT['automation']['chain'] = {'repo_id': repo_id, 'fixture_full_revision': CONTEXT['revision'],
            'clone_observed_revision': cloned['observed_revision'],
            'pull_observed_revision': pulled['observed_revision'],
            'build_observed_revision': built['observed_revision'],
            'build_definition_preserved_except_core_version_advance': True,
            'builder_id': builder_id, 'server_ids': [CONTEXT['server_id']], 'build_id': build_id,
            'build_version': version, 'image': image, 'independent_repo_digests': digests,
            'deployment_id': deployment_id, 'explicit_deployment_update_id': deployed_receipt['update_id'],
            'source_contents_verified_in_container': True, 'docker_health': 'healthy', 'mcp_fleet_observation': health,
            'dependent_deployment_update_link': None}
        # Runtime arguments merge, while the private saved definition stays byte-for-byte intact.
        sentinel = 'private-arguments-' + secrets.token_hex(16)
        s.SECRETS.append(sentinel)
        await patch(mcp, 'action', action_id, {'default_arguments': {'nested': {'saved': sentinel},
            'array': ['saved'], 'delay_ms': 0}})
        before = api(editor).read('GetAction', {'action': action_id})['config']
        action_update = await run(mcp, 'action', action_id, args={'nested': {'run': True}, 'array': ['run']})
        after = api(editor).read('GetAction', {'action': action_id})['config']
        raw = api(editor).read('GetUpdate', {'id': action_update['id']})
        stdout = '\n'.join(x.get('stdout', '') for x in raw['logs'])
        line = next(x for x in stdout.splitlines() if 'automation_args' in x)
        merged = json.loads(line)['automation_args']
        s.require(merged['nested'] == {'saved': sentinel, 'run': True} and merged['array'] == ['saved', 'run']
                  and before == after, 'Action runtime merge or definition preservation failed')
        encoded = json.dumps(action_update)
        s.require(sentinel not in encoded and 'automation_args' not in encoded
            and action_update['observed_action_result']['output_omitted']
            and all(not x.get('stdout') for x in action_update['stages']),
            'Action private logs escaped MCP omission')
        s.REPORT['automation']['action_arguments'] = {'saved_definition_unchanged': True,
            'nested_object_merge_verified': True, 'arrays_append_verified': True,
            'raw_action_output_omitted_from_mcp': True, 'update_id': action_update['id']}
        stages = [
            {'name': 'sequential-prepare', 'enabled': True, 'executions': [
                {'enabled': True, 'execution': {'kind': 'sleep', 'duration_ms': 1}}]},
            {'name': 'parallel-actions', 'enabled': True, 'executions': [
                {'enabled': True, 'execution': {'kind': 'run_action', 'resource_id': action_id,
                    'args': {'parallel': i}}} for i in [1, 2]]}]
        procedure_id = await create(mcp, 'procedure', PREFIX + 'procedure', {'stages': stages})
        CONTEXT['procedure_id'] = procedure_id
        before = api(editor).read('GetProcedure', {'procedure': procedure_id})['config']
        await run(mcp, 'procedure', procedure_id)
        s.require(api(editor).read('GetProcedure', {'procedure': procedure_id})['config'] == before,
                  'running Procedure changed its saved definition')
        s.REPORT['automation']['procedure_definition_unchanged'] = True
        await schedule_cycle(mcp, 'action', action_id)
        await schedule_cycle(mcp, 'procedure', procedure_id)
        await fault_cases(mcp, repo_id, build_id, procedure_id, action_id, stages)
        await permission_profiles(repo_id, action_id)
        await lost_ack_and_concurrency(action_id)
        await cancel_action(mcp, action_id)
        # The successful Build remains successful when a later deployment fails health.
        await patch(mcp, 'deployment', deployment_id, {'command': 'sh -c "exit 19"'})
        unhealthy = await mcp.tool('deploy_deployment', {'id': deployment_id})
        await finish(mcp, unhealthy)
        s.poll(lambda: s.run([*s.COMPOSE, 'exec', '-T', 'docker', 'docker', 'inspect',
            PREFIX + 'deployment', '--format', '{{.State.Status}}']) == 'exited', 'unhealthy workload exit', 20)
        fleet = await mcp.tool('fleet_health', {})
        s.REPORT['automation']['successful_build_unhealthy_deployment'] = {
            'build_outcome': built['outcome'], 'deployment_update_id': unhealthy['update_id'],
            'execution_outcome': 'succeeded', 'docker_state': 'exited',
            'mcp_fleet_observation': next(x for x in fleet['deployments'] if x['id'] == deployment_id)}
        await patch(mcp, 'deployment', deployment_id, {'command': ''})
        await finish(mcp, await mcp.tool('deploy_deployment', {'id': deployment_id}))
        s.poll(lambda: s.run([*s.COMPOSE, 'exec', '-T', 'docker', 'docker', 'inspect',
            PREFIX + 'deployment', '--format', '{{.State.Health.Status}}']) == 'healthy',
            'restored final workload Docker health', 30)
        for _ in range(20):
            fleet = await mcp.tool('fleet_health', {})
            final = next(x for x in fleet['deployments'] if x['id'] == deployment_id)
            if final['health'] == 'healthy':
                break
            await asyncio.sleep(1)
        s.require(final['health'] == 'healthy', 'final MCP workload health not confirmed')
        s.REPORT['automation']['final_health'] = {'docker_health': 'healthy', 'mcp_observation': final}
    s.REPORT['automation']['mutation_audit_summary'] = {
        'explicit_tool_calls': len(s.REPORT['automation']['per_call_mutation_audit']),
        'execute_attempts': mutation_proxy.execute_attempts, 'write_attempts': mutation_proxy.write_attempts,
        'maximum_mutations_per_call': max(x['execute_attempts'] + x['write_attempts']
            for x in s.REPORT['automation']['per_call_mutation_audit']),
        'read_only_mutation_attempts': 0}
    s.persist()
    print('Verified cross-family automation, source/image/deployment evidence and recovery.', flush=True)


def disable_automatic_execution():
    """Always runs before Docker teardown, including partially failed acceptance."""
    if not CONTEXT:
        s.REPORT['automatic_execution_cleanup'] = {'owners': 0, 'verified': True}
        return
    admin = CONTEXT['provisioner']
    disabled = []
    for kind, resource_id, name in OWNED:
        current = admin.read('Get' + kind, {kind.lower(): resource_id})
        s.require(s.identity(current) == resource_id and current['name'] == name and name.startswith(PREFIX),
                  'owned automation identity changed before teardown')
        if kind not in ['Action', 'Procedure', 'Build', 'Repo']:
            continue
        patch_value = {'webhook_enabled': False}
        if kind in ['Action', 'Procedure']:
            patch_value.update(schedule='', schedule_enabled=False)
        if kind == 'Action':
            patch_value['run_at_startup'] = False
        admin.write('Update' + kind, {'id': resource_id, 'config': patch_value})
        checked = admin.read('Get' + kind, {kind.lower(): resource_id})['config']
        s.require(all(checked[k] == v for k, v in patch_value.items()),
                  'automatic execution was not disabled before teardown')
        disabled.append({'kind': kind, 'id': resource_id, 'verified_disabled': True})
    s.REPORT['automatic_execution_cleanup'] = {'owners': disabled, 'verified': True,
        'before_compose_down': True, 'in_flight_limit': 'Disabling does not cancel already dispatched work; isolated Core/Periphery teardown removes remaining owned processes.'}
    print('Disabled and verified owned automatic execution before teardown.', flush=True)


def entry(argv=None):
    CONTEXT.clear()
    OWNED.clear()
    ACTIVE.clear()
    return s.entry(argv, fixture_customizer=customize, exercise_extension=provision,
                   before_cleanup=disable_automatic_execution)


if __name__ == '__main__':
    raise SystemExit(entry())
