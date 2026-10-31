#!/usr/bin/env bash
# Block fixtures operate only on temporary files and scratch git repositories.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 - "$SCRIPT_DIR" <<'PY'
import importlib.util
import json
import pathlib
import sys
import tempfile
import unittest

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('blocks', pathlib.Path(sys.argv.pop()) / 'codegen_blocks.py')
blocks = importlib.util.module_from_spec(spec)
spec.loader.exec_module(blocks)

BEGIN = b'<!-- generated:begin Sample -->'
END = b'<!-- generated:end -->'
FRESH = b'\n`fresh`\n'


class Blocks(unittest.TestCase):
    def test_splice_preserves_every_surrounding_byte(self):
        before = 'Handwritten café\r\n\x00'.encode()
        after = b'\r\nUntouched tail without newline'
        document = before + BEGIN + b'\nstale\n' + END + after
        expected = before + BEGIN + FRESH + END + after
        self.assertEqual(blocks.splice(document, {'Sample': FRESH}), expected)
        self.assertEqual(blocks.splice(expected, {'Sample': FRESH}), expected)

    def test_markers_fail_closed(self):
        valid = BEGIN + FRESH + END
        for document in (b'', BEGIN, END, END + BEGIN, valid + valid,
                         BEGIN + BEGIN + END + END,
                         valid + b'<!-- generated:begin Unknown -->x' + END,
                         b'<!-- generated:begin Sample-->x' + END,
                         valid + b'<!-- generated:end extra -->'):
            with self.subTest(document=document), self.assertRaises(ValueError):
                blocks.splice(document, {'Sample': FRESH})

    def test_comparison_ignores_prose_but_checks_block_bytes(self):
        document = b'Edited prose\n' + BEGIN + FRESH + END + b'new tail'
        blocks.check(document, {'Sample': FRESH})
        with self.assertRaises(ValueError):
            blocks.check(document.replace(b'fresh', b'stale'), {'Sample': FRESH})

    def test_invalid_manifests_fail_closed(self):
        manifest = {'schema_version': 1, 'documents': {
            'system-knowhow/sample.md': {'Sample': FRESH.decode()}}}
        invalid = ('', '{}', '[]', 'null', '{"schema_version":1,"documents":{}}',
                   json.dumps(manifest).replace('system-knowhow/sample.md', '../escape.md'),
                   json.dumps(manifest).replace('Sample', 'bad name'),
                   json.dumps(manifest).replace('fresh', '<!-- generated:end -->'),
                   '{"schema_version":1,"schema_version":1,"documents":{}}')
        with tempfile.TemporaryDirectory() as directory:
            path = pathlib.Path(directory) / 'fragments.json'
            for body in invalid:
                path.write_text(body)
                with self.subTest(body=body), self.assertRaises(ValueError):
                    list(blocks.load(path))

    def test_sync_validates_all_documents_before_writing(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / 'system-knowhow').mkdir()
            first = root / 'system-knowhow/first.md'
            first.write_bytes(BEGIN + b'\nstale\n' + END)
            second = root / 'system-knowhow/second.md'
            second.write_text('Missing markers')
            manifest = root / 'fragments.json'
            manifest.write_text(json.dumps({'schema_version': 1, 'documents': {
                'system-knowhow/first.md': {'Sample': FRESH.decode()},
                'system-knowhow/second.md': {'Sample': FRESH.decode()}}}))
            before = first.read_bytes()
            with self.assertRaises(ValueError):
                blocks.run('sync', manifest, root)
            self.assertEqual(first.read_bytes(), before)

    def test_sync_rejects_markers_in_unlisted_documents_before_writing(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / 'system-knowhow').mkdir()
            first = root / 'system-knowhow/first.md'
            first.write_bytes(BEGIN + b'\nstale\n' + END)
            (root / 'system-knowhow/unlisted.md').write_bytes(BEGIN + FRESH + END)
            manifest = root / 'fragments.json'
            manifest.write_text(json.dumps({'schema_version': 1, 'documents': {
                'system-knowhow/first.md': {'Sample': FRESH.decode()}}}))
            before = first.read_bytes()
            with self.assertRaises(ValueError):
                blocks.run('sync', manifest, root)
            self.assertEqual(first.read_bytes(), before)


unittest.main()
PY
