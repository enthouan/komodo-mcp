"""Production stdio terminal acceptance with owned exact parent resources."""
import asyncio
import hashlib
import json
import secrets
import time

import disposable_core_smoke as s
import swarm_admin_fixture as f


def sessions(api, target, name=None):
    rows = api.read('ListTerminals', {'target': target, 'terms': [name] if name else [],
        'page': 0, 'limit': 16, 'use_names': False})
    s.require(isinstance(rows, list) and len(rows) < 16, 'terminal namespace proof incomplete')
    return [row for row in rows if name is None or row['name'] == name]


def fingerprint(row):
    return {'name': row['name'], 'created_at': row['created_at'],
            'command_sha256': hashlib.sha256(row['command'].encode()).hexdigest()}


def successful(value, code, expected=None):
    f.checked(value)
    obs = value['observation']
    s.require(obs['reported_exit_code'] == code and obs['end'] == 'upstream_reported_exit',
              'harmless command did not report expected status')
    if expected is not None:
        s.require(expected in (obs['output'] or '').splitlines(), 'harmless command output did not match')
    s.require(value['cleanup']['state'] == 'absence_observed'
              and value['attempts'] == {'create': 1, 'command': 1, 'delete': 1},
              'terminal lifecycle count/cleanup mismatch')


async def profiles():
    fixture = f.F
    fixture.report['profiles'] = []
    for operate in [False, True]:
        for edit in [False, True]:
            for prune in [False, True]:
                for terminal in [False, True]:
                    async with f.Proxy('reader') as proxy:
                        async with await f.Mcp.start(proxy.address, fixture.roles['reader']['credentials'],
                                                    operate, edit, prune, terminal) as process:
                            tools = f.checked(await process.rpc('tools/list', {}))['tools']
                            names = {tool['name'] for tool in tools}
                            expected = 64 + 20*operate + 55*edit + 4*(operate and edit) + 2*(operate and prune) + (operate and terminal)
                            s.require(len(tools) == expected, 'combined capability discovery count mismatch')
                            for name, visible in [('apply_variable_change', edit),
                                ('apply_access_change', edit and operate), ('test_alerter', operate),
                                ('apply_docker_prune', operate and prune), ('execute_terminal', operate and terminal)]:
                                s.require((name in names) == visible, 'capability exposed another independent grant')
                                before = len(proxy.events)
                                result = await f.raw(process, name, {})
                                s.require(('protocol_error' in result) if not visible else
                                    result.get('isError') and result['structuredContent']['kind'] == 'invalid_arguments',
                                    'local direct-call guard was bypassed')
                                s.require(len(proxy.events) == before, 'local rejected call reached Core')
                            s.require(not proxy.mutations(), 'discovery implicitly mutated')
                        fixture.report['profiles'].append({'operate': operate, 'edit': edit, 'prune': prune,
                            'terminal': terminal, 'tools': len(tools), 'local_denial_core_requests': 0,
                            'implicit_mutations': 0})
    for role, name, args in [
        ('reader', 'preview_docker_prune', {'target': {'id': fixture.sid}, 'category': 'system'}),
        ('reader', 'execute_terminal', {'target': {'kind': 'server', 'target': {'id': fixture.sid}}, 'command': 'true'}),
        ('pruner', 'execute_terminal', {'target': {'kind': 'server', 'target': {'id': fixture.sid}}, 'command': 'true'}),
    ]:
        async with f.Proxy(role) as proxy:
            async with await f.Mcp.start(proxy.address, fixture.roles[role]['credentials'], prune=True, terminal=True) as process:
                value = await f.raw(process, name, args)
                s.require(value['structuredContent']['kind'] == 'permission_denied' and not proxy.mutations(),
                          'upstream denial was bypassed or caused an implicit mutation')
            fixture.report.setdefault('core_denials', []).append({'role': role, 'tool': name, 'proxy': proxy.summary()})
    s.persist()


def prepare():
    fixture = f.F
    admin, sid = fixture.admin, fixture.sid
    names = [fixture.prefix + 'terminal-stack', fixture.prefix + 'terminal-deployment']
    stack = admin.write('CreateStack', {'name': names[0], 'config': {'server_id': sid,
        'project_name': names[0], 'auto_pull': False, 'send_alerts': False,
        'file_contents': 'services:\n  web:\n    image: ' + f.BUSYBOX + '\n    pull_policy: never\n    network_mode: none\n    command: ["sh", "-c", "sleep 3600"]\n    labels:\n      ' + f.LABEL + ': ' + s.OWNER + '\n'}})
    deployment = admin.write('CreateDeployment', {'name': names[1], 'config': {'server_id': sid,
        'image': {'type': 'Image', 'params': {'image': f.BUSYBOX}}, 'network': 'none',
        'command': 'sh -c "sleep 3600"', 'send_alerts': False,
        'extra_args': ['--label=' + f.LABEL + '=' + s.OWNER]}})
    stack_id, did = s.identity(stack), s.identity(deployment)
    for kind, rid, name in [('Stack', stack_id, names[0]), ('Deployment', did, names[1])]:
        fixture.remember(kind, rid, name)
        fixture.grant('terminal', kind, rid, 'Read', ['Inspect', 'Logs', 'Terminal'])
    updates = []
    for operation, params in [('DeployStack', {'stack': stack_id, 'services': [], 'stop_time': None}),
                              ('Deploy', {'deployment': did, 'stop_time': None, 'stop_signal': None})]:
        value = admin.request('/execute', operation, params)
        uid = s.identity(value)
        updates.append(uid)
        def finished():
            row = admin.read('GetUpdate', {'id': uid})
            if row['status'] == 'Complete':
                s.require(row['success'], 'owned terminal parent preparation failed')
                return True
            return False
        s.poll(finished, 'terminal parent provisioning', seconds=120)
    s.poll(lambda: len(admin.read('ListStackServices', {'stack': stack_id})) == 1,
           'exact Stack service discovery')
    containers = [admin.read(op, args) for op, args in [('InspectStackContainer', {'stack': stack_id, 'service': 'web'}),
                                                      ('InspectDeploymentContainer', {'deployment': did})]]
    s.require(all(len(c['Id']) == 64 and c['State']['Running'] and
                  c['Config']['Labels'].get(f.LABEL) == s.OWNER for c in containers), 'parent container identity mismatch')
    targets = [
        ('server', {'kind': 'server', 'target': {'id': sid}}, {'type': 'Server', 'params': {'server': sid}}),
        ('container', {'kind': 'container', 'server': {'id': sid}, 'container': {'id': containers[1]['Id']}},
         {'type': 'Container', 'params': {'server': sid, 'container': containers[1]['Id']}}),
        ('stack', {'kind': 'stack', 'target': {'id': stack_id}, 'service': 'web'},
         {'type': 'Stack', 'params': {'stack': stack_id, 'service': None}}),
        ('deployment', {'kind': 'deployment', 'target': {'id': did}},
         {'type': 'Deployment', 'params': {'deployment': did}}),
    ]
    fixture.report['terminal_fixture_preparation'] = {'execute_updates': updates,
        'separate_provisioner_used': True, 'container_ids': [c['Id'] for c in containers]}
    return targets, containers, stack_id, did


async def raw_start(process, target, command):
    process.next_id += 1
    rid = process.next_id
    await process.send({'jsonrpc': '2.0', 'id': rid, 'method': 'tools/call', 'params': {
        'name': 'execute_terminal', 'arguments': {'target': target, 'command': command, 'observation_seconds': 30}}})
    return rid


async def exercise():
    fixture = f.F
    targets, containers, stack_id, did = prepare()
    api, admin = fixture.api('terminal'), fixture.admin
    server, raw_server = targets[0][1:]
    report = fixture.report['terminal'] = {'cases': [], 'lifecycle': []}
    s.require(sessions(admin, raw_server) == [], 'unexpected preexisting Server terminal')
    initializer = 'timeout -k 1 300 sh -s ' + secrets.token_hex(16)
    foreign = fingerprint(admin.write('CreateTerminal', {'name': 'default', 'target': raw_server,
        'command': initializer, 'mode': None, 'recreate': 'Never'}))
    try:
        async with f.Proxy('terminal') as proxy:
            async with await f.Mcp.start(proxy.address, fixture.roles['terminal']['credentials'], terminal=True) as process:
                mcp = f.Audited(process, proxy)
                for kind, target, raw_target in targets:
                    for command, code, expected in [("printf 'v07-terminal-output\\n'", 0, 'v07-terminal-output'), ('exit 7', 7, None)]:
                        value = await mcp.tool('execute_terminal', {'target': target, 'command': command, 'observation_seconds': 12})
                        successful(value, code, expected)
                        s.require(not sessions(api, raw_target, value['terminal']), 'owned terminal remains independently')
                        report['cases'].append({'kind': kind, 'reported_exit_code': code,
                            'expected_output_matched': expected is not None, 'terminal': value['terminal'],
                            'attempts': value['attempts'], 'cleanup': value['cleanup']})
                # Real bounded output (malformed wire remains deterministic).
                value = await mcp.tool('execute_terminal', {'target': server,
                    'command': "printf '%0200d\\n' 0", 'observation_seconds': 12, 'max_output_bytes': 16})
                successful(value, 0)
                s.require(value['observation']['output_truncated'] and
                          len(value['observation']['output'].encode()) <= 16, 'live terminal output bound failed')
                report['bounded_output'] = {'bytes': len(value['observation']['output'].encode()), 'truncated': True}
            report['four_targets_proxy'] = proxy.summary()
        # A real task on this Server, rather than an inaccessible worker ID,
        # proves the fresh Docker-label rejection of Swarm task shells.
        service_name = fixture.prefix + 'terminal-reject'
        task_service = fixture.manager.call('service', 'create', '--detach=true', '--name', service_name,
            '--label', f.LABEL + '=' + s.OWNER, '--container-label', f.LABEL + '=' + s.OWNER,
            '--constraint', 'node.role==manager', '--no-resolve-image', f.BUSYBOX, 'sleep', '300')
        try:
            def task_container():
                ids = fixture.manager.call('ps', '-q', '--no-trunc', '--filter',
                    'label=com.docker.swarm.service.id=' + task_service).splitlines()
                return ids[0] if len(ids) == 1 else None
            cid = s.poll(task_container, 'manager task container')
            await f.poll_async(lambda: any(c.get('id') == cid for c in admin.read('ListContainers', {'server': fixture.sid})),
                               'Core task-container discovery')
            async with f.Proxy('terminal') as proxy:
                async with await f.Mcp.start(proxy.address, fixture.roles['terminal']['credentials'], terminal=True) as process:
                    denied = await f.raw(process, 'execute_terminal', {'target': {'kind': 'container',
                        'server': {'id': fixture.sid}, 'container': {'id': cid}}, 'command': 'true'})
                    s.require(denied.get('isError') and not proxy.mutations(), 'Swarm task shell was allowed')
                    report['swarm_task_rejection'] = {'container_id': cid,
                        'kind': denied['structuredContent']['kind'], 'mutations': 0}
        finally:
            fixture.manager.call('service', 'rm', task_service)
            for cid in fixture.manager.call('ps', '-aq', '--no-trunc', '--filter',
                'label=com.docker.swarm.service.id=' + task_service).splitlines(): fixture.manager.remove_container(cid)
        # Temporarily add only Terminal to a separate non-admin actor.
        fixture.grant('affected', 'Server', fixture.sid, 'Read', ['Terminal'])
        try:
            async with f.Proxy('terminal') as first, f.Proxy('affected') as second:
                async with await f.Mcp.start(first.address, fixture.roles['terminal']['credentials'], terminal=True) as p1, \
                           await f.Mcp.start(second.address, fixture.roles['affected']['credentials'], terminal=True) as p2:
                    values = await asyncio.gather(*[m.tool('execute_terminal', {'target': server,
                        'command': "sleep 1; printf 'v07-terminal-output\\n'", 'observation_seconds': 12}) for m in [p1, p2]])
                    for value in values: successful(value, 0, 'v07-terminal-output')
                    s.require(values[0]['terminal'] != values[1]['terminal'], 'two callers shared state')
                report['two_callers'] = {'ids': [fixture.roles[r]['id'] for r in ['terminal', 'affected']],
                    'names': [v['terminal'] for v in values], 'proxies': [first.summary(), second.summary()]}
        finally: fixture.grant('affected', 'Server', fixture.sid, 'Read')
        async with f.Proxy('terminal') as proxy:
            async with await f.Mcp.start(proxy.address, fixture.roles['terminal']['credentials'], terminal=True) as process:
                value = await process.tool('execute_terminal', {'target': server, 'command': 'sleep 20', 'observation_seconds': 5})
                s.require(value['observation']['end'] == 'deadline_remote_completion_unknown'
                          and value['cleanup']['state'] == 'absence_observed' and len(proxy.commands()) == 1,
                          'deadline claimed completion or replayed')
                report['lifecycle'].append({'case': 'deadline', 'terminal': value['terminal'], 'proxy': proxy.summary()})
        for mode in ['cancellation', 'disconnect', 'crash']:
            async with f.Proxy('terminal') as proxy:
                process = await f.Mcp.start(proxy.address, fixture.roles['terminal']['credentials'], terminal=True)
                closed = False
                try:
                    rid = await raw_start(process, server, 'sleep 120' if mode == 'crash' else 'sleep 20')
                    await f.poll_async(lambda: len(proxy.commands()) == 1, 'terminal command start')
                    name = proxy.commands()[0]['terminal']
                    rows = sessions(api, raw_server, name)
                    s.require(len(rows) == 1, 'owned namespace not independently visible')
                    old = fingerprint(rows[0])
                    if mode == 'cancellation':
                        await process.send({'jsonrpc': '2.0', 'method': 'notifications/cancelled',
                            'params': {'requestId': rid, 'reason': 'owned acceptance cancellation'}})
                        await f.poll_async(lambda: not sessions(api, raw_server, name), 'cancelled terminal removal')
                    if mode == 'crash': await process.crash()
                    else: await process.close()
                    closed = True
                    if mode == 'crash':
                        s.require(fingerprint(sessions(api, raw_server, name)[0]) == old, 'hard crash claimed immediate cleanup')
                        async with await f.Mcp.start(proxy.address, fixture.roles['terminal']['credentials'], terminal=True) as fresh:
                            value = await fresh.tool('execute_terminal', {'target': server,
                                'command': "printf 'v07-terminal-output\\n'", 'observation_seconds': 12})
                            successful(value, 0, 'v07-terminal-output')
                            s.require(value['terminal'] != name and fingerprint(sessions(api, raw_server, name)[0]) == old,
                                      'restart adopted/deleted old namespace')
                        await f.poll_async(lambda: not sessions(api, raw_server, name), 'actual remote shell TTL', seconds=105)
                    s.require(not sessions(api, raw_server, name) and len(proxy.commands()) == (2 if mode == 'crash' else 1),
                              'owned lifecycle cleanup or no-replay proof failed')
                    report['lifecycle'].append({'case': mode, 'terminal': name, 'absence_observed': True,
                        'remote_ttl_observed': mode == 'crash', 'descendant_termination_claimed': False,
                        'proxy': proxy.summary()})
                finally:
                    if not closed: await process.close()
        s.require(fingerprint(sessions(admin, raw_server, 'default')[0]) == foreign, 'foreign default terminal changed')
        report['foreign_default_preserved'] = True
        for _, _, raw_target in targets:
            s.require(all(row['name'] == 'default' for row in sessions(api, raw_target)), 'owned MCP terminal remains')
        report.update(passed=True, owned_mcp_sessions_absent=True)
    finally:
        rows = sessions(admin, raw_server, foreign['name'])
        s.require(len(rows) == 1 and fingerprint(rows[0]) == foreign, 'foreign fixture changed; cleanup refused')
        admin.write('DeleteTerminal', {'target': raw_server, 'terminal': foreign['name']})
        s.require(not sessions(admin, raw_server, foreign['name']), 'foreign fixture cleanup not observed')
        for container in containers: fixture.manager.remove_container(container['Id'])
        admin.write('DeleteStack', {'id': stack_id})
        admin.write('DeleteDeployment', {'id': did})
        report['fixture_cleanup_verified'] = True
        s.persist()
