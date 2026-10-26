#!/usr/bin/env bash
# Freshness fixtures use a stub renderer in disposable repos. No cargo runs.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 - "$SCRIPT_DIR/../check-knowhow-generated.sh" <<'PY'
import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

CHECK = pathlib.Path(sys.argv.pop()).resolve()
SCRATCH = CHECK.parents[1] / '.lucidos'
SCRATCH.mkdir(exist_ok=True)


class Freshness(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=SCRATCH)
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        subprocess.run(['git', 'init', '-q', str(self.root)], check=True)
        self.manifest = 'docs/generated/agent-surface.json'
        self.surface = json.dumps({'schema_version': 1, 'cli': {'commands': ['sample']},
                                   'tools': {'offered': ['tool']}, 'events': {'thread': ['A'],
                                   'system': ['B'], 'legacy': ['C']}, 'sdk_exports': ['sdk']})
        self.put(self.manifest, self.surface)
        self.put('source.json', self.surface)
        self.doc = 'system-knowhow/sample.md'
        self.document = 'Prose\n<!-- generated:begin Sample -->\n`fresh`\n<!-- generated:end -->\nTail'
        self.put(self.doc, self.document)
        self.fragments = json.dumps({'schema_version': 1, 'documents': {
            self.doc: {'Sample': '\n`fresh`\n'}}})
        self.put('fragments.json', self.fragments)
        self.put('scripts/lib/codegen_blocks.py', CHECK.with_name('lib').joinpath('codegen_blocks.py').read_text())
        self.put('scripts/lib/codegen.sh', """codegen_render_to() {
    mkdir -p "$2/docs/generated" "$2/.codegen"
    cp "$1/source.json" "$2/docs/generated/agent-surface.json"
    cp "$1/fragments.json" "$2/.codegen/knowhow-blocks.json"
}
CODEGEN_BLOCKS=.codegen/knowhow-blocks.json
""")
        subprocess.run(['git', 'add', '.'], cwd=self.root, check=True)
        subprocess.run(['git', '-c', 'user.name=Test', '-c', 'user.email=test@example.com',
                        'commit', '-qm', 'fixture'], cwd=self.root, check=True)

    def put(self, name, body):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body)

    def check(self, expected):
        result = subprocess.run(['bash', str(CHECK)], cwd=self.root, text=True, capture_output=True)
        self.assertEqual(result.returncode, expected, result.stdout + result.stderr)

    def test_fresh_and_stale_committed_output(self):
        self.check(0)
        changed = self.surface.replace('sample', 'new-flag')
        self.put('source.json', changed)
        self.put(self.manifest, changed)
        self.check(1)
        self.assertEqual((self.root / self.manifest).read_text(), changed)

    def test_missing_empty_or_malformed_inputs(self):
        for name in (self.manifest, 'source.json'):
            for body in (None, '', '{}', 'malformed'):
                self.put(name, self.surface)
                if body is None:
                    (self.root / name).unlink()
                else:
                    self.put(name, body)
                self.check(1)
            self.put(name, self.surface)

    def test_renderer_failure_is_not_a_pass(self):
        self.put('scripts/lib/codegen.sh', 'codegen_render_to() { return 1; }')
        self.check(1)

    def test_stale_committed_block_fails_after_working_copy_regeneration(self):
        self.put('fragments.json', self.fragments.replace('fresh', 'renamed'))
        self.put(self.doc, self.document.replace('fresh', 'renamed'))
        self.check(1)
        self.assertIn('renamed', (self.root / self.doc).read_text())

    def test_handwritten_prose_does_not_affect_freshness(self):
        self.put(self.doc, self.document.replace('Prose', 'Different handwritten prose'))
        subprocess.run(['git', 'add', self.doc], cwd=self.root, check=True)
        subprocess.run(['git', '-c', 'user.name=Test', '-c', 'user.email=test@example.com',
                        'commit', '-qm', 'prose'], cwd=self.root, check=True)
        self.check(0)

    def test_committed_markers_fail_even_with_repaired_working_copy(self):
        for body in ('', self.document.replace('Sample', 'Unknown'), self.document * 2,
                     self.document.replace('<!-- generated:end -->', '')):
            self.put(self.doc, body)
            subprocess.run(['git', 'add', self.doc], cwd=self.root, check=True)
            subprocess.run(['git', '-c', 'user.name=Test', '-c', 'user.email=test@example.com',
                            'commit', '--allow-empty', '-qm', 'bad markers'], cwd=self.root, check=True)
            self.put(self.doc, self.document)
            self.check(1)

    def test_invalid_or_missing_fragment_manifest_fails(self):
        for body in ('', '{}', 'malformed', '{"schema_version":1,"documents":{}}',
                     self.fragments.replace('system-knowhow/sample.md', '../escape.md')):
            self.put('fragments.json', body)
            self.check(1)
        (self.root / 'fragments.json').unlink()
        self.check(1)

    def test_unlisted_committed_markers_fail_even_when_working_copy_is_clean(self):
        unlisted = 'system-knowhow/unlisted.md'
        self.put(unlisted, self.document)
        subprocess.run(['git', 'add', unlisted], cwd=self.root, check=True)
        subprocess.run(['git', '-c', 'user.name=Test', '-c', 'user.email=test@example.com',
                        'commit', '-qm', 'unlisted markers'], cwd=self.root, check=True)
        self.put(unlisted, 'Handwritten prose only')
        self.check(1)


unittest.main()
PY
