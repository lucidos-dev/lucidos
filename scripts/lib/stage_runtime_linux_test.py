#!/usr/bin/env python3
"""Offline regression tests for the Linux PostgreSQL dependency closure."""
import importlib.util
import shutil
import struct
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.dont_write_bytecode = True

SPEC = importlib.util.spec_from_file_location(
    "stage_runtime_linux", Path(__file__).with_name("stage_runtime_linux.py")
)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


class ClosureTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name).resolve()
        (self.root / "bin").mkdir()
        (self.root / "lib").mkdir()
        self.objects = {}
        self.elf("bin/postgres", ["libxml2.so.2", "libc.so.6"])
        self.elf("lib/libxml2.so.2", ["libicuuc.so.70"])
        self.elf("lib/libicuuc.so.70", ["libm.so.6"])
        self.addCleanup(patch.stopall)
        patch.object(MODULE, "inspect", side_effect=lambda p: self.objects[p.resolve()]).start()

    def elf(self, name, needed, machine=62, runpath=None, versions=()):
        path = self.root / name
        path.write_bytes(b"\x7fELF\x02\x01" + b"\0" * 10 + struct.pack("<HH", 3, machine))
        self.objects[path.resolve()] = {
            "needed": needed,
            "runpath": runpath if runpath is not None else MODULE.origin_path(path, self.root),
            "versions": versions,
        }
        return path

    def test_complete_transitive_closure(self):
        MODULE.check(self.root)

    def test_missing_direct_dependency_fails_even_when_host_has_it(self):
        (self.root / "lib/libxml2.so.2").unlink()
        with self.assertRaisesRegex(MODULE.BundleError, "libxml2.so.2"):
            MODULE.check(self.root)

    def test_missing_transitive_dependency_fails(self):
        (self.root / "lib/libicuuc.so.70").unlink()
        with self.assertRaisesRegex(MODULE.BundleError, "libicuuc.so.70"):
            MODULE.check(self.root)

    def test_library_without_origin_search_path_fails(self):
        self.elf("lib/libxml2.so.2", ["libicuuc.so.70"], runpath="")
        with self.assertRaisesRegex(MODULE.BundleError, "search path"):
            MODULE.check(self.root)

    def test_absolute_host_search_path_fails(self):
        self.elf("bin/postgres", ["libxml2.so.2"], runpath="/usr/lib")
        with self.assertRaisesRegex(MODULE.BundleError, "search path"):
            MODULE.check(self.root)

    def test_nested_extension_needs_relative_path_to_bundle_lib(self):
        (self.root / "lib/plugins").mkdir()
        self.elf("lib/plugins/example.so", ["libxml2.so.2"])
        MODULE.check(self.root)

    def test_foreign_architecture_dependency_fails(self):
        self.elf("lib/libxml2.so.2", [], machine=183)
        with self.assertRaisesRegex(MODULE.BundleError, "architecture"):
            MODULE.check(self.root)

    def test_aarch64_closure(self):
        for name, info in list(self.objects.items()):
            self.elf(str(name.relative_to(self.root)), info["needed"], machine=183)
        MODULE.check(self.root)

    def test_newer_glibc_symbol_requirement_fails(self):
        self.elf("lib/libxml2.so.2", [], versions=[(2, 36)])
        with self.assertRaisesRegex(MODULE.BundleError, "glibc 2.35"):
            MODULE.check(self.root)

    def test_soname_path_escape_fails(self):
        self.elf("bin/postgres", ["/usr/lib/libxml2.so.2"])
        with self.assertRaisesRegex(MODULE.BundleError, "SONAME"):
            MODULE.check(self.root)

    def test_bundled_glibc_is_refused(self):
        self.elf("lib/libc.so.6", [])
        with self.assertRaisesRegex(MODULE.BundleError, "glibc"):
            MODULE.check(self.root)

    def test_non_elf_dependency_is_not_accepted(self):
        (self.root / "lib/libxml2.so.2").write_text("not a library")
        with self.assertRaisesRegex(MODULE.BundleError, "ELF"):
            MODULE.check(self.root)

    def test_symlink_outside_bundle_fails(self):
        (self.root / "lib/libxml2.so.2").unlink()
        (self.root / "lib/libxml2.so.2").symlink_to("/usr/lib/libxml2.so.2")
        with self.assertRaisesRegex(MODULE.BundleError, "bundle"):
            MODULE.check(self.root)

    def test_only_glibc_is_allowed_from_host(self):
        for name in ["libc.so.6", "libm.so.6", "libpthread.so.0", "ld-linux-aarch64.so.1"]:
            self.assertTrue(MODULE.is_glibc(name), name)
        for name in ["libxml2.so.2", "libgcc_s.so.1", "libstdc++.so.6", "libcrypt.so.1"]:
            self.assertFalse(MODULE.is_glibc(name), name)

    def test_bundle_collects_transitive_dependencies_and_patches_consumers(self):
        host = self.root / "host"
        host.mkdir()
        for name in ["libxml2.so.2", "libicuuc.so.70"]:
            (self.root / "lib" / name).rename(host / name)
        # Host inputs are outside the bundle that the collector scans.
        host.rename(self.root.parent / (self.root.name + "-host"))
        host = self.root.parent / (self.root.name + "-host")
        self.addCleanup(lambda: shutil.rmtree(host))
        host_info = {
            host / "libxml2.so.2": {"needed": ["libicuuc.so.70"], "runpath": "", "versions": []},
            host / "libicuuc.so.70": {"needed": ["libc.so.6"], "runpath": "", "versions": []},
        }
        self.objects.update(host_info)
        patched = []

        def run(*args):
            if args[0] == "patchelf":
                dest = Path(args[-1])
                self.objects[dest.resolve()] = {
                    **host_info.get(host / dest.name, self.objects.get(dest.resolve(), {})),
                    "runpath": args[2],
                }
                patched.append(dest.name)
            return ""

        with patch.object(MODULE, "system_libraries", return_value={
            name: [host / name] for name in ["libxml2.so.2", "libicuuc.so.70"]
        }), patch.object(MODULE, "copy_license"), patch.object(MODULE, "run", side_effect=run), \
                patch.object(MODULE, "inspect", side_effect=lambda p: self.objects[p.resolve()]
                             if p.resolve() in self.objects else host_info[host / p.name]):
            MODULE.bundle(self.root)
        self.assertEqual(set(patched), {"postgres", "libxml2.so.2", "libicuuc.so.70"})
        MODULE.check(self.root)


class MetadataTests(unittest.TestCase):
    def test_readelf_needed_runpath_and_glibc_versions(self):
        dynamic = """
 0x0000000000000001 (NEEDED) Shared library: [libxml2.so.2]
 0x0000000000000001 (NEEDED) Shared library: [libc.so.6]
 0x000000000000001d (RUNPATH) Library runpath: [$ORIGIN/../lib]
"""
        with patch.object(MODULE, "run", side_effect=[dynamic, "Name: GLIBC_2.34 Name: GLIBC_2.2.5"]):
            metadata = MODULE.inspect(Path("postgres"))
        self.assertEqual(metadata["needed"], ["libxml2.so.2", "libc.so.6"])
        self.assertEqual(metadata["runpath"], "$ORIGIN/../lib")
        self.assertEqual(metadata["versions"], [(2, 34), (2, 2, 5)])

    def test_missing_readelf_fails_closed(self):
        with patch.object(MODULE.subprocess, "check_output", side_effect=FileNotFoundError("readelf")):
            with self.assertRaisesRegex(MODULE.BundleError, "readelf"):
                MODULE.inspect(Path("postgres"))


if __name__ == "__main__":
    unittest.main()
