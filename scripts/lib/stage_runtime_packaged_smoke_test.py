"""Cheap negative checks; these do not certify the signed packaged run."""

import os
import plistlib
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

import stage_runtime_packaged_smoke as smoke
import stage_runtime_packaged_lifecycle as lifecycle


class PackagedSmokeTests(unittest.TestCase):
    def test_controller_preserves_only_the_inherited_sandbox_marker(self):
        harness = (Path(__file__).resolve().parents[2] / "scripts/e2e-packaged.sh").read_text()
        launch = harness[harness.index('launch() {'):harness.index('seed_cache() {')]
        with tempfile.TemporaryDirectory() as scratch:
            app = Path(scratch) / "app"
            app.write_text('#!/bin/sh\n/usr/bin/env\n')
            app.chmod(0o755)
            log = Path(scratch) / "service.log"
            for marker in (None, "1", "0"):
                env = {**os.environ, "APP_BIN": str(app), "SVC_LOG": str(log),
                       "SCOPE": str(Path(scratch) / "run"), "PORT": "5300",
                       "SYNTHETIC_CREDENTIAL": "fixture-secret"}
                env.pop("LUCIDOS_AGENT_SANDBOXED", None)
                if marker is not None:
                    env["LUCIDOS_AGENT_SANDBOXED"] = marker
                result = subprocess.run(["bash", "-uc", 'log() { :; }; ' + launch + '\nlaunch\nwait "$SVC_PID"'],
                                        env=env, capture_output=True, text=True, timeout=5)
                self.assertEqual(result.returncode, 0, result.stderr)
                lines = log.read_text().splitlines()
                self.assertEqual("LUCIDOS_AGENT_SANDBOXED=1" in lines, marker == "1")
                self.assertFalse(any(line.startswith("SYNTHETIC_CREDENTIAL=") for line in lines))
                self.assertNotIn("LUCIDOS_AGENT_SANDBOXED=0", lines)

    def test_large_shell_does_not_fail_a_valid_base_href_under_pipefail(self):
        harness = (Path(__file__).resolve().parents[2] / "scripts/e2e-packaged.sh").read_text()
        helpers = harness[harness.index('log()'):harness.index('# An explicitly')]
        start = harness.index('printf \'%s\' "$SHELL_HTML" |')
        assertion = harness[start:harness.index('log "PASS: workspace app shell', start)]
        # The matching tag is first, well before the writer finishes. grep -q
        # would close the pipe and turn a correct response into SIGPIPE failure.
        script = 'set -o pipefail\n' + helpers + '\nSLUG=smoke\n'
        script += '''SHELL_HTML="$(printf '<head><base href="/smoke/">\\n'; for i in {1..50000}; do printf 'content\\n'; done)"
'''
        result = subprocess.run(["bash", "-c", script + assertion],
                                capture_output=True, text=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_codesign_requests_xml_with_legacy_compatibility(self):
        harness = (Path(__file__).resolve().parents[2] / "scripts/e2e-packaged.sh").read_text()
        start = harness.index('ENGINE_ENTITLEMENTS=')
        snippet = harness[start:harness.index('# Short paths', start)]
        with tempfile.TemporaryDirectory() as scratch:
            fixture = Path(scratch) / "entitlements.plist"
            fixture.write_bytes(plistlib.dumps({"com.apple.security.cs.allow-unsigned-executable-memory": True}))
            codesign = Path(scratch) / "codesign"
            codesign.write_text('''#!/bin/sh
if [ "$2" = --xml ]; then
    [ "$FIXTURE_MODE" = modern ] || exit 1
elif [ "$3" != ':-' ]; then
    printf '[Dict]\\n'
    exit 0
fi
cat "$FIXTURE_XML"
''')
            codesign.chmod(0o755)
            for mode in ("modern", "legacy"):
                with self.subTest(mode=mode):
                    result = subprocess.run(
                        ["bash", "-c", 'fail() { return 1; }; ENGINE_BIN=fixture; ' + snippet],
                        env={**os.environ, "PATH": scratch + os.pathsep + os.environ["PATH"],
                             "FIXTURE_MODE": mode, "FIXTURE_XML": str(fixture)},
                        capture_output=True, text=True, timeout=5,
                    )
                    self.assertEqual(result.returncode, 0, result.stderr)

    def test_engine_entitlements_are_validated_from_a_nonseekable_pipe(self):
        harness = (Path(__file__).resolve().parents[2] / "scripts/e2e-packaged.sh").read_text()
        # Run the actual shell-embedded validator with subprocess.PIPE, exactly
        # like codesign output reaches it. A file/BytesIO hides Python's seek.
        validator = harness.split('printf \'%s\' "$ENGINE_ENTITLEMENTS" | python3 -c \'', 1)[1].split("\n' ", 1)[0]
        for value, expected in ((True, 0), (False, 1), ("true", 1)):
            with self.subTest(value=value):
                data = plistlib.dumps({"com.apple.security.cs.allow-unsigned-executable-memory": value})
                result = subprocess.run([sys.executable, "-c", validator], input=data,
                                        capture_output=True, timeout=5)
                self.assertEqual(result.returncode, expected, result.stderr.decode())

    def test_cleanup_rejects_a_surviving_listener(self):
        with tempfile.TemporaryDirectory() as root, socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen()
            evidence = {"processes": {}, "ports": [listener.getsockname()[1]]}
            with self.assertRaisesRegex(RuntimeError, "listeners=\\[[0-9]+\\]"):
                lifecycle.verify(root, evidence, timeout=0)
            listener.close()
            lifecycle.verify(root, evidence, timeout=0)

    def test_cleanup_rejects_postgres_evidence_even_without_a_listener(self):
        with tempfile.TemporaryDirectory() as root:
            pidfile = Path(root) / "gateway/pgdata/postmaster.pid"
            pidfile.parent.mkdir(parents=True)
            pidfile.write_text("123\n")
            with self.assertRaisesRegex(RuntimeError, "postmaster.pid=True"):
                lifecycle.verify(root, {"processes": {}, "ports": []}, timeout=0)

    def test_cleanup_tracks_process_identity_without_signalling_reused_pids(self):
        evidence = {"processes": {"123": "original start"}}
        self.assertEqual(lifecycle.remaining(evidence, {123: (100, "original start")}), ["123"])
        self.assertEqual(lifecycle.remaining(evidence, {123: (100, "later start")}), [])

    def test_snapshot_captures_owned_group_postgres_and_engine_ports(self):
        with tempfile.TemporaryDirectory() as root:
            scope = Path(root)
            (scope / "supervisor.pid").write_text("101")
            (scope / "gateway.pid").write_text("102")
            pg = scope / "gateway/pgdata"
            pg.mkdir(parents=True)
            (pg / "postmaster.pid").write_text("104\n")
            ws = scope / "gateway/workspaces/smoke/.lucidos"
            ws.mkdir(parents=True)
            (ws / "ports").write_text("API_PORT=54321\nPROTO=http\n")
            rows = {101: (101, "a"), 102: (102, "b"), 103: (102, "c"),
                    104: (104, "d"), 105: (105, "unrelated")}
            with patch.object(lifecycle, "processes", return_value=rows):
                evidence = lifecycle.snapshot(root, 5300)
            self.assertEqual(set(evidence["processes"]), {"101", "102", "103", "104"})
            self.assertEqual(evidence["ports"], [5300, 54321])

    def test_missing_injected_credential_fails(self):
        with patch.dict(os.environ, {}, clear=True):
            namespace = {}
            exec(smoke.credential_helper(), namespace)
            with self.assertRaisesRegex(RuntimeError, "did not receive its credential"):
                namespace["credential"]()

    def test_credential_is_read_from_execution_environment(self):
        # Compile before injection: the test fixture must not embed the value
        # that the orchestrating host happens to hold.
        namespace = {}
        exec(smoke.credential_helper(), namespace)
        with patch.dict(os.environ, {"CRED_PACKAGED_SMOKE": "synthetic-first"}):
            self.assertEqual(namespace["credential"](), "synthetic-first")
        with patch.dict(os.environ, {"CRED_PACKAGED_SMOKE": "synthetic-second"}):
            self.assertEqual(namespace["credential"](), "synthetic-second")

    def test_missing_token_is_a_nonzero_fixture_failure(self):
        with tempfile.TemporaryDirectory() as scratch:
            result = subprocess.run(
                [sys.executable, smoke.__file__, "http://127.0.0.1:1", str(Path(scratch) / "absent")],
                capture_output=True, text=True, timeout=5,
            )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("FAIL: packaged proxy/script smoke", result.stderr)
        self.assertNotIn("PASS:", result.stdout)

    def preflight(self, worktree, args=(), host="Darwin", stale_app=False, identity=""):
        root = Path(__file__).resolve().parents[2]
        with tempfile.TemporaryDirectory() as scratch:
            scripts = Path(scratch) / (".lucidos/worktrees/smoke/scripts" if worktree else "scripts")
            scripts.mkdir(parents=True)
            if stale_app:
                # An unproven artifact cannot bypass this checkout's build.
                (scripts.parent / "target/release/bundle/macos/Stale.app").mkdir(parents=True)
            shutil.copyfile(root / "scripts/e2e-packaged.sh", scripts / "e2e-packaged.sh")
            # Exercise signing preflight on every host; never build in this fixture.
            uname = scripts / "uname"
            uname.write_text(f"#!/bin/sh\nprintf '{host}\\n'\n")
            uname.chmod(0o755)
            build = scripts / "build-dmg.sh"
            build.write_text(
                '#!/bin/sh\n[ "$1" = --check ] && exit 0\n'
                'printf "BUILD ATTEMPTED CI=%s IGNORE=%s\\n" "$CI" "$TAURI_BUNDLER_DMG_IGNORE_CI" >&2\n'
                'exit 1\n'
            )
            build.chmod(0o755)
            result = subprocess.run(
                ["bash", str(scripts / "e2e-packaged.sh"), *args],
                env={**os.environ, "APPLE_SIGNING_IDENTITY": identity, "CI": "false",
                     "TAURI_BUNDLER_DMG_IGNORE_CI": "true",
                     "PATH": str(scripts) + os.pathsep + os.environ["PATH"]},
                capture_output=True, text=True, timeout=5,
            )
        return result

    def test_build_preserves_ci_environment_and_propagates_failure(self):
        result = self.preflight(worktree=True, identity="synthetic-signing-identity")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("BUILD ATTEMPTED CI=false IGNORE=true", result.stderr)
        self.assertIn("build-dmg.sh failed", result.stderr)
        self.assertNotIn("launched disposable", result.stdout)

    def test_worktree_reaches_signing_preflight_without_building(self):
        result = self.preflight(worktree=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("APPLE_SIGNING_IDENTITY", result.stderr)
        self.assertNotIn("BUILD ATTEMPTED", result.stderr)
        self.assertNotIn("building the .app", result.stdout)

    def test_missing_signing_identity_is_refused_before_building(self):
        for args in ((), ("--rebuild",)):
            with self.subTest(args=args):
                result = self.preflight(worktree=False, args=args)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("APPLE_SIGNING_IDENTITY", result.stderr)
                self.assertNotIn("BUILD ATTEMPTED", result.stderr)

    def test_stale_app_cannot_bypass_build_signing_prerequisite(self):
        result = self.preflight(worktree=True, stale_app=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("APPLE_SIGNING_IDENTITY", result.stderr)

    def test_unsupported_host_fails_without_claiming_a_pass(self):
        result = self.preflight(worktree=False, host="Linux")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("requires macOS", result.stderr)
        self.assertNotIn("PASS:", result.stdout)
        self.assertNotIn("BUILD ATTEMPTED", result.stderr)


if __name__ == "__main__":
    unittest.main()
