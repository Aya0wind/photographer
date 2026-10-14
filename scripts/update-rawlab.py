#!/usr/bin/env python3
"""Manage a pinned, unmodified core snapshot. This is build tooling, not app code."""
import argparse
import hashlib
import json
import re
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
NATIVE = ROOT / 'native' / 'rawlab'
LOCK = NATIVE / 'upstream.lock.json'
PATHS = ['lutools/src', 'lutools/include', 'lutools/CMakeLists.txt', 'lutools/cmake',
         'lutools/third_party', 'lutools/docs/color-contract.md', 'LICENSE', 'docs/licensing.md']
REPOSITORY = 'https://github.com/dancancer/RawLab'


def digest(data):
    return hashlib.sha256(data).hexdigest()


def snapshot_digest(files):
    return digest(b''.join(p.encode() + b'\0' + h.encode() + b'\0'
                           for p, h in sorted(files.items())))


def verify_snapshot(lock):
    if lock['version'] != 1 or snapshot_digest(lock['files']) != lock['snapshotSha256']:
        raise RuntimeError('Unsupported or inconsistent upstream lock')
    tree = NATIVE / 'upstream'
    actual = {str(p.relative_to(tree)).replace('\\', '/'): digest(p.read_bytes())
              for p in tree.rglob('*') if p.is_file()}
    if actual != lock['files']:
        changed = sorted(set(actual) ^ set(lock['files']) | {
            p for p in actual.keys() & lock['files'].keys() if actual[p] != lock['files'][p]})
        raise RuntimeError('Upstream snapshot was edited; restore/review before updating: ' + ', '.join(changed[:8]))


def git(repo, *arguments):
    return subprocess.check_output(['git', '-C', str(repo), *arguments])


def snapshot(repo, ref):
    commit = git(repo, 'rev-parse', '--verify', ref + '^{commit}').decode().strip()
    entries = git(repo, 'ls-tree', '-rz', commit, '--', *PATHS).split(b'\0')
    data = {}
    for entry in entries:
        if not entry:
            continue
        metadata, path_bytes = entry.split(b'\t', 1)
        mode, kind, object_id = metadata.split()
        path = path_bytes.decode('utf-8')
        if mode not in (b'100644', b'100755') or kind != b'blob' or '..' in Path(path).parts:
            raise RuntimeError('Unsupported upstream tree entry: ' + path)
        data[path] = git(repo, 'cat-file', 'blob', object_id.decode())
    if not data or 'lutools/CMakeLists.txt' not in data:
        raise RuntimeError('Ref has no supported core source tree')
    files = {p: digest(b) for p, b in sorted(data.items())}
    lock = {'version': 1, 'repository': REPOSITORY, 'trackingRef': ref,
            'commit': commit, 'sourcePaths': PATHS, 'snapshotSha256': snapshot_digest(files), 'files': files}
    bootstrap = git(repo, 'show', commit + ':lutools/platform/ios/build-framework.sh').decode()
    dependencies = {}
    for key, archive in [('libraw', 'LibRaw-'), ('librawCmake', 'LibRaw-cmake-')]:
        pattern = r'fetch (' + re.escape(archive) + r'\S+) ([0-9a-f]{64})\s+\\\s+(https://\S+)'
        match = re.search(pattern, bootstrap)
        if match is None:
            raise RuntimeError('Upstream native dependency layout changed: ' + key)
        dependencies[key] = {'archive': match[1], 'sha256': match[2], 'url': match[3]}
    lock['nativeDependencies'] = dependencies
    return data, lock


def write_snapshot(data, lock):
    with tempfile.TemporaryDirectory(prefix='.rawlab-update-', dir=NATIVE) as directory:
        temporary = Path(directory)
        tree = temporary / 'upstream'
        for name, content in data.items():
            target = tree / name
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(content)
        manifest = temporary / 'upstream.lock.json'
        manifest.write_text(json.dumps(lock, indent=2) + '\n', encoding='utf-8')
        target = NATIVE / 'upstream'
        backup = temporary / 'previous'
        target.rename(backup)
        try:
            tree.rename(target)
            manifest.replace(LOCK)
        except BaseException:
            if target.exists():
                shutil.rmtree(target)
            backup.rename(target)
            raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', type=Path, help='Local RawLab checkout; working tree is never changed')
    parser.add_argument('--ref', default='origin/development')
    parser.add_argument('--fetch', action='store_true')
    parser.add_argument('--write', action='store_true', help='Replace snapshot after integrity verification')
    parser.add_argument('--check', action='store_true', help='Verify vendored files against lock without network')
    args = parser.parse_args()
    old = json.loads(LOCK.read_text()) if LOCK.exists() else None
    if old:
        verify_snapshot(old)
    if args.check:
        if old is None:
            raise RuntimeError('Missing upstream lock')
        print('Verified RawLab ' + old['commit'][:12] + ': ' + str(len(old['files'])) + ' unmodified files')
        return
    if args.repo is None:
        parser.error('--repo is required to resolve an upstream ref')
    if args.fetch:
        subprocess.run(['git', '-C', str(args.repo), 'fetch', 'origin'], check=True)
    data, lock = snapshot(args.repo, args.ref)
    before = old['files'] if old else {}
    changed = sum(before.get(p) != h for p, h in lock['files'].items())
    removed = len(set(before) - set(lock['files']))
    print('Candidate RawLab ' + lock['commit'][:12] + ': changed/new=' + str(changed) + ', removed=' + str(removed))
    if args.write:
        write_snapshot(data, lock)
        print('Snapshot and lock updated; review ABI, CMake dependencies and color CI before delivery')


if __name__ == '__main__':
    main()
