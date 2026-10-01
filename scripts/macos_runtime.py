"""Stage and audit a relocatable gphoto Mach-O dependency closure.

Failed staging never overwrites the previous runtime. Existing outputs are
retained as backups. License/source-distribution review remains a release gate.
"""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import uuid


def run(*args):
    return subprocess.check_output(args, text=True, stderr=subprocess.PIPE).strip()


def is_macho(path):
    return path.is_file() and "Mach-O" in run("/usr/bin/file", "-b", str(path))


def dependencies(path):
    return [
        line.strip().split(" (compatibility version", 1)[0]
        for line in run("/usr/bin/otool", "-L", str(path)).splitlines()[1:]
    ]


def library_id(path):
    lines = run("/usr/bin/otool", "-D", str(path)).splitlines()
    return lines[1].strip() if len(lines) > 1 else None


def rpaths(path):
    lines = run("/usr/bin/otool", "-l", str(path)).splitlines()
    result = []
    for index, line in enumerate(lines):
        if line.strip() == "cmd LC_RPATH":
            value = lines[index + 2].strip()
            if not value.startswith("path ") or " (offset " not in value:
                raise ValueError(f"Invalid LC_RPATH in {path}")
            result.append(value[5:].rsplit(" (offset ", 1)[0])
    return result


def system_library(value):
    return value.startswith(("/usr/lib/", "/System/Library/"))


def validate_release_signature(details, team_id):
    """Additional release policy, after codesign has verified signature integrity."""
    if not team_id or not team_id.isascii() or not team_id.isalnum():
        raise ValueError("A valid expected Apple Team ID is required")
    lines = details.splitlines()
    if not any(line.startswith("Authority=Developer ID Application:") for line in lines):
        raise ValueError("Developer ID Application signature required (not ad-hoc)")
    if f"TeamIdentifier={team_id}" not in lines:
        raise ValueError("Signing team does not match the expected Apple Team ID")
    if not any(line.startswith("Timestamp=") and line != "Timestamp=none" for line in lines):
        raise ValueError("Secure signing timestamp required")


def audit_release_signature(path, team_id):
    run("/usr/bin/codesign", "--verify", "--strict", str(path))
    details = subprocess.run(
        ["/usr/bin/codesign", "--display", "--verbose=4", str(path)],
        capture_output=True, text=True, check=True,
    )
    validate_release_signature(details.stdout + details.stderr, team_id)


def audit(root, team_id=None):
    root = Path(root).resolve(strict=True)
    for name in ("libgphoto2.6.dylib", "libgphoto2_port.12.dylib",
                 "camlibs/ptp2.so", "iolibs/usb1.so"):
        if not is_macho(root / name):
            raise ValueError(f"Missing Mach-O runtime: {name}")
    count = 0
    for path in root.rglob("*"):
        if not is_macho(path):
            continue
        count += 1
        if run("/usr/bin/lipo", "-archs", str(path)) != "arm64":
            raise ValueError(f"Not arm64-only: {path}")
        for dep in dependencies(path):
            if system_library(dep):
                continue
            if not dep.startswith("@loader_path/"):
                raise ValueError(f"Non-relocatable dependency in {path}: {dep}")
            target = (path.parent / dep.removeprefix("@loader_path/")).resolve(strict=True)
            if not target.is_relative_to(root) or not is_macho(target):
                raise ValueError(f"Dependency escapes runtime or is invalid: {dep}")
        run("/usr/bin/codesign", "--verify", "--strict", str(path))
        if team_id is not None:
            audit_release_signature(path, team_id)
    return count


def assemble(prefix, output, signing_identity=None, team_id=None):
    if signing_identity is not None and (
        not signing_identity.startswith("Developer ID Application:") or not team_id
    ):
        raise ValueError("Release assembly requires Developer ID Application and Apple Team ID")
    if team_id is not None and signing_identity is None:
        raise ValueError("Apple Team ID requires an explicit signing identity")
    prefix = Path(prefix).resolve(strict=True)
    output = Path(output).absolute()
    if output.is_symlink():
        raise ValueError("Output must not be a symlink")
    output.parent.mkdir(parents=True, exist_ok=True)
    brew = Path(os.environ.get("BREW_PREFIX", "/opt/homebrew"))
    stage = Path(tempfile.mkdtemp(prefix=".gphoto-stage-", dir=output.parent))
    queue, sources, notices = [], {}, set()

    def enqueue(source, relative):
        source = source.resolve(strict=True)
        target = stage / relative
        previous = sources.get(relative)
        if previous:
            if previous != source and previous.read_bytes() != source.read_bytes():
                raise ValueError(f"Conflicting dependency basename: {relative}")
            return target
        if not is_macho(source):
            raise ValueError(f"Not a Mach-O library: {source}")
        sources[relative] = source
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)
        target.chmod(0o755)
        queue.append((source, target))
        for parent in source.parents:
            if parent == prefix or parent.parent.parent == brew / "Cellar":
                for pattern in ("COPYING*", "LICENSE*", "AUTHORS", "INSTALL_RECEIPT.json", "sbom.spdx.json"):
                    for notice in parent.glob(pattern):
                        if notice.is_file():
                            notices.add(notice)
                break
        return target

    def expand_path(dep, source):
        if dep.startswith("@@HOMEBREW_PREFIX@@/"):
            dep = str(brew / dep.removeprefix("@@HOMEBREW_PREFIX@@/"))
        elif dep.startswith("@@HOMEBREW_CELLAR@@/"):
            dep = str(brew / "Cellar" / dep.removeprefix("@@HOMEBREW_CELLAR@@/"))
        if dep == "@loader_path":
            return source.parent
        if dep.startswith("@loader_path/"):
            return source.parent / dep.removeprefix("@loader_path/")
        if dep.startswith("@"):
            raise ValueError(f"Unresolved install name in {source}: {dep}")
        if not Path(dep).is_absolute():
            raise ValueError(f"Relative install name in {source}: {dep}")
        return Path(dep)

    def resolve(dep, source):
        if Path(dep).name in ("libgphoto2.6.dylib", "libgphoto2_port.12.dylib"):
            return prefix / "lib" / Path(dep).name
        if dep.startswith("@rpath/"):
            for value in rpaths(source):
                candidate = expand_path(value, source) / dep.removeprefix("@rpath/")
                if candidate.is_file():
                    return candidate
            raise ValueError(f"Unresolved @rpath dependency in {source}: {dep}")
        return expand_path(dep, source)

    try:
        for name in ("libgphoto2.6.dylib", "libgphoto2_port.12.dylib"):
            enqueue(prefix / "lib" / name, name)
        for folder, marker, destination in (
            ("libgphoto2", "ptp2.so", "camlibs"),
            ("libgphoto2_port", "usb1.so", "iolibs"),
        ):
            hits = sorted((prefix / "lib" / folder).rglob(marker))
            if len(hits) != 1:
                raise ValueError(f"Expected one {marker}, found {hits}")
            for plugin in sorted(hits[0].parent.iterdir()):
                if is_macho(plugin):
                    enqueue(plugin, f"{destination}/{plugin.name}")
        for source, target in queue:  # enqueue appends transitive dependencies
            identity = library_id(source)
            if identity:
                run("/usr/bin/install_name_tool", "-id", f"@loader_path/{target.name}", str(target))
            for dep in dependencies(source):
                if dep == identity or system_library(dep):
                    continue
                resolved = resolve(dep, source)
                bundled = enqueue(resolved, resolved.name)
                replacement = "@loader_path/" + os.path.relpath(bundled, target.parent)
                run("/usr/bin/install_name_tool", "-change", dep, replacement, str(target))
            # All dependencies are explicit loader-relative paths now. Source
            # rpaths are unnecessary and may leak the build machine prefix.
            for value in dict.fromkeys(rpaths(source)):
                run("/usr/bin/install_name_tool", "-delete_rpath", value, str(target))
        for _, target in queue:
            if signing_identity:
                run("/usr/bin/codesign", "--force", "--sign", signing_identity,
                    "--timestamp", str(target))
            else:
                run("/usr/bin/codesign", "--force", "--sign", "-", str(target))
        count = audit(stage, team_id)
        for notice in notices:
            destination = stage / "licenses" / notice.parent.parent.name / notice.parent.name
            destination.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(notice, destination / notice.name)
        (stage / ".gitkeep").touch()
        backup = None
        if output.exists():
            backup = output.with_name(f".gphoto-backup-{uuid.uuid4().hex}")
            output.rename(backup)
        try:
            stage.rename(output)
        except BaseException:
            if backup:
                backup.rename(output)
            raise
        print(f"Verified {count} arm64 Mach-O files: {output}")
        if backup:
            print(f"Previous runtime retained: {backup}")
    except BaseException:
        print(f"Assembly failed; previous runtime untouched. Staging retained: {stage}")
        raise


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=["assemble", "audit"])
    parser.add_argument("path")
    parser.add_argument("output", nargs="?")
    parser.add_argument("--signing-identity")
    parser.add_argument("--team-id")
    args = parser.parse_args()
    if args.action == "assemble":
        if not args.output:
            parser.error("assemble requires an output path")
        assemble(args.path, args.output, args.signing_identity, args.team_id)
    else:
        print(f"Verified {audit(args.path, args.team_id)} Mach-O files")
