#!/usr/bin/env bash
# Check tracked text against docs/retired-terms.tsv (ADR 0565).
# Legacy or retired wording on the same line qualifies a mention.
# History and wire exceptions live in docs/retired-term-exceptions.tsv.
# Missing inputs, malformed rules and unreadable tracked files fail closed.
# Run from any directory inside the checkout. No cargo or network access.
set -euo pipefail

if [ "$#" -ne 0 ]; then
    echo 'check-retired-terms: takes no arguments.' >&2
    exit 1
fi

python3 - <<'PY'
import csv
import fnmatch
import pathlib
import re
import subprocess
import sys


def git(*args):
    return subprocess.check_output(['git', *args])


def rows(path, width):
    parsed = []
    for number, row in enumerate(csv.reader(path.read_text().splitlines(), delimiter='\t'), 1):
        if not row or row[0].startswith('#'):
            continue
        if len(row) != width or any(not cell.strip() for cell in row):
            raise ValueError(f'{path}:{number}: expected {width} nonempty tab-separated fields')
        parsed.append(row)
    if not parsed:
        raise ValueError(f'{path}: registry is empty')
    return parsed


def pattern(value):
    compiled = re.compile(value, re.IGNORECASE)
    if compiled.search(''):
        raise ValueError(f'pattern matches empty text: {value}')
    return compiled


def main():
    root = pathlib.Path(git('rev-parse', '--show-toplevel').decode().strip())
    paths = [pathlib.Path(p.decode()) for p in git('-C', str(root), 'ls-files', '-z').split(b'\0') if p]
    if not paths:
        raise ValueError('no tracked files')
    registry = pathlib.Path('docs/retired-terms.tsv')
    exceptions_path = pathlib.Path('docs/retired-term-exceptions.tsv')
    for required in (registry, exceptions_path):
        if required not in paths:
            raise ValueError(f'{required}: required registry is not tracked')
    terms = []
    for expression, replacement, adr in rows(root / registry, 3):
        if pathlib.Path(adr) not in paths or not (root / adr).is_file():
            raise ValueError(f'{registry}: missing tracked ADR: {adr}')
        terms.append((pattern(expression), replacement, adr))
    exceptions = []
    for glob, kind, context, reason in rows(root / exceptions_path, 4):
        if kind not in ('history', 'literal'):
            raise ValueError(f'{exceptions_path}: unknown exception kind: {kind}')
        if kind == 'history' and context != '-':
            raise ValueError(f'{exceptions_path}: history context must be -')
        if not any(fnmatch.fnmatchcase(p.as_posix(), glob) for p in paths):
            raise ValueError(f'{exceptions_path}: exception matches no tracked path: {glob}')
        exceptions.append((glob, None if kind == 'history' else pattern(context), reason))
    qualifier = re.compile(r'\b(?:legacy|retired)\b', re.IGNORECASE)
    violations = []
    checked = 0
    for relative in paths:
        if relative in (registry, exceptions_path):
            continue
        path = root / relative
        if path.is_symlink():
            continue
        data = path.read_bytes()
        if b'\0' in data:
            continue
        content = data.decode('utf-8')
        allowed = [context for glob, context, _ in exceptions
                   if fnmatch.fnmatchcase(relative.as_posix(), glob)]
        if None in allowed:
            continue
        checked += 1
        for number, line in enumerate(content.splitlines(), 1):
            if qualifier.search(line):
                continue
            for term, replacement, adr in terms:
                for match in term.finditer(line):
                    if any(context.start() <= match.start() and context.end() >= match.end()
                           for rule in allowed for context in rule.finditer(line)):
                        continue
                    violations.append(f'{relative}:{number}: {match.group()!r}: use {replacement} ({adr})')
    if violations:
        print('\n'.join(violations), file=sys.stderr)
        print(f'retired-terms: {len(violations)} unqualified mentions', file=sys.stderr)
        return 1
    print(f'retired-terms: {len(terms)} rules, {checked} tracked text files checked')
    return 0


try:
    sys.exit(main())
except (OSError, UnicodeError, ValueError, re.error, subprocess.CalledProcessError) as error:
    print(f'retired-terms: cannot check: {error}', file=sys.stderr)
    sys.exit(1)
PY
