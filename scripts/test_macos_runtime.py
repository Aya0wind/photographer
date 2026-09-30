"""Real Mach-O fixtures: run with python3 -m unittest discover -s scripts -p test_macos_runtime.py."""
import contextlib
import io
from pathlib import Path
import platform
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from macos_runtime import assemble, audit, dependencies, rpaths, run, validate_release_signature


class ReleaseSignatureTests(unittest.TestCase):
    def test_release_identity_team_and_timestamp_are_all_required(self):
        valid = "\n".join([
            "Authority=Developer ID Application: Photo Hub (ABCDE12345)",
            "TeamIdentifier=ABCDE12345",
            "Timestamp=Sep 30, 2026 at 00:00:00",
        ])
        validate_release_signature(valid, "ABCDE12345")
        for invalid in (
            valid.replace("Authority=Developer ID Application:", "Authority=Apple Development:"),
            valid.replace("TeamIdentifier=ABCDE12345", "TeamIdentifier=ZZZZZ99999"),
            valid.replace("Timestamp=Sep 30, 2026 at 00:00:00", "Signed Time=local clock"),
            valid.replace("Timestamp=Sep 30, 2026 at 00:00:00", "Timestamp=none"),
        ):
            with self.subTest(details=invalid), self.assertRaises(ValueError):
                validate_release_signature(invalid, "ABCDE12345")

    def test_release_parameters_cannot_silently_fall_back_to_adhoc(self):
        with self.assertRaises(ValueError):
            assemble("unused", "unused", "-", "ABCDE12345")
        with self.assertRaises(ValueError):
            assemble("unused", "unused", team_id="ABCDE12345")


@unittest.skipUnless(platform.system() == "Darwin" and platform.machine() == "arm64",
                     "requires Apple Silicon and Xcode Command Line Tools")
class RuntimeTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="photo runtime test ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.prefix = self.root / "prefix"
        self.output = self.root / "runtime"
        self.leaf = self.library(self.root / "deps" / "leaf.dylib", "leaf", [])
        self.middle = self.library(self.root / "deps" / "middle.dylib", "middle", [self.leaf])
        self.port = self.library(self.prefix / "lib/libgphoto2_port.12.dylib",
                                 "port", [self.middle])
        self.main = self.library(self.prefix / "lib/libgphoto2.6.dylib", "main_lib", [self.port])
        self.plugin = self.library(self.prefix / "lib/libgphoto2/2.5/ptp2.so",
                                   "ptp", [self.main, self.middle])
        self.library(self.prefix / "lib/libgphoto2_port/0.12/usb1.so", "usb", [self.port])

    def library(self, path, symbol, deps):
        path.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run([
            "/usr/bin/xcrun", "clang", "-arch", "arm64", "-dynamiclib",
            "-Wl,-headerpad_max_install_names",
            "-install_name", str(path), *map(str, deps), "-x", "c", "-",
            "-o", str(path),
        ], input=f"int {symbol}(void) {{ return 1; }}\n", text=True,
            check=True)
        return path

    def build(self):
        with contextlib.redirect_stdout(io.StringIO()):
            assemble(self.prefix, self.output)

    def test_transitive_closure_and_relocation_load(self):
        self.build()
        self.assertEqual(audit(self.output), 6)
        self.assertIn("@loader_path/../middle.dylib",
                      dependencies(self.output / "camlibs/ptp2.so"))
        # Prove dlopen resolves from a new path with the original prefix absent.
        relocated = self.root / "relocated"
        self.output.rename(relocated)
        self.prefix.rename(self.root / "hidden-prefix")
        self.leaf.parent.rename(self.root / "hidden-deps")
        subprocess.run([
            "/usr/bin/python3", "-c",
            "import ctypes,sys; ctypes.CDLL(sys.argv[1]); ctypes.CDLL(sys.argv[2])",
            str(relocated / "libgphoto2.6.dylib"), str(relocated / "camlibs/ptp2.so"),
        ], check=True, capture_output=True)

    def test_missing_dependency_preserves_previous_runtime(self):
        self.build()
        before = (self.output / "libgphoto2.6.dylib").read_bytes()
        self.leaf.rename(self.leaf.with_suffix(".missing"))
        with self.assertRaises(FileNotFoundError):
            self.build()
        self.assertEqual((self.output / "libgphoto2.6.dylib").read_bytes(), before)
        self.assertEqual(audit(self.output), 6)

    def test_rpath_dependency_relocated_using_loader_search_path(self):
        run("/usr/bin/install_name_tool", "-change", str(self.leaf),
            "@rpath/leaf.dylib", "-add_rpath", "@loader_path", str(self.middle))
        self.build()
        self.assertIn("@loader_path/leaf.dylib", dependencies(self.output / "middle.dylib"))
        self.assertEqual(rpaths(self.output / "middle.dylib"), [])
        self.leaf.parent.rename(self.root / "hidden-deps")
        subprocess.run(["/usr/bin/python3", "-c",
                        "import ctypes,sys;ctypes.CDLL(sys.argv[1])",
                        str(self.output / "libgphoto2.6.dylib")], check=True, capture_output=True)

    def test_unresolved_rpath_fails_without_guessing_a_sibling(self):
        run("/usr/bin/install_name_tool", "-change", str(self.leaf),
            "@rpath/leaf.dylib", str(self.middle))
        with self.assertRaisesRegex(ValueError, "Unresolved @rpath"):
            self.build()

    def test_audit_rejects_placeholder_in_nonexecutable_plugin(self):
        self.build()
        plugin = self.output / "camlibs/ptp2.so"
        run("/usr/bin/install_name_tool", "-change", "@loader_path/../middle.dylib",
            "@@HOMEBREW_PREFIX@@/opt/middle.dylib", str(plugin))
        run("/usr/bin/codesign", "--force", "--sign", "-", str(plugin))
        plugin.chmod(0o644)
        with self.assertRaisesRegex(ValueError, "Non-relocatable"):
            audit(self.output)

    def test_audit_rejects_missing_transitive_dependency(self):
        self.build()
        (self.output / "leaf.dylib").rename(self.output / "leaf.missing")
        with self.assertRaises(FileNotFoundError):
            audit(self.output)

    def test_release_audit_rejects_real_adhoc_runtime(self):
        self.build()
        self.assertEqual(audit(self.output), 6)
        with self.assertRaisesRegex(ValueError, "Developer ID"):
            audit(self.output, "ABCDE12345")

    def test_release_signing_failure_preserves_previous_runtime(self):
        self.build()
        before = (self.output / "libgphoto2.6.dylib").read_bytes()

        def failing_signer(*args):
            if args[0] == "/usr/bin/codesign" and "--sign" in args:
                raise subprocess.CalledProcessError(1, args, stderr="Signing key unavailable")
            return run(*args)

        with patch("macos_runtime.run", side_effect=failing_signer):
            with contextlib.redirect_stdout(io.StringIO()):
                with self.assertRaises(subprocess.CalledProcessError):
                    assemble(self.prefix, self.output,
                             "Developer ID Application: Photo Hub (ABCDE12345)", "ABCDE12345")
        self.assertEqual((self.output / "libgphoto2.6.dylib").read_bytes(), before)
        self.assertEqual(audit(self.output), 6)

    def test_audit_rejects_incomplete_runtime_before_packaging(self):
        self.build()
        for name in ("libgphoto2.6.dylib", "libgphoto2_port.12.dylib",
                     "camlibs/ptp2.so", "iolibs/usb1.so"):
            with self.subTest(missing=name):
                required = self.output / name
                saved = required.with_suffix(".saved")
                required.rename(saved)
                try:
                    with self.assertRaisesRegex(ValueError, "Missing Mach-O runtime"):
                        audit(self.output)
                finally:
                    saved.rename(required)


if __name__ == "__main__":
    unittest.main()
