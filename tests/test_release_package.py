import contextlib
import importlib.util
import io
from pathlib import Path
import shutil
import tarfile
import tempfile
import unittest
import uuid
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('package_self_host', ROOT / 'scripts/package-self-host.py')
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class ReleasePackageTest(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name)
        for source in package.files():
            destination = self.root / source.relative_to(ROOT)
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)
        self.patch = patch.object(package, 'ROOT', self.root)
        self.patch.start()
        self.addCleanup(self.patch.stop)

    def build(self, **kwargs):
        with contextlib.redirect_stdout(io.StringIO()):
            return package.build(**kwargs)

    def test_reproducible_archive_excludes_private_files_and_normalizes_headers(self):
        private_marker = uuid.uuid4().hex
        for name in ['.env', '.local/config.env', 'artifacts/private.dump', 'deploy/pilot.env',
                     'cloud/.env', 'cloud/app/models/user.rb', 'cloud/config/master.key']:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(private_marker)
        first = self.build()
        content = first.read_bytes()
        source = self.root / 'Cargo.toml'
        source.touch()
        self.assertEqual(content, self.build().read_bytes())
        with tarfile.open(first) as archive:
            names = archive.getnames()
            self.assertIn('visiconvert/README.md', names)
            self.assertIn('visiconvert/scripts/package-self-host.py', names)
            self.assertIn('visiconvert/crates/convert-server/assets/tracker.js', names)
            self.assertIn('visiconvert/crates/convert-store/migrations/0005_website_capture.sql', names)
            self.assertIn('visiconvert/docs/WEBSITE_TRACKING.md', names)
            self.assertIn('visiconvert/crates/convert-store/migrations/0006_inquiry_grouping.sql', names)
            self.assertIn('visiconvert/docs/INQUIRY_GROUPING.md', names)
            self.assertIn('visiconvert/docs/DYNAMIC_NUMBERS.md', names)
            self.assertIn('visiconvert/crates/convert-store/migrations/0007_dynamic_numbers.sql', names)
            self.assertIn('visiconvert/web/tests/verify-dni.mjs', names)
            self.assertIn('visiconvert/web/tests/verify-grouping.mjs', names)
            self.assertIn('visiconvert/web/tests/verify-sites.mjs', names)
            for member in archive.getmembers():
                self.assertTrue(member.isfile())
                self.assertEqual((member.uid, member.gid, member.mtime), (0, 0, 0))
                self.assertEqual((member.uname, member.gname), ('', ''))
                self.assertNotIn(private_marker.encode(), archive.extractfile(member).read())

    def test_public_release_requires_license(self):
        (self.root / 'LICENSE').unlink(missing_ok=True)
        with self.assertRaisesRegex(ValueError, 'requires a selected'):
            self.build(release=True)

    def test_symlinked_source_is_rejected(self):
        path = self.root / 'Cargo.toml'
        contents = path.read_bytes()
        path.unlink()
        other = self.root / 'private.toml'
        other.write_bytes(contents)
        path.symlink_to(other)
        with self.assertRaisesRegex(ValueError, 'regular source'):
            self.build()

    def test_symlinked_source_directory_is_rejected(self):
        source = self.root / 'crates'
        private = self.root / 'private-crates'
        source.rename(private)
        source.symlink_to(private, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, 'regular source'):
            self.build()


if __name__ == '__main__':
    unittest.main()
