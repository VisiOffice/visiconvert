#!/usr/bin/env python3
"""Exercise real Compose installs and restore using only disposable test projects."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import urllib.request
import uuid
from datetime import datetime, timezone

ROOT = Path(__file__).resolve().parents[1]


def free_port():
    with socket.socket() as sock:
        sock.bind(('127.0.0.1', 0))
        return sock.getsockname()[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, default=ROOT, help='test an extracted source bundle')
    parser.add_argument('--image', help='use an already-built app image; otherwise exercise the source build')
    args = parser.parse_args()
    source = args.source.resolve()
    spec = importlib.util.spec_from_file_location('self_host', source / 'scripts/self_host.py')
    host = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(host)
    os.umask(0o077)
    run = uuid.uuid4().hex[:12]
    artifact = ROOT / 'artifacts/self-host-verification' / run
    artifact.mkdir(parents=True, mode=0o700)
    result = {'status': 'running', 'started_at': datetime.now(timezone.utc).isoformat(), 'checks': []}
    instances = []
    log = (artifact / 'services.log').open('w')

    def step(name):
        print('PASS: ' + name, flush=True)
        result['checks'].append(name)

    def command(directory, *argv, fail=False):
        process = subprocess.run([sys.executable, str(source / 'scripts/self_host.py'),
                                  '--directory', str(directory), *argv],
                                 cwd=source, stdout=log, stderr=log)
        if (process.returncode != 0) != fail:
            raise AssertionError(f'Command {argv[0]} returned {process.returncode}; inspect services.log')

    def api(instance, path, data=None, method=None):
        body = json.dumps(data).encode() if data is not None else None
        headers = {'Authorization': 'Bearer ' + instance.values['VISICONVERT_API_KEY'], 'Content-Type': 'application/json'}
        origin = 'http://127.0.0.1:' + instance.values['VISICONVERT_HOST_PORT']
        req = urllib.request.Request(origin + '/api' + path, data=body, headers=headers, method=method)
        with urllib.request.build_opener(urllib.request.ProxyHandler({})).open(req, timeout=10) as response:
            return json.load(response)

    def initialize(suffix):
        directory = artifact / suffix
        command(directory, 'init', '--project', f'visiconvert-qa-{run}-{suffix}', '--port', str(free_port()))
        instance = host.Installation(directory)
        instances.append(instance)
        return instance

    def up(directory, image):
        command(directory, 'up', *(['--image', image] if image else []))

    try:
        print(f'Clean self-host verification. Logs: {artifact}', flush=True)
        original = initialize('install')
        before = original.config.read_bytes()
        command(original.directory, 'init')
        assert before == original.config.read_bytes()
        step('generated credentials survive repeated setup')
        up(original.directory, args.image)
        original = host.Installation(original.directory)
        instances[0] = original
        assert api(original, '/leads') == []
        original.migrate(stdout=log, stderr=log)
        step('clean installation, empty workspace, and repeatable migrations')

        config = json.loads(original.compose('config', '--format', 'json', stdout=subprocess.PIPE, text=True).stdout)
        assert 'ports' not in config['services']['db']
        assert config['services']['app']['ports'][0]['host_ip'] == '127.0.0.1'
        assert config['services']['app']['read_only'] is True
        step('database unexposed and app bound to loopback with read-only filesystem')

        lead = api(original, '/leads', {'external_id': 'self-host-check', 'name': 'Self-host QA',
                   'contact': 'selfhost@example.com', 'source': 'Package QA', 'campaign': 'Clean install', 'notes': ''})['id']
        api(original, '/leads/' + lead, {'qualification': 'qualified', 'notes': 'Must survive restart and restore', 'version': 1}, 'PATCH')
        api(original, '/leads/' + lead + '/outcomes', {'external_id': 'booking-qa', 'amount_minor': 12345,
            'currency': 'USD', 'status': 'booked', 'revision': 1}, 'PUT')
        assert api(original, '/summary')[0]['booked_minor'] == 12345
        command(original.directory, 'backup')
        backup = sorted((original.directory / 'backups').glob('*/manifest.json'))[-1].parent
        step('lead qualification, exact-cent booking, and database backup')

        command(original.directory, 'stop')
        up(original.directory, original.image)
        assert before == original.config.read_bytes()
        rows = api(original, '/leads')
        assert len(rows) == 1 and rows[0]['id'] == lead and rows[0]['qualification'] == 'qualified'
        assert rows[0]['notes'] == 'Must survive restart and restore' and rows[0]['booked_minor'] == 12345
        step('data and credentials survive database restart and repeated installation')

        recovery = initialize('restore')
        recovery_key = recovery.values['VISICONVERT_API_KEY']
        command(recovery.directory, 'restore', str(backup))
        up(recovery.directory, original.image)
        recovery = host.Installation(recovery.directory)
        instances[1] = recovery
        assert recovery.values['VISICONVERT_API_KEY'] == recovery_key
        assert recovery.values['VISICONVERT_WORKSPACE_ID'] == original.values['VISICONVERT_WORKSPACE_ID']
        assert api(recovery, '/leads') == rows
        assert api(recovery, '/summary')[0]['booked_minor'] == 12345
        step('backup restored into a separate installation with data intact and new login credentials')

        command(recovery.directory, 'restore', str(backup), fail=True)
        assert api(recovery, '/leads') == rows
        step('restore refuses to overwrite a populated database')
        result['status'] = 'passed'
    except BaseException as error:
        result['status'] = 'failed'
        result['error'] = str(error)
        raise
    finally:
        for instance in reversed(instances):
            # Only projects allocated by this test are eligible for volume deletion.
            assert instance.values['COMPOSE_PROJECT_NAME'].startswith(f'visiconvert-qa-{run}-')
            try:
                instance.compose('down', '--volumes', '--remove-orphans', stdout=log, stderr=log)
            except Exception as error:
                result.setdefault('cleanup_errors', []).append(str(error))
                result['status'] = 'failed'
        log.close()
        (artifact / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
        print(f'Verification {result["status"]}: {artifact / "result.json"}', flush=True)
    return 0 if result['status'] == 'passed' else 1


if __name__ == '__main__':
    def interrupted(*_):
        raise KeyboardInterrupt()
    signal.signal(signal.SIGTERM, interrupted)
    sys.exit(main())
