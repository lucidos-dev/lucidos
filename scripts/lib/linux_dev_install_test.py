"""Hermetic source-bootstrap regressions. Never starts or signals a real service."""
from pathlib import Path
import re
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


def function(path, name):
    text = (ROOT / path).read_text()
    match = re.search(rf"^{name}\(\) \{{\n.*?^\}}", text, re.M | re.S)
    assert match is not None, f"Missing function {name} in {path}"
    return match[0]


def shell(script, cwd=None):
    return subprocess.run(["bash", "-e", "-o", "pipefail", "-c", script], cwd=cwd, text=True,
                          capture_output=True, timeout=5)


class LinuxDevInstall(unittest.TestCase):
    def test_client_minimum_tracks_source_postgres_server(self):
        constants = (ROOT / "scripts/lib/workspace_constants.sh").read_text()
        major = re.search(r'PG_IMAGE="[^"\n]*:pg([0-9]+)"', constants)[1]
        helper = function("install.sh", "ensure_postgres_client")
        self.assertEqual(re.findall(r'-lt ([0-9]+)', helper), [major, major])
        self.assertIn(f"postgresql-client-{major}", helper)

    def test_linux_build_packages(self):
        for manager in ("apt-get", "dnf", "yum", "pacman", "zypper", ""):
            with self.subTest(manager=manager):
                script = function("install.sh", "ensure_build_deps")
                result = shell('OS=linux; PKG="' + manager + '"; '
                               'step() { :; }; ok() { :; }; warn() { echo "$*"; }; '
                               'die() { echo "$*"; exit 1; }; '
                               'installed=0; have() { [ "$1" != sccache ] || [ "$installed" = 1 ]; }; '
                               'pkg_install() { echo "$*"; [ "$1" != sccache ] || installed=1; return 0; }; '
                               + script + '\nensure_build_deps')
                self.assertIn("sccache", result.stdout)
                self.assertIn("cmake", result.stdout)

    def test_unavailable_required_build_tools_fail_with_guidance(self):
        script = function("install.sh", "ensure_build_deps")
        for missing in ("cmake", "sccache"):
            result = shell('OS=linux; PKG=""; '
                           'step() { :; }; ok() { :; }; warn() { echo "$*"; }; '
                           'die() { echo "$*"; exit 1; }; '
                           f'have() {{ [ "$1" != {missing} ]; }}; '
                           'pkg_install() { return 1; }; '
                           + script + '\nensure_build_deps')
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(missing, result.stdout)
            self.assertIn("Install", result.stdout)

    def test_postgres_client_packages_and_version(self):
        script = function("install.sh", "ensure_postgres_client")
        for version in ("17.6", "18.0", "19.1"):
            result = shell('OS=linux; PKG=apt-get; '
                           'step() { :; }; ok() { :; }; warn() { echo "$*"; }; '
                           'die() { echo "$*"; exit 1; }; '
                           'have() { return 0; }; pkg_install() { echo "install $*"; }; '
                           f'pg_dump() {{ echo "pg_dump (PostgreSQL) {version}"; }}; '
                           f'pg_restore() {{ echo "pg_restore (PostgreSQL) {version}"; }}; '
                           + script + '\nensure_postgres_client')
            if version.startswith("17"):
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("18", result.stdout)
                self.assertIn("postgresql-client-18", result.stdout)
            else:
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_missing_postgres_tools_install_or_fail_with_guidance(self):
        script = function("install.sh", "ensure_postgres_client")
        for manager, available in (("apt-get", True), ("apt-get", False), ("", False)):
            result = shell('OS=linux; PKG="' + manager + '"; installed=0; '
                           'step() { :; }; ok() { :; }; warn() { echo "$*"; }; '
                           'die() { echo "$*"; exit 1; }; '
                           'have() { [ "$installed" = 1 ]; }; '
                           'pg_dump() { [ "$installed" = 1 ] && echo "pg_dump (PostgreSQL) 18.0"; }; '
                           'pg_restore() { [ "$installed" = 1 ] && echo "pg_restore (PostgreSQL) 18.0"; }; '
                           'pkg_install() { echo "$*"; installed=' + str(int(available)) + '; }; '
                           + script + '\nensure_postgres_client')
            self.assertEqual(result.returncode == 0, available, result.stdout)
            self.assertIn("postgresql-client-18", result.stdout)

    def test_launcher_accepts_completion_flag(self):
        script = function("scripts/lib/workspace.sh", "parse_dev_args")
        result = shell('SCRIPT_NAME=test; ' + script +
                       '\nparse_dev_args -w fake --exit-after-ready --help')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("--exit-after-ready", result.stdout)

    def test_installer_returns_after_noninteractive_startup(self):
        with tempfile.TemporaryDirectory() as directory:
            scripts = Path(directory) / "scripts"
            scripts.mkdir()
            launcher = scripts / "run.sh"
            # Run the real web-dev completion path with simulated lock waiting.
            completion = (ROOT / "scripts/web-dev.sh").read_text().split('if [ -n "$FOLLOW_LOG" ]; then', 1)[1]
            launcher.write_text('#!/bin/bash\nFOLLOW_LOG=""\nENGINE_SUPERVISOR_PID=""\n'
                                'WORKSPACE=fake\nENGINE_LOG=/dev/null\n'
                                'EXIT_AFTER_READY=""\n'
                                'for arg in "$@"; do [ "$arg" != --exit-after-ready ] || EXIT_AFTER_READY=1; done\n'
                                'engine_lock_path() { echo /dev/null; }\n'
                                'flock_open() { echo UNEXPECTED_WAIT; return 1; }\n'
                                'if [ -n "$FOLLOW_LOG" ]; then' + completion)
            launcher.chmod(0o755)
            script = function("install.sh", "launch")
            for mode in ("mock", "real"):
                result = shell('step() { :; }; die() { exit 1; }; '
                               'C_DIM=""; C_BOLD=""; C_RESET=""; '
                               f'LUCIDOS_HOME="{directory}"; LUCIDOS_WORKSPACE=fake; '
                               f'PROVIDER_MODE={mode}; ' + script + '\nlaunch')
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertNotIn("UNEXPECTED_WAIT", result.stdout)

            ordinary = subprocess.run(["bash", str(launcher)], text=True,
                                      capture_output=True, timeout=5)
            self.assertIn("UNEXPECTED_WAIT", ordinary.stdout)
            launcher.write_text("#!/bin/bash\nexit 7\n")
            failed = shell('step() { :; }; die() { echo "$*"; exit 1; }; '
                           'C_DIM=""; C_BOLD=""; C_RESET=""; '
                           f'LUCIDOS_HOME="{directory}"; LUCIDOS_WORKSPACE=fake; '
                           'PROVIDER_MODE=real; ' + script + '\nlaunch')
            self.assertNotEqual(failed.returncode, 0)
            self.assertIn("scripts/run.sh failed", failed.stdout)

    def test_noninteractive_gateway_lifetime(self):
        completion = (ROOT / "scripts/web-dev.sh").read_text().split(
            'if [ -n "$FOLLOW_LOG" ]; then', 1)[1]
        setup = ('FOLLOW_LOG=""; EXIT_AFTER_READY=""; GATEWAY_PID=123; '
                 'ENGINE_SUPERVISOR_PID=456; checks=0; '
                 'kill() { checks=$((checks + 1)); [ "$checks" -lt 4 ]; }; '
                 'sleep() { echo GATEWAY_ALIVE; }; '
                 'wait() { echo ENGINE_WAIT; return 0; }; '
                 'if [ -n "$FOLLOW_LOG" ]; then')
        result = shell(setup + completion)
        self.assertEqual(result.stdout.count("GATEWAY_ALIVE"), 2, result.stdout)
        self.assertNotIn("ENGINE_WAIT", result.stdout)
        self.assertNotEqual(result.returncode, 0)

        fallback = shell('FOLLOW_LOG=""; EXIT_AFTER_READY=""; GATEWAY_PID=""; '
                         'ENGINE_SUPERVISOR_PID=456; '
                         'wait() { echo ENGINE_WAIT; return 0; }; '
                         'if [ -n "$FOLLOW_LOG" ]; then' + completion)
        self.assertEqual(fallback.returncode, 0, fallback.stderr)
        self.assertIn("ENGINE_WAIT", fallback.stdout)

    def test_banner_uses_host_os(self):
        script = function("scripts/lib/workspace.sh", "show_banner")
        for os_name, label in (("Linux", "Linux"), ("Darwin", "macOS")):
            result = shell(f'uname() {{ echo {os_name}; }}; '
                           'ipconfig() { return 1; }; tailscale() { return 1; }; '
                           'workspace_database_name() { echo fake; }; '
                           'sleep_prevention_status() { echo mac-sleep; }; '
                           'WORKSPACE=fake; PROTO=http; ENGINE_PORT=1; PG_PORT=2; ENGINE_LOG=log; '
                           + script + '\nshow_banner')
            self.assertIn("Native " + label, result.stdout)
            self.assertEqual("Sleep:" in result.stdout, os_name == "Darwin")

    def test_both_log_rotation_sizes_are_portable(self):
        text = (ROOT / "scripts/lib/workspace.sh").read_text()
        for variable in ("ENGINE_LOG", "gw_log"):
            pattern = rf'    if \[ -f "\${variable}" \]; then\n.*?^    fi'
            block = re.search(pattern, text, re.M | re.S)[0]
            for size in (10, 10485761):
                with tempfile.TemporaryDirectory() as directory:
                    log = Path(directory) / "log"
                    with log.open("wb") as stream:
                        stream.truncate(size)
                    result = shell('stat() { echo "GNU filesystem status"; return 1; }; '
                                   f'{variable}="{log}"; SCRIPT_NAME=test; '
                                   'rotate() {\n' + block + '\n}; rotate')
                    self.assertEqual(result.stderr, "")
                    self.assertEqual(log.stat().st_size > 10485760, False)
                    if size == 10:
                        self.assertEqual(log.stat().st_size, size)


if __name__ == "__main__":
    unittest.main()
