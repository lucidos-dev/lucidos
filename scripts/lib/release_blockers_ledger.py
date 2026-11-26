#!/usr/bin/env python3
"""Read a repository's release blocker ledger out of the workspace event store.

The Software Factory app writes the records. This is their strictest reader:
any doubt about the input exits 1, which the release gate turns into a refusal.

Usage: release_blockers_ledger.py --snapshot FILE --root HEX [--root HEX ...]
                                  [--until-sequence N]

Exit 0 writes the snapshot (the old TSV header and one row per blocker) to
FILE and prints `ledger <root> sequence <n>`, then one line per unconfirmed
record. Exit 1 prints `ERROR:` lines. Release judgment stays in
release_blockers.sh, which owns git ancestry.
"""

import argparse
import json
import re
import subprocess
import sys

LEDGER = "ReleaseBlockerLedgerOpened"
OPENED = "ReleaseBlockerOpened"
FIXED = "ReleaseBlockerFixed"
REOPENED = "ReleaseBlockerReopened"
WITHDRAWN = "ReleaseBlockerWithdrawn"
WAIVED = "ReleaseBlockerWaived"
VOIDED = "ReleaseBlockerRecordVoided"
ALL_TYPES = (LEDGER, OPENED, FIXED, REOPENED, WITHDRAWN, WAIVED, VOIDED)
DEVICE_ONLY = {LEDGER, WITHDRAWN, WAIVED, VOIDED}
HEADER = "id\tstate\tref\twork_item\ttitle\treason"
PAGE = 1000
MAX_SAFE = 2**53 - 1

HEX40 = re.compile(r"[0-9a-f]{40}")
COMMIT = re.compile(r"[0-9a-f]{7,40}")
BLOCKER_ID = re.compile(r"[a-z0-9]+(-[a-z0-9]+)*")
VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+")
UUID = re.compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}")
THREAD = re.compile(r"(?:[A-Za-z0-9._-]+@[A-Za-z0-9._-]+/)?" + UUID.pattern)
TOKEN = re.compile(r"[!-~]+")
CONTROL = re.compile(r"[\x00-\x1f\x7f-\x9f\u2028\u2029]")


class LedgerError(Exception):
    pass


def matches(pattern, value):
    return isinstance(value, str) and pattern.fullmatch(value) is not None


def is_line(value):
    return isinstance(value, str) and value.strip() not in ("", "-") and not CONTROL.search(value)


def is_device(event):
    actor = event["payload"].get("actor")
    return isinstance(actor, dict) and actor.get("kind") == "device"


def is_v1(value):
    return type(value) in (int, float) and value == 1


def field_problem(kind, p):
    if kind == OPENED:
        if not is_line(p.get("title")):
            return "bad title"
        work_item = p.get("work_item")
        if work_item is not None and not (matches(TOKEN, work_item) and work_item != "-"):
            return "bad work_item"
        if p.get("thread_id") is not None and not matches(THREAD, p.get("thread_id")):
            return "bad thread_id"
        return None
    if kind == FIXED:
        return None if matches(COMMIT, p.get("commit")) else "bad commit"
    if kind in (REOPENED, WITHDRAWN):
        return None if is_line(p.get("reason")) else "bad reason"
    if kind == WAIVED:
        if not matches(VERSION, p.get("version")):
            return "bad version"
        return None if is_line(p.get("reason")) else "bad reason"
    if kind == VOIDED:
        if not matches(UUID, p.get("event_id")):
            return "bad event_id"
        return None if is_line(p.get("reason")) else "bad reason"
    return None


def readable_ledger(p):
    return p.get("ledger") if matches(HEX40, p.get("ledger")) else None


def envelope_problem(p):
    if not readable_ledger(p):
        return "no readable ledger"
    if not is_v1(p.get("v")):
        return "unknown v"
    return None


class Ledger:
    def __init__(self, key):
        self.key = key
        self.opened = None
        self.blockers = {}
        self.errors = []
        self.unconfirmed = []


def sort_events(events):
    if not isinstance(events, list):
        raise LedgerError("release blocker events are not a list")
    seen = set()
    for e in events:
        ok = (
            isinstance(e, dict)
            and isinstance(e.get("id"), str)
            and type(e.get("sequence")) is int
            and 0 <= e["sequence"] <= MAX_SAFE
            and isinstance(e.get("payload"), dict)
        )
        if not ok:
            raise LedgerError("a release blocker event has no id, sequence or payload")
        if e["id"] in seen:
            raise LedgerError(f"release blocker event {e['id']} appears twice")
        seen.add(e["id"])
    return sorted((e for e in events if e.get("event_type") in ALL_TYPES), key=lambda e: e["sequence"])


def collect_voids(events):
    voids = {}
    for e in events:
        p = e["payload"]
        if e["event_type"] != VOIDED or not is_device(e):
            continue
        if envelope_problem(p) or field_problem(VOIDED, p):
            continue
        voids.setdefault(p["event_id"], []).append(e)
    return voids


def evaluate(e, ledger_of):
    """Classify one record: ok (with an effect), void, unconfirmed or rejected."""
    p, kind = e["payload"], e["event_type"]
    key = readable_ledger(p)
    if kind in DEVICE_ONLY and not is_device(e):
        return {"kind": "unconfirmed", "ledger": key, "problem": envelope_problem(p) or field_problem(kind, p)}
    env = envelope_problem(p)
    if env:
        return {"kind": "rejected", "scope": "ledger" if key else "global", "ledger": key, "problem": env}
    ledger = ledger_of(key)
    if kind == VOIDED:
        problem = field_problem(kind, p)
        if problem:
            return {"kind": "rejected", "scope": "ledger", "ledger": key, "problem": problem}
        return {"kind": "void", "ledger": key}
    if kind == LEDGER:
        if ledger.opened:
            return {"kind": "rejected", "scope": "ledger", "ledger": key, "problem": "ledger opened twice"}
        return {"kind": "ok", "ledger": key, "apply": lambda: setattr(ledger, "opened", e)}
    blocker_id = p.get("blocker_id")
    if not matches(BLOCKER_ID, blocker_id):
        return {"kind": "rejected", "scope": "ledger", "ledger": key, "problem": "no usable blocker_id"}
    known = ledger.blockers.get(blocker_id)
    problem = field_problem(kind, p)
    if kind == OPENED:
        if known:
            return {"kind": "rejected", "scope": "blocker", "ledger": key, "blocker": blocker_id, "problem": "opened twice"}
        if problem:
            return {"kind": "rejected", "scope": "ledger", "ledger": key, "problem": problem + " on a new blocker"}

        def open_blocker():
            ledger.blockers[blocker_id] = {
                "id": blocker_id, "title": p["title"], "work_item": p.get("work_item"),
                "thread_id": p.get("thread_id"), "state": "open", "ref": "-", "reason": "-", "invalid": None,
            }
        return {"kind": "ok", "ledger": key, "apply": open_blocker}
    if not known:
        return {"kind": "rejected", "scope": "ledger", "ledger": key, "problem": "transition on an unknown blocker"}
    if problem:
        return {"kind": "rejected", "scope": "blocker", "ledger": key, "blocker": blocker_id, "problem": problem}

    def transition():
        known["invalid"] = None
        if kind == FIXED:
            known.update(state="fixed", ref=p["commit"], reason="-")
        elif kind == REOPENED:
            known.update(state="open", ref="-", reason=p["reason"])
        elif kind == WITHDRAWN:
            known.update(state="withdrawn", ref="-", reason=p["reason"])
        else:
            known.update(state="waived", ref=p["version"], reason=p["reason"])
    return {"kind": "ok", "ledger": key, "apply": transition}


def project(events, until_sequence=None):
    ordered = sort_events(events)
    if until_sequence is not None:
        ordered = [e for e in ordered if e["sequence"] <= until_sequence]
    ledgers, global_errors = {}, []

    def ledger_of(key):
        return ledgers.setdefault(key, Ledger(key))

    def fail(key, message):
        (ledger_of(key).errors if key else global_errors).append(message)

    voids = collect_voids(ordered)
    used = set()
    sequence = 0
    for e in ordered:
        sequence = max(sequence, e["sequence"])
        r = evaluate(e, ledger_of)
        voided = False
        for v in voids.get(e["id"], []):
            used.add(v["id"])
            if r["kind"] != "rejected":
                fail(v["payload"]["ledger"], f"void {v['id']} names a record that is not rejected: {e['id']}")
            elif r["ledger"] and r["ledger"] != v["payload"]["ledger"]:
                fail(v["payload"]["ledger"], f"void {v['id']} names a record in another ledger: {e['id']}")
            else:
                voided = True
        if voided:
            continue
        if r["kind"] == "unconfirmed":
            if r["ledger"]:
                ledger_of(r["ledger"]).unconfirmed.append((e, r["problem"]))
            continue
        if r["kind"] == "void":
            continue
        if r["kind"] == "ok":
            r["apply"]()
            continue
        if r["scope"] == "blocker":
            ledger_of(r["ledger"]).blockers[r["blocker"]]["invalid"] = (r["problem"], e["id"])
        else:
            fail(r["ledger"] if r["scope"] == "ledger" else None,
                 f"record {e['id']} ({e['event_type']}) is rejected: {r['problem']}")
    for target, listed in voids.items():
        for v in listed:
            if v["id"] not in used:
                fail(v["payload"]["ledger"], f"void {v['id']} names no record of this app: {target}")
    return ledgers, global_errors, sequence


def work_item_cell(b):
    if b["work_item"]:
        return b["work_item"]
    return "thread:" + b["thread_id"] if b["thread_id"] else "-"


def snapshot(ledger):
    lines = [HEADER]
    for blocker_id in sorted(ledger.blockers):
        b = ledger.blockers[blocker_id]
        if b["invalid"]:
            problem, event_id = b["invalid"]
            cells = [b["id"], "invalid", "-", work_item_cell(b), b["title"], f"{problem} in record {event_id}"]
        else:
            cells = [b["id"], b["state"], b["ref"], work_item_cell(b), b["title"], b["reason"]]
        lines.append("\t".join(cells))
    return "\n".join(lines) + "\n"


def choose(ledgers, global_errors, roots):
    keys = [k for k, ledger in ledgers.items() if ledger.opened and k in roots]
    if not keys:
        raise LedgerError("no device-opened ledger names a root commit of this cut")
    if len(keys) > 1:
        raise LedgerError("two device-opened ledgers name root commits of this cut")
    ledger = ledgers[keys[0]]
    errors = global_errors + ledger.errors
    for b in ledger.blockers.values():
        if b["invalid"]:
            errors.append(f"blocker {b['id']} is invalid: {b['invalid'][0]} in record {b['invalid'][1]}")
    if errors:
        raise LedgerError("; ".join(errors))
    return ledger


def settled(ledger, event):
    """An unconfirmed request a device has since carried out needs no tap."""
    p, kind = event["payload"], event["event_type"]
    if kind == LEDGER:
        return ledger.opened is not None
    b = ledger.blockers.get(p.get("blocker_id"))
    if not b or b["invalid"]:
        return False
    if kind == WITHDRAWN:
        return b["state"] == "withdrawn" and b["reason"] == p.get("reason")
    if kind == WAIVED:
        return b["state"] == "waived" and b["ref"] == p.get("version") and b["reason"] == p.get("reason")
    return False


def cli(args):
    try:
        result = subprocess.run(["lucidos", *args], capture_output=True, text=True, check=False)
    except OSError as err:
        raise LedgerError(f"the lucidos CLI cannot run: {err}") from err
    if result.returncode != 0:
        raise LedgerError(f"lucidos {args[0]} {args[1]} failed: {result.stderr.strip()[:300]}")
    try:
        return json.loads(result.stdout)
    except ValueError as err:
        raise LedgerError(f"lucidos {args[0]} {args[1]} printed no JSON") from err


def count(kind):
    value = cli(["events", "count", "--type", kind])
    n = value.get("count") if isinstance(value, dict) else None
    if type(n) is not int or n < 0:
        raise LedgerError(f"the {kind} count is unreadable")
    return n


def read_type(kind):
    rows, seen, before = [], set(), None
    while True:
        args = ["events", "query", "--type", kind, "--limit", str(PAGE)]
        page = cli(args + (["--before-event-id", before] if before else []))
        if not isinstance(page, list):
            raise LedgerError(f"the {kind} query did not return a list")
        for e in page:
            if not isinstance(e, dict) or not isinstance(e.get("id"), str):
                raise LedgerError(f"the {kind} query returned a row with no id")
            if e.get("event_type") != kind:
                raise LedgerError(f"the {kind} query returned a {e.get('event_type')} row")
            if e["id"] in seen:
                raise LedgerError(f"{kind} paging repeated a row")
            seen.add(e["id"])
            rows.append(e)
        if len(page) < PAGE:
            return rows
        before = page[-1]["id"]


def read_all():
    for _attempt in range(2):
        before = [count(kind) for kind in ALL_TYPES]
        by_type = [read_type(kind) for kind in ALL_TYPES]
        after = [count(kind) for kind in ALL_TYPES]
        if before == after and all(len(rows) == n for rows, n in zip(by_type, before)):
            return [e for rows in by_type for e in rows]
    raise LedgerError("the release blocker events changed twice during the read")


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--snapshot", required=True)
    parser.add_argument("--root", action="append", required=True)
    parser.add_argument("--until-sequence", type=int)
    opts = parser.parse_args(argv)
    try:
        if not all(matches(HEX40, root) for root in opts.root):
            raise LedgerError("a root commit is not 40 lowercase hex")
        ledgers, global_errors, sequence = project(read_all(), opts.until_sequence)
        ledger = choose(ledgers, global_errors, set(opts.root))
    except LedgerError as err:
        print(f"ERROR: {err}")
        return 1
    with open(opts.snapshot, "w", encoding="utf-8", newline="\n") as out:
        out.write(snapshot(ledger))
    print(f"ledger {ledger.key} sequence {sequence}")
    for event, problem in ledger.unconfirmed:
        if settled(ledger, event):
            continue
        note = f" ({problem})" if problem else ""
        print(f"unconfirmed: {event['event_type']} {event['id']} needs a tap on a paired device{note}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
