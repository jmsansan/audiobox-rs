#!/usr/bin/env python3
"""Audit and test the actual offline Cargo distribution, not the repository tree."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import posixpath
import re
import subprocess
import tarfile

ROOT = Path(__file__).resolve().parents[1]
MAX_COMPRESSED_BYTES = 1024 * 1024
ROOT_FILES = {
    'Cargo.toml', 'Cargo.toml.orig', 'Cargo.lock', '.cargo_vcs_info.json',
    'README.md', 'CHANGELOG.md', 'LICENSE',
}
REQUIRED_FILES = {
    'Cargo.toml', 'Cargo.lock', 'src/lib.rs', 'README.md', 'CHANGELOG.md',
    'LICENSE', 'src/ogg/LICENSE-TPT-MIT', 'src/ogg/LICENSE-OPUS-BSD',
    'src/ogg/NOTICE.md',
}


def run(*args, **kwargs):
    subprocess.run(args, cwd=ROOT, check=True, **kwargs)


def allowed(name):
    path = PurePosixPath(name)
    if name in ROOT_FILES:
        return True
    if path.parts[0] == 'src':
        return path.suffix == '.rs' or name in REQUIRED_FILES
    if len(path.parts) == 2 and path.parts[0] in ('docs', 'examples'):
        return path.suffix == ('.md' if path.parts[0] == 'docs' else '.rs')
    return name == 'benches/engine.rs'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--allow-dirty', action='store_true', help='local iteration only')
    options = parser.parse_args()
    metadata = json.loads(subprocess.check_output(
        ['cargo', 'metadata', '--locked', '--offline', '--no-deps', '--format-version', '1'],
        cwd=ROOT, text=True))
    package = next(p for p in metadata['packages']
                   if Path(p['manifest_path']).resolve() == ROOT / 'Cargo.toml')
    stem = f"{package['name']}-{package['version']}"
    target = Path(metadata['target_directory'])
    command = ['cargo', 'package', '--locked', '--offline']
    if options.allow_dirty:
        command.append('--allow-dirty')
    run(*command)
    archive = target / 'package' / f'{stem}.crate'
    size = archive.stat().st_size
    if size > MAX_COMPRESSED_BYTES:
        raise SystemExit(f'Package is {size:,} bytes; budget is {MAX_COMPRESSED_BYTES:,}')
    files = {}
    with tarfile.open(archive) as bundle:
        for member in bundle.getmembers():
            path = PurePosixPath(member.name)
            if path.is_absolute() or '..' in path.parts or path.parts[0] != stem:
                raise SystemExit(f'Invalid package path: {member.name}')
            if member.isdir():
                continue
            name = path.relative_to(stem).as_posix()
            if not member.isfile() or not allowed(name):
                raise SystemExit(f'Unexpected distribution entry: {name}')
            files[name] = bundle.extractfile(member).read()
    missing = REQUIRED_FILES - files.keys()
    if missing:
        raise SystemExit(f'Missing distribution files: {sorted(missing)}')
    # Guides may link to versioned GitHub fixtures; local links must ship too.
    for name, data in files.items():
        if name == 'README.md' or name.startswith('docs/'):
            for link in re.findall(r'\]\(([^)\s]+)\)', data.decode('utf-8')):
                if link.startswith(('#', 'http:', 'https:', 'mailto:')):
                    continue
                destination = link.split('#')[0].split('?')[0]
                if not destination:
                    continue
                local = posixpath.normpath(posixpath.join(posixpath.dirname(name), destination))
                if local not in files and not any(f.startswith(local.rstrip('/') + '/') for f in files):
                    raise SystemExit(f'Unshipped local link in {name}: {link}')
    report = {
        'package': stem, 'files': len(files), 'compressed_bytes': size,
        'budget_bytes': MAX_COMPRESSED_BYTES,
        'sha256': hashlib.sha256(archive.read_bytes()).hexdigest(),
    }
    (target / 'package-report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(f"Audited {len(files)} distribution files: {size / 1024:.1f} KiB; no integration tests or media fixtures.", flush=True)
    manifest = target / 'package' / stem / 'Cargo.toml'
    env = dict(os.environ, CARGO_TARGET_DIR=str(target / 'package-check'))
    for args in [
        ('check', '--all-targets', '--all-features'),
        ('test', '--lib', '--all-features'),
        ('test', '--doc', '--all-features'),
    ]:
        run('cargo', *args, '--locked', '--offline', '--manifest-path', str(manifest), env=env)
    print('Packaged targets, library tests and doctests passed with all features.', flush=True)


if __name__ == '__main__':
    main()
