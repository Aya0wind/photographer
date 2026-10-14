# RawLab core snapshot

Source repository: https://github.com/dancancer/RawLab.
Fixed commit: `dcecbf82189360288085ee2d8017ed44f1eef919`.

The `upstream/` tree is a git-archive snapshot of `lutools/{src,include,cmake,third_party,CMakeLists.txt,docs/color-contract.md}`, `LICENSE`, and `docs/licensing.md`. No LUTs, RAW fixtures, client apps, generated builds or scripts/runtime dependencies are included. No upstream files are modified. Photographer's adapter/build/tests live outside that tree. Update by replacing the full snapshot from a reviewed commit, then verify color/ABI/platform regressions; do not hand-patch vendored algorithms.

Windows compatibility is outside the snapshot: MSVC force-includes `include/windows_rgb_compat.h` for core/bridge. It preloads the Windows SDK and undefines its function-like `RGB` macro so `sony2fuji::RGB(...)` remains a C++ constructor. This fixes the observed CI COLORREF conversion error without rewriting upstream algorithms.
