"""Real manager/worker placement, saved scale, execution and independent health."""
import asyncio
import json
import time

import disposable_core_smoke as s
import swarm_admin_fixture as f
from swarm_admin_pruning import complete


async def exercise():
    fixture = f.F
    name = fixture.prefix + 'swarm'
    swarm = fixture.admin.write('CreateSwarm', {'name': name,
        'config': {'server_ids': [fixture.sid], 'send_unhealthy_alerts': False}})
    swarm_id = s.identity(swarm)
    fixture.remember('Swarm', swarm_id, name)
    fixture.grant('reader', 'Swarm', swarm_id, 'Read', ['Inspect', 'Logs'])
    deployment_name = fixture.prefix + 'scaled'
    network_name = fixture.prefix + 'overlay'
    network_id = fixture.manager.call('network', 'create', '--driver', 'overlay',
        '--label', f.LABEL + '=' + s.OWNER, network_name)
    constraint = '--constraint=node.labels.mcp_absent==true'
    extra = [constraint, '--label=' + f.LABEL + '=' + s.OWNER,
             '--detach=true', '--no-resolve-image', '--replicas=1']
    deployment = fixture.admin.write('CreateDeployment', {'name': deployment_name, 'config': {
        'swarm_id': swarm_id, 'replicas': 1, 'network': network_name,
        'image': {'type': 'Image', 'params': {'image': f.BUSYBOX}},
        'command': "sh -c 'mkdir -p /tmp/www; echo v07-owned-health > /tmp/www/index.html; exec httpd -f -p 8080 -h /tmp/www'",
        'extra_args': extra, 'send_alerts': False, 'auto_update': False, 'poll_for_updates': False}})
    did = s.identity(deployment)
    fixture.remember('Deployment', did, deployment_name)
    report = fixture.report['swarm'] = {'swarm_id': swarm_id, 'deployment_id': did,
        'scales': [], 'independent_health': []}
    def exact_service():
        ids = fixture.manager.call('service', 'ls', '-q', '--filter', 'name=' + deployment_name).splitlines()
        matches = [json.loads(fixture.manager.call('service', 'inspect', rid))[0] for rid in ids]
        matches = [r for r in matches if r['Spec']['Name'] == deployment_name]
        return matches[0] if len(matches) == 1 else None
    async with f.Proxy('ordinary-admin') as proxy:
        async with await f.Mcp.start(proxy.address, fixture.roles['ordinary-admin']['credentials'],
                                    configuration_edit=True) as process:
            mcp = f.Audited(process, proxy)
            result = await mcp.tool('deploy_deployment', {'id': did})
            await complete(mcp, result['update_id'])
            report['initial_deploy_update_id'] = result['update_id']
            service = s.poll(exact_service, 'managed service creation')
            service_id = service['ID']
            target = {'target': {'id': swarm_id}, 'object': {'id': service_id}}
            diagnosis = None
            for _ in range(30):
                try:
                    diagnosis = await mcp.tool('diagnose_swarm_service', target)
                    if diagnosis['data']['cached_pending'] > 0: break
                except s.Failure: pass
                await asyncio.sleep(1)
            s.require(diagnosis and diagnosis['data']['under_replicated_in_cache'] is True
                      and diagnosis['data']['cached_pending'] > 0, 'pending placement not diagnosed')
            report['pending_placement'] = diagnosis
            args = {'target': {'id': did}, 'replicas': 1, 'timeout_seconds': 2}
            observed = await mcp.tool('observe_deployment_scale', args)
            s.require(not observed.get('latest', {}).get('matching_replica_observations'),
                      'pending tasks falsely established convergence')
            # Fixture-only constraint correction; the scale tool must preserve it.
            extra[0] = '--constraint=node.role==worker'
            fixture.admin.write('UpdateDeployment', {'id': did, 'config': {'extra_args': extra}})
            for replicas in [3, 1, 0]:
                before = fixture.admin.read('GetDeployment', {'deployment': did})['config']
                args = {'target': {'id': did}, 'replicas': replicas}
                preview = await mcp.tool('preview_deployment_scale', args)
                saved = await mcp.tool('apply_deployment_scale', dict(args, preview_id=preview['preview_id']))
                s.require(saved['configuration']['saved_configuration_verified'] and not saved['deployment_submitted'],
                          'scale save conflated with execution')
                after = fixture.admin.read('GetDeployment', {'deployment': did})['config']
                s.require({k:v for k,v in before.items() if k not in ['replicas', 'extra_args']}
                          == {k:v for k,v in after.items() if k not in ['replicas', 'extra_args']}
                          and after['extra_args'][:-1] == extra[:-1], 'scale changed unrelated configuration')
                # Configuration-only process can save, then fails locally to deploy.
                async with await f.Mcp.start(proxy.address, fixture.roles['ordinary-admin']['credentials'],
                                            operate=False, configuration_edit=True) as save_only:
                    denied = await f.raw(save_only, 'deploy_scaled_deployment', args)
                    s.require('protocol_error' in denied, 'configuration-only profile enabled deployment')
                reviewed = await mcp.tool('review_deployment_scale_deploy', args)
                deployed = await mcp.tool('deploy_scaled_deployment', dict(args, preview_id=reviewed['preview_id']))
                receipt = deployed['receipt']
                s.require(receipt and receipt['submission'] == 'accepted', 'scale execution not accepted')
                await complete(mcp, receipt['update_id'])
                latest = None
                for _ in range(30):
                    latest = await mcp.tool('observe_deployment_scale', dict(args, timeout_seconds=2))
                    if latest.get('latest') and latest['latest']['matching_replica_observations']: break
                    await asyncio.sleep(1)
                s.require(latest and latest.get('latest') and latest['latest']['matching_replica_observations'],
                          'real current replicas did not converge')
                fresh = exact_service()
                s.require(fresh and fresh['ID'] != service_id, 'pinned deployment did not recreate service')
                service_id = fresh['ID']
                running = fixture.worker.call('ps', '-q', '--no-trunc', '--filter',
                    'label=com.docker.swarm.service.id=' + service_id).splitlines()
                s.require(len(running) == replicas, 'independent worker running-task count mismatch')
                if replicas:
                    response = fixture.worker.call('exec', running[0], 'wget', '-qO-', 'http://127.0.0.1:8080')
                    s.require(response == 'v07-owned-health', 'independent worker HTTP health failed')
                    await mcp.tool('get_swarm_service', {'target': {'id': swarm_id}, 'object': {'id': service_id}})
                    await mcp.tool('get_swarm_service_logs', {'target': {'id': swarm_id},
                        'service': {'id': service_id}, 'tail': 10, 'max_bytes': 1024})
                    report['exact_service_logs_read_without_mutation'] = True
                    report['independent_health'].append({'replicas': replicas, 'healthy': True,
                        'worker_container_id': running[0], 'health_body_matched': True})
                else:
                    s.require(fresh['Spec']['Mode']['Replicated']['Replicas'] == 0,
                              'zero inferred from empty historical cache')
                report['scales'].append({'requested': replicas, 'save_verified': True,
                    'update_id': receipt['update_id'], 'execution_succeeded': True, 'service_id': service_id,
                    'convergence': latest, 'independent_worker_running_count': len(running)})
                s.persist()
            report['proxy'] = proxy.summary()
    # Independent exact service removal, including residual historical tasks,
    # precedes the causal prune fixtures. No node-administration product tool.
    service = exact_service()
    if service:
        s.require(service['Spec']['Labels'].get(f.LABEL) == s.OWNER, 'Swarm service owner mismatch')
        fixture.manager.call('service', 'rm', service['ID'])
    for engine in [fixture.manager, fixture.worker]:
        for cid in engine.call('ps', '-aq', '--no-trunc', '--filter', 'label=' + f.LABEL + '=' + s.OWNER).splitlines():
            engine.remove_container(cid)
    fixture.admin.write('DeleteDeployment', {'id': did})
    fixture.admin.write('DeleteSwarm', {'id': swarm_id})
    fixture.manager.call('network', 'rm', network_id)
    s.require(fixture.manager.call('service', 'ls', '-q') == '', 'owned Swarm services remain')
    report.update(passed=True, services_removed_before_pruning=True, historical_tasks_not_counted_as_replicas=True)
    s.persist()
