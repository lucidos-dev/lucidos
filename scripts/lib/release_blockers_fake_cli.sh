#!/bin/bash
# Test helper: a fake `lucidos` that serves release blocker events from a file.
# Put its directory FIRST on PATH, so no test reaches a live workspace.
# It answers only `events count` and `events query`, and refuses anything else.
#
#   release_blockers_fake_cli <bin-dir> <events-file>
#   ledger_event <type> <device|agent> <payload-json-members>
#
# ledger_event numbers each record from the file, so a subshell may append too.
# It leaves the new record's sequence in FAKE_LEDGER_SEQUENCE.
#
# FAKE_LEDGER_MODE: unset, `fail` (every call exits 1), `garbage` (no JSON) or
# `moving` (each count call reports one more record than the last).

release_blockers_fake_cli() {
    local bin="$1"
    FAKE_LEDGER_EVENTS="$2"
    export FAKE_LEDGER_EVENTS
    : > "$FAKE_LEDGER_EVENTS"
    mkdir -p "$bin"
    cat > "$bin/lucidos" <<'FAKE'
#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
mode = os.environ.get("FAKE_LEDGER_MODE", "")
if mode == "fail":
    sys.exit("fake lucidos: failing on purpose")
def flag(name):
    return args[args.index(name) + 1] if name in args else None
events = [json.loads(line) for line in open(os.environ["FAKE_LEDGER_EVENTS"]) if line.strip()]
if args[:2] == ["events", "count"]:
    n = sum(1 for e in events if e["event_type"] == flag("--type"))
    if mode == "moving":
        state = os.environ["FAKE_LEDGER_EVENTS"] + ".moves"
        moves = int(open(state).read()) + 1 if os.path.exists(state) else 0
        open(state, "w").write(str(moves))
        n += moves
    print("not json" if mode == "garbage" else json.dumps({"count": n}))
elif args[:2] == ["events", "query"]:
    rows = sorted((e for e in events if e["event_type"] == flag("--type")), key=lambda e: -e["sequence"])
    before = flag("--before-event-id")
    if before:
        rows = rows[[e["id"] for e in rows].index(before) + 1:]
    print(json.dumps(rows[: int(flag("--limit"))]))
else:
    sys.exit("fake lucidos: refusing an unserved command: " + " ".join(args))
FAKE
    chmod +x "$bin/lucidos"
}

ledger_event() {
    local actor='{"kind":"api","mode":"agent"}'
    [ "$2" != device ] || actor='{"kind":"device"}'
    FAKE_LEDGER_SEQUENCE=$((101 + $(grep -c . "$FAKE_LEDGER_EVENTS" || true)))
    printf '{"id":"00000000-0000-4000-8000-%012d","event_type":"%s","sequence":%d,"payload":{"v":1,"actor":%s%s}}\n' \
        "$FAKE_LEDGER_SEQUENCE" "$1" "$FAKE_LEDGER_SEQUENCE" "$actor" "${3:+,$3}" >> "$FAKE_LEDGER_EVENTS"
}
