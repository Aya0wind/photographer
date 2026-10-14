#!/usr/bin/env python3
"""Build and stage the pinned native core for app bundling (project tooling)."""
import argparse
from pathlib import Path
import shutil
import subprocess
import sys
ROOT=Path(__file__).resolve().parents[1]

def run(*args):
    subprocess.run(list(map(str,args)),cwd=ROOT,check=True)

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--toolchain',type=Path)
    parser.add_argument('--triplet',default='x64-windows-static-md')
    parser.add_argument('--arch',default='arm64')
    args=parser.parse_args()
    run(sys.executable,'scripts/update-rawlab.py','--check')
    build=ROOT/'native/rawlab/build-app'
    configure=['cmake','-S','native/rawlab','-B',build,'-DCMAKE_BUILD_TYPE=Release','-DBUILD_TESTING=OFF']
    if args.toolchain:configure += ['-DCMAKE_TOOLCHAIN_FILE='+str(args.toolchain.resolve()),'-DVCPKG_TARGET_TRIPLET='+args.triplet]
    if sys.platform=='darwin':configure += ['-DCMAKE_OSX_ARCHITECTURES='+args.arch]
    run(*configure)
    run('cmake','--build',build,'--config','Release','--parallel','2')
    name='photographer_raw.dll' if sys.platform=='win32' else 'libphotographer_raw.dylib' if sys.platform=='darwin' else 'libphotographer_raw.so'
    binary=build/'Release'/name if sys.platform=='win32' else build/name
    destination=ROOT/'src-tauri/resources/rawlab'
    destination.mkdir(parents=True,exist_ok=True)
    shutil.copy2(binary,destination/name)
    shutil.copy2(ROOT/'native/rawlab/upstream.lock.json',destination/'upstream.lock.json')
    # Bundle corresponding source and original licenses for the static dependencies.
    source=destination/'sources'
    if source.exists():shutil.rmtree(source)
    source.mkdir()
    core=source/'RawLab/lutools'
    core.mkdir(parents=True)
    for entry in ['src','include','cmake','third_party','docs']:
        shutil.copytree(ROOT/'native/rawlab/upstream/lutools'/entry,core/entry)
    shutil.copy2(ROOT/'native/rawlab/upstream/lutools/CMakeLists.txt',core/'CMakeLists.txt')
    shutil.copy2(ROOT/'native/rawlab/upstream/LICENSE',source/'RawLab/LICENSE')
    shutil.copytree(ROOT/'native/rawlab/upstream/docs',source/'RawLab/docs')
    for directory in build.joinpath('_deps').glob('*-src'):
        shutil.copytree(directory,source/directory.name,ignore=shutil.ignore_patterns('.git','__pycache__'))
    shutil.copytree(ROOT/'native/rawlab/src',source/'bridge/src')
    shutil.copytree(ROOT/'native/rawlab/include',source/'bridge/include')
    shutil.copy2(ROOT/'native/rawlab/CMakeLists.txt',source/'bridge/CMakeLists.txt')
    shutil.copy2(ROOT/'native/rawlab/upstream.lock.json',source/'bridge/upstream.lock.json')
    # Ship a compact source archive rather than thousands of loose bundle files.
    shutil.make_archive(str(build/('photographer-rawlab-source-'+sys.platform)),'gztar',source)
    shutil.rmtree(source)
    licenses=destination/'licenses'
    shutil.copytree(ROOT/'native/rawlab/upstream/lutools/third_party',licenses,ignore=shutil.ignore_patterns('*.xml','*.inc','*.hpp','*.h','*.cpp','*.c'),dirs_exist_ok=True)
    shutil.copy2(ROOT/'native/rawlab/upstream/LICENSE',licenses/'RawLab-GPL-3.0.txt')
    shutil.copy2(ROOT/'native/rawlab/upstream/docs/licensing.md',licenses/'RawLab-licensing.md')
    for entry in ['COPYRIGHT','LICENSE.LGPL','LICENSE.CDDL']:
        shutil.copy2(build/'_deps/pc_libraw_source-src'/entry,licenses/('LibRaw-'+entry))
    if args.toolchain:
        zlib_license=args.toolchain.resolve().parents[2]/'installed'/args.triplet/'share/zlib/copyright'
        shutil.copy2(zlib_license,licenses/'zlib-copyright.txt')
    (destination/'SOURCE.txt').write_text('Corresponding sources: https://github.com/Aya0wind/photographer/releases (photographer-rawlab-source archives). App sources: https://github.com/Aya0wind/photographer ; initialize its pinned RawLab submodule.\n',encoding='utf-8')
    print('Staged pinned RawLab core and corresponding sources')
if __name__=='__main__':main()
