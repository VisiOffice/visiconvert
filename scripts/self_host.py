#!/usr/bin/env python3
"""Configure, run, verify, and back up a standalone VisiConvert installation."""
import argparse
import datetime
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[1]
KEYS = {'COMPOSE_PROJECT_NAME', 'VISICONVERT_POSTGRES_PASSWORD', 'VISICONVERT_API_KEY',
        'VISICONVERT_WORKSPACE_ID', 'VISICONVERT_HOST_PORT', 'PUBLIC_URL',
        'TWILIO_ACCOUNT_SID', 'TWILIO_AUTH_TOKEN'}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def read_config(path):
    values = {}
    for line in path.read_text().splitlines():
        if not line or line.startswith('#'):
            continue
        key, separator, value = line.partition('=')
        require(separator and key in KEYS and key not in values,
                'Unknown or duplicate configuration key; use the generated config.env format.')
        require(not any(c in value for c in '\r\n\x00$\'"\\') and not any(c.isspace() for c in value),
                f'{key}: use an unquoted value without whitespace or shell interpolation.')
        values[key] = value
    validate(values)
    return values


def validate(v):
    require(KEYS == v.keys(), 'Incomplete config.env; run init in a new directory for a reference.')
    require(re.fullmatch(r'[a-z0-9][a-z0-9_-]{0,62}', v['COMPOSE_PROJECT_NAME'])
            and v['COMPOSE_PROJECT_NAME'] != 'visiconvert-pilot', 'Choose a valid, separate Compose project name.')
    for key in ['VISICONVERT_POSTGRES_PASSWORD', 'VISICONVERT_API_KEY']:
        require(re.fullmatch(r'[0-9a-f]{64}', v[key]), f'{key} must be 64 lowercase hexadecimal characters.')
    uuid.UUID(v['VISICONVERT_WORKSPACE_ID'])
    require(v['VISICONVERT_HOST_PORT'].isdigit() and 1024 <= int(v['VISICONVERT_HOST_PORT']) <= 65535,
            'Host port must be between 1024 and 65535.')
    origin = v['PUBLIC_URL']
    u = urllib.parse.urlsplit(origin)
    require(u.scheme in ('http', 'https') and u.hostname and not u.path and not u.query
            and not u.fragment and not u.username and not u.password
            and not any(c.isspace() for c in origin), 'PUBLIC_URL must be an HTTP(S) origin, without a path or trailing slash.')
    _ = u.port  # Reject malformed ports before starting Docker.
    require(u.scheme == 'https' or u.hostname in ('127.0.0.1', 'localhost', '::1'),
            'Use HTTPS for a public domain; HTTP is supported only for local access.')
    account, token = v['TWILIO_ACCOUNT_SID'], v['TWILIO_AUTH_TOKEN']
    require(bool(account) == bool(token), 'Set both Twilio values or leave both empty.')
    if account:
        require(re.fullmatch(r'AC[0-9a-fA-F]{32}', account) and re.fullmatch(r'[0-9a-fA-F]{32}', token)
                and u.scheme == 'https', 'Twilio requires valid credentials and an HTTPS public origin.')


def write_config(path, values):
    validate(values)
    temporary = path.with_suffix('.tmp')
    temporary.write_text(''.join(f'{key}={value}\n' for key, value in values.items()))
    temporary.chmod(0o600)
    temporary.replace(path)


def stamp():
    return datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%SZ') + '-' + secrets.token_hex(4)


class Installation:
    def __init__(self, directory):
        self.directory = directory
        self.config = directory / 'config.env'
        self.state = directory / 'deployment.json'
        self.values = read_config(self.config)
        self.image = json.loads(self.state.read_text())['image'] if self.state.exists() else 'visiconvert-selfhost:local'
        # Explicit config wins over an unrelated development .env or shell export.
        self.env = {k: v for k, v in os.environ.items()
                    if k not in KEYS and not k.startswith('COMPOSE_') and k != 'VISICONVERT_IMAGE'}
        self.env.update(self.values)

    def compose(self, *args, **kwargs):
        env = dict(self.env, VISICONVERT_IMAGE=self.image)
        result = subprocess.run(['docker', 'compose', '--project-name', self.values['COMPOSE_PROJECT_NAME'],
                                 '--env-file', str(self.config), '-f', str(ROOT / 'compose.yaml'), *args],
                                cwd=ROOT, env=env, **kwargs)
        require(result.returncode == 0, f'Docker Compose {args[0]} failed; existing data and backups are retained.')
        return result

    def database(self):
        self.compose('up', '-d', '--no-build', '--wait', '--wait-timeout', '120', 'db')

    def sql(self, sql):
        return self.compose('exec', '-T', 'db', 'psql', '-U', 'visiconvert', '-d', 'visiconvert',
                            '-v', 'ON_ERROR_STOP=1', '-Atc', sql, stdout=subprocess.PIPE, text=True).stdout.strip()

    def check_workspace(self):
        if self.sql("SELECT to_regclass('public.workspaces') IS NOT NULL") == 't':
            ids = self.sql('SELECT id FROM workspaces').splitlines()
            require(not ids or ids == [self.values['VISICONVERT_WORKSPACE_ID']],
                    'Database workspace differs from config.env. Restore the original workspace ID before proceeding.')

    def backup(self):
        name = stamp()
        parent = self.directory / 'backups'
        parent.mkdir(mode=0o700, exist_ok=True)
        partial = parent / (name + '.incomplete')
        partial.mkdir(mode=0o700)
        dump = partial / 'database.dump'
        with dump.open('xb') as stream:
            self.compose('exec', '-T', 'db', 'pg_dump', '-U', 'visiconvert', '-d', 'visiconvert',
                         '--format=custom', '--no-owner', '--no-acl', stdout=stream)
        require(dump.stat().st_size > 0, 'Database backup was empty; upgrade stopped.')
        # Confirm the archive is readable before treating it as a completed backup.
        with dump.open('rb') as stream:
            self.compose('exec', '-T', 'db', 'pg_restore', '--list', stdin=stream, stdout=subprocess.DEVNULL)
        shutil.copyfile(self.config, partial / 'config.env')
        with dump.open('rb') as stream:
            digest = hashlib.file_digest(stream, 'sha256').hexdigest()
        metadata = {'format': 1, 'postgres_major': 17, 'image': self.image,
                    'workspace_id': self.values['VISICONVERT_WORKSPACE_ID'],
                    'sha256': digest}
        (partial / 'manifest.json').write_text(json.dumps(metadata, indent=2) + '\n')
        destination = parent / name
        partial.rename(destination)
        print(f'Backup: {destination}', flush=True)
        return destination

    def verify(self):
        origin = 'http://127.0.0.1:' + self.values['VISICONVERT_HOST_PORT']
        # No proxy or redirects: the operator key goes only to the local listener.
        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, req, fp, code, msg, headers, newurl):
                return None
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRedirect())

        def fetch(path, authenticated=False):
            headers = {'User-Agent': 'VisiConvert-SelfHost-Check/1.0'}
            if authenticated:
                headers['Authorization'] = 'Bearer ' + self.values['VISICONVERT_API_KEY']
            with opener.open(urllib.request.Request(origin + path, headers=headers), timeout=10) as response:
                return response.read()

        require(json.loads(fetch('/health'))['status'] == 'ok', 'Health check failed.')
        require(b'visiconvert' in fetch('/').lower(), 'Frontend is missing.')
        bootstrap = json.loads(fetch('/api/bootstrap', True))
        require(bootstrap['id'] == self.values['VISICONVERT_WORKSPACE_ID'], 'Wrong workspace served.')
        for key, path in [('voice_url', 'voice'), ('status_url', 'status')]:
            require(bootstrap[key] == self.values['PUBLIC_URL'] + '/webhooks/twilio/' + path,
                    'Public callback origin is incorrect.')
        require(bootstrap['twilio_configured'] == bool(self.values['TWILIO_AUTH_TOKEN']), 'Carrier configuration mismatch.')
        require(isinstance(json.loads(fetch('/api/summary', True)), list), 'Database-backed API check failed.')
        try:
            fetch('/api/leads')
        except urllib.error.HTTPError as error:
            require(error.code == 401, 'Expected unauthenticated API to return 401.')
        else:
            raise ValueError('Private API allowed an unauthenticated request.')
        print('PASS: frontend, health, workspace authentication, database, callback URLs, and API access protection.', flush=True)

    def migrate(self, **kwargs):
        # `run` has no --no-build flag. Use the prebuilt image without pulling.
        return self.compose('run', '--rm', '--no-deps', '--pull', 'never', 'migrate', **kwargs)

    def up(self, image=None):
        previous = self.image
        if image is not None:
            require(re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._/:@-]*', image), 'Invalid image reference.')
            result = subprocess.run(['docker', 'image', 'inspect', image], env=self.env, stdout=subprocess.DEVNULL)
            require(result.returncode == 0, 'Requested image is not loaded. Pull or build it before running up.')
            candidate = image
        else:
            candidate = 'visiconvert-selfhost:' + stamp().lower()
            self.image = candidate
            try:
                self.compose('build', 'app')
            finally:
                self.image = previous
        # Build first. The existing app is stopped only when we are ready to migrate.
        self.database()
        self.check_workspace()
        self.compose('stop', 'app')
        backup = self.backup()
        self.image = candidate
        try:
            # Always run the current migrations, even if a previous one-shot container exited 0.
            self.migrate()
            self.compose('up', '-d', '--no-build', '--pull', 'never', '--no-deps', '--wait', '--wait-timeout', '120', 'app')
            self.verify()
        except (ValueError, OSError):
            print(f'Upgrade did not verify. Backup retained at {backup}. No automatic schema rollback was attempted.', file=sys.stderr)
            raise
        temporary = self.state.with_suffix('.tmp')
        temporary.write_text(json.dumps({'image': candidate, 'previous_image': previous, 'backup': str(backup)}, indent=2) + '\n')
        temporary.replace(self.state)
        print(f'Ready: http://127.0.0.1:{self.values["VISICONVERT_HOST_PORT"]}\nWorkspace key: VISICONVERT_API_KEY in {self.config}')

    def restore(self, backup):
        manifest = json.loads((backup / 'manifest.json').read_text())
        require(manifest['format'] == 1 and manifest['postgres_major'] == 17, 'Unsupported backup format.')
        uuid.UUID(manifest['workspace_id'])
        with (backup / 'database.dump').open('rb') as stream:
            require(hashlib.file_digest(stream, 'sha256').hexdigest() == manifest['sha256'], 'Backup checksum mismatch.')
        self.database()
        require(self.sql("SELECT count(*) FROM pg_tables WHERE schemaname='public'") == '0',
                'Restore requires an empty database. Use a new Compose project and configuration directory.')
        self.compose('stop', 'app')
        with (backup / 'database.dump').open('rb') as stream:
            self.compose('exec', '-T', 'db', 'pg_restore', '-U', 'visiconvert', '-d', 'visiconvert',
                         '--exit-on-error', '--single-transaction', '--no-owner', '--no-acl', stdin=stream)
        # Preserve the destination credentials/origin; keep the original workspace's identity.
        self.values['VISICONVERT_WORKSPACE_ID'] = manifest['workspace_id']
        write_config(self.config, self.values)
        print('Restore complete. Run up using the source version or retained image matching the backup.')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--directory', type=Path, default=ROOT / '.local/self-host', help='configuration and backup directory')
    commands = parser.add_subparsers(dest='command', required=True)
    init = commands.add_parser('init', help='generate private configuration without overwriting an existing installation')
    init.add_argument('--port', type=int)
    init.add_argument('--origin')
    init.add_argument('--project')
    up = commands.add_parser('up', help='build, back up, migrate, start, and verify')
    up.add_argument('--image', help='use an already-loaded image instead of building')
    for name in ['verify', 'status', 'logs', 'stop', 'backup']:
        commands.add_parser(name)
    commands.add_parser('restore', help='restore a backup into an empty database').add_argument('backup', type=Path)
    args = parser.parse_args()
    os.umask(0o077)
    directory = args.directory.expanduser().resolve()
    directory.mkdir(parents=True, mode=0o700, exist_ok=True)
    # Prevent concurrent upgrades/backups/restores for this installation.
    with (directory / '.lock').open('a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise ValueError('Another self-host command is running for this installation.') from None
        config = directory / 'config.env'
        if args.command == 'init':
            if config.exists():
                read_config(config)
                require(not any(value is not None for value in [args.port, args.origin, args.project]),
                        'Configuration already exists; edit config.env explicitly to change it. Credentials were preserved.')
                print(f'Existing configuration preserved: {config}')
                return
            port = args.port if args.port is not None else 3100
            values = {'COMPOSE_PROJECT_NAME': args.project or 'visiconvert-selfhost',
                      'VISICONVERT_POSTGRES_PASSWORD': secrets.token_hex(32), 'VISICONVERT_API_KEY': secrets.token_hex(32),
                      'VISICONVERT_WORKSPACE_ID': str(uuid.uuid4()), 'VISICONVERT_HOST_PORT': str(port),
                      'PUBLIC_URL': args.origin or f'http://127.0.0.1:{port}',
                      'TWILIO_ACCOUNT_SID': '', 'TWILIO_AUTH_TOKEN': ''}
            write_config(config, values)
            print(f'Created {config} (private, mode 600).\nNext: run self-host.sh up. No carrier account is required to start.')
            return
        require(config.is_file(), 'Run self-host.sh init first.')
        require(config.stat().st_mode & 0o077 == 0, 'config.env must be private: chmod 600 the file.')
        require(shutil.which('docker'), 'Docker Engine/Desktop and Docker Compose are required.')
        daemon = subprocess.run(['docker', 'info', '--format', '{{.ServerVersion}}'],
                                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        require(daemon.returncode == 0,
                'Cannot access the Docker daemon. Start Docker and run from a terminal authorized to use it.')
        installation = Installation(directory)
        installation.compose('config', '--quiet')
        if args.command == 'up':
            installation.up(args.image)
        elif args.command == 'verify':
            installation.verify()
        elif args.command == 'status':
            installation.compose('ps', '-a')
        elif args.command == 'logs':
            installation.compose('logs', '--tail', '100', 'app', 'db')
        elif args.command == 'stop':
            installation.compose('stop')
        elif args.command == 'backup':
            installation.database()
            installation.check_workspace()
            installation.backup()
        elif args.command == 'restore':
            installation.restore(args.backup.expanduser().resolve())


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, KeyError) as error:
        print(f'ERROR: {error}', file=sys.stderr)
        sys.exit(1)
