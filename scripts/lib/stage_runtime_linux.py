#!/usr/bin/env python3
"""Bundle and validate PostgreSQL's Linux ELF dependencies. See ADR 0577."""
import argparse
import os
import re
import shutil
import struct
import subprocess
import sys
from pathlib import Path


class BundleError(RuntimeError):
    pass


def run(*args):
    try:
        return subprocess.check_output(args, text=True, stderr=subprocess.PIPE,
                                       env={**os.environ, "LC_ALL": "C"})
    except (OSError, subprocess.CalledProcessError) as error:
        detail = getattr(error, "stderr", "")
        raise BundleError(f"{' '.join(map(str, args))}: {detail or error}") from error


def is_glibc(name):
    return name in {
        "libc.so.6", "libm.so.6", "libpthread.so.0", "libdl.so.2", "librt.so.1",
        "libresolv.so.2", "libutil.so.1", "libanl.so.1", "libmvec.so.1",
        "ld-linux-x86-64.so.2", "ld-linux-aarch64.so.1",
    }


def elf_identity(path):
    try:
        with path.open("rb") as stream:
            header = stream.read(20)
    except OSError as error:
        raise BundleError(f"Cannot read bundle ELF {path}: {error}") from error
    if len(header) < 20 or header[:4] != b"\x7fELF":
        return None
    endian = "<" if header[5] == 1 else ">"
    kind, machine = struct.unpack(endian + "HH", header[16:20])
    if kind not in (2, 3):
        return None
    return header[4], header[5], machine


def objects(root):
    result = []
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            if not path.resolve().is_relative_to(root):
                raise BundleError(f"Symlink leaves PostgreSQL bundle: {path}")
            if not path.exists():
                raise BundleError(f"Broken symlink in PostgreSQL bundle: {path}")
        elif path.is_file() and elf_identity(path):
            result.append(path)
    if root / "bin/postgres" not in result:
        raise BundleError(f"Missing ELF bin/postgres in {root}")
    return result


def inspect(path):
    dynamic = run("readelf", "--wide", "--dynamic", str(path))
    needed = re.findall(r"\(NEEDED\).*?\[([^\]]+)\]", dynamic)
    paths = re.findall(r"\((?:RUNPATH|RPATH)\).*?\[([^\]]*)\]", dynamic)
    versions = run("readelf", "--wide", "--version-info", str(path))
    return {
        "needed": needed,
        "runpath": ":".join(paths),
        "versions": [tuple(map(int, version.split(".")))
                     for version in re.findall(r"\bGLIBC_(\d+(?:\.\d+)+)\b", versions)],
    }


def origin_path(path, root):
    relative = os.path.relpath(root / "lib", path.parent)
    return "$ORIGIN" if relative == "." else "$ORIGIN/" + relative


def validate_object(path, identity):
    if elf_identity(path) != identity:
        raise BundleError(f"Wrong ELF architecture in {path}")
    if is_glibc(path.name):
        raise BundleError(f"Do not bundle glibc: {path}")
    metadata = inspect(path)
    if any(version > (2, 35) for version in metadata["versions"]):
        raise BundleError(f"{path} exceeds the glibc 2.35 baseline")
    for name in metadata["needed"]:
        if "/" in name or not name:
            raise BundleError(f"Invalid dependency SONAME {name!r} in {path}")
    return metadata


def check(root):
    root = root.resolve()
    identity = elf_identity(root / "bin/postgres")
    for path in objects(root):
        metadata = validate_object(path, identity)
        if metadata["needed"] and metadata["runpath"] != origin_path(path, root):
            raise BundleError(f"Non-relocatable library search path in {path}: {metadata['runpath']!r}")
        for name in metadata["needed"]:
            if is_glibc(name):
                continue
            dependency = root / "lib" / name
            if not dependency.resolve().is_relative_to(root) or not dependency.is_file():
                raise BundleError(f"Missing bundle dependency {name} needed by {path}")
            if elf_identity(dependency) != identity:
                raise BundleError(f"Invalid ELF architecture for {dependency} needed by {path}")
    print(f"PostgreSQL ELF closure verified: {root}", file=sys.stderr)


def system_libraries():
    ldconfig = shutil.which("ldconfig") or "/sbin/ldconfig"
    result = {}
    for line in run(ldconfig, "-p").splitlines():
        match = re.match(r"\s*(\S+)\s+\([^)]*\)\s+=>\s+(\S+)", line)
        if match:
            result.setdefault(match[1], []).append(Path(match[2]))
    return result


def copy_license(source, root):
    candidates = [source, source.resolve()]
    for candidate in list(candidates):
        if str(candidate).startswith("/usr/"):
            candidates.append(Path(str(candidate)[4:]))
        else:
            candidates.append(Path("/usr" + str(candidate)))
    for candidate in candidates:
        try:
            owner = run("dpkg-query", "-S", str(candidate)).splitlines()[0].split(": ")[0]
        except BundleError:
            continue
        package = owner.split(":")[0]
        notice = Path("/usr/share/doc") / package / "copyright"
        if notice.is_file():
            destination = root / "share/licenses" / package
            destination.mkdir(parents=True, exist_ok=True)
            shutil.copy2(notice, destination / "copyright")
            return
    raise BundleError(f"No Debian copyright notice found for {source}; use the Ubuntu build baseline")


def bundle(root):
    root = root.resolve()
    identity = elf_identity(root / "bin/postgres")
    pending = objects(root)
    libraries = system_libraries()
    seen = set()
    while pending:
        path = pending.pop()
        if path in seen:
            continue
        seen.add(path)
        metadata = validate_object(path, identity)
        for name in metadata["needed"]:
            if is_glibc(name):
                continue
            destination = root / "lib" / name
            if not destination.exists():
                source = next((candidate for candidate in libraries.get(name, [])
                               if elf_identity(candidate) == identity), None)
                if source is None:
                    raise BundleError(f"Missing build-host dependency {name} needed by {path}; "
                                      "install the Linux PostgreSQL build dependencies")
                copy_license(source, root)
                shutil.copy2(source.resolve(), destination)
            if not destination.resolve().is_relative_to(root):
                raise BundleError(f"Dependency leaves PostgreSQL bundle: {destination}")
            pending.append(destination.resolve())
        run("patchelf", "--set-rpath", origin_path(path, root), str(path))
    check(root)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["bundle", "check"])
    parser.add_argument("prefix", type=Path)
    args = parser.parse_args()
    try:
        {"bundle": bundle, "check": check}[args.action](args.prefix)
    except BundleError as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
