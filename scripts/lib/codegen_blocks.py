"""Splice cached fragments, or compare them with committed generated blocks."""

import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile


def spans(document, expected):
    found = {}
    opened = None
    for token in re.finditer(rb'<!--\s*generated:[\s\S]*?(?:-->|$)', document):
        begin = re.fullmatch(rb'<!-- generated:begin ([A-Za-z][A-Za-z0-9_-]*) -->', token[0])
        if begin:
            name = begin[1].decode('ascii')
            if opened or name in found or name not in expected:
                raise ValueError(f'duplicate, nested or unknown generated block: {name}')
            opened = (name, token.end())
        elif token[0] == b'<!-- generated:end -->' and opened:
            name, start = opened
            found[name] = (start, token.start())
            opened = None
        else:
            raise ValueError('malformed or unmatched generated marker')
    if opened or found.keys() != expected.keys():
        raise ValueError('missing generated block or end marker')
    return found


def splice(document, fragments):
    result = document
    for name, (start, end) in sorted(spans(document, fragments).items(),
                                    key=lambda entry: entry[1][0], reverse=True):
        result = result[:start] + fragments[name] + result[end:]
    return result


def check(document, fragments):
    for name, (start, end) in spans(document, fragments).items():
        if document[start:end] != fragments[name]:
            raise ValueError(f'committed generated block {name} is stale; run ./scripts/codegen.sh and commit it')


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f'duplicate fragment key: {key}')
        result[key] = value
    return result


def load(path):
    manifest = json.loads(path.read_bytes(), object_pairs_hook=unique_object)
    if (not isinstance(manifest, dict) or manifest.get('schema_version') != 1
            or not isinstance(manifest.get('documents'), dict) or not manifest['documents']):
        raise ValueError('empty or malformed block fragment manifest')
    for name, fragments in manifest['documents'].items():
        path = pathlib.PurePosixPath(name)
        if (not name.startswith('system-knowhow/') or not name.endswith('.md')
                or '..' in path.parts or '\\' in name or str(path) != name):
            raise ValueError(f'invalid generated document path: {name}')
        if not isinstance(fragments, dict) or not fragments:
            raise ValueError(f'empty or malformed fragments: {name}')
        for key, value in fragments.items():
            if (not re.fullmatch(r'[A-Za-z][A-Za-z0-9_-]*', key)
                    or not isinstance(value, str) or not value.strip()
                    or '<!--' in value or not value.startswith('\n') or not value.endswith('\n')):
                raise ValueError(f'invalid generated fragment: {key}')
        yield name, {key: value.encode('utf-8') for key, value in fragments.items()}


def run(mode, manifest, root):
    documents = dict(load(manifest))
    if mode == 'check':
        tracked = subprocess.run(['git', '-C', str(root), 'ls-tree', '-r', '--name-only',
                                  '-z', 'HEAD', '--', 'system-knowhow/'],
                                 check=True, capture_output=True).stdout
        names = {os.fsdecode(name) for name in tracked.split(b'\0') if name.endswith(b'.md')}
    else:
        names = {path.relative_to(root).as_posix()
                 for path in (root / 'system-knowhow').rglob('*.md')}
    updates = []
    for name in sorted(names | documents.keys()):
        fragments = documents.get(name, {})
        path = root / name
        if mode == 'check':
            document = subprocess.run(['git', '-C', str(root), 'show', f'HEAD:{name}'],
                                      check=True, capture_output=True).stdout
            check(document, fragments)
        else:
            document = path.read_bytes()
            rendered = splice(document, fragments)
            if rendered != document:
                updates.append((path, rendered))
    for path, rendered in updates:
        fd, temporary = tempfile.mkstemp(dir=path.parent, prefix=path.name + '.')
        try:
            with os.fdopen(fd, 'wb') as output:
                output.write(rendered)
            os.chmod(temporary, path.stat().st_mode)
            os.replace(temporary, path)
        finally:
            if os.path.exists(temporary):
                os.unlink(temporary)


if __name__ == '__main__':
    try:
        if len(sys.argv) != 4 or sys.argv[1] not in ('sync', 'check'):
            raise ValueError('usage: codegen_blocks.py sync|check <fragments.json> <repo-root>')
        run(sys.argv[1], pathlib.Path(sys.argv[2]), pathlib.Path(sys.argv[3]))
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        sys.exit(f'knowhow-generated: {error}')
