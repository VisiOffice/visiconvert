#!/usr/bin/env python3
"""Create a source-only self-hosting bundle from an explicit file allowlist."""
import hashlib
import argparse
import gzip
from pathlib import Path
import tarfile
import io
import tomllib

ROOT = Path(__file__).resolve().parents[1]
FILES = ['Cargo.toml', 'Cargo.lock', 'Dockerfile', '.dockerignore', '.gitignore',
         '.github/workflows/self-host.yml',
         'CONTRIBUTING.md', 'CHANGELOG.md', 'docs/PUBLIC_README.md',
         'compose.yaml', 'docs/SELF_HOSTING.md', 'scripts/self-host.sh', 'scripts/self_host.py',
         'scripts/verify-self-host.py', 'scripts/package-self-host.py', 'scripts/sync-brand.py',
         'tests/test_self_host.py', 'tests/test_release_package.py',
         'web/package.json', 'web/pnpm-lock.yaml', 'web/tsconfig.json', 'web/vite.config.ts',
         'web/index.html', 'web/tests/verify-local.mjs']
TREES = {'crates': {'.rs', '.toml', '.sql'}, 'web/src': {'.tsx', '.ts', '.css'},
         'assets/brand': {'.svg', '.md', '.txt'}, 'web/public': {'.svg', '.txt'}}


def files():
    result = [ROOT / name for name in FILES]
    if (ROOT / 'LICENSE').exists():
        result.append(ROOT / 'LICENSE')
    for tree, extensions in TREES.items():
        result.extend(p for p in (ROOT / tree).rglob('*')
                      if p.is_file() and p.suffix in extensions
                      and not {'generated', 'target', 'node_modules'}.intersection(p.relative_to(ROOT).parts))
    for path in result:
        ancestors = [ROOT / parent for parent in path.relative_to(ROOT).parents]
        if any(p.is_symlink() for p in [path, *ancestors]) or not path.is_file():
            raise ValueError(f'Expected a regular source file: {path.relative_to(ROOT)}')
    return sorted(result)


def build(release=False):
    if release:
        metadata = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['package']
        if not metadata.get('license') or not (ROOT / 'LICENSE').is_file():
            raise ValueError('Public release requires a selected Cargo license and LICENSE file')
    selected = files()
    readme = (ROOT / 'docs/PUBLIC_README.md').read_bytes()
    digest = hashlib.sha256()
    for path in selected:
        digest.update(str(path.relative_to(ROOT)).encode() + b'\0' + path.read_bytes())
    output = ROOT / 'artifacts/self-host'
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f'visiconvert-selfhost-{digest.hexdigest()[:12]}.tar.gz'
    prefix = 'visiconvert'
    # Stable headers prevent local usernames, ownership, and timestamps leaking
    # into the package, and give identical source identical archive bytes.
    with archive.open('wb') as raw, gzip.GzipFile(filename='', fileobj=raw, mode='wb', mtime=0) as zipped:
        with tarfile.open(fileobj=zipped, mode='w', format=tarfile.PAX_FORMAT) as tar:
            for name, data, mode in [
                *[(str(path.relative_to(ROOT)), path.read_bytes(),
                   0o755 if path.suffix == '.sh' else 0o644) for path in selected],
                ('README.md', readme, 0o644),
            ]:
                member = tarfile.TarInfo(f'{prefix}/{name}')
                member.size = len(data)
                member.mode = mode
                tar.addfile(member, io.BytesIO(data))
    checksum = hashlib.sha256(archive.read_bytes()).hexdigest()
    archive.with_suffix(archive.suffix + '.sha256').write_text(f'{checksum}  {archive.name}\n')
    print(archive)
    print(f'SHA256: {checksum}')
    return archive


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--release', action='store_true', help='require license metadata before packaging')
    args = parser.parse_args()
    try:
        build(release=args.release)
    except ValueError as error:
        parser.exit(1, f'ERROR: {error}\n')
