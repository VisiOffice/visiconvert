import contextlib
import importlib.util
import io
import json
import os
import shutil
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('self_host', ROOT / 'scripts/self_host.py')
host = importlib.util.module_from_spec(spec)
spec.loader.exec_module(host)


class SelfHostTest(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.directory = Path(self.scratch.name)
        self.command = [sys.executable, str(ROOT / 'scripts/self_host.py'), '--directory', str(self.directory)]
        subprocess.run(self.command + ['init'], check=True, stdout=subprocess.DEVNULL)
        self.install = host.Installation(self.directory)

    def test_init_preserves_secrets_and_refuses_implicit_reconfiguration(self):
        before = self.install.config.read_bytes()
        subprocess.run(self.command + ['init'], check=True, stdout=subprocess.DEVNULL)
        result = subprocess.run(self.command + ['init', '--port', '43200'], capture_output=True)
        self.assertEqual(result.returncode, 1)
        self.assertEqual(before, self.install.config.read_bytes())
        self.assertEqual(self.install.config.stat().st_mode & 0o777, 0o600)
        other = self.directory / 'other'
        subprocess.run([*self.command[:2], '--directory', str(other), 'init'], check=True, stdout=subprocess.DEVNULL)
        values = host.read_config(other / 'config.env')
        for key in ['VISICONVERT_API_KEY', 'VISICONVERT_POSTGRES_PASSWORD', 'VISICONVERT_WORKSPACE_ID']:
            self.assertNotEqual(values[key], self.install.values[key])

    def test_rejects_public_http_unsafe_interpolation_and_duplicate_keys(self):
        for origin in ['http://example.com', 'https://example.com/path', 'https://user:pass@example.com', 'https://example.com/']:
            with self.subTest(origin=origin), self.assertRaises(ValueError):
                host.validate(dict(self.install.values, PUBLIC_URL=origin))
        original = self.install.config.read_text()
        for suffix in ['PUBLIC_URL=https://other.example\n', 'UNEXPECTED=$(id)\n']:
            self.install.config.write_text(original + suffix)
            with self.assertRaises(ValueError):
                host.read_config(self.install.config)

    def test_external_shell_values_cannot_replace_installation(self):
        with patch.dict(os.environ, {'VISICONVERT_API_KEY': 'wrong', 'PUBLIC_URL': 'https://wrong.example',
                                     'COMPOSE_FILE': '/tmp/other.yml', 'COMPOSE_PROJECT_NAME': 'visiconvert-pilot',
                                     'VISICONVERT_IMAGE': 'wrong:image'}):
            installation = host.Installation(self.directory)
        with patch.object(host.subprocess, 'run', return_value=subprocess.CompletedProcess([], 0)) as run:
            installation.compose('config', '--quiet')
        env = run.call_args.kwargs['env']
        self.assertEqual(env['VISICONVERT_API_KEY'], self.install.values['VISICONVERT_API_KEY'])
        self.assertEqual(env['COMPOSE_PROJECT_NAME'], 'visiconvert-selfhost')
        self.assertNotIn('COMPOSE_FILE', env)
        self.assertEqual(env['VISICONVERT_IMAGE'], 'visiconvert-selfhost:local')

    def test_failed_migration_never_starts_new_app_or_records_success(self):
        calls = []
        def compose(*args, **kwargs):
            calls.append(args)
            if args[0] == 'run':
                raise ValueError('migration failure')
        with patch.object(self.install, 'compose', side_effect=compose), \
             patch.object(self.install, 'database'), patch.object(self.install, 'check_workspace'), \
             patch.object(self.install, 'backup', return_value=self.directory / 'backup'), \
             contextlib.redirect_stderr(io.StringIO()), self.assertRaises(ValueError):
            self.install.up()
        self.assertFalse(any(c[0] == 'up' for c in calls))
        self.assertFalse(self.install.state.exists())

    def test_failed_backup_never_runs_migrations(self):
        with patch.object(self.install, 'compose') as compose, patch.object(self.install, 'database'), \
             patch.object(self.install, 'check_workspace'), \
             patch.object(self.install, 'backup', side_effect=ValueError('dump failure')), self.assertRaises(ValueError):
            self.install.up()
        self.assertFalse(any(c.args[0] == 'run' for c in compose.call_args_list))

    def test_missing_requested_image_leaves_current_services_untouched(self):
        with patch.object(host.subprocess, 'run', return_value=subprocess.CompletedProcess([], 1)), \
             patch.object(self.install, 'compose') as compose, self.assertRaisesRegex(ValueError, 'not loaded'):
            self.install.up('visiconvert-selfhost:missing')
        compose.assert_not_called()

    def test_empty_image_reference_does_not_trigger_rebuild(self):
        with patch.object(self.install, 'compose') as compose, self.assertRaisesRegex(ValueError, 'Invalid image'):
            self.install.up('')
        compose.assert_not_called()

    @unittest.skipUnless(shutil.which('docker'), 'Docker CLI is needed for argument validation')
    def test_migration_flags_accepted_by_real_compose_parser(self):
        with patch.object(self.install, 'compose') as compose:
            self.install.migrate()
        args = list(compose.call_args.args)
        self.assertEqual(args[-1], 'migrate')
        # Help parses the actual options without connecting to Docker or running migrations.
        result = subprocess.run(['docker', 'compose', *args[:-1], '--help'], capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_concurrent_management_command_is_rejected(self):
        with (self.directory / '.lock').open('a') as lock:
            host.fcntl.flock(lock, host.fcntl.LOCK_EX | host.fcntl.LOCK_NB)
            result = subprocess.run(self.command + ['init'], capture_output=True, text=True)
        self.assertEqual(result.returncode, 1)
        self.assertIn('Another self-host command is running', result.stderr)

    def test_workspace_change_is_rejected_before_migration(self):
        with patch.object(self.install, 'sql', side_effect=['t', 'some-other-workspace']), self.assertRaises(ValueError):
            self.install.check_workspace()

    def test_nonempty_restore_refused_without_mutation(self):
        backup = self.directory / 'backup'
        backup.mkdir()
        (backup / 'database.dump').write_bytes(b'example')
        manifest = {'format': 1, 'postgres_major': 17, 'workspace_id': self.install.values['VISICONVERT_WORKSPACE_ID'],
                    'sha256': host.hashlib.sha256(b'example').hexdigest()}
        (backup / 'manifest.json').write_text(json.dumps(manifest))
        with patch.object(self.install, 'database'), patch.object(self.install, 'sql', return_value='7'), \
             patch.object(self.install, 'compose') as compose, self.assertRaisesRegex(ValueError, 'empty database'):
            self.install.restore(backup)
        compose.assert_not_called()

    def test_corrupt_backup_rejected_before_docker(self):
        backup = self.directory / 'backup'
        backup.mkdir()
        (backup / 'database.dump').write_bytes(b'damaged')
        (backup / 'manifest.json').write_text(json.dumps({'format': 1, 'postgres_major': 17,
            'workspace_id': self.install.values['VISICONVERT_WORKSPACE_ID'], 'sha256': 'wrong'}))
        with patch.object(self.install, 'database') as database, self.assertRaisesRegex(ValueError, 'checksum'):
            self.install.restore(backup)
        database.assert_not_called()


if __name__ == '__main__':
    unittest.main()
