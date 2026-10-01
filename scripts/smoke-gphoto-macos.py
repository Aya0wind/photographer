"""Load the packaged runtime and driver tables without opening a camera.

Run in its own process: libgphoto2/ltdl cache their environment on first use.
This proves native loading, not actual USB capture or device compatibility.
"""
import ctypes
import os
from pathlib import Path
import sys

from macos_runtime import audit, is_macho


def check(code, operation):
    if code < 0:
        raise RuntimeError(f"{operation}: libgphoto2 error {code}")
    return code


def function(library, name, args, result=ctypes.c_int):
    fn = getattr(library, name)
    fn.argtypes = args
    fn.restype = result
    return fn


def smoke(root):
    root = Path(root).resolve(strict=True)
    audit(root)
    os.environ["CAMLIBS"] = str(root / "camlibs")
    os.environ["IOLIBS"] = str(root / "iolibs")
    port = ctypes.CDLL(str(root / "libgphoto2_port.12.dylib"))
    camera = ctypes.CDLL(str(root / "libgphoto2.6.dylib"))
    # Retain handles until exit: loading only ptp2 would miss broken dependencies
    # in less common camera drivers included in the same distribution.
    plugins = [ctypes.CDLL(str(path)) for folder in ("camlibs", "iolibs")
               for path in sorted((root / folder).iterdir()) if is_macho(path)]
    ptr = ctypes.c_void_p
    context_new = function(camera, "gp_context_new", [], ptr)
    context_free = function(camera, "gp_context_unref", [ptr], None)
    abilities_new = function(camera, "gp_abilities_list_new", [ctypes.POINTER(ptr)])
    abilities_free = function(camera, "gp_abilities_list_free", [ptr])
    abilities_load = function(camera, "gp_abilities_list_load", [ptr, ptr])
    abilities_count = function(camera, "gp_abilities_list_count", [ptr])
    ports_new = function(port, "gp_port_info_list_new", [ctypes.POINTER(ptr)])
    ports_free = function(port, "gp_port_info_list_free", [ptr])
    ports_load = function(port, "gp_port_info_list_load", [ptr])
    ports_count = function(port, "gp_port_info_list_count", [ptr])
    context = context_new()
    if not context:
        raise RuntimeError("gp_context_new returned null")
    abilities, ports = ptr(), ptr()
    try:
        check(abilities_new(ctypes.byref(abilities)), "abilities allocation")
        check(abilities_load(abilities, context), "camera driver table loading")
        models = check(abilities_count(abilities), "camera model count")
        if not models:
            raise RuntimeError("No camera models loaded")
        check(ports_new(ctypes.byref(ports)), "port allocation")
        check(ports_load(ports), "port driver table loading")
        count = check(ports_count(ports), "port count")
        if not count:
            raise RuntimeError("No port types loaded")
        print(f"Loaded {len(plugins)} plugins; {models} camera models; {count} port entries")
    finally:
        if ports:
            ports_free(ports)
        if abilities:
            abilities_free(abilities)
        context_free(context)


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: smoke-gphoto-macos.py /absolute/path/to/gphoto")
    smoke(sys.argv[1])
