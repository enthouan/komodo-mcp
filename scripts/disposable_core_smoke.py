#!/usr/bin/env python3
"""Opt-in compiled-stdio smoke in a new disposable Core/Periphery 2.3.2 project.

Requires Python 3.11+, local Docker/Compose and a clean committed checkout.
Creates a privileged nested Docker daemon without mounting the host socket.
Provisioning admin credentials never enter an MCP process. Always verifies cleanup.
"""
import argparse
import asyncio
import datetime
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import signal
import socket
import tempfile
import threading
import tomllib
import uuid
import subprocess
import time
import urllib.parse

WORKSPACE = Path(__file__).resolve().parents[1]
CORE_VERSION = '2.3.2'
MAX_BODY = 2 * 1024 * 1024
MAX_COMMAND_OUTPUT = 8 * 1024 * 1024
HTTP_TIMEOUT = 10.0
SECRETS = []
REPORT = {}
ROOT = None
PROJECT = OWNER = ''
DOCKER = COMPOSE = []
BINARY = EVIDENCE = None
RESERVED = False

class Failure(Exception):
    pass


class CoreHttpFailure(Failure):
    """Bounded private error body; only safe status/operation enters the message."""
    def __init__(self, operation, status, body):
        super().__init__(f'{operation or "Core request"}: Core HTTP {status}')
        self.status, self.body = status, body


def require(condition, message):
    if not condition:
        raise Failure(message)


def run(args, timeout=60):
    """Bound a noninteractive command and terminate its owned process group."""
    env = os.environ.copy()
    for key in ['CARGO_TARGET_DIR', 'CARGO_BUILD_TARGET', 'DOCKER_HOST']:
        env.pop(key, None)
    with tempfile.TemporaryFile() as output:
        process = subprocess.Popen(args, cwd=WORKSPACE, env=env, stdin=subprocess.DEVNULL,
            stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            end = time.monotonic() + timeout
            while process.poll() is None:
                if time.monotonic() >= end or os.fstat(output.fileno()).st_size > MAX_COMMAND_OUTPUT:
                    raise Failure('command deadline/output limit: ' + args[0])
                time.sleep(.05)
            if process.returncode:
                raise Failure('command failed: ' + args[0] + ' ' + args[-1])
            output.seek(0)
            result = output.read(MAX_COMMAND_OUTPUT + 1)
            if len(result) > MAX_COMMAND_OUTPUT:
                raise Failure('command output exceeded bound')
            return result.decode(errors='replace').strip()
        finally:
            # The leader can exit while descendants still own this session's
            # group. Terminate the owned group on every exit, then reap leader.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait(timeout=5)

def safe(value):
    result = json.dumps(value)
    for secret in SECRETS:
        if secret:
            result = result.replace(secret, '[redacted]')
    return json.loads(result)

def persist():
    EVIDENCE.parent.mkdir(parents=True, exist_ok=True)
    EVIDENCE.write_text(json.dumps(safe(REPORT), indent=2) + '\n')

def identity(value):
    result = value.get('id') or value.get('_id')
    if isinstance(result, dict):
        result = result.get('$oid')
    if not isinstance(result, str) or not re.fullmatch('[0-9a-f]{24}', result):
        raise Failure('Core response lacks a stable identity')
    return result

class Api:
    def __init__(self, address, headers=None):
        parsed = urllib.parse.urlsplit(address)
        if (parsed.scheme != 'http' or parsed.hostname != '127.0.0.1'
                or parsed.username or parsed.password or parsed.path or parsed.query or parsed.fragment):
            raise Failure('API target must be the disposable loopback Core endpoint')
        self.address, self.headers = address, headers or {}
        self.port = parsed.port

    def request(self, endpoint, operation=None, params=None):
        headers = {'content-type': 'application/json', **self.headers}
        data = None if operation is None else json.dumps({'type': operation, 'params': params or {}}).encode()
        # Direct loopback HTTP never follows redirects or consults proxy env.
        # A watchdog shuts down the actual socket, including during slow header
        # reads; cancelling a to_thread waiter alone cannot stop that I/O.
        deadline = time.monotonic() + HTTP_TIMEOUT
        expired = threading.Event()
        connection = http.client.HTTPConnection('127.0.0.1', self.port, timeout=HTTP_TIMEOUT)
        transport = timer = None

        def abort():
            expired.set()
            try:
                transport.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass

        try:
            connection.connect()
            transport = connection.sock
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise Failure('Core request deadline exceeded')
            transport.settimeout(remaining)
            timer = threading.Timer(remaining, abort)
            timer.daemon = True
            timer.start()
            connection.request('GET' if data is None else 'POST', endpoint, body=data, headers=headers)
            with connection.getresponse() as response:
                body = bytearray()
                while True:
                    if expired.is_set() or time.monotonic() >= deadline:
                        raise Failure('Core request deadline exceeded')
                    block = response.read1(min(65536, MAX_BODY + 1 - len(body)))
                    body.extend(block)
                    if len(body) > MAX_BODY:
                        raise Failure('bounded Core response exceeded')
                    if not block:
                        break
                require(not expired.is_set() and time.monotonic() < deadline,
                        'Core request deadline exceeded')
                if not 200 <= response.status < 300:
                    raise CoreHttpFailure(operation, response.status, bytes(body))
                return json.loads(body)
        except (OSError, ValueError, http.client.HTTPException):
            if expired.is_set() or time.monotonic() >= deadline:
                raise Failure('Core request deadline exceeded') from None
            raise Failure(f'{operation or endpoint}: unavailable or invalid Core response') from None
        finally:
            if timer is not None:
                timer.cancel()
                timer.join(timeout=1)
            if transport is not None:
                try:
                    transport.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
                transport.close()
            connection.close()

    def read(self, operation, params=None):
        return self.request('/read', operation, params)

    def write(self, operation, params):
        return self.request('/write', operation, params)

def poll(callback, description, seconds=120):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        try:
            value = callback()
            if value:
                return value
        except Failure:
            pass
        time.sleep(1)
    raise Failure('bounded readiness deadline: ' + description)

class Mcp:
    async def __aenter__(self):
        return self

    async def __aexit__(self, *_):
        await self.close()

    @classmethod
    async def start(cls, address, credentials, operate=True, configuration_edit=False):
        self = cls()
        self.next_id, self.stderr = 0, bytearray()
        env = {'PATH': os.environ.get('PATH', ''), 'KOMODO_ADDRESS': address,
               'KOMODO_API_KEY': credentials['key'], 'KOMODO_API_SECRET': credentials['secret'],
               'KOMODO_ALLOW_OPERATE': str(operate).lower(),
               'KOMODO_ALLOW_CONFIGURATION_EDIT': str(configuration_edit).lower(), 'RUST_LOG': 'info'}
        self.process = await asyncio.create_subprocess_exec(str(BINARY), env=env,
            stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE, limit=MAX_BODY)
        self.drain = asyncio.create_task(self.drain_stderr())
        try:
            initialized = await self.rpc('initialize', {'protocolVersion': '2025-11-25', 'capabilities': {},
                'clientInfo': {'name': 'komodo-mcp-disposable-smoke', 'version': '1'}})
            if initialized['serverInfo']['name'] != 'komodo-mcp' or initialized['serverInfo']['version'] != REPORT['package_version']:
                raise Failure('compiled MCP identity/version mismatch')
            REPORT['mcp_version'] = initialized['serverInfo']['version']
            await self.send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})
            return self
        except BaseException:
            await self.close()
            raise

    async def drain_stderr(self):
        while True:
            block = await self.process.stderr.read(4096)
            if not block:
                return
            self.stderr.extend(block[:max(0, 1024*1024-len(self.stderr))])

    async def send(self, message):
        self.process.stdin.write(json.dumps(message).encode() + b'\n')
        await asyncio.wait_for(self.process.stdin.drain(), 5)

    async def rpc(self, method, params, allow_error=False):
        self.next_id += 1
        expected = self.next_id
        await self.send({'jsonrpc': '2.0', 'id': expected, 'method': method, 'params': params})
        end = time.monotonic() + 30
        while True:
            line = await asyncio.wait_for(self.process.stdout.readline(), max(.01, end-time.monotonic()))
            if not line:
                raise Failure('production MCP exited before response')
            if len(line) > MAX_BODY:
                raise Failure('MCP response exceeded smoke bound')
            response = json.loads(line)
            if response.get('id') != expected:
                if 'method' in response:
                    continue
                raise Failure('MCP response ID mismatch')
            if 'error' in response:
                if allow_error:
                    return {'protocol_error': response['error']['code']}
                raise Failure('MCP protocol error during ' + method)
            return response['result']

    async def tool(self, name, arguments):
        result = await self.rpc('tools/call', {'name': name, 'arguments': arguments})
        if result.get('isError'):
            kind = result.get('structuredContent', {}).get('kind', 'unknown')
            raise Failure(f'MCP tool {name}: {kind}')
        return result['structuredContent']

    async def close(self):
        if self.process.stdin:
            self.process.stdin.close()
        forced = False
        try:
            await asyncio.wait_for(self.process.wait(), 5)
        except asyncio.TimeoutError:
            forced = True
            self.process.kill()
            await asyncio.wait_for(self.process.wait(), 5)
        await asyncio.wait_for(self.drain, 5)
        stderr = self.stderr.decode(errors='replace')
        if any(secret and secret in stderr for secret in SECRETS):
            raise Failure('credential leaked into captured MCP stderr')
        if forced or self.process.returncode:
            raise Failure('production MCP did not shut down cleanly')

async def wait_final(mcp, update_id):
    end = time.monotonic() + 90
    for _ in range(10):
        remaining = int(end - time.monotonic())
        if remaining < 1: break
        value = await mcp.tool('wait_for_update', {'id':update_id,'timeout_seconds':min(30,remaining)})
        REPORT.setdefault('wait_end_reasons',[]).append(value['end'])
        if value['end'] == 'complete': return value
        if value['end'] not in ['deadline','poll_limit']: raise Failure('live update observation unavailable')
    raise Failure('bounded repeated observation deadline')

async def observe(mcp, update_id):
    lookup = await mcp.tool('get_update', {'id':update_id})
    if lookup['id'] != update_id:
        raise Failure('update lookup changed identity')
    return (await wait_final(mcp,update_id))['latest_update']


def containers():
    ids = run([*COMPOSE, 'exec', '-T', 'docker', 'docker', 'ps', '-aq', '--filter', 'label=com.docker.compose.project=mcp-smoke-stack']).splitlines()
    if not ids or len(ids) > 10:
        raise Failure('bounded disposable workload inventory missing')
    lines = run([*COMPOSE, 'exec', '-T', 'docker', 'docker', 'inspect', '--format', '{{.Id}} {{index .Config.Labels "com.docker.compose.service"}} {{.State.Status}} {{.State.StartedAt}}', *ids]).splitlines()
    return {service: {'id': identity, 'state': state, 'started': started} for identity,service,state,started in (line.split() for line in lines)}

class LossProxy:
    def __init__(self, address, credentials, lose_reply=True):
        self.api = Api(address, {'x-api-key': credentials['key'], 'x-api-secret': credentials['secret']})
        self.lose_reply = lose_reply
        self.execute_attempts = 0
        self.write_attempts = 0
        self.write_operations = []
        self.read_operations = {}
        self.ids = []
        self.connections = set()
        self.failures = []

    async def __aenter__(self):
        self.server = await asyncio.start_server(self.handle, '127.0.0.1', 0, limit=32768)
        self.address = 'http://127.0.0.1:' + str(self.server.sockets[0].getsockname()[1])
        return self

    async def __aexit__(self, *_):
        self.server.close()
        await self.server.wait_closed()
        for task in list(self.connections): task.cancel()
        await asyncio.wait_for(asyncio.gather(*self.connections, return_exceptions=True), 10)
        if self.failures:
            raise Failure('fault proxy failed: ' + ','.join(self.failures))

    async def handle(self, reader, writer):
        task = asyncio.current_task(); self.connections.add(task)
        try:
            headers = await asyncio.wait_for(reader.readuntil(b'\r\n\r\n'), 5)
            if len(headers) > 32768: raise Failure('proxy headers exceeded bound')
            lines = headers.decode('ascii').split('\r\n')
            method, endpoint, _ = lines[0].split(' ')
            fields = {k.lower():v.strip() for k,v in (line.split(':',1) for line in lines[1:] if ':' in line)}
            if method != 'POST' or endpoint not in ['/read','/execute','/write']: raise Failure('proxy unexpected endpoint')
            if any(fields.get(k) != v for k,v in self.api.headers.items()): raise Failure('proxy unexpected identity')
            if endpoint == '/execute':
                self.execute_attempts += 1  # Count attempts before parsing/forwarding/acknowledgement.
            if endpoint == '/write':
                self.write_attempts += 1
            size = int(fields['content-length'])
            if not 0 <= size <= MAX_BODY: raise Failure('proxy body exceeded bound')
            request = json.loads(await asyncio.wait_for(reader.readexactly(size), 5))
            if endpoint == '/write':
                # Record field names only; configuration values can be secret.
                self.write_operations.append({'type': request['type'], 'fields': sorted(request['params'].get('config',{}))})
            if endpoint == '/read':
                operation = request['type']
                self.read_operations[operation] = self.read_operations.get(operation,0) + 1
            try:
                reply = await asyncio.wait_for(asyncio.to_thread(self.api.request, endpoint, request['type'], request['params']), 12)
            except CoreHttpFailure as error:
                status, body = error.status, error.body
            else:
                status, body = 200, json.dumps(reply).encode()
                if endpoint == '/execute':
                    self.ids.append(identity(reply))
                    if self.lose_reply:
                        return  # Core accepted; deliberately lose only its acknowledgement.
            if len(body) > MAX_BODY: raise Failure('proxy reply exceeded bound')
            writer.write(b'HTTP/1.1 ' + str(status).encode() + b' Core response\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: ' + str(len(body)).encode() + b'\r\n\r\n' + body)
            await asyncio.wait_for(writer.drain(), 5)
        except Exception as error:
            self.failures.append(type(error).__name__)
        finally:
            writer.close()
            try:
                await asyncio.wait_for(writer.wait_closed(), 5)
            except (OSError, asyncio.TimeoutError):
                pass
            self.connections.discard(task)

def saved_hook_stages(update):
    stages = [{'stage':stage['stage'],'success':stage['success']} for stage in update['stages']]
    require(all(any(stage['stage'] == name and stage['success'] for stage in stages)
                for name in ['Pre Deploy','Post Deploy']),
            'saved pre/post deploy hooks must complete, including service-filtered deploy')
    return stages


async def exercise(address, operator, reader, stack_id, deployment_id):
    async with await Mcp.start(address, operator) as mcp:
        async def operation(tool, service=None):
            arguments = {'target': {'id': stack_id}, 'scope': {'kind': 'whole_stack'} if service is None else {'kind':'service','service':service}} if tool.endswith('_stack') else {'id': deployment_id}
            receipt = await mcp.tool(tool, arguments)
            if receipt['submission'] != 'accepted' or not receipt['update_id']:
                raise Failure(tool + ': submission not acknowledged')
            update = await observe(mcp, receipt['update_id'])
            REPORT['operations'].append({'tool':tool, 'scope':receipt['requested_scope'],
                'submission':receipt['submission'],'configured_effects':receipt['configured_effects'],
                'update_id':receipt['update_id'], 'outcome':update['outcome'],
                'stages':[{'stage':s['stage'],'success':s['success']} for s in update['stages']]})
            persist()
            if update['outcome'] != 'succeeded':
                raise Failure(tool + ': final Core execution failed')
            effects = receipt['configured_effects']
            if tool == 'deploy_stack':
                require(effects['pre_deploy_hook'] and effects['post_deploy_hook']
                        and effects['auto_pull'] is False, 'saved Compose hook/image-pull policy changed')
                saved_hook_stages(update)
            if not tool.startswith('deploy') and any('deploy' in s['stage'].lower() for s in update['stages']):
                raise Failure('unexpected deploy hooks in lifecycle update')
            if not tool.startswith('deploy'):
                require(not effects['pre_deploy_hook'] and not effects['post_deploy_hook']
                        and effects['auto_pull'] is None,'lifecycle must not claim deploy hooks/image pull')
            print('Verified real Core lifecycle:', tool, service or 'whole', flush=True)
        await operation('deploy_stack')
        await operation('deploy_deployment')
        initial = containers()
        await operation('stop_stack', 'web')
        stopped = containers()
        require(stopped['web']['state'] == 'exited' and stopped['db']['state'] == 'running', 'named stop must leave the db dependency running')
        await operation('stop_stack', 'db')
        require(all(c['state'] == 'exited' for c in containers().values()), 'explicit db stop must leave both existing containers stopped')
        await operation('start_stack', 'web')
        started = containers()
        require(all(c['state'] == 'running' for c in started.values()), 'named start must start existing dependency containers')
        await operation('restart_stack', 'db')
        restarted = containers()
        require(all(restarted[s]['started'] != started[s]['started'] for s in ['web','db']), 'restarting db must restart the configured web dependent')
        require(all(restarted[s]['id'] == initial[s]['id'] for s in ['web','db']), 'lifecycle must retain existing container identities')
        REPORT['dependency_checks'] = {'named_stop_kept_db_running':True, 'named_start_started_existing_db':True, 'restart_db_restarted_configured_web_dependent':True, 'existing_container_ids_retained':True}
        persist()
        for tool in ['stop_stack','start_stack','restart_stack','stop_deployment','start_deployment','restart_deployment']:
            await operation(tool)
    async with await Mcp.start(address, reader) as mcp:
        receipt = await mcp.tool('restart_deployment', {'id':deployment_id})
        update = await observe(mcp, receipt['update_id'])
        require(receipt['submission'] == 'accepted' and update['outcome'] == 'failed', 'Read identity must receive an accepted failed Core update')
        denial = ' '.join(s['stderr'] or '' for s in update['stages']).lower()
        require('permission' in denial and 'execute' in denial, 'failed Read caller update must expose Execute permission denial')
        REPORT['denied_lifecycle'] = {'update_id':receipt['update_id'], 'submission':receipt['submission'], 'outcome':update['outcome'],'permission_denial_observed':True}
        persist()

    async with LossProxy(address,operator) as proxy, LossProxy(address,reader,lose_reply=False) as observer_proxy:
        args = {'target':{'id':stack_id},'scope':{'kind':'service','service':'web'}}
        async with await Mcp.start(proxy.address,operator) as first:
            r = await first.tool('restart_stack',args)
            require(r['submission'] == 'unknown' and r['update_id'] is None, 'lost ACK must retain unknown submission and no known update ID')
        require(proxy.execute_attempts == len(proxy.ids) == 1, 'lost ACK original call must execute exactly once')
        context = {'target':{'kind':'stack','id':stack_id},'requested_scope':r['requested_scope'],'operation':r['operation'],'submission_window':r['submission_window']}
        async with await Mcp.start(observer_proxy.address,reader,False) as fresh:
            recovered = await fresh.tool('reconcile_operation',context)
            require(observer_proxy.read_operations.get('ListUpdates',0) > 0,
                    'reconciliation history reads must pass through the observed Read identity')
            require(recovered['original_submission'] == 'unknown' and recovered['attribution'] == 'unconfirmed', 'recovery must retain unknown acceptance and unconfirmed attribution')
            require(not recovered['automatic_resubmission'] and 'selected_update_id' not in recovered, 'recovery must neither select an update nor resubmit execution')
            REPORT['initial_reconciliation'] = recovered
            persist()
            observed = await wait_final(fresh,proxy.ids[0])
            require(observed['end'] == 'complete' and observed['latest_update']['outcome'] == 'succeeded', 'instrumented accepted update must complete successfully')
            recovered = await fresh.tool('reconcile_operation',context)
            REPORT['completed_reconciliation'] = recovered
            persist()
            candidates = recovered['candidates']
            require(any(c['update']['id'] == proxy.ids[0] and c['scope_evidence'] == 'named_service_matches' for c in candidates), 'completed service update must corroborate the requested service')
        require(proxy.execute_attempts == len(proxy.ids) == 1 and observer_proxy.execute_attempts == 0,
                'reconciliation must add no execution requests')
        REPORT['lost_ack_recovery'] = {'tool':'restart_stack','scope':r['requested_scope'],'original_submission':recovered['original_submission'],'attribution':recovered['attribution'],'candidate_count':len(candidates),'service_scope_evidence':'named_service_matches','accepted_execute_requests':len(proxy.ids),'submission_execute_attempts':proxy.execute_attempts,'recovery_execute_requests':observer_proxy.execute_attempts,'recovery_read_requests':dict(observer_proxy.read_operations),'read_only_identity':True,'known_instrumented_update_final':'succeeded'}
        async with await Mcp.start(proxy.address,operator) as one, await Mcp.start(proxy.address,operator) as two:
            a,b = await asyncio.gather(one.tool('deploy_stack',args),two.tool('deploy_stack',args))
            require(a['submission'] == b['submission'] == 'unknown', 'independent lost-ACK calls must both retain uncertainty')
        require(proxy.execute_attempts == len(proxy.ids) == 3 and len(set(proxy.ids)) == 3, 'two independent calls must produce exactly two distinct updates')
        context.update(operation='deploy',submission_window={'started_ts':min(a['submission_window']['started_ts'],b['submission_window']['started_ts']),'ended_ts':max(a['submission_window']['ended_ts'],b['submission_window']['ended_ts'])})
        history_before = observer_proxy.read_operations.get('ListUpdates',0)
        async with await Mcp.start(observer_proxy.address,reader,False) as fresh:
            recovered = await fresh.tool('reconcile_operation',context)
            require(observer_proxy.read_operations.get('ListUpdates',0) > history_before,
                    'new-session reconciliation reads must be instrumented')
            require(recovered['original_submission'] == 'unknown' and recovered['attribution'] == 'unconfirmed', 'independent-call recovery must retain uncertainty')
            require('selected_update_id' not in recovered and not recovered['automatic_resubmission'], 'independent-call recovery must not select or resubmit')
            found = {c['update']['id'] for c in recovered['candidates']}
            require(set(proxy.ids[1:]) <= found, 'bounded history must expose both instrumented accepted updates')
            outcomes = []
            busy_failure = False
            for update_id in proxy.ids[1:]:
                observed = await wait_final(fresh,update_id)
                require(observed['end'] == 'complete', 'each acknowledged candidate must remain observable to completion')
                update = observed['latest_update']
                outcomes.append(update['outcome'])
                if update['outcome'] == 'succeeded':
                    REPORT.setdefault('service_deploy_hook_evidence',[]).append({
                        'instrumented_update_id':update_id,'stages':saved_hook_stages(update)})
                else:
                    require(any('busy' in (stage['stderr'] or '').lower() for stage in update['stages']),
                            'overlapping service deploy failed without the expected Core busy evidence')
                    busy_failure = True
            require('succeeded' in outcomes,'an independent service deploy must succeed')
        require(proxy.execute_attempts == len(proxy.ids) == 3 and observer_proxy.execute_attempts == 0,
                'independent-call reconciliation must add no execution requests')
        REPORT['independent_lost_ack_callers'] = {'explicit_calls':2,'accepted_execute_requests':2,'submission_execute_attempts':proxy.execute_attempts-1,'distinct_updates':2,'original_submission':'unknown','attribution':'unconfirmed','candidate_count':len(found),'instrumented_outcomes':outcomes,'busy_failure_observed':busy_failure,'recovery_execute_requests':observer_proxy.execute_attempts,'recovery_read_requests':dict(observer_proxy.read_operations)}
    async with await Mcp.start(address,reader,False) as observer:
        tools = (await observer.rpc('tools/list',{}))['tools']
        names = {tool['name'] for tool in tools}
        require(len(names) == 16 and not any(name.startswith(('deploy_','start_','stop_','restart_')) for name in names), 'default read-only discovery must expose sixteen readers')
        diagnosis = await observer.tool('stack_diagnose',{'id':stack_id})
        fleet = await observer.tool('fleet_health',{})
        REPORT['final_health'] = {'stack_diagnosis':diagnosis['outcome'],
            'stack_id':stack_id,'gaps':[gap['source'] for gap in diagnosis['gaps']],
            'stacks':[{'id':x['id'],'health':x['health']} for x in fleet['stacks']],
            'deployments':[{'id':x['id'],'health':x['health']} for x in fleet['deployments']],
            'readonly_discovery_count':len(names)}
        require(any(x['id'] == stack_id for x in fleet['stacks']), 'final fleet observation must include the disposable Stack')
        require(any(x['id'] == deployment_id for x in fleet['deployments']), 'final fleet observation must include the disposable Deployment')
    persist()
    print('Verified production stdio operations, recovery and independent health observation.',flush=True)

async def exercise_configuration(address, operator, stack_id):
    """Same non-admin Write identity, with independent local startup grants."""
    api = Api(address, {'x-api-key':operator['key'], 'x-api-secret':operator['secret']})
    async with LossProxy(address,operator,lose_reply=False) as proxy:
        async with await Mcp.start(proxy.address,operator,operate=True,configuration_edit=False) as mcp:
            names = {t['name'] for t in (await mcp.rpc('tools/list',{}))['tools']}
            require('restart_stack' in names and 'apply_stack_update' not in names,
                    'operate-only discovery must exclude actual configuration editor')
            receipt = await mcp.tool('restart_stack',{'target':{'id':stack_id},'scope':{'kind':'whole_stack'}})
            update = await observe(mcp,receipt['update_id'])
            require(update['outcome'] == 'succeeded','same-identity staging restart failed')
            reads = dict(proxy.read_operations)
            denied = await mcp.rpc('tools/call',{'name':'apply_stack_update','arguments':{
                'target':{'id':stack_id},'patch':{'auto_pull':True},'preview_id':'disabled-direct-call'}},allow_error=True)
            require('protocol_error' in denied,'direct configuration invocation was not rejected locally')
            require(proxy.write_attempts == 0 and proxy.read_operations == reads,
                    'disabled direct editor reached Core')
            REPORT['configuration_permission_boundary'] = {
                'same_non_admin_identity':True,'core_workload_permission':'Write',
                'operate_enabled':True,'configuration_edit_enabled':False,
                'restart_update_id':receipt['update_id'],'restart_outcome':update['outcome'],
                'actual_edit_tool':'apply_stack_update','direct_edit_denied_locally':True,
                'direct_edit_core_reads':0,'configuration_write_attempts':proxy.write_attempts}
    before = await asyncio.to_thread(api.read,'GetStack',{'stack':stack_id})
    async with LossProxy(address,operator,lose_reply=False) as proxy:
        async with await Mcp.start(proxy.address,operator,operate=False,configuration_edit=True) as mcp:
            input = {'target':{'id':stack_id},'patch':{'auto_pull':not before['config']['auto_pull']}}
            preview = await mcp.tool('preview_stack_update',input)
            require(preview['writes_submitted'] == 0 and proxy.write_attempts == 0,'preview submitted a write')
            saved = await mcp.tool('apply_stack_update',{**input,'preview_id':preview['preview_id']})
            require(saved['outcome'] == 'saved' and proxy.write_attempts == 1,'typed edit did not save once')
            after = await asyncio.to_thread(api.read,'GetStack',{'stack':stack_id})
            require(after['config']['auto_pull'] == input['patch']['auto_pull'],'saved setting not observed')
            preserved = all(before['config'][key] == after['config'][key] for key in before['config'] if key != 'auto_pull')
            require(preserved,'partial edit changed unrelated authoritative fields')
            noop = await mcp.tool('preview_stack_update',input)
            applied = await mcp.tool('apply_stack_update',{**input,'preview_id':noop['preview_id']})
            require(noop['outcome'] == applied['outcome'] == 'no_op' and proxy.write_attempts == 1,
                    'unchanged edit must perform zero additional writes')
            # Restore the owned fixture's original saved policy through the same editor.
            restore = {'target':{'id':stack_id},'patch':{'auto_pull':before['config']['auto_pull']}}
            p = await mcp.tool('preview_stack_update',restore)
            require((await mcp.tool('apply_stack_update',{**restore,'preview_id':p['preview_id']}))['outcome']=='saved',
                    'fixture setting restoration failed')
            require(proxy.execute_attempts == 0,'configuration-only session submitted execution')
            require(proxy.write_operations == [{'type':'UpdateStack','fields':['auto_pull']}]*2,
                    'one-field patch included unrelated fields')
            REPORT['configuration_edits'] = {'preview_writes':0,'apply_writes':1,'no_op_writes':0,
                'restoration_writes':1,'writes':proxy.write_operations,'unrelated_fields_preserved':preserved,
                'configuration_only_execute_attempts':proxy.execute_attempts,
                'saved_configuration_verified':True,'source_target':'inline_compose_on_server'}
    persist()
    print('Verified real Core configuration preview/apply and same-identity local permission boundary.',flush=True)


async def exercise_stack_definitions(address, editor, stack_id, server_id, alternate_server_id):
    """Create through MCP, preserve sources, and observe actual reassignment effects."""
    api = Api(address, {'x-api-key':editor['key'], 'x-api-secret':editor['secret']})

    def containers(project):
        return run([*COMPOSE, 'exec', '-T', 'docker', 'docker', 'ps', '-aq',
                    '--filter', 'label=com.docker.compose.project='+project]).splitlines()

    async with LossProxy(address,editor,lose_reply=False) as proxy:
        async with await Mcp.start(proxy.address,editor,operate=False,configuration_edit=True) as mcp:
            create = {'name':'mcp-config-created','config':{
                'attachment':{'kind':'server','target':{'id':server_id}},
                'source':{'kind':'inline','contents':'services:\n  app:\n    image: busybox:1.37.0\n    command: ["sleep", "600"]\n'},
                'auto_pull':False}}
            p = await mcp.tool('preview_stack_create',create)
            require(proxy.write_attempts == 0 and p['resource']['id'] is None,'creation preview submitted a write')
            saved = await mcp.tool('apply_stack_create',{**create,'preview_id':p['preview_id']})
            require(saved['outcome'] == 'saved' and proxy.write_attempts == 1,'creation was not saved once')
            created_id = saved['resource']['id']
            inspected = await mcp.tool('get_stack',{'id':created_id})
            before = await asyncio.to_thread(api.read,'GetStack',{'stack':created_id})
            edit = {'target':{'id':created_id},'patch':{'auto_pull':True}}
            p = await mcp.tool('preview_stack_update',edit)
            require(p['definition']['details']['before']['source']['kind'] == 'inline','inline source was not identified')
            changed = await mcp.tool('apply_stack_update',{**edit,'preview_id':p['preview_id']})
            require(changed['outcome'] == 'saved','created Stack partial update failed')
            after = await asyncio.to_thread(api.read,'GetStack',{'stack':created_id})
            preserved = all(v == after['config'][k] for k,v in before['config'].items() if k != 'auto_pull')
            require(preserved and after['config']['auto_pull'],'one-field update changed the saved source or unrelated configuration')
            require(not await asyncio.to_thread(containers,'mcp-config-created'),'saving the new definition deployed a workload')

            old_containers = await asyncio.to_thread(containers,'mcp-smoke-stack')
            require(len(old_containers) == 2,'owned running Stack containers were not available for reassignment proof')
            target_changes = []
            for target_id in [alternate_server_id,server_id]:
                edit = {'target':{'id':stack_id},'patch':{
                    'attachment':{'kind':'server','target':{'id':target_id}}}}
                p = await mcp.tool('preview_stack_update',edit)
                require(p['definition']['details']['target_changed'],'target reassignment effect was not identified')
                changed = await mcp.tool('apply_stack_update',{**edit,'preview_id':p['preview_id']})
                require(changed['outcome'] == 'saved','target reassignment was not saved')
                readback = await asyncio.to_thread(api.read,'GetStack',{'stack':stack_id})
                require(readback['config']['server_id'] == target_id,'saved target was not observed')
                require(set(await asyncio.to_thread(containers,'mcp-smoke-stack')) == set(old_containers),
                        'Core reassignment unexpectedly removed or replaced old-target containers')
                target_changes.append({'target_id':target_id,'saved':True,'old_target_container_ids_preserved':True})
            require(proxy.execute_attempts == 0,'definition editor submitted execution')
            require([w['type'] for w in proxy.write_operations] == ['CreateStack','UpdateStack','UpdateStack','UpdateStack'],
                    'definition editor submitted an unrelated write')
            require(proxy.write_operations[1]['fields'] == ['auto_pull'],'one-field update sent unrelated fields')
            source_edit = {'target':{'id':created_id},'patch':{
                'source':{'kind':'inline','contents':'services: {}'}}}
            p = await mcp.tool('preview_stack_update',source_edit)
            require(p['definition']['details']['requires_operate_to_apply'],'source preview omitted the operate requirement')
            reads, writes = dict(proxy.read_operations), proxy.write_attempts
            denied = await mcp.rpc('tools/call',{'name':'apply_stack_update','arguments':{
                **source_edit,'preview_id':p['preview_id']}})
            require(denied.get('isError') and denied['structuredContent']['kind'] == 'capability_disabled',
                    'configuration-only source update bypassed the operate grant')
            require(proxy.read_operations == reads and proxy.write_attempts == writes,
                    'configuration-only source update reached Core')
            REPORT['stack_definitions'] = {'created_id':created_id,'inspected_id':inspected['id'],
                'creation_preview_writes':0,'creation_apply_writes':1,
                'one_field_source_and_unrelated_configuration_preserved':preserved,
                'new_definition_workload_count':0,'target_reassignments':target_changes,
                'configuration_only_execute_attempts':proxy.execute_attempts,'writes':proxy.write_operations,
                'source_update_without_operate_denied_locally':True,
                'denied_source_update_core_reads':0,'denied_source_update_core_writes':0,
                'live_matrix':['inline Compose on Server','Server target reassignment and restoration'],
                'fixture_only_matrix':['direct Git','linked Repo','host files','Swarm attachment']}
    persist()
    print('Verified real Core Stack creation, source-preserving edit and old-target reassignment behavior.',flush=True)


async def exercise_deployment_definitions(address, editor, deployment_id, server_id, alternate_server_id):
    """Compare authoritative saved configuration with the owned running container."""
    api = Api(address, {'x-api-key':editor['key'], 'x-api-secret':editor['secret']})

    def created_containers():
        return run([*COMPOSE, 'exec', '-T', 'docker', 'docker', 'ps', '-aq',
                    '--filter', 'name=^/mcp-config-deployment$']).splitlines()

    def running_container():
        template = '{"id":{{json .Id}},"image":{{json .Config.Image}},"running":{{json .State.Running}}}'
        return json.loads(run([*COMPOSE, 'exec', '-T', 'docker', 'docker', 'inspect',
                               '--format', template, 'mcp-smoke-deployment']))

    create = {'name':'mcp-config-deployment','config':{
        'attachment':{'kind':'server','target':{'id':server_id}},
        'image':{'kind':'image','reference':'busybox:1.37.0'},
        'network':'bridge','ports':'19090:8080',
        'environment':'REFERENCE=[[MCP_DISPOSABLE_REFERENCE]]\n','command':'sleep 600'}}
    async with LossProxy(address,editor,lose_reply=False) as proxy:
        async with await Mcp.start(proxy.address,editor,operate=False,configuration_edit=True) as mcp:
            p = await mcp.tool('preview_deployment_create',create)
            require(proxy.write_attempts == 0,'Deployment create preview submitted a write')
            saved = await mcp.tool('apply_deployment_create',{**create,'preview_id':p['preview_id']})
            require(saved['outcome'] == 'saved' and proxy.write_attempts == 1,'Deployment creation did not save once')
            created_id = saved['resource']['id']
            inspected = await mcp.tool('get_deployment',{'id':created_id})
            require(inspected['id'] == created_id,'new Deployment identity was not inspectable')
            edit = {'target':{'id':created_id},'patch':{'image':{'kind':'image','reference':'busybox:1.36.1'}}}
            p = await mcp.tool('preview_deployment_update',edit)
            require(p['definition']['details']['requires_operate_to_apply'],'image preview omitted operate requirement')
            reads, writes = dict(proxy.read_operations), proxy.write_attempts
            denied = await mcp.rpc('tools/call',{'name':'apply_deployment_update','arguments':{
                **edit,'preview_id':p['preview_id']}})
            require(denied.get('isError') and denied['structuredContent']['kind'] == 'capability_disabled',
                    'configuration-only image update bypassed operate policy')
            require(proxy.read_operations == reads and proxy.write_attempts == writes,
                    'configuration-only image update reached Core')
            require(proxy.execute_attempts == 0,'configuration-only creation submitted execution')
            REPORT['deployment_creation'] = {'id':created_id,'inspected_id':inspected['id'],
                'preview_writes':0,'apply_writes':1,'image_apply_without_operate_denied_locally':True,
                'denied_image_apply_core_reads':0,'denied_image_apply_core_writes':0,
                'execute_attempts':proxy.execute_attempts,'writes':proxy.write_operations}

    async with LossProxy(address,editor,lose_reply=False) as proxy:
        async with await Mcp.start(proxy.address,editor,operate=True,configuration_edit=True) as mcp:
            before = await asyncio.to_thread(api.read,'GetDeployment',{'deployment':created_id})
            p = await mcp.tool('preview_deployment_update',edit)
            require(proxy.write_attempts == 0,'image preview submitted a write')
            saved = await mcp.tool('apply_deployment_update',{**edit,'preview_id':p['preview_id']})
            require(saved['outcome'] == 'saved','one-field image edit did not save')
            after = await asyncio.to_thread(api.read,'GetDeployment',{'deployment':created_id})
            preserved = all(v == after['config'][k] for k,v in before['config'].items() if k != 'image')
            require(preserved,'image edit changed ports, environment, target or other configuration')
            require(after['config']['image']['params']['image'] == 'busybox:1.36.1','saved image not observed')
            require(not await asyncio.to_thread(created_containers),'new Deployment definition implicitly created a container')
            noop = await mcp.tool('preview_deployment_update',edit)
            require((await mcp.tool('apply_deployment_update',{**edit,'preview_id':noop['preview_id']}))['outcome'] == 'no_op'
                    and proxy.write_attempts == 1,'unchanged image update submitted another write')

            original = await asyncio.to_thread(api.read,'GetDeployment',{'deployment':deployment_id})
            require(not original['config']['auto_update'],'owned image-separation fixture must disable auto-update')
            running_before = await asyncio.to_thread(running_container)
            require(running_before['running'],'owned Deployment was not running for image-separation proof')
            edit_running = {'target':{'id':deployment_id},'patch':{'image':{'kind':'image','reference':'busybox:1.36.1'}}}
            p = await mcp.tool('preview_deployment_update',edit_running)
            require(not p['definition']['details']['runtime_observation']['running_configuration_verified'],
                    'configuration preview claimed running configuration was verified')
            saved = await mcp.tool('apply_deployment_update',{**edit_running,'preview_id':p['preview_id']})
            require(saved['outcome'] == 'saved','running Deployment desired-image update failed')
            stored = await asyncio.to_thread(api.read,'GetDeployment',{'deployment':deployment_id})
            running_after = await asyncio.to_thread(running_container)
            require(stored['config']['image']['params']['image'] == 'busybox:1.36.1'
                    and running_after == running_before,'saving desired image replaced or changed running container')
            require(stored['config']['image']['params']['image'] != running_after['image'],
                    'fixture did not distinguish saved desired and actual running image')
            restore = {'target':{'id':deployment_id},'patch':{'image':{
                'kind':'image','reference':original['config']['image']['params']['image']}}}
            p = await mcp.tool('preview_deployment_update',restore)
            require((await mcp.tool('apply_deployment_update',{**restore,'preview_id':p['preview_id']}))['outcome'] == 'saved',
                    'original desired-image restoration failed')
            targets = []
            for target_id in [alternate_server_id,server_id]:
                target_edit = {'target':{'id':deployment_id},'patch':{
                    'attachment':{'kind':'server','target':{'id':target_id}}}}
                p = await mcp.tool('preview_deployment_update',target_edit)
                require(p['definition']['details']['target_changed'],'Deployment target effect was not identified')
                require((await mcp.tool('apply_deployment_update',{**target_edit,'preview_id':p['preview_id']}))['outcome'] == 'saved',
                        'Deployment target reassignment failed')
                stored = await asyncio.to_thread(api.read,'GetDeployment',{'deployment':deployment_id})
                require(stored['config']['server_id'] == target_id and
                        await asyncio.to_thread(running_container) == running_before,
                        'Deployment reassignment altered or removed the old-target running container')
                targets.append({'target_id':target_id,'saved':True,'old_target_running_container_preserved':True})
            require(proxy.execute_attempts == 0,'definition editor submitted lifecycle/pull execution')
            require(proxy.write_operations == [{'type':'UpdateDeployment','fields':['image']}]*3 +
                    [{'type':'UpdateDeployment','fields':['server_id']}]*2,
                    'Deployment edits submitted unrelated fields or operations')
            REPORT['deployment_definitions'] = {'created_id':created_id,'source':'direct image on Server',
                'image_only_unrelated_configuration_preserved':preserved,'new_definition_container_count':0,
                'no_op_writes':0,'saved_desired_image':'busybox:1.36.1',
                'actual_running_container_before':running_before,'actual_running_container_after_save':running_after,
                'desired_and_running_image_distinguished':True,'desired_image_restored':True,
                'target_reassignments':targets,'execute_attempts':proxy.execute_attempts,'writes':proxy.write_operations,
                'fixture_only_matrix':['Build references with latest/pinned versions','Swarm attachments']}
    persist()
    print('Verified real Core Deployment creation, image preservation and separate running-container observations.',flush=True)


async def exercise_server_definitions(address, editor, denied_creator, initial_public_key):
    """Save owned Server records while checking connectivity independently."""
    api = Api(address, {'x-api-key':editor['key'], 'x-api-secret':editor['secret']})
    def workload_snapshot():
        return sorted(run([*COMPOSE,'exec','-T','docker','docker','ps','-a',
                           '--format','{{.ID}}:{{.Image}}:{{.State}}']).splitlines())
    before_workloads = await asyncio.to_thread(workload_snapshot)
    create = {'name':'mcp-config-server','config':{
        'address':'http://127.0.0.1:9','enabled':False,'insecure_tls':False,'region':'owned'}}
    async with LossProxy(address,editor,lose_reply=False) as proxy:
        async with await Mcp.start(proxy.address,editor,operate=False,configuration_edit=True) as mcp:
            p = await mcp.tool('preview_server_create',create)
            require(proxy.write_attempts == 0,'Server preview submitted a write')
            saved = await mcp.tool('apply_server_create',{**create,'preview_id':p['preview_id']})
            require(saved['outcome'] == 'saved' and saved['saved_configuration_verified'],
                    'disabled Server creation was not verified')
            server_id = saved['resource']['id']
            inspected = await mcp.tool('get_server',{'id':server_id})
            require(inspected['id'] == server_id and not inspected['enabled'],
                    'new disabled Server was not inspectable')
            stored = await asyncio.to_thread(api.read,'GetServer',{'server':server_id})
            require(not stored['config']['auto_prune'] and not stored['config']['auto_rotate_keys'],
                    'Server creation inherited destructive background defaults')
            edit = {'target':{'id':server_id},'patch':{
                'send_unreachable_alerts':not stored['config']['send_unreachable_alerts']}}
            p = await mcp.tool('preview_server_update',edit)
            changed = await mcp.tool('apply_server_update',{**edit,'preview_id':p['preview_id']})
            after = await asyncio.to_thread(api.read,'GetServer',{'server':server_id})
            preserved = all(v == after['config'][k] for k,v in stored['config'].items()
                            if k != 'send_unreachable_alerts') and stored['info']['public_key'] == after['info']['public_key']
            require(changed['outcome'] == 'saved' and preserved,'alert-only save changed unrelated config or trust')
            require(changed['definition']['details']['connectivity']['core_reported_state'] == 'Disabled'
                    and not changed['definition']['details']['connectivity']['connection_verified'],
                    'saved Server definition was conflated with connectivity')
            noop = await mcp.tool('preview_server_update',edit)
            writes = proxy.write_attempts
            require((await mcp.tool('apply_server_update',{**edit,'preview_id':noop['preview_id']}))['outcome'] == 'no_op'
                    and proxy.write_attempts == writes,'unchanged Server save wrote again')
            connection = {'target':{'id':server_id},'patch':{'address':'http://127.0.0.1:10'}}
            p = await mcp.tool('preview_server_update',connection)
            reads = dict(proxy.read_operations)
            denied = await mcp.rpc('tools/call',{'name':'apply_server_update','arguments':{
                **connection,'preview_id':p['preview_id']}})
            require(denied.get('isError') and denied['structuredContent']['kind'] == 'capability_disabled'
                    and proxy.read_operations == reads and proxy.write_attempts == writes,
                    'configuration-only connection apply reached Core')
            require(proxy.execute_attempts == 0,'Server record session submitted execution')
            REPORT['server_definitions'] = {'created_id':server_id,'inspected_id':inspected['id'],
                'preview_writes':0,'alert_only_unrelated_configuration_and_trust_preserved':preserved,
                'stored_enabled':False,'core_reported_connectivity':'Disabled','connection_verified':False,
                'no_op_writes':0,'connection_apply_without_operate_denied_locally':True,
                'denied_connection_core_reads':0,'denied_connection_core_writes':0,
                'auto_prune':False,'auto_rotate_keys':False,'execute_attempts':0,'writes':proxy.write_operations}

    # The provisioner supplies independently observed initial trust. It never
    # enters MCP as an admin credential, and the unreachable endpoint is inside
    # the owned Core container, not an unrelated host.
    SECRETS.append(initial_public_key)
    create = {'name':'mcp-config-unreachable-server','config':{
        'address':'http://127.0.0.1:9','enabled':True,'insecure_tls':False},
        'initial_public_key':initial_public_key}
    async with LossProxy(address,editor,lose_reply=False) as proxy:
        async with await Mcp.start(proxy.address,editor,operate=True,configuration_edit=True) as mcp:
            p = await mcp.tool('preview_server_create',create)
            saved = await mcp.tool('apply_server_create',{**create,'preview_id':p['preview_id']})
            server_id = saved['resource']['id']
            stored = await asyncio.to_thread(api.read,'GetServer',{'server':server_id})
            require(saved['outcome'] == 'saved' and saved['saved_configuration_verified']
                    and stored['config']['enabled'] and stored['info']['public_key'] == initial_public_key,
                    'unreachable Server did not retain its saved config and explicit trust')
            connection = saved['definition']['details']['connectivity']
            require(connection['core_reported_state'] == 'NotOk' and not connection['connection_verified']
                    and connection['error']['operation'] == 'ServerConnectivity',
                    'unreachable Server did not report a separate actionable connection failure')
            require(initial_public_key not in json.dumps(p) + json.dumps(saved),
                    'initial trust leaked into preview or saved result')
            require(proxy.execute_attempts == 0 and proxy.write_attempts == 1,
                    'unreachable Server triggered an MCP fallback or execution')
            REPORT['server_unreachable'] = {'id':server_id,'saved_configuration_verified':True,
                'stored_enabled':True,'initial_pinned_trust_preserved':True,
                'core_reported_connectivity':connection['core_reported_state'],
                'connection_verified':False,'sanitized_error':connection['error'],
                'execute_attempts':0,'writes':proxy.write_operations}

    async with LossProxy(address,denied_creator,lose_reply=False) as proxy:
        async with await Mcp.start(proxy.address,denied_creator,operate=False,configuration_edit=True) as mcp:
            create = {'name':'mcp-denied-server','config':{'address':'','enabled':False,'insecure_tls':False}}
            p = await mcp.tool('preview_server_create',create)
            denied = await mcp.tool('apply_server_create',{**create,'preview_id':p['preview_id']})
            require(denied['outcome'] == 'submission_unknown'
                    and denied['observation_error']['kind'] == 'permission_denied'
                    and 'create_server_permissions' in denied['observation_error']['message']
                    and proxy.execute_attempts == 0 and proxy.write_attempts == 1,
                    'denied Server creation lacked sanitized permissions or attempted a fallback')
            REPORT['server_creation_denial'] = {'same_non_admin_identity':True,
                'core_create_server_permission':False,'write_attempts':1,'execute_attempts':0,
                'sanitized_error':denied['observation_error']}
    unchanged = before_workloads == await asyncio.to_thread(workload_snapshot)
    require(unchanged,'Server record writes changed owned container IDs/images/states')
    REPORT['server_definitions']['workload_ids_images_states_unchanged'] = unchanged
    persist()
    print('Verified real Core Server creation/alert edit, pinned trust, separate unreachable state and permission denial.',flush=True)


def exercise_environment():
    addresses = run([*COMPOSE, 'port', 'core', '9120']).splitlines()
    if len(addresses) != 1 or not re.fullmatch(r'127\.0\.0\.1:\d+', addresses[0]):
        raise Failure('Core is not published exclusively on the expected loopback endpoint')
    address = 'http://' + addresses[0]
    provision = json.loads((ROOT/'provisioning.json').read_text())
    SECRETS.append(provision['admin_password'])
    anonymous = Api(address)
    poll(lambda: anonymous.request('/auth/login', 'GetLoginOptions'), 'Core local-auth readiness')
    login = anonymous.request('/auth/login', 'LoginLocalUser', {'username': provision['admin_username'], 'password': provision['admin_password']})
    jwt = login['data']['jwt']
    SECRETS.append(jwt)
    admin = Api(address, {'authorization': jwt})
    version = admin.read('GetVersion')['version']
    if version != CORE_VERSION:
        raise Failure('disposable Core version mismatch')
    REPORT['core_version'] = version
    servers = admin.read('ListServers', {'limit': 10})
    matches = [s for s in servers if s['name'] == 'mcp-smoke-server']
    if len(matches) != 1:
        raise Failure('exact disposable Server was not provisioned')
    server_id = identity(matches[0])
    alternate = admin.write('CreateServer', {'name':'mcp-smoke-alternate','config':{
        'address':'http://unreachable.invalid:8120','enabled':False}})
    alternate_server_id = identity(alternate)
    poll(lambda: admin.read('GetServerState', {'server': server_id})['status'] == 'Ok', 'private Periphery/daemon readiness')
    periphery = admin.read('GetPeripheryInformation', {'server': server_id})
    if periphery['version'] != CORE_VERSION:
        raise Failure('disposable Periphery version mismatch')
    REPORT['periphery_version'] = periphery['version']
    REPORT['docker_version'] = poll(lambda: run([*COMPOSE, 'exec', '-T', 'docker', 'docker', 'version', '--format', '{{.Server.Version}}']), 'isolated Docker daemon readiness')
    REPORT['scope'] = {'server_id': server_id, 'alternate_server_id':alternate_server_id,
        'non_admin_identities': [], 'host_docker_socket_mounted': False,
        'core_non_admin_creation_enabled_for_fixture':True}
    stack = admin.write('CreateStack', {'name': 'mcp-smoke-stack', 'config': {
        'server_id': server_id, 'project_name': 'mcp-smoke-stack', 'auto_pull': False,
        'file_contents': 'services:\n  db:\n    image: busybox:1.37.0\n    command: ["sh", "-c", "sleep 600"]\n  web:\n    image: busybox:1.37.0\n    command: ["sh", "-c", "sleep 600"]\n    depends_on:\n      db:\n        condition: service_started\n        restart: true\n',
        'pre_deploy': {'path': '.', 'command': 'printf mcp_pre_hook', 'shell_mode': True},
        'post_deploy': {'path': '.', 'command': 'printf mcp_post_hook', 'shell_mode': True}}})
    stack_id = identity(stack)
    deployment = admin.write('CreateDeployment', {'name': 'mcp-smoke-deployment', 'config': {
        'server_id': server_id, 'image': {'type': 'Image', 'params': {'image': 'busybox:1.37.0'}},
        'network': 'bridge', 'command': 'sh -c "sleep 600"'}})
    deployment_id = identity(deployment)
    REPORT['scope'].update(stack_id=stack_id, deployment_id=deployment_id)
    credentials = []
    for username, level in [('mcp-smoke-operator', 'Execute'), ('mcp-smoke-reader', 'Read'),
                            ('mcp-smoke-editor', 'Write')]:
        user = admin.write('CreateServiceUser', {'username': username, 'description': 'Disposable komodo-mcp acceptance identity'})
        user_id = identity(user)
        if user['admin'] or user['super_admin']:
            raise Failure('test identity unexpectedly administrative')
        admin.write('UpdateUserBasePermissions', {'user_id': user_id, 'enabled': True, 'create_servers': level == 'Write', 'create_builds': False})
        for kind, resource_id, grant in [('Server', server_id, 'Read'), ('Stack', stack_id, level), ('Deployment', deployment_id, level)]:
            admin.write('UpdatePermissionOnTarget', {'user_target': {'type': 'User', 'id': user_id},
                'resource_target': {'type': kind, 'id': resource_id}, 'permission': {'level': grant,
                'specific': ['Attach'] if kind == 'Server' and level == 'Write' else ['Inspect', 'Logs']}})
        if level == 'Write':
            admin.write('UpdatePermissionOnTarget', {'user_target': {'type':'User','id':user_id},
                'resource_target':{'type':'Server','id':alternate_server_id},
                'permission':{'level':'Read','specific':['Attach']}})
        keys = admin.write('CreateApiKeyForServiceUser', {'user_id': user_id, 'name': 'disposable-mcp-smoke', 'expires': int(time.time()*1000)+3600000})
        SECRETS.extend([keys['key'], keys['secret']])
        caller = Api(address, {'x-api-key': keys['key'], 'x-api-secret': keys['secret']}).request('/user')
        if not caller['enabled'] or caller['admin'] or caller['super_admin']:
            raise Failure('test caller does not have the intended non-admin identity')
        credentials.append(keys)
        REPORT['scope']['non_admin_identities'].append({'id': user_id, 'username': username, 'workload_permission': level})
    fd = os.open(ROOT/'test-identities.json', os.O_CREAT|os.O_EXCL|os.O_WRONLY, 0o600)
    with os.fdopen(fd,'w') as output:
        json.dump({'operator': credentials[0], 'reader': credentials[1], 'editor':credentials[2]}, output)
    poll(lambda: {'web','db'} <= {s['service_name'] for s in admin.read('GetStack', {'stack': stack_id})['info']['latest_services']}, 'latest saved Compose services')
    persist()
    REPORT['compose_version'] = run([*COMPOSE, 'exec', '-T', 'periphery', 'docker', 'compose', 'version', '--short'])
    asyncio.run(exercise(address, credentials[0],credentials[1],stack_id,deployment_id))
    asyncio.run(exercise_configuration(address,credentials[2],stack_id))
    asyncio.run(exercise_stack_definitions(address,credentials[2],stack_id,server_id,alternate_server_id))
    asyncio.run(exercise_deployment_definitions(address,credentials[2],deployment_id,server_id,alternate_server_id))
    asyncio.run(exercise_server_definitions(address,credentials[2],credentials[0],periphery['public_key']))
    services = Api(address, {'x-api-key': credentials[0]['key'], 'x-api-secret': credentials[0]['secret']}).read('ListAllStackServices', {'stacks': [stack_id]})
    REPORT['observed_services'] = [{'service': s['service'], 'state': s['state']} for s in services]
    if not {'web','db'} <= {s['service'] for s in services}:
        raise Failure('requested service/dependency were not observed')
    REPORT['passed'] = True
    print('Disposable real-Core acceptance passed.', flush=True)

def cleanup():
    # Namespaced ownership was reserved before creation; refuse any container
    # bearing this project label if it lacks this execution session's label.
    ids = run([*DOCKER, 'ps', '-aq', '--filter', 'label=com.docker.compose.project='+PROJECT]).splitlines()
    for container in ids:
        owner = run([*DOCKER, 'inspect', '--format', '{{ index .Config.Labels "komodo-mcp.test.session" }}', container])
        if owner != OWNER:
            raise Failure('refusing cleanup: unexpected ownership in reserved project')
    for kind,args in [('network',['network','ls','-q']),('volume',['volume','ls','-q'])]:
        for resource in run([*DOCKER,*args,'--filter','label=com.docker.compose.project='+PROJECT]).splitlines():
            owner = run([*DOCKER,kind,'inspect','--format','{{ index .Labels "komodo-mcp.test.session" }}',resource])
            if owner != OWNER:
                raise Failure('refusing cleanup: unexpected '+kind+' ownership')
    run([*COMPOSE, 'down', '-v', '--timeout', '10'], timeout=120)
    remaining = {kind: run([*DOCKER, *args, '--filter', 'label=com.docker.compose.project='+PROJECT]).splitlines()
        for kind,args in [('containers',['ps','-aq']),('networks',['network','ls','-q']),('volumes',['volume','ls','-q'])]}
    REPORT['cleanup_remaining'] = remaining
    REPORT['cleanup_verified'] = not any(remaining.values())
    if not REPORT['cleanup_verified']:
        raise Failure('owned disposable resources remain after cleanup')
    # Provisioning and test credentials cease to exist with the private database.
    for filename in ['provisioning.json', 'test-identities.json', 'compose.json']:
        (ROOT/filename).unlink(missing_ok=True)
    print('Verified cleanup: no project containers, networks or volumes remain.', flush=True)

def bootstrap():
    global RESERVED
    for args in [['ps','-aq'], ['network','ls','-q'], ['volume','ls','-q']]:
        if run([*DOCKER, *args, '--filter', 'label=com.docker.compose.project='+PROJECT]):
            raise Failure('reserved disposable project is already in use')
    RESERVED = True
    values = {'__DATABASE_PASSWORD__':secrets.token_hex(32), '__ADMIN_PASSWORD__':secrets.token_hex(32), '__JWT_SECRET__':secrets.token_hex(32)}
    template = (WORKSPACE/'tests/fixtures/disposable-core/compose.json').read_text().replace('__OWNER__',OWNER)
    for key,value in values.items():
        template = template.replace(key,value)
        SECRETS.append(value)
    for name,content in [('compose.json',template), ('provisioning.json',json.dumps({'admin_username':'smoke-provisioner','admin_password':values['__ADMIN_PASSWORD__']}))]:
        fd=os.open(ROOT/name,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
        with os.fdopen(fd,'w') as output: output.write(content)
    run([*COMPOSE,'up','-d'],timeout=360)

def project_name(value):
    if not re.fullmatch(r'komodo-mcp-smoke-[a-z0-9][a-z0-9-]{0,40}',value):
        raise argparse.ArgumentTypeError('use an unused komodo-mcp-smoke-... project name')
    return value


def build_candidate():
    if run(['git','status','--porcelain']):
        raise Failure('commit the candidate and use a clean checkout before the live smoke')
    REPORT['built_commit'] = run(['git','rev-parse','HEAD'])
    package = tomllib.loads((WORKSPACE/'Cargo.toml').read_text())['package']
    REPORT['package_version'] = package['version']
    output = run(['cargo','build','--locked','--release','--bin','komodo-mcp',
        '--target-dir',str(WORKSPACE/'target'),'--message-format=json'],timeout=1800)
    artifacts = []
    for line in output.splitlines():
        try: value = json.loads(line)
        except ValueError: continue
        if value.get('reason') == 'compiler-artifact' and value.get('target',{}).get('name') == 'komodo-mcp' and value.get('executable'):
            artifacts.append(Path(value['executable']).resolve())
    expected = (WORKSPACE/'target/release/komodo-mcp').resolve()
    if artifacts != [expected] or not expected.is_file():
        raise Failure('Cargo did not identify the expected native release executable')
    if run(['git','rev-parse','HEAD']) != REPORT['built_commit'] or run(['git','status','--porcelain']):
        raise Failure('candidate changed during build')
    REPORT['executable_sha256'] = hashlib.sha256(expected.read_bytes()).hexdigest()
    return expected


def entry(argv=None):
    global ROOT, PROJECT, OWNER, DOCKER, COMPOSE, BINARY, EVIDENCE, RESERVED
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--docker-context',required=True,help='explicit local Unix-socket Docker context')
    parser.add_argument('--project',required=True,type=project_name,help='unused project; only this project is created/removed')
    parser.add_argument('--evidence',required=True,type=Path,help='new sanitized JSON evidence file')
    args = parser.parse_args(argv)
    RESERVED = False
    SECRETS.clear()
    REPORT.clear()
    PROJECT, OWNER = args.project, uuid.uuid4().hex
    DOCKER = ['docker','--context',args.docker_context]
    EVIDENCE = args.evidence.resolve()
    EVIDENCE.parent.mkdir(parents=True,exist_ok=True)
    try:
        fd = os.open(EVIDENCE,os.O_CREAT|os.O_EXCL|os.O_WRONLY,0o600)
    except FileExistsError:
        parser.error('evidence file exists; retain it and choose a new path')
    os.close(fd)
    ROOT = Path(tempfile.mkdtemp(prefix=PROJECT+'-'))
    COMPOSE = [*DOCKER,'compose','--project-name',PROJECT,'--file',str(ROOT/'compose.json')]
    REPORT.update(project=PROJECT,docker_context=args.docker_context,owner_label=OWNER,
        started=datetime.datetime.now(datetime.timezone.utc).isoformat(),operations=[],cleanup_verified=False,
        phase='preflight',private_cleanup_directory=str(ROOT))
    persist()
    result = 0
    try:
        endpoint = run([*DOCKER,'context','inspect',args.docker_context,'--format','{{.Endpoints.docker.Host}}'])
        if not endpoint.startswith('unix://'):
            raise Failure('select a local Unix-socket Docker context for loopback-only Core')
        BINARY = build_candidate()
        REPORT['phase'] = 'provisioning'
        persist()
        bootstrap()
        REPORT['phase'] = 'exercising'
        persist()
        exercise_environment()
    except BaseException as error:
        result = 1
        REPORT['passed'] = False
        import traceback
        REPORT['failure_location'] = [{'file':Path(frame.filename).name,'line':frame.lineno} for frame in traceback.extract_tb(error.__traceback__)[-2:]]
        REPORT['failure'] = str(error) if isinstance(error,Failure) else type(error).__name__
        print('Disposable smoke did not pass:',REPORT['failure'],flush=True)
    finally:
        REPORT['phase'] = 'cleanup'
        # Evidence failures must never prevent cleanup of the owned project.
        try:
            persist()
        except BaseException as error:
            result = 1
            REPORT['passed'] = False
            REPORT['evidence_error'] = type(error).__name__
        try:
            if RESERVED:
                cleanup()
            else:
                REPORT.update(cleanup_verified=True,cleanup_action='no resources created')
            if REPORT['cleanup_verified']:
                shutil.rmtree(ROOT)
                REPORT.pop('private_cleanup_directory',None)
        except BaseException as error:
            result = 1
            REPORT['passed'] = False
            REPORT['cleanup_verified'] = False
            REPORT['cleanup_error'] = str(error) if isinstance(error,Failure) else type(error).__name__
            REPORT['private_cleanup_directory'] = str(ROOT)
            print('Cleanup needs attention; retain the private directory identified in evidence.',flush=True)
        REPORT['finished'] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        REPORT['phase'] = 'complete' if result == 0 else 'failed'
        try:
            persist()
        except BaseException:
            result = 1
            print('Evidence could not be written; cleanup_verified=' + str(REPORT['cleanup_verified']),flush=True)
    return result


if __name__ == '__main__':
    raise SystemExit(entry())
