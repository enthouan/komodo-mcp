"""Time complete visible stack inventory against a new, owned Core/Periphery project."""
import argparse
import json
import os
import platform
import tempfile
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
import disposable_core_smoke as smoke
from distribution import sha256
from bench import preflight, read_json, write_json
from live_inventory import run as measure_inventory
from packaged import packaged_entry


def native_target():
    # The measured process runs on the host. Docker Desktop runs only Core/Periphery.
    machine = {"amd64": "x86_64", "arm64": "aarch64"}.get(platform.machine().lower(), platform.machine().lower())
    suffix = {"Darwin": "apple-darwin", "Linux": "unknown-linux-musl"}.get(platform.system())
    smoke.require(machine in ("x86_64", "aarch64") and suffix, "unsupported native benchmark host")
    return f"{machine}-{suffix}"


def exercise(context, args, pending):
    admin = context['provisioner']
    reader_id = next(user['id'] for user in context['users'] if user['username'] == 'mcp-smoke-reader')
    initial = smoke.REPORT['scope']['stack_id']
    expected = {initial: 'mcp-smoke-stack'}
    for index in range(1, args.fixture_size):
        name = f'bench-live-stack-{index:03}'
        item = admin.write('CreateStack', {'name': name, 'config': {
            'server_id': context['server_id'], 'auto_pull': False,
            'file_contents': 'services:\n  app:\n    image: busybox:1.37.0\n'}})
        stack_id = smoke.identity(item)
        expected[stack_id] = name
        admin.write('UpdatePermissionOnTarget', {
            'user_target': {'type': 'User', 'id': reader_id},
            'resource_target': {'type': 'Stack', 'id': stack_id},
            'permission': {'level': 'Read', 'specific': []}})

    def oracle():
        rows = admin.read('ListStacks', {'limit': args.fixture_size + 1})
        if (len(rows) != args.fixture_size or {row['id']: row['name'] for row in rows} != expected
                or any(row['info']['state'].lower() != 'down' for row in rows)):
            return None
        return sorted(({'id': row['id'], 'name': row['name'], 'state': row['info']['state'].lower()}
                       for row in rows), key=lambda item: item['id'])

    rows = smoke.poll(oracle, 'owned inventory visible to the independent provisioner')
    measure_inventory(args.artifacts, context['address'], context['reader'], rows, pending,
                      core_version=smoke.REPORT['core_version'],
                      periphery_version=smoke.REPORT['periphery_version'])
    smoke.require(oracle() == rows, 'owned stack state changed during the benchmark')
    smoke.REPORT['live_inventory'] = {'stack_count': len(rows), 'state': 'down',
        'pre_and_post_oracles_match': True, 'reader': 'enabled non-admin with Read on every selected Stack',
        'candidate_target': args.candidate_target}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--candidate-record', required=True, type=Path)
    parser.add_argument('--candidate-binary', required=True, type=Path)
    parser.add_argument('--candidate-target', default=native_target(), help='native executable target on this host')
    parser.add_argument('--artifacts', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--fixture-size', default=7, type=int, choices=range(2, 32))
    parser.add_argument('--docker-context', required=True)
    parser.add_argument('--project', required=True, type=smoke.project_name)
    parser.add_argument('--evidence', required=True, type=Path)
    args = parser.parse_args()
    smoke.require(args.candidate_target == native_target(), 'candidate target differs from the native benchmark host')
    if args.output.exists() or args.output.resolve() in {args.evidence.resolve(), args.artifacts.resolve(), args.candidate_record.resolve()}:
        parser.error('choose a new live report separate from evidence, manifest and candidate record')
    record = json.loads(args.candidate_record.read_text())
    manifest = record['distribution']
    selected = packaged_entry(record, args.candidate_target)

    def candidate():
        binary = args.candidate_binary.resolve(strict=True)
        smoke.require(sha256(binary) == selected['executable_sha256'], 'packaged binary digest mismatch')
        timed = preflight(args.artifacts)['rust']
        smoke.require(timed.get('candidate') == record and timed['target'] == args.candidate_target
                      and timed['sha256'] == selected['executable_sha256']
                      and timed['candidate_record_sha256'] == sha256(args.candidate_record)
                      and [Path(path).resolve() for path in timed['argv']] == [binary],
                      'timed executable or shared candidate differs from the fixture arguments')
        smoke.REPORT.update(built_commit=manifest['revision'], package_version=manifest['version'],
                            executable_sha256=selected['executable_sha256'])
        return binary

    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=args.output.parent) as scratch:
        pending = Path(scratch) / args.output.name
        result = smoke.entry(['--docker-context', args.docker_context, '--project', args.project,
                              '--evidence', str(args.evidence)],
                             exercise_extension=lambda context: exercise(context, args, pending),
                             retain_legacy=False, candidate_provider=candidate)
        if result == 0:
            smoke.require(pending.is_file() and smoke.REPORT.get('cleanup_verified') is True,
                          'successful fixture result lacks a live report or verified cleanup')
        if pending.exists():
            report = read_json(pending)
            if result != 0 or smoke.REPORT.get('cleanup_verified') is not True:
                report['status'] = 'failed'
                write_json(pending, report)
            # Both paths live on the same filesystem. An existing output is always preserved.
            os.link(pending, args.output)
        return result


if __name__ == '__main__':
    raise SystemExit(main())
