"""Check cached native staging without downloading or compiling dependencies."""
import contextlib
import importlib.util
import io
import hashlib
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location("build_rawlab", Path(__file__).with_name("build-rawlab.py"))
build_rawlab = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build_rawlab)


class CachedStagingTests(unittest.TestCase):
    def test_cached_binary_keeps_corresponding_sources_and_licenses(self):
        for platform, binary in (("linux", "libphotographer_raw.so"),
                                 ("darwin", "libphotographer_raw.dylib"),
                                 ("win32", "photographer_raw.dll"),
                                 ("win32", "Release/photographer_raw.dll")):
            with self.subTest(platform=platform, binary=binary), tempfile.TemporaryDirectory(prefix="rawlab cache ") as directory:
                root = Path(directory)
                files = {
                    f"native/rawlab/build-app/{binary}": "cached native binary",
                    "native/rawlab/upstream.lock.json": "pinned revision",
                    "native/rawlab/upstream/LICENSE": "RawLab license",
                    "native/rawlab/upstream/docs/licensing.md": "distribution notices",
                    "native/rawlab/upstream/lutools/CMakeLists.txt": "core build",
                    "native/rawlab/CMakeLists.txt": "bridge build",
                    "native/rawlab/src/bridge.cpp": "bridge source",
                    "native/rawlab/cmake/lensfun-macos.cmake": "Lensfun build",
                    "native/rawlab/include/bridge.h": "bridge header",
                    "native/rawlab/build-app/_deps/rawlab_dng_source-src/source.cpp": "DNG SDK source",
                    "vcpkg/scripts/buildsystems/vcpkg.cmake": "toolchain",
                    "vcpkg/installed/x64-windows-static-md/share/zlib/copyright": "zlib license",
                }
                for entry in ("src", "include", "cmake", "third_party", "docs"):
                    files[f"native/rawlab/upstream/lutools/{entry}/fixture.txt"] = "core source"
                for entry in ("COPYRIGHT", "LICENSE.LGPL", "LICENSE.CDDL"):
                    files[f"native/rawlab/build-app/_deps/pc_libraw_source-src/{entry}"] = "LibRaw license"
                for filename, contents in files.items():
                    path = root / filename
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text(contents)
                argv = ["build-rawlab.py", "--skip-build"]
                if platform == "win32":
                    argv += ["--toolchain", str(root / "vcpkg/scripts/buildsystems/vcpkg.cmake")]
                with patch.object(build_rawlab, "ROOT", root), patch.object(build_rawlab, "stage_builtin_films"), patch.object(build_rawlab, "stage_lensfun"), patch.object(build_rawlab, "stage_native_binary",side_effect=lambda binary,destination: __import__("shutil").copy2(binary,destination/binary.name)), patch.object(build_rawlab, "run") as run, \
                        patch.object(build_rawlab.sys, "platform", platform), patch.object(build_rawlab.sys, "argv", argv), \
                        contextlib.redirect_stdout(io.StringIO()):
                    build_rawlab.main()
                # Integrity still runs on hits, but CMake never runs during staging.
                self.assertEqual(len(run.call_args_list), 1)
                self.assertEqual(run.call_args.args[1:], ("scripts/update-rawlab.py", "--check"))
                destination = root / "src-tauri/resources/rawlab"
                self.assertEqual((destination / Path(binary).name).read_text(), "cached native binary")
                self.assertEqual((destination / "licenses/LibRaw-LICENSE.LGPL").read_text(), "LibRaw license")
                if platform == "win32":
                    self.assertEqual((destination / "licenses/zlib-copyright.txt").read_text(), "zlib license")
                with tarfile.open(root / f"native/rawlab/build-app/photographer-rawlab-source-{platform}.tar.gz") as archive:
                    self.assertEqual(archive.extractfile("./rawlab_dng_source-src/source.cpp").read(), b"DNG SDK source")
                    self.assertEqual(archive.extractfile("./bridge/src/bridge.cpp").read(), b"bridge source")
                self.assertFalse((destination / "sources").exists())

    def test_missing_cached_binary_fails_before_staging(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(build_rawlab, "ROOT", Path(directory)), \
                patch.object(build_rawlab, "run"), patch.object(build_rawlab.sys, "argv", ["build-rawlab.py", "--skip-build"]):
            with self.assertRaisesRegex(RuntimeError, "Native core missing"):
                build_rawlab.main()
            self.assertFalse((Path(directory) / "src-tauri/resources/rawlab").exists())

class BuiltinFilmTests(unittest.TestCase):
    def test_master_lut_headers_accept_checkout_line_endings_without_changing_bytes(self):
        for newline in (b"\n", b"\r\n"):
            with self.subTest(newline=newline), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                upstream = root / "native/rawlab/upstream"
                recipe = {"lutFileName": "test.cube", "name": "Test", "subtitle": "", "summary": ""}
                manifest = upstream / "Shared/master-recipes.json"
                manifest.parent.mkdir(parents=True)
                manifest.write_text(json.dumps([recipe]), encoding="utf-8")
                source = upstream / "RawLabMac/Resources/MasterRecipes/test.cube"
                source.parent.mkdir(parents=True)
                data = newline.join([b"#Gamma:sRGB to sRGB", b"LUT_3D_SIZE 65", b"0 0 0", b""])
                source.write_bytes(data)
                destination = root / "staged"
                with patch.object(build_rawlab, "ROOT", root):
                    build_rawlab.stage_builtin_films(destination)
                    catalog = json.loads((destination / "films/catalog.json").read_text(encoding="utf-8"))
                    identifier = hashlib.sha256(data).hexdigest()
                    self.assertEqual(catalog[0]["id"], identifier)
                    self.assertEqual((destination / f"films/{identifier}.cube").read_bytes(), data)
                    for invalid in (data.replace(b"sRGB to sRGB", b"FLog to sRGB"),
                                    data.replace(b"LUT_3D_SIZE 65", b"LUT_3D_SIZE 33")):
                        source.write_bytes(invalid)
                        with self.assertRaisesRegex(ValueError, "transfer or size changed"):
                            build_rawlab.stage_builtin_films(destination)

    def test_all_upstream_master_films_are_staged_byte_for_byte(self):
        upstream = build_rawlab.ROOT / "native/rawlab/upstream"
        recipes = json.loads((upstream / "Shared/master-recipes.json").read_text(encoding="utf-8"))
        with tempfile.TemporaryDirectory(prefix="builtin films ") as directory:
            destination = Path(directory)
            build_rawlab.stage_builtin_films(destination)
            entries = json.loads((destination / "films/catalog.json").read_text(encoding="utf-8"))
            self.assertEqual(len(entries), 14)
            self.assertEqual(len({entry["id"] for entry in entries}), 14)
            for recipe, entry in zip(recipes, entries):
                original = (upstream / "RawLabMac/Resources/MasterRecipes" / recipe["lutFileName"]).read_bytes()
                self.assertEqual(entry["id"], hashlib.sha256(original).hexdigest())
                self.assertEqual((destination / "films" / (entry["id"] + ".cube")).read_bytes(), original)
                self.assertEqual(entry["name"], recipe["name"])
                self.assertTrue(entry["builtin"])
                self.assertEqual((entry["size"], entry["inputTransfer"], entry["outputTransfer"]), (65, 5, 5))
            self.assertEqual((destination / "films/master-recipes.json").read_bytes(), (upstream / "Shared/master-recipes.json").read_bytes())


if __name__ == "__main__":
    unittest.main()
