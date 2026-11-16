"""Private, owned test infrastructure for disposable_swarm_admin_smoke.py.

This is fixture provisioning, never a product Docker/API passthrough. No helper
accepts a remote Docker endpoint or changes credentials after an MCP denial.
"""
import asyncio
import collections
import copy
import hashlib
import http.client
import http.server
import json
import os
from pathlib import Path
import re
import secrets
import socket
import subprocess
import threading
import time

import disposable_core_smoke as s
import disposable_resource_sync_smoke as retained

LABEL = 'komodo-mcp.test.session'
BUSYBOX = 'busybox@sha256:a7dbc319b1438115a207e11930dc4bb3e643dcb3ec0bdd694bb36cd8fad289ec'
PINS = {
    'mongo:7.0.16': 'mongo@sha256:c630c59342c1493d50345136df2af14a76b9e827dd5316bfabee07a0880a5f3a',
    'docker:27.5.1-dind': 'docker@sha256:aa3df78ecf320f5fafdce71c659f1629e96e9de0968305fe1de670e0ca9176ce',
    'ghcr.io/moghtech/komodo-core:2.3.2': 'ghcr.io/moghtech/komodo-core@sha256:7ae1d1547b16f1fd88902050105d80bbefc41b3023baf1f6dbc2b8df47ecb526',
    'ghcr.io/moghtech/komodo-periphery:2.3.2': 'ghcr.io/moghtech/komodo-periphery@sha256:639e019751642518a0fbdc2720e5f39789e76aa495a499852b9585ffacf71149',
}
PYTHON = 'python@sha256:9ef1a599f863231c3d91e941389bfdc4a2d6070557243d54df1a4e57015b960e'
F = None
CACHE = None


def checked(value):
    text = json.dumps(value, ensure_ascii=False)
    s.require(not any(secret and (secret in text or json.dumps(secret)[1:-1] in text)
                      for secret in s.SECRETS), 'private fixture material escaped MCP/evidence')
    return value


def permission(value):
    if isinstance(value, str):
        s.require(value in ['None', 'Read', 'Execute', 'Write'], 'invalid scalar permission')
        return {'level': value, 'specific': []}
    s.require(isinstance(value, dict) and value.get('level') in ['None', 'Read', 'Execute', 'Write']
              and isinstance(value.get('specific'), list), 'incomplete effective permission')
    return value


def customize(config):
    global CACHE
    retained.customize(config)
    for svc in config['services'].values():
        svc['image'] = PINS.get(svc['image'], svc['image'])
        svc['restart'] = 'no'
    for name in ['PERIPHERY_DISABLE_TERMINALS', 'PERIPHERY_DISABLE_CONTAINER_TERMINALS']:
        config['services']['periphery']['environment'][name] = 'false'
    worker = copy.deepcopy(config['services']['docker'])
    worker['volumes'] = ['worker-data:/var/lib/docker', 'worker-socket:/var/run']
    config['services']['docker-worker'] = worker
    labels = {LABEL: s.OWNER, 'komodo-mcp.test.scope': 'disposable-swarm-admin'}
    for name in ['worker-data', 'worker-socket', 'receiver-data']:
        config['volumes'][name] = {'labels': dict(labels)}
    nonce = secrets.token_hex(24)
    s.SECRETS.append(nonce)
    receiver = s.ROOT / 'receiver.py'
    receiver.write_bytes((s.WORKSPACE / 'tests/fixtures/disposable-core/receiver.py').read_bytes())
    receiver.chmod(0o600)
    config['services']['receiver'] = {'image': PYTHON,
        'entrypoint': ['python3', '/fixture/receiver.py'],
        'environment': {'RECEIVER_NONCE': nonce, 'RECEIVER_STATE': '/state/receipts.jsonl'},
        'volumes': [str(receiver) + ':/fixture/receiver.py:ro', 'receiver-data:/state'],
        'read_only': True, 'cap_drop': ['ALL'], 'security_opt': ['no-new-privileges:true'],
        'labels': labels}
    CACHE = ImageCache({svc['image'] for svc in config['services'].values()})
    for node in ['docker', 'docker-worker']:
        s.require('ports' not in config['services'][node]
                  and all(not v.startswith('/') for v in config['services'][node]['volumes']),
                  'nested engines must use owned named volumes without outer ports/socket')


class ImageCache:
    """Remove only fixture-pulled, unchanged explicit refs; never outer prune."""
    def __init__(self, refs):
        self.refs = refs
        self.before = self.rows()
        self.before_ids = {row['ID'] for row in self.before}
        self.observed = {}
        self.outer = s.run([*s.DOCKER, 'info', '--format', '{{.ID}}'])
    @staticmethod
    def rows():
        rows = [json.loads(line) for line in s.run([*s.DOCKER, 'image', 'ls', '--all',
            '--digests', '--no-trunc', '--format', '{{json .}}']).splitlines()]
        s.require(len(rows) <= 4096, 'outer image-cache inventory incomplete')
        return rows
    @staticmethod
    def references(row):
        result = set()
        if row['Repository'] != '<none>':
            if row['Tag'] != '<none>': result.add(row['Repository'] + ':' + row['Tag'])
            if row['Digest'] != '<none>': result.add(row['Repository'] + '@' + row['Digest'])
        return result
    def observe(self):
        s.require(s.run([*s.DOCKER, 'info', '--format', '{{.ID}}']) == self.outer, 'outer daemon changed')
        ids = s.run([*s.DOCKER, 'ps', '-aq', '--filter', 'label=com.docker.compose.project=' + s.PROJECT]).splitlines()
        for cid in ids:
            row = json.loads(s.run([*s.DOCKER, 'inspect', cid]))[0]
            s.require(row['Config']['Labels'].get(LABEL) == s.OWNER, 'image provenance owner mismatch')
            ref = row['Config']['Image']
            s.require(ref in self.refs, 'unexpected image in owned project')
            self.observed[ref] = row['Image']
    def cleanup(self):
        s.require(s.run([*s.DOCKER, 'info', '--format', '{{.ID}}']) == self.outer, 'outer daemon changed')
        results = []
        for ref, expected in self.observed.items():
            if expected in self.before_ids:
                results.append({'reference': ref, 'preexisting_preserved': True})
                continue
            matches = [row for row in self.rows() if ref in self.references(row)]
            if not matches:
                results.append({'reference': ref, 'already_absent': True})
                continue
            s.require({row['ID'] for row in matches} == {expected}, 'fixture image reference changed; removal refused')
            # All containers, including concurrent unrelated projects, protect
            # their image. No force, image-ID deletion, or host-wide prune.
            users = s.run([*s.DOCKER, 'ps', '-aq', '--filter', 'ancestor=' + expected]).splitlines()
            if users:
                results.append({'reference': ref, 'concurrent_use_preserved': True})
                continue
            s.run([*s.DOCKER, 'image', 'rm', ref])
            s.require(not any(ref in self.references(row) for row in self.rows()), 'owned image ref remains')
            results.append({'reference': ref, 'new_unchanged_reference_removed': True})
        return {'verified': True, 'outer_prune_calls': 0, 'references': results}


class Engine:
    def __init__(self, service):
        self.service = service
        self.cid = s.run([*s.COMPOSE, 'ps', '-q', service])
        record = json.loads(s.run([*s.DOCKER, 'inspect', self.cid]))[0]
        s.require(record['Config']['Labels'].get(LABEL) == s.OWNER,
                  'nested engine ownership mismatch')
        info = json.loads(s.poll(lambda: self.call('info', '--format', '{{json .}}'), service))
        self.daemon = info['ID']
        s.require(self.daemon and self.daemon != F.outer, 'refusing outer daemon as nested engine')
        self.version = info['ServerVersion']
        s.require(self.version == '27.5.1', 'nested Docker version mismatch')

    def call(self, *args, input_data=None, timeout=90):
        # Subprocess output is private and bounded; failures never include argv,
        # which may contain a Swarm join token or fixture-only database command.
        with __import__('tempfile').TemporaryFile() as output:
            process = subprocess.Popen([*s.DOCKER, 'exec', *(['-i'] if input_data is not None else []),
                self.cid, 'docker', '--host', 'unix:///var/run/docker.sock', *args],
                stdin=subprocess.PIPE if input_data is not None else subprocess.DEVNULL,
                stdout=output, stderr=output, start_new_session=True)
            try:
                process.communicate(input=input_data, timeout=timeout)
                s.require(process.returncode == 0, 'owned nested Docker command failed')
                s.require(output.tell() <= s.MAX_COMMAND_OUTPUT, 'nested Docker output exceeded bound')
                output.seek(0)
                return output.read(s.MAX_COMMAND_OUTPUT).decode().strip()
            finally:
                try: os.killpg(process.pid, __import__('signal').SIGKILL)
                except ProcessLookupError: pass
                process.wait(timeout=5)

    def verify(self):
        s.require(s.run([*s.DOCKER, 'info', '--format', '{{.ID}}']) == F.outer
                  and self.call('info', '--format', '{{.ID}}') == self.daemon,
                  'fixture Docker identity changed')
        raw = json.loads(s.run([*s.DOCKER, 'inspect', self.cid]))[0]
        s.require(raw['Config']['Labels'].get(LABEL) == s.OWNER, 'fixture owner changed')

    def inventory(self):
        self.verify()
        return {kind: set(self.call(*args).splitlines()) for kind, args in [
            ('containers', ['ps', '-aq', '--no-trunc']),
            ('images', ['image', 'ls', '--all', '--no-trunc', '--format', '{{.ID}}']),
            ('networks', ['network', 'ls', '-q', '--no-trunc']),
            ('volumes', ['volume', 'ls', '--format', '{{.Name}}'])]}

    def remove_container(self, cid):
        if cid not in self.inventory()['containers']: return
        raw = json.loads(self.call('inspect', cid))[0]
        s.require(raw['Id'] == cid and raw['Config']['Labels'].get(LABEL) == s.OWNER,
                  'refusing foreign container cleanup')
        self.call('rm', '--force', cid)


class Proxy:
    """Same-identity, bounded observer; records operation names, never payloads."""
    def __init__(self, role):
        self.role = F.roles[role]
        self.events, self.accepted, self.failures = [], [], []
        self.lock = threading.Lock()
        self.lose = None
        outer = self
        class Server(http.server.ThreadingHTTPServer):
            daemon_threads = True
            def handle_error(self, *_): pass
        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'
            def log_message(self, *_): pass
            def do_GET(self): self.forward()
            def do_POST(self): self.forward()
            def reply(self, status, body):
                self.send_response(status)
                self.send_header('Content-Type', 'application/json')
                self.send_header('Content-Length', str(len(body)))
                self.send_header('Connection', 'close')
                self.end_headers()
                self.wfile.write(body)
            def forward(self):
                connection = timer = transport = None
                try:
                    s.require(self.path in ['/user', '/read', '/write', '/execute', '/terminal/execute']
                              and (self.command == 'GET') == (self.path == '/user'), 'unexpected audit route')
                    credentials = outer.role['credentials']
                    s.require(self.headers.get('x-api-key') == credentials['key']
                              and self.headers.get('x-api-secret') == credentials['secret']
                              and not self.headers.get('authorization'), 'audit identity mismatch')
                    data = None
                    if self.command == 'POST':
                        s.require(not self.headers.get('transfer-encoding'), 'unsupported audit framing')
                        length = int(self.headers.get('content-length', '-1'))
                        s.require(0 < length <= 65536, 'audit request budget')
                        self.connection.settimeout(10)
                        data = self.rfile.read(length)
                        s.require(len(data) == length, 'incomplete audit request')
                        decoded = json.loads(data)
                        op = decoded.get('type', 'TerminalCommand')
                    else: op = 'CurrentCaller'
                    event = {'route': self.path, 'operation': op}
                    if self.path == '/terminal/execute': event['terminal'] = decoded['terminal']
                    with outer.lock: outer.events.append(event)
                    connection = http.client.HTTPConnection('127.0.0.1', s.Api(F.address).port, timeout=45)
                    connection.connect()
                    transport = connection.sock
                    def abort():
                        try: transport.shutdown(socket.SHUT_RDWR)
                        except OSError: pass
                    timer = threading.Timer(45, abort)
                    timer.daemon = True
                    timer.start()
                    connection.request(self.command, self.path, body=data,
                        headers={'Content-Type': 'application/json', 'x-api-key': credentials['key'],
                                 'x-api-secret': credentials['secret']})
                    response = connection.getresponse()
                    budget = 65536 if self.path == '/terminal/execute' else s.MAX_BODY
                    if self.path == '/terminal/execute':
                        self.send_response(response.status)
                        self.send_header('Connection', 'close')
                        self.end_headers()
                        size = 0
                        while True:
                            block = response.read1(min(4096, budget + 1 - size))
                            if not block: break
                            size += len(block)
                            s.require(size <= budget, 'terminal response budget')
                            self.wfile.write(block)
                            self.wfile.flush()
                        self.close_connection = True
                    else:
                        body = response.read(budget + 1)
                        s.require(len(body) <= budget, 'audit Core response budget')
                        if self.path in ['/write', '/execute'] and 200 <= response.status < 300:
                            value = json.loads(body)
                            record = {'route': self.path, 'operation': op}
                            try: record['id'] = s.identity(value)
                            except s.Failure: pass
                            with outer.lock: outer.accepted.append(record)
                            if outer.lose == op:
                                outer.lose = None
                                self.reply(502, b'{"fixture":"accepted acknowledgement deliberately lost"}')
                                return
                        self.reply(response.status, body)
                except (BrokenPipeError, ConnectionResetError): pass
                except Exception:
                    with outer.lock: outer.failures.append('bounded audit forwarding failed')
                    try: self.reply(502, b'{"fixture":"audit forwarding failed"}')
                    except OSError: pass
                finally:
                    if timer: timer.cancel()
                    if connection: connection.close()
        self.http = Server(('127.0.0.1', 0), Handler)
        self.address = 'http://127.0.0.1:' + str(self.http.server_address[1])
        self.thread = threading.Thread(target=self.http.serve_forever, daemon=True)

    async def __aenter__(self):
        self.thread.start()
        return self

    async def __aexit__(self, *_):
        await asyncio.to_thread(self.http.shutdown)
        self.http.server_close()
        self.thread.join(timeout=5)
        s.require(not self.thread.is_alive() and not self.failures, 'audit proxy did not finish cleanly')

    def mutations(self):
        with self.lock:
            return [e.copy() for e in self.events if e['route'] in ['/write', '/execute', '/terminal/execute']]

    def commands(self):
        return [e for e in self.mutations() if e['route'] == '/terminal/execute']

    def summary(self):
        with self.lock:
            return {'counts': dict(collections.Counter(e['route'] + ':' + e['operation'] for e in self.events)),
                    'accepted': self.accepted.copy(), 'forwarding_failures': self.failures.copy()}


class Mcp(s.Mcp):
    async def tool(self, name, arguments):
        return checked(await super().tool(name, arguments))

    async def close(self):
        if self.process.stdin and not self.process.stdin.is_closing(): self.process.stdin.close()
        try: await asyncio.wait_for(self.process.wait(), 30)
        except asyncio.TimeoutError:
            self.process.kill()
            await asyncio.wait_for(self.process.wait(), 5)
            raise s.Failure('MCP exceeded graceful terminal cleanup deadline') from None
        await asyncio.wait_for(self.drain, 5)
        text = self.stderr.decode(errors='replace')
        s.require(not any(secret and secret in text for secret in s.SECRETS), 'private value in MCP audit')
        s.require('v07-terminal-output' not in text and 'printf %s' not in text, 'raw terminal input/output in audit')
        s.require(self.process.returncode == 0, 'MCP did not shut down cleanly')

    async def crash(self):
        self.process.kill()
        await asyncio.wait_for(self.process.wait(), 5)
        await asyncio.wait_for(self.drain, 5)
        s.require(self.process.returncode != 0, 'crash fixture was graceful')
        checked(self.stderr.decode(errors='replace'))


class Audited:
    def __init__(self, mcp, proxy): self.mcp, self.proxy = mcp, proxy
    async def tool(self, name, arguments):
        before = len(self.proxy.mutations())
        try: return await self.mcp.tool(name, arguments)
        finally:
            events = self.proxy.mutations()[before:]
            record = {'tool': name, 'mutations': events}
            F.report.setdefault('per_call_audit', []).append(record)
            s.persist()
            readonly = name.startswith(('get_', 'list_', 'preview_', 'wait_', 'diagnose_', 'reconcile_'))
            if readonly: s.require(not events, 'observation submitted an implicit mutation')
            elif name == 'execute_terminal':
                s.require(sum(e['route'] == '/terminal/execute' for e in events) <= 1,
                          'terminal command replayed')
                s.require(all(e['route'] == '/terminal/execute' or e['operation'] in
                              ['CreateTerminal', 'DeleteTerminal'] for e in events),
                          'terminal tool submitted unrelated mutation')
            else: s.require(len(events) <= 1, 'explicit operation submitted multiple mutations')


async def raw(mcp, name, args):
    return checked(await mcp.rpc('tools/call', {'name': name, 'arguments': args}, allow_error=True))


async def poll_async(callback, description, seconds=90):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        if callback(): return
        await asyncio.sleep(.25)
    raise s.Failure('bounded independent observation: ' + description)


class Fixture:
    def __init__(self, context):
        global F
        F = self
        self.context, self.admin, self.address = context, context['provisioner'], context['address']
        self.sid, self.roles, self.owned, self.foreign = context['server_id'], {}, [], None
        self.prefix = 'mcp-v07-' + s.OWNER[:10] + '-'
        self.report = s.REPORT['swarm_admin'] = {'passed': False, 'provisioner_used_by_mcp': False,
            'per_call_audit': [], 'evidence_limits': [
                'Core has no atomic compare-and-swap, rollback or cross-client exactly-once guarantee.',
                'Core updates and upstream-reported terminal exit are distinct from workload health.',
                'Lost acknowledgements are injected after observed acceptance; no automatic replay.',
                'Terminal observer deletion does not prove descendant termination; Core may log commands.',
                'Malformed streams and private-state expiry remain deterministic compiled-stdio coverage.']}
        self.outer = s.run([*s.DOCKER, 'info', '--format', '{{.ID}}'])
        self.manager, self.worker = Engine('docker'), Engine('docker-worker')
        s.require(self.manager.daemon != self.worker.daemon, 'nested engines are not distinct')
        self.report['engines'] = {e.service: {'daemon_id': e.daemon, 'version': e.version}
                                  for e in [self.manager, self.worker]}
        self.private = s.ROOT / 'swarm-admin-identities.json'
        self.receiver_nonce = json.loads((s.ROOT / 'compose.json').read_text())['services']['receiver']['environment']['RECEIVER_NONCE']
        self.setup_swarm()
        self.setup_roles()

    def private_save(self):
        fd = os.open(self.private, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, 'w') as output: json.dump(self.roles, output)

    def api(self, role):
        keys = self.roles[role]['credentials']
        return s.Api(self.address, {'x-api-key': keys['key'], 'x-api-secret': keys['secret']})

    def grant(self, role, kind, rid, level, specifics=()):
        self.admin.write('UpdatePermissionOnTarget', {'user_target': {'type': 'User', 'id': self.roles[role]['id']},
            'resource_target': {'type': kind, 'id': rid}, 'permission': {'level': level, 'specific': list(specifics)}})

    def super_flag(self, row, enabled):
        cid = s.run([*s.COMPOSE, 'ps', '-q', 'mongo'])
        record = json.loads(s.run([*s.DOCKER, 'inspect', cid]))[0]
        s.require(record['Config']['Labels'].get(LABEL) == s.OWNER, 'Mongo fixture owner mismatch')
        # Only this fixture's exact service identity; no database product API.
        code = "const a=db.getSiblingDB('admin');if(!a.auth(process.env.MONGO_INITDB_ROOT_USERNAME,process.env.MONGO_INITDB_ROOT_PASSWORD))throw Error('auth');const d=db.getSiblingDB('komodo');const q={_id:ObjectId(" + json.dumps(row['id']) + "),username:" + json.dumps(row['username']) + ",'config.type':'Service'};const ns=d.getCollectionNames();if(ns.length>64)throw Error('bound');const cs=ns.filter(n=>d.getCollection(n).countDocuments(q)===1);if(cs.length!==1)throw Error('identity');const r=d.getCollection(cs[0]).updateOne({...q,admin:true,super_admin:" + str(not enabled).lower() + "},{$set:{super_admin:" + str(enabled).lower() + "}});if(r.matchedCount!==1||r.modifiedCount!==1)throw Error('seed');"
        s.run([*s.DOCKER, 'exec', cid, 'mongosh', '--quiet', '--norc', '--eval', code])

    def setup_roles(self):
        for role in ['ordinary-admin', 'super-admin', 'reader', 'pruner', 'terminal', 'affected']:
            name = self.prefix + role
            user = self.admin.write('CreateServiceUser', {'username': name, 'description': s.OWNER + ' isolated fixture'})
            row = self.roles[role] = {'id': s.identity(user), 'username': name}
            self.private_save()
            self.admin.write('UpdateUserBasePermissions', {'user_id': row['id'], 'enabled': True,
                'create_servers': False, 'create_builds': False})
            level = 'Execute' if role == 'pruner' else 'Read'
            specifics = ['Inspect', 'Logs', 'Terminal'] if role == 'terminal' else ['Inspect', 'Logs'] if role == 'reader' else []
            self.grant(role, 'Server', self.sid, level, specifics)
            if role in ['ordinary-admin', 'super-admin']:
                self.admin.write('UpdateUserAdmin', {'user_id': row['id'], 'admin': True})
            keys = self.admin.write('CreateApiKeyForServiceUser', {'user_id': row['id'],
                'name': self.prefix + role, 'expires': int(time.time()*1000) + 12*3600000})
            s.SECRETS.extend([keys['key'], keys['secret']])
            row['credentials'] = keys
            if role == 'super-admin': self.super_flag(row, True)
            self.private_save()
            current = self.api(role).request('/user')
            s.require(s.identity(current) == row['id'] and current['enabled']
                and current['admin'] == (role in ['ordinary-admin', 'super-admin'])
                and current['super_admin'] == (role == 'super-admin'), 'isolated actor authority mismatch')
        self.report['actors'] = [{'role': role, 'id': row['id']} for role, row in self.roles.items()]

    def setup_swarm(self):
        for engine in [self.manager, self.worker]:
            s.require(engine.call('info', '--format', '{{.Swarm.LocalNodeState}}') == 'inactive',
                      'refusing preexisting Swarm adoption')
        raw = json.loads(s.run([*s.DOCKER, 'inspect', self.manager.cid]))[0]
        ip = raw['NetworkSettings']['Networks'][s.PROJECT + '_default']['IPAddress']
        self.manager.call('swarm', 'init', '--advertise-addr', ip,
            '--default-addr-pool', '10.71.0.0/16', '--default-addr-pool-mask-length', '24')
        token = self.manager.call('swarm', 'join-token', '-q', 'worker')
        s.SECRETS.append(token)
        self.worker.call('swarm', 'join', '--token', token, ip + ':2377')
        def ready():
            rows = [json.loads(x) for x in self.manager.call('node', 'ls', '--format', '{{json .}}').splitlines()]
            return rows if len(rows) == 2 and all(r['Status'] == 'Ready' and r['Availability'] == 'Active' for r in rows) else None
        self.report['nodes'] = s.poll(ready, 'real manager/worker Ready')
        # Copy the exact cached fixture image into both owned engines. Pulling
        # registry content is provisioning, never an MCP terminal fallback.
        for engine in [self.manager, self.worker]: engine.call('pull', BUSYBOX, timeout=180)

    def receipts(self):
        raw = s.run([*s.COMPOSE, 'exec', '-T', 'receiver', 'python3', '-c',
            "from pathlib import Path; p=Path('/state/receipts.jsonl'); print(p.read_text() if p.exists() else '')"])
        rows = [json.loads(line) for line in raw.splitlines()]
        s.require(len(rows) <= 64 and all(row['complete'] for row in rows), 'incomplete/unbounded receiver receipt')
        return checked(rows)

    def remember(self, kind, rid, name):
        s.require(name.startswith(self.prefix), 'resource outside owned fixture namespace')
        self.owned.append((kind, rid, name))
        self.report.setdefault('resources', []).append({'kind': kind, 'id': rid, 'name': name})
        s.persist()

    def cleanup_identities(self):
        rows = self.admin.read('ListUsers')
        s.require(len(rows) <= 128, 'identity cleanup directory incomplete')
        targets = [r for r in rows if r['username'].startswith(('mcp-smoke-', 'mcp-automation-', 'mcp-sync-', self.prefix))]
        removed = []
        designated = self.roles.get('super-admin')
        for row in targets:
            uid = s.identity(row)
            if row['super_admin']:
                s.require(designated and uid == designated['id'], 'refusing unrecognized super-admin cleanup')
                self.super_flag(designated, False)
            keys = self.admin.read('ListApiKeysForServiceUser', {'user': uid})
            s.require(len(keys) <= 32, 'owned credential cleanup incomplete')
            for key in keys: self.admin.write('DeleteApiKeyForServiceUser', {'key': key['key']})
            s.require(self.admin.read('ListApiKeysForServiceUser', {'user': uid}) == [], 'owned credentials remain')
            self.admin.write('DeleteUser', {'user': uid})
            removed.append({'id': uid, 'api_keys_removed': len(keys)})
        after = self.admin.read('ListUsers')
        s.require(not any(s.identity(row) in {r['id'] for r in removed} for row in after), 'owned user remains')
        self.report['identity_cleanup'] = {'removed': removed, 'verified_before_database_teardown': True,
            'provisioner_removed_with_owned_database': True,
            'orphan_permission_records_removed_with_owned_database': True}
        self.private.unlink(missing_ok=True)
