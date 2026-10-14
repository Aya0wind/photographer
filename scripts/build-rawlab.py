#!/usr/bin/env python3
"""Build and stage the pinned native core for app bundling (project tooling)."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
ROOT=Path(__file__).resolve().parents[1]

def run(*args):
    subprocess.run(list(map(str,args)),cwd=ROOT,check=True)

def stage_builtin_films(destination):
    """Keep the upstream master LUT bytes and their transfer declarations intact."""
    upstream=ROOT/'native/rawlab/upstream'
    recipes=json.loads((upstream/'Shared/master-recipes.json').read_text(encoding='utf-8'))
    films=destination/'films'
    films.mkdir(parents=True,exist_ok=True)
    entries=[]
    for recipe in recipes:
        filename=recipe['lutFileName']
        if Path(filename).name!=filename or not filename.endswith('.cube'):
            raise ValueError('Invalid built-in master LUT filename')
        source=upstream/'RawLabMac/Resources/MasterRecipes'/filename
        data=source.read_bytes()
        header=data.split(b'\n',8)[:8]
        # LUTTransfer::SRGB is 5 in the pinned native core. Refuse a silently
        # changed contract rather than publishing incorrect catalog metadata.
        if b'#Gamma:sRGB to sRGB' not in header or b'LUT_3D_SIZE 65' not in header:
            raise ValueError('Built-in master LUT transfer or size changed')
        identifier=hashlib.sha256(data).hexdigest()
        shutil.copy2(source,films/(identifier+'.cube'))
        entries.append({'id':identifier,'name':recipe['name'],'size':65,
                        'inputTransfer':5,'outputTransfer':5,'builtin':True,
                        'subtitle':recipe['subtitle'],'summary':recipe['summary']})
    if len({entry['id'] for entry in entries})!=len(entries):
        raise ValueError('Duplicate built-in master LUT content')
    (films/'catalog.json').write_text(json.dumps(entries,ensure_ascii=False,indent=2),encoding='utf-8')
    shutil.copy2(upstream/'Shared/master-recipes.json',films/'master-recipes.json')

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--toolchain',type=Path)
    parser.add_argument('--triplet',default='x64-windows-static-md')
    parser.add_argument('--arch',default='arm64')
    parser.add_argument('--skip-build',action='store_true',help='Stage an already built or exactly cache-matched native core')
    args=parser.parse_args()
    run(sys.executable,'scripts/update-rawlab.py','--check')
    build=ROOT/'native/rawlab/build-app'
    configure=['cmake','-S','native/rawlab','-B',build,'-DCMAKE_BUILD_TYPE=Release','-DBUILD_TESTING=OFF']
    if args.toolchain:configure += ['-DCMAKE_TOOLCHAIN_FILE='+str(args.toolchain.resolve()),'-DVCPKG_TARGET_TRIPLET='+args.triplet]
    if sys.platform=='darwin':configure += ['-DCMAKE_OSX_ARCHITECTURES='+args.arch]
    if not args.skip_build:
        run(*configure)
        run('cmake','--build',build,'--config','Release','--parallel','2')
    name='photographer_raw.dll' if sys.platform=='win32' else 'libphotographer_raw.dylib' if sys.platform=='darwin' else 'libphotographer_raw.so'
    binary=build/'Release'/name if sys.platform=='win32' else build/name
    # Ninja is single-config and puts Windows binaries at the build root.
    if sys.platform=='win32' and not binary.is_file():binary=build/name
    if not binary.is_file():raise RuntimeError('Native core missing; build it before staging')
    destination=ROOT/'src-tauri/resources/rawlab'
    destination.mkdir(parents=True,exist_ok=True)
    shutil.copy2(binary,destination/name)
    shutil.copy2(ROOT/'native/rawlab/upstream.lock.json',destination/'upstream.lock.json')
    stage_builtin_films(destination)
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
    (destination/'SOURCE.txt').write_text('Corresponding sources: https://github.com/Aya0wind/photographer ; use the app release revision and initialize its pinned RawLab submodule. Native dependency URLs and checksums are in upstream.lock.json and the upstream CMake files.\n',encoding='utf-8')
    print('Staged pinned RawLab core and corresponding sources')
if __name__=='__main__':main()
