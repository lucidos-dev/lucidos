#!/usr/bin/env bash
# Hermetic coupling fixtures: disposable repositories, no services or signals.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 - "$SCRIPT_DIR/../check-knowhow-coupling.sh" <<'PY'
import pathlib
import fnmatch
import json
import subprocess
import sys
import tempfile
import unittest

CHECK = pathlib.Path(sys.argv.pop()).resolve()
SCRATCH = CHECK.parents[1] / '.lucidos'
SCRATCH.mkdir(exist_ok=True)


class CouplingTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=SCRATCH)
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        self.git('init', '-q', '-b', 'main')
        self.git('config', 'user.name', 'Test Agent')
        self.git('config', 'user.email', 'agent@example.test')
        self.put('docs/knowhow-surfaces.tsv', 'tools\tsrc/tools/*\tsystem-knowhow/tools.md\tTool contract.\n')
        self.put('src/tools/tool.rs', 'initial')
        self.put('src/tools/remaining.rs', 'remaining')
        self.put('system-knowhow/tools.md', '# Tools')
        self.put('system-knowhow/workspace-audit/tools.md',
                 '---\nname: Tools audit\nsources: ["system-knowhow/tools.md", "src/tools/*"]\n---\nAudit\n')
        self.commit()
        self.git('checkout', '-q', '-b', 'change')

    def git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.root), *args], stderr=subprocess.STDOUT)

    def put(self, path, body):
        dest = self.root / path
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_text(body)

    def commit(self, message='fixture'):
        self.git('add', '-A')
        self.git('commit', '-qm', message)

    def run_gate(self, code, cwd=None):
        result = subprocess.run(['bash', str(CHECK)], cwd=cwd or self.root, text=True, capture_output=True)
        self.assertEqual(result.returncode, code, result.stdout + result.stderr)
        return result.stdout + result.stderr

    def update_audit(self):
        path = self.root / 'system-knowhow/workspace-audit/tools.md'
        path.write_text(path.read_text() + 'Reviewed.\n')

    def test_empty_and_unrelated_diffs_pass(self):
        self.run_gate(0)
        self.put('unrelated.txt', 'unrelated')
        self.commit()
        self.run_gate(0, self.root / 'src')

    def test_surface_only_fails_both_obligations(self):
        self.put('src/tools/tool.rs', 'changed')
        self.commit()
        output = self.run_gate(1)
        self.assertRegex(output, r'(?m)^(?:knowhow-coupling: )?tools: ')
        self.assertIn('audit/tools:', output)
        self.assertIn('Tool contract.', output)

    def test_documentation_and_audit_update_pass(self):
        self.put('src/tools/tool.rs', 'changed')
        self.put('system-knowhow/tools.md', '# Updated tools')
        self.update_audit()
        self.commit()
        self.run_gate(0)

    def test_documentation_alone_pulls_in_audit(self):
        self.put('system-knowhow/tools.md', '# Updated tools')
        self.commit()
        self.assertIn('audit/tools:', self.run_gate(1))
        self.update_audit()
        self.commit()
        self.run_gate(0)

    def test_trailers_pass_without_document_edits(self):
        self.put('src/tools/tool.rs', 'changed')
        self.commit('Refactor\n\nKnowhow-unchanged: tools: Contract unchanged.\n'
                    'Knowhow-unchanged: audit/tools: Checks unchanged.')
        self.run_gate(0)

    def test_subject_is_not_a_trailer(self):
        self.put('src/tools/tool.rs', 'changed')
        self.commit('Knowhow-unchanged: tools: Looks like a subject\n\nBody.')
        self.run_gate(1)

    def test_trailer_does_not_cover_later_change(self):
        self.put('src/tools/tool.rs', 'first')
        self.commit('First\n\nKnowhow-unchanged: tools: No contract change.\n'
                    'Knowhow-unchanged: audit/tools: No check change.')
        self.put('src/tools/tool.rs', 'second')
        self.commit()
        self.run_gate(1)

    def test_malformed_or_unknown_trailers_fail(self):
        self.put('unrelated.txt', 'unrelated')
        for trailer in ('tools:', 'missing: Reason.', 'tools: -'):
            self.commit('Check\n\nKnowhow-unchanged: ' + trailer)
            self.assertIn('trailer', self.run_gate(1))
            self.git('reset', '--soft', 'HEAD^')

    def test_missing_or_bad_registry_fails(self):
        path = self.root / 'docs/knowhow-surfaces.tsv'
        for value in (None, '', '# Empty\n', 'bad\trow\n',
                      'tools\tsrc/absent/*\tsystem-knowhow/tools.md\tReason\n',
                      'tools\tsrc/tools/*\tabsent.md\tReason\n',
                      'tools\t../src/*\tsystem-knowhow/tools.md\tReason\n'):
            if value is None:
                path.unlink()
            else:
                path.write_text(value)
            self.commit()
            self.run_gate(1)
            self.git('reset', '--hard', 'main')

    def test_bad_audit_sources_fail(self):
        path = 'system-knowhow/workspace-audit/tools.md'
        for field in ('', 'sources: []\n', 'sources: not-a-list\n',
                      'sources: ["missing.md"]\n', 'sources: ["../escape"]\n',
                      'sources: ["src/tools/*"]\nsources: ["src/tools/*"]\n'):
            self.put(path, '---\nname: Audit\n' + field + '---\nAudit')
            self.commit()
            self.run_gate(1)
            self.git('reset', '--hard', 'main')

    def test_missing_audit_corpus_fails(self):
        self.git('rm', 'system-knowhow/workspace-audit/tools.md')
        self.commit()
        self.run_gate(1)

    def test_missing_main_fails(self):
        self.git('branch', '-D', 'main')
        self.run_gate(1)

    def test_deleted_or_renamed_source_still_couples(self):
        for rename in (False, True):
            if rename:
                self.git('mv', 'src/tools/tool.rs', 'outside.rs')
            else:
                self.git('rm', 'src/tools/tool.rs')
            self.commit()
            self.run_gate(1)
            self.put('system-knowhow/tools.md', '# Removal documented')
            self.update_audit()
            self.commit()
            self.run_gate(0)
            self.git('reset', '--hard', 'main')

    def test_working_copy_cannot_mask_committed_inputs_or_doc_diff(self):
        self.put('src/tools/tool.rs', 'changed')
        self.commit()
        self.put('docs/knowhow-surfaces.tsv', '')
        self.put('system-knowhow/tools.md', '# Uncommitted update')
        self.update_audit()
        self.assertRegex(self.run_gate(1), r'(?m)^(?:knowhow-coupling: )?tools: ')

    def test_both_rename_paths_and_filenames_with_spaces(self):
        self.git('mv', 'src/tools/tool.rs', 'src/tools/renamed tool.rs')
        self.commit()
        self.run_gate(1)

    def test_removed_mapping_does_not_drop_obligation(self):
        self.put('docs/knowhow-surfaces.tsv',
                 'other\tunrelated.txt\tsystem-knowhow/tools.md\tOther contract.\n')
        self.put('unrelated.txt', 'new')
        self.put('src/tools/tool.rs', 'changed')
        self.commit()
        self.assertRegex(self.run_gate(1), r'(?m)^(?:knowhow-coupling: )?tools: ')

    def test_deleted_source_must_leave_valid_head_declarations(self):
        for declaration in ('registry', 'audit'):
            if declaration == 'registry':
                self.put('docs/knowhow-surfaces.tsv',
                         'tools\tsrc/tools/tool.rs\tsystem-knowhow/tools.md\tTool contract.\n')
            else:
                self.put('system-knowhow/workspace-audit/tools.md',
                         '---\nname: Audit\nsources: ["src/tools/tool.rs"]\n---\nAudit')
            self.commit()
            self.git('branch', '-f', 'main', 'HEAD')
            self.git('rm', 'src/tools/tool.rs')
            self.put('system-knowhow/tools.md', '# Removal documented')
            self.update_audit()
            self.commit()
            self.assertIn('source glob matches no tracked path', self.run_gate(1))
            self.put('docs/knowhow-surfaces.tsv',
                     'tools\tsrc/tools/remaining.rs\tsystem-knowhow/tools.md\tRemaining tools.\n')
            self.put('system-knowhow/workspace-audit/tools.md',
                     '---\nname: Audit\nsources: ["src/tools/remaining.rs"]\n---\nReviewed')
            self.commit()
            self.run_gate(0)
            self.git('branch', '-f', 'main', 'HEAD')
            self.put('unrelated.txt', declaration)
            self.commit()
            self.run_gate(0)
            self.put('src/tools/tool.rs', 'restored')
            self.commit()

    def test_merge_from_main_preserves_valid_trailers(self):
        original = 'branch=initial\nkeep 1\nkeep 2\nkeep 3\nkeep 4\nmain=initial\n'
        self.put('src/tools/tool.rs', original)
        self.commit()
        self.git('branch', '-f', 'main', 'HEAD')
        self.put('src/tools/tool.rs', original.replace('branch=initial', 'branch=changed'))
        self.commit('Refactor\n\nKnowhow-unchanged: tools: Same contract.\n'
                    'Knowhow-unchanged: audit/tools: Same checks.')
        self.git('checkout', '-q', 'main')
        self.put('src/tools/tool.rs', original.replace('main=initial', 'main=changed'))
        self.commit()
        self.git('checkout', '-q', 'change')
        self.git('merge', '-q', 'main', '--no-edit')
        self.run_gate(0)

    def test_merge_resolution_needs_its_own_trailers(self):
        for branch, body in (('change', 'branch change'), ('main', 'main change')):
            self.git('checkout', '-q', branch)
            self.put('src/tools/tool.rs', body)
            self.commit('Refactor\n\nKnowhow-unchanged: tools: Same contract.\n'
                        'Knowhow-unchanged: audit/tools: Same checks.')
        self.git('checkout', '-q', 'change')
        result = subprocess.run(['git', '-C', str(self.root), 'merge', 'main', '--no-edit'], capture_output=True)
        self.assertEqual(result.returncode, 1)
        self.put('src/tools/tool.rs', 'resolution unlike either parent')
        self.commit('Resolve')
        self.assertRegex(self.run_gate(1), r'(?m)^(?:knowhow-coupling: )?tools: ')
        self.git('commit', '--amend', '-qm', 'Resolve\n\nKnowhow-unchanged: tools: Same contract.\n'
                 'Knowhow-unchanged: audit/tools: Same checks.')
        self.run_gate(0)

    def test_embedded_prompt_is_a_valid_documentation_target(self):
        self.put('src/prompt.rs', 'prompt')
        self.put('docs/knowhow-surfaces.tsv', 'tools\tsrc/tools/*\tsrc/prompt.rs\tAdvertised tools.\n')
        self.commit()
        self.git('branch', '-f', 'main', 'HEAD')
        self.put('src/tools/tool.rs', 'changed')
        self.put('src/prompt.rs', 'updated prompt')
        self.update_audit()
        self.commit()
        self.run_gate(0)

    def test_self_documenting_mapping_is_rejected(self):
        self.put('docs/knowhow-surfaces.tsv',
                 'tools\tsystem-knowhow/tools.md\tsystem-knowhow/tools.md\tVacuous mapping.\n')
        self.commit()
        self.assertIn('overlap', self.run_gate(1))

    def test_duplicate_audit_ids_are_rejected(self):
        self.put('system-knowhow/workspace-audit/nested/tools.md',
                 '---\nname: Another audit\nsources: ["src/tools/*"]\n---\nAudit')
        self.commit()
        self.assertIn('duplicate audit', self.run_gate(1))

    def test_live_mappings_follow_the_chat_tool_owner(self):
        repo = CHECK.parents[1]
        owners = subprocess.check_output(['git', '-C', str(repo), 'grep', '-l', '-F',
                                         'pub(crate) fn chat_tools(', '--',
                                         'crates/lucidos-engine/src/*.rs']).decode().splitlines()
        self.assertEqual(len(owners), 1, owners)
        owner = owners[0]
        self.assertIn('CONTENT TAXONOMY', (repo / owner).read_text())
        rows = {row[0]: row for line in (repo / 'docs/knowhow-surfaces.tsv').read_text().splitlines()
                if line and not line.startswith('#') for row in [line.split('\t')]}
        self.assertTrue(any(fnmatch.fnmatchcase(owner, p) for p in rows['system-prompt'][1].split()))
        self.assertIn(owner, rows['llm-tools'][2].split())
        for section in ('intents', 'triggers'):
            body = (repo / f'system-knowhow/workspace-audit/{section}.md').read_text()
            sources = json.loads(next(line[8:] for line in body.splitlines() if line.startswith('sources:')))
            self.assertTrue(any(fnmatch.fnmatchcase(owner, p) for p in sources), section)


unittest.main()
PY
