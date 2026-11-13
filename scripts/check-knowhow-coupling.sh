#!/usr/bin/env bash
# Couple main...HEAD surface changes to knowhow and declared audit sources.
# Registry format and trailer names live in docs/knowhow-surfaces.tsv.
# Reads committed inputs only. Missing or malformed inputs fail closed.
set -euo pipefail
if [ "$#" -ne 0 ]; then
    echo 'check-knowhow-coupling: takes no arguments.' >&2
    exit 1
fi
python3 - <<'PY'
import fnmatch
import json
import pathlib
import re
import subprocess
import sys


def git(*args, data=None):
    return subprocess.check_output(['git', *args], input=data, stderr=subprocess.PIPE)


def paths(ref):
    return {p.decode() for p in git('ls-tree', '-r', '--name-only', '-z', ref).split(b'\0') if p}


def read(ref, path):
    return git('show', f'{ref}:{path}').decode()


def pattern(value):
    if (not value or value.startswith('/') or any(c.isspace() for c in value)
            or any(c in value for c in '\\[]{}:')
            or any(p in ('', '.', '..') for p in value.split('/'))):
        raise ValueError(f'invalid repository glob: {value!r}')
    return value


def matches(path, globs):
    return any(fnmatch.fnmatchcase(path, glob) for glob in globs)


def declarations(ref, tracked):
    registry = 'docs/knowhow-surfaces.tsv'
    if registry not in tracked:
        raise ValueError(f'{ref}: missing tracked {registry}')
    rules = {}
    for number, line in enumerate(read(ref, registry).splitlines(), 1):
        if not line or line.startswith('#'):
            continue
        row = line.split('\t')
        if len(row) != 4 or any(not c.strip() for c in row):
            raise ValueError(f'{registry}:{number}: expected four nonempty TSV fields')
        name, sources, targets, guidance = row
        if not re.fullmatch(r'[a-z][a-z0-9-]*', name) or name in rules:
            raise ValueError(f'{registry}:{number}: invalid or duplicate surface: {name}')
        sources = [pattern(p) for p in sources.split()]
        targets = targets.split()
        for target in targets:
            pattern(target)
            if target not in tracked:
                raise ValueError(f'{registry}:{number}: missing tracked documentation: {target}')
            if matches(target, sources):
                raise ValueError(f'{registry}:{number}: documentation and sources overlap: {target}')
        rules[name] = (sources, targets, guidance)
    if not rules:
        raise ValueError(f'{registry}: empty registry')
    sections = sorted(p for p in tracked if p.startswith('system-knowhow/workspace-audit/') and p.endswith('.md'))
    if not sections:
        raise ValueError('missing workspace-audit sections')
    for section in sections:
        lines = read(ref, section).splitlines()
        if not lines or lines[0] != '---' or '---' not in lines[1:]:
            raise ValueError(f'{section}: missing frontmatter')
        fields = [line[len('sources:'):].strip() for line in lines[1:lines.index('---', 1)]
                  if line.startswith('sources:')]
        if len(fields) != 1:
            raise ValueError(f'{section}: expected one sources declaration')
        sources = json.loads(fields[0])
        if (not isinstance(sources, list) or not sources
                or any(not isinstance(p, str) for p in sources) or len(set(sources)) != len(sources)):
            raise ValueError(f'{section}: sources must be a nonempty JSON array of unique repository globs')
        name = f'audit/{pathlib.PurePosixPath(section).stem}'
        if name in rules:
            raise ValueError(f'{section}: duplicate audit surface: {name}')
        rules[name] = ([pattern(p) for p in sources], [section], f'Review {section} against its declared sources.')
        if matches(section, sources):
            raise ValueError(f'{section}: cannot declare itself as a source')
    for name, (sources, _, _) in rules.items():
        for glob in sources:
            if not any(matches(p, [glob]) for p in tracked):
                raise ValueError(f'{name}: source glob matches no tracked path: {glob}')
    return rules


def changed(*args):
    return {p.decode() for p in git('diff', '--no-renames', '--name-only', '-z', *args).split(b'\0') if p}


def main():
    root = git('rev-parse', '--show-toplevel').decode().strip()
    # All following git calls use this checkout, including nested invocations.
    import os
    os.chdir(root)
    base = git('merge-base', 'main', 'HEAD').decode().strip()
    before, after = paths(base), paths('HEAD')
    current = declarations('HEAD', after)
    versions = [current]
    # The first landing introduces the registry and declarations together.
    if 'docs/knowhow-surfaces.tsv' in before:
        versions.append(declarations(base, before))
    names = set().union(*(r.keys() for r in versions))
    diff = changed(base, 'HEAD')
    commits = []
    for commit in git('rev-list', 'main..HEAD').decode().splitlines():
        message = git('show', '-s', '--format=%B', commit)
        parsed = git('interpret-trailers', '--parse', data=message).decode().splitlines()
        exemptions = set()
        for line in parsed:
            key, _, value = line.partition(':')
            if key.lower() != 'knowhow-unchanged':
                continue
            name, colon, reason = value.strip().partition(':')
            if not colon or name not in names or not re.search(r'[A-Za-z0-9]', reason):
                raise ValueError(f'invalid Knowhow-unchanged trailer: {value.strip()}')
            exemptions.add(name)
        parents = git('rev-list', '--parents', '-n', '1', commit).decode().split()[1:]
        if len(parents) == 2:
            contribution = {p.decode() for p in git('show', '--remerge-diff', '--no-renames', '--format=',
                                                   '--name-only', '-z', commit).split(b'\0') if p}
        else:
            contribution = set.intersection(*(changed(parent, commit) for parent in parents))
        commits.append((contribution, exemptions))
    failures = set()
    for rules in versions:
        for name, (sources, targets, guidance) in rules.items():
            touched = {p for p in diff if matches(p, sources)}
            if not touched or diff.intersection(targets):
                continue
            for path in touched:
                related = [exempt for changes, exempt in commits if path in changes]
                if not related or any(name not in exemptions for exemptions in related):
                    failures.add(f'{name}: {path} needs {" or ".join(targets)} or a Knowhow-unchanged trailer'
                                 f'\n  Guidance: {guidance}')
    if failures:
        raise ValueError('\n'.join(sorted(failures)))
    print(f'knowhow-coupling: {len(diff)} changed paths checked against {len(names)} surfaces and audit sections')


try:
    main()
except (OSError, ValueError, UnicodeError, subprocess.CalledProcessError) as error:
    print(f'knowhow-coupling: {error}', file=sys.stderr)
    sys.exit(1)
PY
