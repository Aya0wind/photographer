# RawLab core snapshot

Source repository: https://github.com/dancancer/RawLab.
Fixed commit: `89cdc5b14402ee9265468f54152c8fb55d661248` (`origin/development`, fetched 2026-10-14).

The machine-readable authority is `upstream.lock.json`, including per-file SHA256, a snapshot hash, tracking ref, and LibRaw/CMake dependency URLs and checksums. CI verifies it before building.

The `upstream/` tree is a git-archive snapshot of `lutools/{src,include,cmake,third_party,CMakeLists.txt,docs/color-contract.md}`, `LICENSE`, and `docs/licensing.md`. No LUTs, RAW fixtures, client apps, generated builds or scripts/runtime dependencies are included. No upstream files are modified. Photographer's adapter/build/tests live outside that tree. Update by replacing the full snapshot from a reviewed commit, then verify color/ABI/platform regressions; do not hand-patch vendored algorithms.

Windows compatibility is outside the snapshot: MSVC force-includes `include/windows_rgb_compat.h` for core/bridge. It preloads the Windows SDK and undefines its function-like `RGB` macro so `sony2fuji::RGB(...)` remains a C++ constructor. The shim loads Winsock 2 before windows.h and defines WIN32_LEAN_AND_MEAN to avoid the SDK importing incompatible Winsock 1 declarations. This fixes the observed CI COLORREF conversion and socket redefinition errors without rewriting upstream algorithms.

Update from a separate local RawLab checkout (its branch/working tree are untouched):

```sh
python scripts/update-rawlab.py --repo /path/to/RawLab --fetch
python scripts/update-rawlab.py --repo /path/to/RawLab --ref origin/development --write
python scripts/update-rawlab.py --check
```

The first command reports a candidate without editing Photographer. `--write` refuses if the current snapshot differs from its lock, replaces the complete selected source tree, removes stale files, and updates the lock. Review the resulting diff, public C++ contracts, CMake capability/dependency changes and license notices before committing. Prefer an integrated upstream branch; unrelated unmerged feature branches are never selected merely because their timestamp is newer. Published app recipes/cache keys must include the resolved core commit, not a moving branch name.
