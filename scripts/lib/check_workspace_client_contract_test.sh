#!/usr/bin/env bash
# Fixtures for check-workspace-client-contract.sh in disposable repos. No cargo runs.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 - "$SCRIPT_DIR/../check-workspace-client-contract.sh" <<'PY'
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

CHECK = pathlib.Path(sys.argv.pop()).resolve()
SCRATCH = CHECK.parents[1] / '.lucidos'
SCRATCH.mkdir(exist_ok=True)
STAMP = 'crates/lucidos-gateway-stamp/src/workspace_client.rs'
RECORD = 'crates/lucidos-gateway-stamp/workspace-client-surface.txt'
API = 'crates/lucidos-agents-api/src/api/threads.rs'
SURFACE = 'docs/generated/agent-surface.json'
WIRE = 'crates/lucidos-app/src/generated/thread-event-wire.ts'


class Contract(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=SCRATCH)
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        subprocess.run(['git', 'init', '-q', str(self.root)], check=True)
        self.put(STAMP, 'pub const VERSION: u32 = 1;\n')
        self.put(API, 'Router::new()\n    .route("/threads", get(list).post(create))\n'
                      '    .route(\n        "/threads/:id",\n        axum::routing::get(move || { map.get(1) }),\n    );\n'
                      '#[cfg(test)]\nmod tests { fn t() { Router::new().route("/throwaway", get(x)); } }\n')
        self.put(SURFACE, json.dumps({'events': {'thread': ['MessageReceived'], 'system': ['NotificationCreated']}}))
        self.put(WIRE, 'export type A = 1;\n')
        self.put('crates/lucidos-app/src/generated/thread-lifecycle.ts', 'export type B = 1;\n')

    def put(self, name, body):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body)

    def check(self, expected, *args):
        result = subprocess.run(['bash', str(CHECK), *args], cwd=self.root, text=True, capture_output=True)
        self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
        return result.stdout + result.stderr

    def accept(self):
        self.check(0, '--accept')

    def test_missing_record_fails_until_accepted(self):
        self.check(1)
        self.accept()
        self.check(0)

    def test_record_lists_production_routes_with_their_methods(self):
        self.accept()
        record = (self.root / RECORD).read_text().splitlines()
        self.assertIn('version 1', record)
        self.assertIn('route /threads get,post', record)
        self.assertIn('route /threads/:id get', record)
        self.assertNotIn('throwaway', '\n'.join(record))
        self.assertIn('event thread MessageReceived', record)

    def test_added_route_fails_until_accepted(self):
        self.accept()
        self.put(API, (self.root / API).read_text().replace('.post(create)', '.post(create).delete(drop)'))
        self.assertIn('+route /threads delete,get,post', self.check(1))
        self.accept()
        self.check(0)

    def test_event_and_wire_changes_fail(self):
        self.accept()
        self.put(WIRE, 'export type A = 2;\n')
        self.check(1)
        self.accept()
        self.put(SURFACE, json.dumps({'events': {'thread': ['MessageReceived'], 'system': []}}))
        self.check(1)

    def test_version_bump_alone_still_needs_the_record(self):
        self.accept()
        self.put(STAMP, 'pub const VERSION: u32 = 2;\n')
        self.assertIn('+version 2', self.check(1))
        self.accept()
        self.check(0)

    def test_missing_or_malformed_inputs_fail_closed(self):
        self.accept()
        for name, body in ((STAMP, 'no version'), (SURFACE, '{}'), (SURFACE, 'malformed')):
            original = (self.root / name).read_text()
            self.put(name, body)
            self.check(1)
            self.put(name, original)
        (self.root / WIRE).unlink()
        self.check(1)

    def test_rejects_unknown_arguments(self):
        self.check(1, '--force')


unittest.main(argv=['check_workspace_client_contract_test'], verbosity=1)
PY
