"""Exact identity, auth transitions and immutable overlay regressions."""
from copy import deepcopy
import importlib.util
import json
import subprocess
import sys
import tempfile
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location('recovery', Path(__file__).with_name('recover-billing.py'))
r = importlib.util.module_from_spec(spec)
spec.loader.exec_module(r)


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        self.row = dict(id='00000000-0000-0000-0000-000000000001',
                        thread_id='00000000-0000-0000-0000-000000000002', sequence=5,
                        request_event_id=None, original_event_sha256='a' * 64,
                        producer='codex', provider=None, usage={'input_tokens': 50, 'output_tokens': 3})
        self.snapshot = {'captures': [self.row]}
        self.item = dict(capture=r.identity(self.row), producer='codex', scope='exact-capture',
                         source='runtime-observation', source_sha256='b' * 64, status='known',
                         provider='chatgpt-plan', billing_mode='plan')

    def run_recovery(self, *items):
        return r.recover(self.snapshot, {'schema_version': 1, 'observations': list(items)})

    def test_exact_match_idempotence_and_token_cost_parity(self):
        before = deepcopy(self.snapshot)
        result = self.run_recovery(self.item)
        self.assertEqual(result, self.run_recovery(self.item))
        self.assertEqual(self.snapshot, before)
        self.assertEqual(result['parity']['usage_totals'], {'input_tokens': 50, 'output_tokens': 3})
        self.assertEqual(result['proven'][0]['billing_mode'], 'plan')
        self.assertIn('no repricing', result['parity']['cost_effect'])

    def test_each_identity_component_required(self):
        for key in r.IDENTITY:
            with self.subTest(key=key):
                item = deepcopy(self.item)
                item['capture'][key] = 99 if key == 'sequence' else ('c' * 64 if 'sha256' in key else '00000000-0000-0000-0000-000000000009')
                with self.assertRaises(ValueError):
                    self.run_recovery(item)

    def test_conflicting_and_changed_auth_not_collapsed(self):
        other = dict(self.item, provider='openai', billing_mode='token-metered')
        result = self.run_recovery(self.item, other)
        self.assertEqual(result['proven'], [])
        self.assertEqual(result['unresolved'][0]['reason'], 'conflicting-evidence')
        result = self.run_recovery(self.item, dict(self.item, status='unresolved'))
        self.assertEqual(result['proven'], [])

    def test_auth_change_between_captures_stays_scoped(self):
        row = dict(self.row, id='00000000-0000-0000-0000-000000000003', sequence=6)
        self.snapshot['captures'].append(row)
        second = dict(self.item, capture=r.identity(row), provider='openai', billing_mode='token-metered')
        result = self.run_recovery(self.item, second)
        self.assertEqual([x['provider'] for x in result['proven']], ['chatgpt-plan', 'openai'])

    def test_no_session_date_or_current_setting_inference(self):
        with self.assertRaises(ValueError):
            self.run_recovery(dict(self.item, scope='session-interval'))
        self.assertEqual(self.run_recovery()['unresolved'][0]['reason'], 'no-bound-evidence')
        result = self.run_recovery(dict(self.item, status='transport-only'))
        self.assertEqual(result['proven'], [])
        self.assertEqual(len(result['partial']), 1)

    def test_reject_wrong_producer_mode_and_duplicate_capture(self):
        for changes in ({'producer': 'claude_code'}, {'billing_mode': 'token-metered'},
                        {'provider': 'anthropic-subscription'}):
            with self.assertRaises(ValueError):
                self.run_recovery(dict(self.item, **changes))
        self.snapshot['captures'].append(deepcopy(self.row))
        with self.assertRaises(ValueError):
            self.run_recovery()

    def test_cache_splits_preserved_and_usage_metadata_not_exported(self):
        self.row['usage'].update(cache_creation_30m_tokens=5, cache_creation_12h_tokens=7,
                                 service_tier='priority', cache_creation_requested_ttl='12h')
        result = self.run_recovery()
        self.assertEqual(result['parity']['usage_totals']['cache_creation_12h_tokens'], 7)
        self.assertNotIn('priority', str(result))
        self.row['usage']['unexpected-secret-field'] = 1
        with self.assertRaises(ValueError):
            self.run_recovery()

    def test_secret_extras_never_output(self):
        item = dict(self.item, api_key='secret-value', endpoint='private-value')
        self.assertNotIn('secret-value', str(self.run_recovery(item)))
        self.assertNotIn('private-value', str(self.run_recovery(item)))
        result = self.run_recovery(self.item, self.item)
        self.assertEqual(len(result['proven'][0]['provenance']), 1)

    def test_cli_dry_run_exclusive_output_and_repeatability(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            captures, evidence = root / 'captures.json', root / 'evidence.json'
            captures.write_text(json.dumps(self.snapshot))
            evidence.write_text(json.dumps({'schema_version': 1, 'observations': [self.item]}))
            originals = (captures.read_bytes(), evidence.read_bytes())
            command = [sys.executable, str(Path(r.__file__)), '--captures', str(captures),
                       '--evidence', str(evidence)]
            dry = subprocess.run(command, check=True, capture_output=True, text=True)
            self.assertEqual(len(list(root.iterdir())), 2)
            first, second = root / 'first.json', root / 'second.json'
            for output in (first, second):
                result = subprocess.run(command + ['--output', str(output)], check=True,
                                        capture_output=True, text=True)
                self.assertEqual(dry.stdout, result.stdout)
            self.assertEqual(first.read_bytes(), second.read_bytes())
            failure = subprocess.run(command + ['--output', str(captures)], capture_output=True)
            self.assertNotEqual(failure.returncode, 0)
            self.assertEqual(originals, (captures.read_bytes(), evidence.read_bytes()))


if __name__ == '__main__':
    unittest.main()
