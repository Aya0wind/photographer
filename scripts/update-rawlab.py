#!/usr/bin/env python3
"""Update a pinned RawLab Git submodule; project tooling, never app runtime."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
NATIVE = ROOT / 'native' / 'rawlab'
UPSTREAM = NATIVE / 'upstream'
LOCK = NATIVE / 'upstream.lock.json'
REPOSITORY = 'https://github.com/dancancer/RawLab.git'


def git(repo, *arguments):
    return subprocess.check_output(['git', '-C', str(repo), *arguments]).decode().strip()


def dependencies(commit):
    bootstrap = git(UPSTREAM, 'show', commit + ':lutools/platform/ios/build-framework.sh')
    result = {}
    for key, archive in [('libraw', 'LibRaw-'), ('librawCmake', 'LibRaw-cmake-')]:
        match = re.search(r'fetch (' + re.escape(archive) + r'\S+) ([0-9a-f]{64})\s+\\\s+(https://\S+)', bootstrap)
        if match is None:
            raise RuntimeError('Upstream native dependency layout changed: ' + key)
        result[key] = {'archive': match[1], 'sha256': match[2], 'url': match[3]}
    return result


def verify(lock):
    # A missing submodule must not silently fall through to the parent repo.
    if not (UPSTREAM / '.git').exists():
        raise RuntimeError('Run git submodule update --init --recursive first')
    stage = git(ROOT, 'ls-files', '--stage', '--', 'native/rawlab/upstream')
    if not stage.startswith('160000 '):
        raise RuntimeError('RawLab must be tracked as a Git submodule')
    url = git(ROOT, 'config', '--file', '.gitmodules', '--get', 'submodule.native/rawlab/upstream.url')
    if lock['version'] != 2 or lock['repository'] != REPOSITORY or url != REPOSITORY:
        raise RuntimeError('Unsupported lock or unexpected RawLab repository')
    if git(UPSTREAM, 'status', '--porcelain', '--untracked-files=all'):
        raise RuntimeError('RawLab has local changes; preserve/review them before updating')
    actual = git(UPSTREAM, 'rev-parse', 'HEAD')
    if actual != lock['commit'] or dependencies(actual) != lock['nativeDependencies']:
        raise RuntimeError('RawLab checkout or native dependencies differ from the lock')


def write_lock(lock):
    with tempfile.NamedTemporaryFile(mode='w', encoding='utf-8', dir=NATIVE, suffix='.tmp', delete=False) as file:
        temporary = Path(file.name)
        json.dump(lock, file, indent=2)
        file.write('\n')
    try:
        temporary.replace(LOCK)
    finally:
        temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ref', default='origin/development')
    parser.add_argument('--fetch', action='store_true', help='Fetch the submodule origin before resolving')
    parser.add_argument('--write', action='store_true', help='Detach checkout at candidate and update lock')
    parser.add_argument('--check', action='store_true', help='Verify checked-out commit and dependencies offline')
    args = parser.parse_args()
    if args.check and (args.fetch or args.write):
        parser.error('--check cannot be combined with --fetch or --write')
    old = json.loads(LOCK.read_text(encoding='utf-8'))
    verify(old)
    if args.check:
        print('Verified RawLab submodule ' + old['commit'][:12])
        return
    if args.fetch:
        subprocess.run(['git', '-C', str(UPSTREAM), 'fetch', 'origin'], check=True)
    commit = git(UPSTREAM, 'rev-parse', '--verify', args.ref + '^{commit}')
    lock = {'version': 2, 'repository': REPOSITORY, 'trackingRef': args.ref,
            'commit': commit, 'nativeDependencies': dependencies(commit)}
    print('RawLab candidate: ' + old['commit'][:12] + ' -> ' + commit[:12])
    if not args.write:
        return
    subprocess.run(['git', '-C', str(UPSTREAM), 'checkout', '--detach', commit], check=True)
    try:
        write_lock(lock)
    except BaseException:
        subprocess.run(['git', '-C', str(UPSTREAM), 'checkout', '--detach', old['commit']], check=True)
        raise
    print('Updated submodule and lock; review and commit both together')


if __name__ == '__main__':
    main()
