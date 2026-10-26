#!/usr/bin/env bash
# Isolated audit-scan fixtures; no services, signals or workspace mutations.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 - "$SCRIPT_DIR/../check-audit-scans.sh" <<'PY'
import pathlib
import subprocess
import sys
import tempfile
import unittest

CHECK = pathlib.Path(sys.argv.pop()).resolve()
SCRATCH = CHECK.parents[1] / '.lucidos'
SCRATCH.mkdir(exist_ok=True)


class AuditScans(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=SCRATCH)
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        subprocess.run(['git', 'init', '-q', str(self.root)], check=True)
        section = self.root / 'system-knowhow/workspace-audit/sample.md'
        section.parent.mkdir(parents=True)
        self.section = section
        self.script = self.root / 'check.sh'

    def run_scan(self, scans, fixtures, expected):
        self.section.write_text('```bash\n' + scans + '\n```\n')
        source = CHECK.read_text()
        start = source.index("cat << 'EOF'\n") + len("cat << 'EOF'\n")
        end = source.index('\nEOF', start)
        self.script.write_text(source[:start] + fixtures + source[end:])
        result = subprocess.run(['bash', str(self.script)], cwd=self.root, text=True, capture_output=True)
        self.assertEqual(result.returncode, expected, result.stdout + result.stderr)

    def test_hit_miss_and_multiple_rows_keep_their_indices(self):
        self.run_scan("scan words 'needle'\nscan digits '[0-9]'",
                      'words hit needle\nwords miss hay\nwords hit prefixneedle\ndigits miss hay\ndigits hit 5', 0)

    def test_both_failure_directions(self):
        self.run_scan("scan words 'needle'", 'words hit hay', 1)
        self.run_scan("scan words 'needle'", 'words hit needle\nwords miss needle', 1)

    def test_missing_hit_unknown_scan_and_empty_scans(self):
        self.run_scan("scan words 'needle'", 'words miss hay', 1)
        self.run_scan("scan words 'needle'", 'missing hit needle', 1)
        self.run_scan('', 'words hit needle', 1)

    def test_duplicate_id_invalid_regex_and_malformed_fixture(self):
        self.run_scan("scan words 'needle'\nscan words 'other'", 'words hit needle', 1)
        self.run_scan("scan words '['", 'words hit needle', 1)
        self.run_scan("scan words 'needle'", 'words neither needle', 1)


unittest.main()
PY
