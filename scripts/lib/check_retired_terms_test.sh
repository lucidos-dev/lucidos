#!/usr/bin/env bash
# Hermetic fixtures for the retired-term gate. No services or signals.
# Each fixture is a disposable git repository under this worktree's .lucidos.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 - "$SCRIPT_DIR/../check-retired-terms.sh" <<'PY'
import pathlib
import subprocess
import sys
import tempfile
import unittest

CHECK = pathlib.Path(sys.argv.pop()).resolve()
SCRATCH = CHECK.parents[1] / '.lucidos'
SCRATCH.mkdir(exist_ok=True)


class GateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=SCRATCH)
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        subprocess.run(['git', 'init', '-q', str(self.root)], check=True)
        self.put('docs/adr/0001-rule.md', '# Decision')
        self.put('docs/retired-terms.tsv', r'\bold[ ]+terms?\b' + '\tnew term\tdocs/adr/0001-rule.md\n')
        self.put('docs/retired-term-exceptions.tsv', 'history.md\thistory\t-\tPublished history.\n')
        self.put('history.md', 'old term')
        self.put('current.md', 'new term')

    def put(self, name, body):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body)
        subprocess.run(['git', '-C', str(self.root), 'add', name], check=True)

    def run_gate(self, expected, cwd=None):
        result = subprocess.run(['bash', str(CHECK)], cwd=cwd or self.root, text=True, capture_output=True)
        self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
        return result.stdout + result.stderr

    def test_history_and_qualified_mentions_pass(self):
        self.put('current.md', 'LEGACY old terms\nold term is retired\nnew term')
        self.run_gate(0)

    def test_current_and_mixed_case_mentions_fail(self):
        self.put('current.md', 'Use OLD TERM here.')
        self.assertIn('current.md:1:', self.run_gate(1))

    def test_nested_working_directory_checks_the_whole_tree(self):
        self.run_gate(0, self.root / 'docs' / 'adr')
        self.put('current.md', 'old term')
        self.assertIn('current.md:1:', self.run_gate(1, self.root / 'docs' / 'adr'))

    def test_literal_exception_covers_only_its_occurrence(self):
        self.put('docs/retired-term-exceptions.tsv', 'current.md\tliteral\t`old term`\tStored spelling.\n')
        self.put('history.md', 'new term')
        self.put('current.md', '`old term`')
        self.run_gate(0)
        self.put('current.md', '`old term` and old term')
        self.assertIn('1 unqualified mentions', self.run_gate(1))

    def test_working_copy_of_tracked_file_is_checked(self):
        (self.root / 'current.md').write_text('old term')
        self.run_gate(1)

    def test_missing_or_empty_registries_fail(self):
        for name in ('docs/retired-terms.tsv', 'docs/retired-term-exceptions.tsv'):
            path = self.root / name
            saved = path.read_text()
            for content in (None, '', '# no rules\n'):
                if content is None:
                    path.unlink()
                else:
                    path.write_text(content)
                self.run_gate(1)
            path.write_text(saved)

    def test_bad_rules_and_missing_adr_fail(self):
        for content in ('bad row', '[\tnew\tdocs/adr/0001-rule.md', '.*\tnew\tdocs/adr/0001-rule.md',
                        'old\tnew\tdocs/adr/missing.md'):
            self.put('docs/retired-terms.tsv', content)
            self.run_gate(1)

    def test_malformed_or_stale_exceptions_fail(self):
        for content in ('bad row', 'history.md\tunknown\t-\treason',
                        'history.md\tliteral\t[\treason', 'missing.md\thistory\t-\treason'):
            self.put('docs/retired-term-exceptions.tsv', content)
            self.run_gate(1)

    def test_unreadable_tracked_text_fails_and_binary_is_skipped(self):
        self.put('binary.dat', 'placeholder')
        (self.root / 'binary.dat').write_bytes(b'\0old term')
        self.run_gate(0)
        (self.root / 'current.md').unlink()
        self.run_gate(1)


unittest.main()
PY
