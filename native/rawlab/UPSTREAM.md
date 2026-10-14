# RawLab core submodule

Upstream repository: https://github.com/dancancer/RawLab.
Pinned commit: `89cdc5b14402ee9265468f54152c8fb55d661248` (`development`, selected 2026-10-14).

`native/rawlab/upstream` is a Git submodule. The parent repository's gitlink pins the complete upstream repository to an exact commit; `.gitmodules` declares its GitHub URL and the intended update branch. Builds never advance that branch automatically. `upstream.lock.json` records the same commit plus LibRaw/CMake download URLs and SHA256 values. CI recursively checks out the submodule and verifies its revision, clean working tree and dependency declarations before building.

Only the `lutools` core target is linked. Having upstream clients, CLI tools or assets in the submodule does not enable or bundle them. Photographer's adapter, build configuration, Windows compatibility header and tests remain outside the submodule. Do not edit upstream algorithms inside this checkout; upstream changes belong in the RawLab repository and are consumed by advancing the parent gitlink.

## Clone and initialize

```sh
git clone --recurse-submodules https://github.com/Aya0wind/photographer.git
# Existing Photographer checkout:
git submodule update --init --recursive
python scripts/update-rawlab.py --check
```

After switching branches or pulling a parent commit, run `git submodule update --init --recursive` to restore the recorded upstream revision. Do not use `git submodule update --remote` during builds.

## Review and update

```sh
# Fetch and report the candidate without changing the checked-out revision:
python scripts/update-rawlab.py --fetch
# After reviewing the upstream diff, select an integrated revision explicitly:
python scripts/update-rawlab.py --ref origin/development --write
# Or use an exact upstream commit instead of the tracking branch.
python scripts/update-rawlab.py --check
git diff --submodule=log -- native/rawlab/upstream native/rawlab/upstream.lock.json
git add native/rawlab/upstream native/rawlab/upstream.lock.json
# Commit both together, with any required adapter changes and CI validation.
```

The updater refuses local modifications, detaches the submodule at the selected commit, and updates the dependency lock atomically. If lock publication fails, it restores the previous checkout. Review C++ contracts, color semantics, CMake dependencies and licensing before delivery; the updater does not establish their compatibility. Published recipes/cache keys use the pinned commit rather than the moving branch name.

## Windows compatibility

MSVC force-includes `include/windows_rgb_compat.h` for core/bridge. It loads Winsock 2 before `windows.h`, defines `WIN32_LEAN_AND_MEAN`, and removes the SDK's function-like `RGB` macro so `sony2fuji::RGB(...)` remains a C++ constructor. Both observed CI failures (COLORREF conversion and Winsock type redefinitions) are addressed outside the upstream submodule.
