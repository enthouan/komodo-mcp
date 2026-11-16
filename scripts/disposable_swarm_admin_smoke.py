#!/usr/bin/env python3
"""Opt-in integrated production-stdio acceptance on owned manager/worker Docker.

Run from a clean, committed checkout. --require-main additionally requires the
explicit commit to equal freshly fetched origin/main. Only the all suite can be
final acceptance; development feature-only runs say so in their evidence.
"""
import argparse
import asyncio
import hashlib
import json
from pathlib import Path
import re
import sys

import disposable_core_smoke as s
import disposable_resource_sync_smoke as retained
import swarm_admin_administration as administration
import swarm_admin_fixture as fixture
import swarm_admin_pruning as pruning
import swarm_admin_swarm as swarm
import swarm_admin_terminal as terminal


EXPECTED = ''
REQUIRE_MAIN = False
SUITE = 'all'


def validate_source(expected, require_main, suite):
    s.require(re.fullmatch('[0-9a-f]{40}', expected), 'supply the full explicit source SHA')
    s.require(suite in ['all', 'features'], 'unknown acceptance suite')
    s.require(not require_main or suite == 'all', 'final main acceptance must retain every previous workflow')
    s.require(s.run(['git', 'rev-parse', 'HEAD']) == expected and not s.run(['git', 'status', '--porcelain']),
              'acceptance needs a clean checkout at the explicit source SHA')
    if require_main:
        s.require(s.run(['git', 'rev-parse', 'origin/main']) == expected,
                  'fetch origin/main and use its exact merged commit for final acceptance')
        actual = s.run(['git', 'ls-remote', '--exit-code', 'origin', 'refs/heads/main'], timeout=45)
        s.require(actual == expected + '\trefs/heads/main',
                  'remote main changed; fetch and validate the actual integrated commit')


def extension(context):
    validate_source(EXPECTED, REQUIRE_MAIN, SUITE)
    s.require(s.REPORT['built_commit'] == EXPECTED and
              hashlib.sha256(s.BINARY.read_bytes()).hexdigest() == s.REPORT['executable_sha256'],
              'production executable/source changed after build')
    owned = fixture.Fixture(context)
    owned.report.update(source_commit=EXPECTED, executable_sha256=s.REPORT['executable_sha256'],
        mcp_version=s.REPORT['mcp_version'] if 'mcp_version' in s.REPORT else None,
        final_merged_main_acceptance=REQUIRE_MAIN, suite=SUITE,
        deterministic_faults=['lost accepted write/prune acknowledgement is replaced with fixture HTTP 502'],
        actual_live=['manager/worker placement and scaling', 'all five Docker prune categories',
            'Core admin/group permission engine', 'variable writes and sanitized history/export',
            'local HTTP Alerter complete receipt and failure', 'PTY command and remote shell TTL'])
    s.persist()
    for label, callback in [('capabilities', terminal.profiles), ('swarm', swarm.exercise),
                            ('pruning', pruning.exercise), ('variables', administration.variables),
                            ('access', administration.access), ('alerters', administration.alerters),
                            ('terminal', terminal.exercise)]:
        owned.report['phase'] = label
        s.persist()
        print('Exercising owned ' + label + '.', flush=True)
        asyncio.run(callback())
    owned.report['passed'] = True
    owned.report['phase'] = 'feature_checks_complete'
    owned.report['mcp_version'] = s.REPORT['mcp_version']
    s.require(terminal.sessions(owned.admin, {'type': 'Server',
        'params': {'server': owned.sid}}) == [], 'Server terminals remain before retained workflows')
    owned.report['remote_terminals_absent_before_legacy'] = True
    s.persist()
    if SUITE == 'all':
        # This invokes the complete automation AND ResourceSync extensions;
        # base exercise_environment subsequently retains its entire regression.
        retained.provision(context)
        s.require(s.REPORT['automation']['final_health']['docker_health'] == 'healthy'
                  and s.REPORT['automation']['mutation_audit_summary']['read_only_mutation_attempts'] == 0,
                  'retained automation acceptance incomplete')
        owned.report['retained_automation_verified'] = True


def before_cleanup():
    failures = []
    if fixture.CACHE:
        try: fixture.CACHE.observe()
        except BaseException as error: failures.append(error)
    if retained.CONTEXT or retained.a.CONTEXT:
        try: retained.disable_automatic_execution()
        except BaseException as error: failures.append(error)
    owned = fixture.F
    if owned:
        try:
            # Every Alerter and automatic Server policy belongs to this private
            # Core. Disable before removing credentials or tearing down storage.
            for raw in owned.admin.read('ListAlerters'):
                aid = s.identity(raw)
                current = owned.admin.read('GetAlerter', {'alerter': aid})
                retained_ids = {row['id'] for row in s.REPORT.get('server_deletion', {}).get('references', [])
                                if row['kind'] == 'alerter'}
                s.require(current['name'].startswith(owned.prefix) or aid in retained_ids,
                          'unrecognized Alerter during cleanup')
                owned.admin.write('UpdateAlerter', {'id': aid, 'config': {'enabled': False}})
                s.require(not owned.admin.read('GetAlerter', {'alerter': aid})['config']['enabled'], 'Alerter disable failed')
            policies = []
            for row in owned.admin.read('ListServers'):
                server_id, name = s.identity(row), row['name']
                s.require(name.startswith('mcp-'), 'unrecognized Server during owned policy cleanup')
                policies.append(s.disable_server_automatic_effects(owned.admin, server_id, name))
            owned.report['automatic_server_policy_cleanup'] = policies
            owned.cleanup_identities()
            # Observe all live terminal namespaces before removing their database
            # and engines. An interrupted hard-crash shell is left to its bounded
            # TTL, never attached or command-replayed by cleanup.
            servers = owned.admin.read('ListServers')
            if any(s.identity(server) == owned.sid for server in servers):
                remaining = terminal.sessions(owned.admin, {'type': 'Server', 'params': {'server': owned.sid}})
                s.require(remaining == [], 'owned Server terminal namespace remains')
            else:
                s.require(owned.report.get('remote_terminals_absent_before_legacy')
                          and owned.report.get('terminal', {}).get('fixture_cleanup_verified'),
                          'retained Server deletion lacks earlier terminal absence proof')
                owned.report['server_deleted_by_retained_regression'] = True
            owned.report['remote_terminals_absent_before_teardown'] = True
            s.persist()
        except BaseException as error: failures.append(error)
    if failures: raise s.Failure('owned pre-teardown cleanup needs attention (' + type(failures[0]).__name__ + ')')


def independent_cleanup(report):
    """A second CLI process reads each project inventory after Compose teardown."""
    remaining = {kind: s.run([*s.DOCKER, *args, '--filter',
        'label=com.docker.compose.project=' + report['project']]).splitlines()
        for kind, args in [('containers', ['ps', '-aq']), ('networks', ['network', 'ls', '-q']),
                           ('volumes', ['volume', 'ls', '-q'])]}
    s.require(not any(remaining.values()), 'independent project cleanup found resources')
    root = report.get('private_cleanup_directory')
    s.require(not root or not Path(root).exists(), 'private fixture directory remains')
    return {'separate_docker_processes': True, 'remaining': remaining, 'verified': True}


def entry(argv=None):
    global EXPECTED, REQUIRE_MAIN, SUITE
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--expected-commit', required=True)
    parser.add_argument('--require-main', action='store_true')
    parser.add_argument('--suite', choices=['all', 'features'], default='all')
    args, rest = parser.parse_known_args(argv)
    EXPECTED, REQUIRE_MAIN, SUITE = args.expected_commit, args.require_main, args.suite
    validate_source(EXPECTED, REQUIRE_MAIN, SUITE)
    fixture.F = None
    fixture.CACHE = None
    for module in [retained, retained.a]:
        module.CONTEXT.clear()
        module.OWNED.clear()
    retained.a.ACTIVE.clear()
    result = s.entry(rest, fixture_customizer=fixture.customize, exercise_extension=extension,
                     before_cleanup=before_cleanup, retain_legacy=SUITE == 'all')
    # Receiver source and isolated identity ledger are private; base removes
    # the entire owned directory only after verified project teardown.
    try:
        s.REPORT['independent_cleanup'] = independent_cleanup(s.REPORT)
        s.REPORT['outer_image_cache_cleanup'] = (fixture.CACHE.cleanup() if fixture.CACHE else
            {'verified': True, 'action': 'no fixture images created'})
        s.REPORT['final_acceptance'] = bool(result == 0 and REQUIRE_MAIN and SUITE == 'all'
            and s.REPORT['swarm_admin']['passed'] and s.REPORT.get('retained_legacy_workflow')
            and s.REPORT['swarm_admin']['retained_automation_verified'] and s.REPORT['resource_sync']['passed'])
        validate_source(EXPECTED, REQUIRE_MAIN, SUITE)
        s.persist()
    except BaseException:
        s.REPORT['final_acceptance'] = False
        s.persist()
        raise
    return result


if __name__ == '__main__':
    try: raise SystemExit(entry())
    except s.Failure as error:
        print(str(error), file=sys.stderr)
        raise SystemExit(1) from None
