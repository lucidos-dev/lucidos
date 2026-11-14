#!/usr/bin/env bash
# Hermetic reference fixtures: scratch repositories, no services or signals.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 - "$SCRIPT_DIR/../check-knowhow-refs.sh" <<'PY'
import json
import os
import re
import pathlib
import subprocess
import sys
import tempfile
import unittest

CHECK = pathlib.Path(sys.argv.pop()).resolve()
SCRATCH = CHECK.parents[1] / '.lucidos'
SCRATCH.mkdir(exist_ok=True)


class References(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=SCRATCH)
        self.addCleanup(self.temp.cleanup)
        self.root = pathlib.Path(self.temp.name)
        subprocess.run(['git', 'init', '-q', str(self.root)], check=True)
        for i in range(20):
            self.put(f'system-knowhow/doc-{i}.md', '# Reference\n')
        self.put('system-knowhow/workspace-audit.md', '| **high** | Meaning |\n')
        self.put('system-knowhow/workspace-learning.md', '# Learning\n')
        self.put('system-knowhow/workspace-audit/apps.md', '# Apps\n')
        self.put('system-knowhow/thread-events.md',
                 '| Event | When it fires | Volume | Persisted | Triggerable |\n'
                 '|---|---|---|---|---|\n'
                 '| `MessageReceived` | A message arrived. | per-action | yes | yes |\n')
        self.put('system-knowhow/lucidos-cli.md', '### `lucidos sample`\nCurrent command.\n')
        self.put('system-knowhow/js-sdk.md', '## lucidos.events: Event store\nRead events.\n')
        self.put('crates/demo/src/lib.rs', 'pub fn current() {}\n')
        self.settings = 'crates/lucidos-app/src/components/search/searchIndex.ts'
        self.put(self.settings, "const INDEX = [{ label: 'Models', path: 'Settings' }];")
        self.manifest = 'docs/generated/agent-surface.json'
        self.put(self.manifest, json.dumps({
            'schema_version': 1,
            'cli': {'name': 'lucidos', 'hidden': False, 'aliases': [], 'flags': ['help'],
                    'commands': [{'name': 'sample', 'aliases': [], 'hidden': False,
                                  'commands': [], 'flags': ['current']}]},
            'tools': {'offered': ['current_tool'], 'aliases': {'old_tool': 'current_tool'},
                      'retired': ['retired_tool']},
            'events': {'thread': ['MessageReceived'], 'system': ['AppCreated'], 'legacy': ['OldCreated']},
            'sdk_exports': ['events'],
        }))

    def put(self, name, body):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(body)
        subprocess.run(['git', '-C', str(self.root), 'add', name], check=True)

    def check(self, body, expected):
        self.put('system-knowhow/doc-0.md', body)
        result = subprocess.run(['bash', str(CHECK)], cwd=self.root, capture_output=True, text=True)
        self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
        return result.stdout + result.stderr

    def test_cli(self):
        self.check('`lucidos sample --current`', 0)
        self.check('`lucidos sample --missing`', 1)
        self.check('`lucidos sample <value> --missing`', 1)
        self.check('`lucidos missing`', 1)
        self.check('```bash\nlucidos sample --missing\n```', 1)
        self.check('lucidos sample ' + chr(92) + '\n --missing', 1)
        self.check('`lucidos sample -- <command> --missing`', 0)

    def test_reverse_event_coverage_requires_a_table_row(self):
        self.check('# Reference', 0)
        for body in ('# Events\n', '`MessageReceived` is mentioned.\n',
                     '```md\n| `MessageReceived` | Example | per-action |\n```\n',
                     '    | `MessageReceived` | Example | per-action |\n',
                     '<!--\n| `MessageReceived` | Hidden | per-action |\n-->\n'):
            self.put('system-knowhow/thread-events.md', body)
            self.assertIn('MessageReceived', self.check('# Reference', 1))

    def test_reverse_cli_coverage_requires_a_section(self):
        for body in ('# CLI\n', '`lucidos sample` in prose.\n',
                     '```md\n### `lucidos sample`\n```\n',
                     '<!-- ### `lucidos sample` -->\n',
                     '<!--\n### `lucidos sample`\n',
                     '```\n```md\n### `lucidos sample`\n```\n'):
            self.put('system-knowhow/lucidos-cli.md', body)
            self.assertIn('lucidos sample', self.check('# Reference', 1))

    def test_reverse_cli_grouped_headings_and_visibility(self):
        surface = json.loads((self.root / self.manifest).read_text())
        surface['cli']['commands'].extend([
            {'name': 'help', 'hidden': False, 'aliases': [], 'flags': [], 'commands': []},
            {'name': 'group', 'hidden': False, 'aliases': [], 'flags': [], 'commands': [
                {'name': name, 'hidden': False, 'aliases': [], 'flags': [], 'commands': []}
                for name in ('one', 'two', 'help')]},
            {'name': 'internal', 'hidden': True, 'aliases': [], 'flags': [], 'commands': [
                {'name': 'nested', 'hidden': False, 'aliases': [], 'flags': [], 'commands': []}]},
        ])
        self.put(self.manifest, json.dumps(surface))
        self.put('system-knowhow/lucidos-cli.md',
                 '### `lucidos sample`\nSample.\n'
                 '### `lucidos group one [--value A|B] | two`\nBoth commands.\n'
                 '### `lucidos help`\nHelp at every level.\n')
        self.check('# Reference', 0)
        self.put('system-knowhow/lucidos-cli.md',
                 '### `lucidos sample`\nSample.\n'
                 '### `lucidos group one` before you `two`\nOne command.\n'
                 '### `lucidos help`\nHelp.\n')
        self.assertIn('lucidos group two', self.check('# Reference', 1))
        self.put('system-knowhow/lucidos-cli.md',
                 '### `lucidos sample`\nSample.\n'
                 '### `lucidos group one` / `two`\nBoth commands.\n'
                 '### `lucidos help`\nHelp.\n')
        self.check('# Reference', 0)
        self.put('system-knowhow/lucidos-cli.md', '### `lucidos sample`\nSample.\n'
                 '### `lucidos group one`\nOne command.\n### `lucidos help`\nHelp.\n')
        self.assertIn('lucidos group two', self.check('# Reference', 1))

    def test_reverse_sdk_coverage_requires_namespace_heading(self):
        for body in ('# SDK\n', '`lucidos.events` in prose.\n',
                     '### lucidos.events\nNested heading.\n',
                     '```md\n## lucidos.events\n```\n',
                     '## lucidos.eventsExtra\nAnother namespace.\n'):
            self.put('system-knowhow/js-sdk.md', body)
            self.assertIn('lucidos.events', self.check('# Reference', 1))

    def test_reverse_missing_and_empty_docs(self):
        for name in ('thread-events', 'lucidos-cli', 'js-sdk'):
            path = self.root / ('system-knowhow/' + name + '.md')
            original = path.read_text()
            for body in (None, ''):
                if body is None:
                    path.unlink()
                else:
                    path.write_text(body)
                self.check('# Reference', 1)
            path.write_text(original)

    def test_tools(self):
        self.check('Use the `current_tool` tool.', 0)
        self.check('Use the `missing_tool` tool.', 1)
        self.check('Use the `missing_tool(value=1)` tool.', 1)
        self.check('Use the `old_tool(value=1)` tool.', 1)
        self.check('Use the `old_tool` tool.', 1)
        self.check('The `old_tool` tool is a legacy alias.', 0)
        self.check('The `retired_tool` tool is retired.', 0)
        self.check('Use the `retired_tool` tool.', 1)

    def test_qualifiers_apply_to_the_reference_not_the_whole_line(self):
        self.check('Use `lucidos sample --missing` to list invalid records.', 1)
        self.check('Use the `missing_tool` tool with no arguments.', 1)
        self.check('This is not a list of events: use `MissingCreated`.', 1)
        self.check('Removed `lucidos sample --missing`.', 0)
        self.check('There is no `missing_tool` tool.', 0)
        self.check('Call `missing_tool(value=1)`.', 1)

    def test_audit_event_names_keep_the_original_strict_check(self):
        for body in ('`MessageRecieved`', 'Retired event renames: `ContextAssembled` → `AppCreated`'):
            self.put('system-knowhow/workspace-audit/apps.md', body)
            self.check('# Reference', 1)
        self.put('system-knowhow/workspace-audit/apps.md', '# Reference')

    def test_events_and_scoped_domain_examples(self):
        self.check('`MessageReceived`, `AppCreated`', 0)
        self.check('`OldCreated`', 1)
        self.check('Legacy alias: `OldCreated`.', 0)
        self.check('on_event: OldCreated', 1)
        self.check('`MissingCreated`', 1)
        self.check('The event `InventedName`.', 1)
        self.check('on_event: InventedName', 1)
        self.check('There is no `InventedName` event.', 0)
        self.check('<!-- domain-event: SampleCreated: Example emitted by this recipe. -->\n`SampleCreated`', 0)
        self.put('system-knowhow/doc-1.md', '`SampleCreated`')
        self.check('<!-- domain-event: SampleCreated: Example emitted by this recipe. -->', 1)
        self.put('system-knowhow/doc-1.md', '# Reference')
        self.check('<!-- domain-event: SampleCreated: -->\n`SampleCreated`', 1)

    def test_symbols(self):
        self.check('`crates/demo/src/lib.rs::current`', 0)
        self.check('`crates/demo/src/lib.rs::missing`', 1)
        self.check('`crates/demo/src/missing.rs::current`', 1)
        self.check('`crates/demo/src/lib.rs::current()`', 0)
        self.check('`crates/demo/src/lib.rs::missing()`', 1)

    def test_cli_event_alias_requires_a_qualifier(self):
        surface = json.loads((self.root / self.manifest).read_text())
        surface['cli']['commands'].append({
            'name': 'events', 'aliases': [], 'hidden': False, 'flags': [],
            'commands': [{'name': 'emit', 'aliases': [], 'hidden': False, 'flags': [], 'commands': []}],
        })
        self.put(self.manifest, json.dumps(surface))
        self.put('system-knowhow/lucidos-cli.md', '### `lucidos sample`\nSample.\n'
                 '### `lucidos events emit`\nEmit a domain event.\n')
        self.check('`lucidos events emit OldCreated`', 1)
        self.check('Legacy example: `lucidos events emit OldCreated`.', 0)
        self.check('`lucidos events emit InventedName`', 1)

    def test_settings(self):
        self.check('Open **Settings → Models**.', 0)
        self.check('Open **Settings → Missing**.', 1)
        self.check('Settings →\nModels shows the registry.', 0)
        self.check('System Settings → Notifications.', 0)
        self.check('Open Settings > Models.', 0)
        self.check('Open Settings > Missing.', 1)

    def test_links(self):
        self.check('[reference](doc-1.md)', 0)
        self.check('[reference](missing.md)', 1)
        self.check('`![example](artifacts/sample.png)`', 0)
        self.check('```md\n![example](artifacts/sample.png)\n```', 0)
        self.check('[web](https://example.com) [anchor](#section)', 0)

    def test_missing_or_empty_inputs(self):
        for name in (self.manifest, self.settings, 'crates/demo/src/lib.rs'):
            path = self.root / name
            original = path.read_text()
            for body in (None, ''):
                if body is None:
                    path.unlink()
                else:
                    path.write_text(body)
                self.check('`crates/demo/src/lib.rs::current`', 1)
            path.write_text(original)

    def test_empty_or_malformed_surface_arms(self):
        path = self.root / self.manifest
        original = json.loads(path.read_text())
        for key in ('cli', 'tools', 'events', 'sdk_exports'):
            broken = dict(original)
            broken[key] = {}
            path.write_text(json.dumps(broken))
            self.check('# Reference', 1)

    def test_harden_runs_lint_for_a_knowhow_only_diff_and_propagates_failure(self):
        source = (CHECK.parent / 'harden-gates.sh').read_text()
        for script in set(re.findall(r'\./(scripts/[A-Za-z0-9_/-]+\.sh)', source)):
            self.put(script, '#!/bin/bash\ntrue\n')
            (self.root / script).chmod(0o755)
        self.put('scripts/harden-gates.sh', source)
        (self.root / 'scripts/harden-gates.sh').chmod(0o755)
        self.put('bin/make', '#!/bin/bash\nset -e\n[ "$1" = lint-knowhow ]\ntouch lint-ran\n')
        (self.root / 'bin/make').chmod(0o755)
        subprocess.run(['git', 'add', '-A'], cwd=self.root, check=True)
        subprocess.run(['git', '-c', 'user.name=Test', '-c', 'user.email=test@example.com',
                        'commit', '-qm', 'fixture'], cwd=self.root, check=True)
        self.put('system-knowhow/doc-0.md', '# Changed knowhow')
        changed = subprocess.check_output(['git', 'diff', 'HEAD', '--name-only'], cwd=self.root, text=True)
        self.assertEqual(changed.strip(), 'system-knowhow/doc-0.md')
        env = dict(os.environ, PATH=str(self.root / 'bin') + os.pathsep + os.environ['PATH'])
        result = subprocess.run(['bash', 'scripts/harden-gates.sh'], cwd=self.root,
                                env=env, text=True, capture_output=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue((self.root / 'lint-ran').is_file())
        self.put('bin/make', '#!/bin/bash\nfalse\n')
        result = subprocess.run(['bash', 'scripts/harden-gates.sh'], cwd=self.root,
                                env=env, text=True, capture_output=True)
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)


unittest.main()
PY
