#!/usr/bin/env python3
"""Prepare the exact CoreML static archive selected by ort-sys 2.0.0-rc.13.

Print its library directory for ORT_LIB_PATH. Never compile or install system packages.
"""
import argparse
import hashlib
import lzma
from pathlib import Path
import re
import shutil
import sys
import tarfile
import tempfile
import urllib.request


# Copied from ort-sys 2.0.0-rc.13/build/download/dist.tsv; update with the crate.
URL = "https://cdn.pyke.io/0/pyke:ort-rs/ms@1.28.0/aarch64-apple-darwin+coreml.tar.lzma2"
SHA256 = "6934874e2e953576d9c1db47ff1af39c62c4f4220dbe6f988e131f72879674c7"


def verified_archive(cache: Path) -> Path:
    archive = cache / "coreml.tar.lzma2"
    if archive.is_file() and hashlib.sha256(archive.read_bytes()).hexdigest() == SHA256:
        return archive
    print("Downloading pinned ONNX Runtime CoreML archive", file=sys.stderr)
    with tempfile.NamedTemporaryFile(dir=cache, delete=False) as target:
        temporary = Path(target.name)
        try:
            with urllib.request.urlopen(URL, timeout=60) as source:
                shutil.copyfileobj(source, target)
            target.flush()
            if hashlib.sha256(temporary.read_bytes()).hexdigest() != SHA256:
                raise RuntimeError("ONNX Runtime archive checksum mismatch")
        except BaseException:
            temporary.unlink(missing_ok=True)
            raise
    temporary.replace(archive)
    return archive


def extract_library(archive: Path, output: Path) -> None:
    # This is a raw LZMA2 stream (64 MiB dictionary), not an .xz container.
    with lzma.LZMAFile(archive, format=lzma.FORMAT_RAW,
                       filters=[{"id": lzma.FILTER_LZMA2, "dict_size": 1 << 26}]) as source:
        with tarfile.open(fileobj=source, mode="r|") as entries:
            for entry in entries:
                # The pinned distribution contains one monolithic static library.
                if entry.name != "libonnxruntime.a" or not entry.isfile():
                    raise RuntimeError(f"Unexpected ONNX Runtime archive entry: {entry.name}")
                contents = entries.extractfile(entry)
                if contents is None:
                    raise RuntimeError("ONNX Runtime static library is unreadable")
                with contents, output.open("wb") as target:
                    shutil.copyfileobj(contents, target)
    if not output.is_file() or output.stat().st_size < 8:
        raise RuntimeError("ONNX Runtime static library is missing or empty")
    with output.open("rb") as library:
        if library.read(8) != b"!<arch>\n":
            raise RuntimeError("ONNX Runtime static library has an invalid archive header")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("cache", type=Path)
    args = parser.parse_args()
    lock = Path(__file__).resolve().parent.parent / "src-tauri" / "Cargo.lock"
    if not re.search(r'name = "ort-sys"\nversion = "2\.0\.0-rc\.13"\n', lock.read_text()):
        raise RuntimeError("Update the pinned CoreML archive for the new ort-sys version")
    cache = args.cache.resolve() / SHA256
    cache.mkdir(parents=True, exist_ok=True)
    archive = verified_archive(cache)
    # Always extract anew: a stale/partial library never passes via a cache hit.
    with tempfile.TemporaryDirectory(dir=cache) as staging:
        library = Path(staging) / "libonnxruntime.a"
        extract_library(archive, library)
        library.replace(cache / library.name)
    print(cache)


if __name__ == "__main__":
    main()
