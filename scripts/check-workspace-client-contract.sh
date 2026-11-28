#!/usr/bin/env bash
# Compare the workspace-client wire surface with its committed record.
#
#   ./scripts/check-workspace-client-contract.sh            check
#   ./scripts/check-workspace-client-contract.sh --accept   record the tree
#
# A peer gateway serves its own client against this surface (trusted pairing),
# so a silent change can break a paired client. The surface is the engine
# /api/v1 route table, the thread and system event names, and the generated
# wire files. The record sits beside VERSION and names the VERSION it saw.
#
# Any difference fails. A breaking change bumps VERSION in
# crates/lucidos-gateway-stamp/src/workspace_client.rs first. A compatible
# change keeps it. Both then run --accept and commit the record.
# Missing inputs fail closed. No cargo or network access.
set -euo pipefail
MODE=check
case "$*" in
    "") ;;
    --accept) MODE=accept ;;
    *) echo 'check-workspace-client-contract: takes only --accept.' >&2; exit 1 ;;
esac
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"
python3 - "$MODE" <<'PY'
import difflib
import hashlib
import json
import pathlib
import re
import sys

MODE = sys.argv[1]
STAMP = pathlib.Path('crates/lucidos-gateway-stamp/src/workspace_client.rs')
RECORD = pathlib.Path('crates/lucidos-gateway-stamp/workspace-client-surface.txt')
ROUTES = pathlib.Path('crates/lucidos-agents-api/src')
SURFACE = pathlib.Path('docs/generated/agent-surface.json')
WIRE = [pathlib.Path('crates/lucidos-app/src/generated/thread-event-wire.ts'),
        pathlib.Path('crates/lucidos-app/src/generated/thread-lifecycle.ts')]
METHOD = re.compile(r'(?<![\w])(get|post|put|patch|delete|head|options|any)\s*\(')
CALL = re.compile(r'\.(route|nest)\(\s*"([^"]*)"')


def fail(message):
    raise SystemExit('workspace-client-contract: ' + message)


def version():
    found = re.search(r'^pub const VERSION: u32 = (\d+);', STAMP.read_text(), re.M)
    if not found:
        fail(f'no VERSION in {STAMP}')
    return found.group(1)


def production(path):
    """Source without test code. route_scan.rs quotes the pattern it hunts for."""
    name = path.name
    if name in ('tests.rs', 'route_scan.rs') or name.endswith('_tests.rs') or 'tests' in path.parts:
        return None
    text = path.read_text()
    text = text.split('#[cfg(test)]', 1)[0]
    return re.sub(r'//[^\n]*', '', text)


def methods(text, start):
    depth, found, i = 1, [], start
    while i < len(text) and depth:
        char = text[i]
        if char == '(':
            depth += 1
        elif char == ')':
            depth -= 1
        elif depth == 1:
            match = METHOD.match(text, i)
            if match and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == '_')):
                found.append(match.group(1))
                i = match.end()
                depth += 1
                continue
        i += 1
    return sorted(set(found))


def routes():
    lines = set()
    files = sorted(ROUTES.rglob('*.rs'))
    if not files:
        fail(f'no sources under {ROUTES}')
    for path in files:
        text = production(path)
        if text is None:
            continue
        for match in CALL.finditer(text):
            kind, route = match.groups()
            if kind == 'nest':
                lines.add(f'nest {route}')
            else:
                lines.add(f'route {route} ' + ','.join(methods(text, match.end())))
    if not lines:
        fail(f'no routes found under {ROUTES}')
    return sorted(lines)


def events():
    surface = json.loads(SURFACE.read_text())['events']
    names = [f'event {kind} {name}' for kind in ('thread', 'system') for name in surface[kind]]
    if not names:
        fail(f'no events in {SURFACE}')
    return sorted(names)


def wire():
    return [f'wire {path} {hashlib.sha256(path.read_bytes()).hexdigest()}' for path in WIRE]


try:
    current = [f'version {version()}', *routes(), *events(), *wire()]
except (OSError, KeyError, ValueError) as error:
    fail(f'unreadable input: {error}')
body = '\n'.join(current) + '\n'

if MODE == 'accept':
    RECORD.write_text(body)
    print(f'✓ recorded the workspace-client surface in {RECORD}')
    sys.exit(0)

if not RECORD.is_file():
    fail(f'{RECORD} is missing. Run --accept and commit it.')
recorded = RECORD.read_text()
if recorded == body:
    print('✓ workspace-client surface matches its record')
    sys.exit(0)
diff = ''.join(difflib.unified_diff(recorded.splitlines(True), body.splitlines(True),
                                    str(RECORD), 'tree', n=0))
sys.stderr.write(diff)
fail('the wire surface changed. Breaking for an older client? Bump VERSION in '
     f'{STAMP} first. Then run ./scripts/check-workspace-client-contract.sh --accept '
     'and commit the record.')
PY
