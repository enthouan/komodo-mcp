"""Actual five-category prune effects on an ownership-checked nested engine."""
import asyncio
import io
import json
import secrets
import tarfile
import time

import disposable_core_smoke as s
import swarm_admin_fixture as f

OPS = {'image': 'PruneImages', 'container': 'PruneContainers', 'network': 'PruneNetworks',
       'volume': 'PruneVolumes', 'system': 'PruneSystem'}


class PruneFixtures:
    def __init__(self, engine): self.e = engine
    def inventory(self): return self.e.inventory()
    def cache_count(self):
        rows = [json.loads(x) for x in self.e.call('system', 'df', '--format', '{{json .}}').splitlines()]
        rows = [r for r in rows if r['Type'] == 'Build Cache']
        s.require(len(rows) == 1, 'missing default-engine cache accounting')
        return int(rows[0]['TotalCount'])
    def seed(self, category):
        e = self.e
        prefix = 'mcp-owned-prune-' + secrets.token_hex(6)
        row = {'category': category, 'containers': [], 'volumes': [], 'networks': [], 'images': []}
        f.F.report.setdefault('prune_fixtures', []).append(row)
        row['used_network'] = e.call('network', 'create', '--label', f.LABEL + '=' + s.OWNER, prefix + '-used')
        row['networks'].append(row['used_network'])
        row['used_volume'] = e.call('volume', 'create', '--label', f.LABEL + '=' + s.OWNER, prefix + '-used')
        row['volumes'].append(row['used_volume'])
        holder = e.call('run', '--pull=never', '-d', '--name', prefix + '-holder', '--label', f.LABEL + '=' + s.OWNER,
            '--network', prefix + '-used', '-v', row['used_volume'] + ':/used-named', '-v', '/used-anonymous', f.BUSYBOX, 'sleep', '900')
        row['holder'] = holder
        row['containers'].append(holder)
        mounts = json.loads(e.call('inspect', '--format', '{{json .Mounts}}', holder))
        row['used_anonymous'] = next(v['Name'] for v in mounts if v['Destination'] == '/used-anonymous')
        row['volumes'].append(row['used_anonymous'])
        row['used_image'] = e.call('inspect', '--format', '{{.Image}}', holder)
        if category == 'image':
            row['unused_image'] = e.call('commit', '--change', 'LABEL komodo-mcp.prune-case=' + prefix,
                                        holder, prefix + ':unused')
            row['images'].append(row['unused_image'])
        if category in ['container', 'system']:
            cid = e.call('run', '--pull=never', '-d', '--label', f.LABEL + '=' + s.OWNER, f.BUSYBOX, 'true')
            row['stopped_container'] = cid
            row['containers'].append(cid)
            s.poll(lambda: e.call('inspect', '--format', '{{.State.Running}}', cid) == 'false', 'stopped prune fixture')
        if category in ['network', 'system']:
            row['unused_network'] = e.call('network', 'create', '--label', f.LABEL + '=' + s.OWNER, prefix + '-unused')
            row['networks'].append(row['unused_network'])
        if category in ['volume', 'system']:
            row['unused_named'] = e.call('volume', 'create', '--label', f.LABEL + '=' + s.OWNER, prefix + '-unused')
            row['volumes'].append(row['unused_named'])
            cid = e.call('run', '--pull=never', '-d', '--label', f.LABEL + '=' + s.OWNER,
                         '-v', '/unused-anonymous', f.BUSYBOX, 'true')
            row['containers'].append(cid)
            mounts = json.loads(e.call('inspect', '--format', '{{json .Mounts}}', cid))
            row['unused_anonymous'] = next(v['Name'] for v in mounts if v['Destination'] == '/unused-anonymous')
            row['volumes'].append(row['unused_anonymous'])
            e.remove_container(cid)
        if category == 'system':
            context = io.BytesIO()
            with tarfile.open(fileobj=context, mode='w') as tar:
                for name, data in [('Dockerfile', b'FROM scratch\nCOPY payload /owned-proof\n'), ('payload', prefix.encode())]:
                    info = tarfile.TarInfo(name)
                    info.size = len(data)
                    tar.addfile(info, io.BytesIO(data))
            e.call('build', '--network=none', '--progress=plain', '--label', f.LABEL + '=' + s.OWNER,
                   '--tag', prefix + ':unused', '-', input_data=context.getvalue(), timeout=180)
            row['unused_image'] = e.call('image', 'inspect', '--format', '{{.Id}}', prefix + ':unused')
            row['images'].append(row['unused_image'])
            row['cache_before'] = self.cache_count()
            s.require(row['cache_before'] > 0, 'no unused default-engine cache fixture')
        s.persist()
        return row
    def verify_effect(self, row):
        inv = self.inventory()
        s.require(row['holder'] in inv['containers'] and row['used_image'] in inv['images']
            and row['used_network'] in inv['networks'] and row['used_volume'] in inv['volumes']
            and row['used_anonymous'] in inv['volumes'], 'prune removed an in-use fixture')
        s.require(self.e.call('inspect', '--format', '{{.State.Running}}', row['holder']) == 'true', 'holder stopped')
        category = row['category']
        for kinds, key, bucket in [(['image', 'system'], 'unused_image', 'images'),
            (['container', 'system'], 'stopped_container', 'containers'),
            (['network', 'system'], 'unused_network', 'networks'),
            (['volume', 'system'], 'unused_anonymous', 'volumes'), (['volume'], 'unused_named', 'volumes')]:
            if category in kinds: s.require(row[key] not in inv[bucket], 'unused category fixture survived')
        if category == 'system':
            s.require(row['unused_named'] in inv['volumes'], 'Docker 27 system named-volume behavior changed')
            row['cache_after'] = self.cache_count()
            s.require(row['cache_after'] == 0, 'unused engine cache survived system prune')
        row['actual_removals_verified'] = row['all_in_use_fixtures_preserved'] = True
    def cleanup(self, row):
        e = self.e
        for cid in row['containers']: e.remove_container(cid)
        for kind, bucket in [('network', 'networks'), ('volume', 'volumes'), ('image', 'images')]:
            for rid in row[bucket]:
                if rid in self.inventory()[bucket]: e.call(kind, 'rm', rid)
        inv = self.inventory()
        s.require(all(not set(row[k]) & inv[k] for k in ['containers', 'images', 'networks', 'volumes']),
                  'owned prune fixture cleanup incomplete')
        row['cleanup_verified'] = True
        s.persist()


async def complete(mcp, uid):
    value = await s.observe(mcp, uid)
    s.require(value['outcome'] == 'succeeded', 'live Core execution did not succeed')
    return value


async def exercise():
    fixture = f.F
    seeds = PruneFixtures(fixture.manager)
    fixture.report['pruning'] = []
    for category in [*OPS, 'image']:
        lost = len(fixture.report['pruning']) == 5
        row = seeds.seed(category)
        try:
            await f.poll_async(lambda: any(c.get('id') == row['holder'] for c in
                fixture.admin.read('ListContainers', {'server': fixture.sid})), 'Core holder identity cache')
            async with f.Proxy('pruner') as proxy:
                async with await f.Mcp.start(proxy.address, fixture.roles['pruner']['credentials'],
                                             prune=True) as process:
                    mcp = f.Audited(process, proxy)
                    for name in ['list_images', 'list_networks', 'list_volumes']:
                        await mcp.tool(name, {'target': {'id': fixture.sid}})
                    args = {'target': {'id': fixture.sid}, 'category': category}
                    preview = await mcp.tool('preview_docker_prune', args)
                    if lost: proxy.lose = OPS[category]
                    result = await mcp.tool('apply_docker_prune', dict(args,
                        preview_id=preview['preview_id'], acknowledge_destructive_effects=True))
                    accepted = [v for v in proxy.accepted if v['operation'] == OPS[category]]
                    s.require(len(accepted) == 1, 'one explicit prune did not produce one accepted update')
                    uid = accepted[0]['id']
                    if not lost:
                        s.require(result['receipt']['update_id'] == uid, 'known prune receipt ID lost')
                        await complete(mcp, uid)
                if lost:
                    async with await f.Mcp.start(proxy.address, fixture.roles['pruner']['credentials'],
                                                 operate=False) as restarted:
                        await complete(f.Audited(restarted, proxy), uid)
                        reconciliation = await f.Audited(restarted, proxy).tool('reconcile_operation', {
                            'target': {'kind': 'server', 'id': fixture.sid}, 'operation': 'prune',
                            'requested_scope': {'kind': 'docker_prune', 'category': category}})
                        s.require(reconciliation['candidates'], 'lost-ack readonly recovery omitted candidate')
                    s.require(result['receipt']['submission'] == 'unknown'
                              and result['receipt']['update_id'] is None,
                              'lost prune acknowledgement claimed certainty')
                seeds.verify_effect(row)
                mutations = proxy.mutations()
                s.require(len(mutations) == 1 and mutations[0]['operation'] == OPS[category], 'prune replay or hidden write')
                fixture.report['pruning'].append({'category': category, 'update_id': uid,
                    'lost_ack_injected_after_acceptance': lost, 'proxy': proxy.summary(),
                    'removals_verified': True, 'in_use_preserved': True})
        finally: seeds.cleanup(row)
