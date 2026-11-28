import argparse
import fcntl
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path


# INVARIANT: See "Main check shadow comparison" in docs/temporary-measures.md.
EVENT = "FactoryMainCheckObserved"
COMPLETION_EVENT = "FactoryMainCheckWorkflowCompleted"
HELPER = Path(__file__).with_name("read-run.sh")
TERMINAL = {
    "MainCheckPassed": ("pass", "flaky"),
    "MainCheckFailed": ("fail", "failures"),
    "MainCheckAbandoned": ("no-verdict", None),
    "MainCheckSuperseded": ("no-verdict", None),
}
PAGE_SIZE = 1000


def object_field(value, key):
    return value.get(key) if isinstance(value, dict) else None


def completed_run(envelope, workflow):
    body = object_field(envelope, "payload")
    if object_field(body, "action") != "completed":
        return None
    if object_field(body.get("repository"), "full_name") != workflow["repo"]:
        return None
    run = body.get("workflow_run")
    if not isinstance(run, dict):
        return None

    branch = run.get("head_branch")
    run_id = run.get("id")
    if (
        type(run_id) is not int or run_id <= 0
        or not isinstance(branch, str) or not branch.startswith(workflow["branch_prefix"])
        or run.get("path") != workflow["path"]
        or run.get("event") != "push"
        or run.get("status") != "completed"
        or object_field(run.get("head_repository"), "full_name") != workflow["repo"]
    ):
        return None
    return f"{workflow['run_base']}{run_id}", branch


def pages(query, size=PAGE_SIZE):
    cursor = None
    while True:
        rows = query(cursor)
        yield from rows
        if len(rows) < size:
            return
        cursor = rows[-1]["id"]


def linked_runs(events):
    starts = {row["check_id"]: row for row in events["MainCheckStarted"]}
    receipts = {row["run_id"] for row in events.get(EVENT, [])}
    results = {
        row["check_id"]: (verdict, row.get(field, []))
        for event, (verdict, field) in TERMINAL.items()
        for row in events.get(event, [])
    }
    for link in reversed(events["MainCheckRunLinked"]):
        check_id = link["check_id"]
        if check_id not in starts or check_id not in results:
            continue
        run_id = link["run_url"].rsplit("/", 1)[-1]
        if run_id in receipts:
            continue
        verdict, findings = results[check_id]
        yield {
            **starts[check_id],
            "run_id": run_id,
            "run_url": link["run_url"],
            "branch": link["branch"],
            "engine_verdict": verdict,
            "engine_findings": findings,
        }


def observation(run, report):
    verdict = report["verdict"]
    findings = report.get("failures" if verdict == "fail" else "flaky", [])
    matches = None if verdict == "unavailable" else (
        verdict == run["engine_verdict"] and findings == run["engine_findings"]
    )
    return {
        **run,
        "verdict": verdict,
        "summary": report.get("summary", ""),
        "findings": findings,
        "matches": matches,
    }


def event_payloads(kind, since):
    def query(cursor):
        args = ["lucidos", "events", "query", "--type", kind, "--since", since,
                "--limit", str(PAGE_SIZE)]
        if cursor:
            args.extend(["--before-event-id", cursor])
        return json.loads(subprocess.check_output(args))

    for row in pages(query):
        payload = row["payload"]
        yield payload if kind == EVENT else payload["data"]


def read_report(run, state_dir):
    with tempfile.TemporaryDirectory(dir=state_dir) as scratch:
        output = subprocess.check_output(
            ["bash", str(HELPER), run["run_url"], run["branch"], scratch], text=True,
        )
    if output.strip() == "main-check pending":
        return None
    prefix = "main-check verdict "
    line = next(line for line in output.splitlines() if line.startswith(prefix))
    report = json.loads(line[len(prefix):])
    if not isinstance(report, dict):
        raise ValueError("Expected a main check verdict object")
    return report


def scan(since, state_dir, target=None):
    kinds = ["MainCheckStarted", "MainCheckRunLinked", *TERMINAL, EVENT]
    snapshot = {kind: list(event_payloads(kind, since)) for kind in kinds}
    source = "webhook" if target else "schedule"
    clean = True
    recorded = set()
    for run in linked_runs(snapshot):
        if target and (run["run_url"], run["branch"]) != target:
            continue
        if run["run_id"] in recorded:
            continue
        try:
            report = read_report(run, state_dir)
            if report is None:
                continue
            record = observation(run, report)
            record["source"] = source
            summary = {
                True: "Main check shadow matched",
                False: "Main check shadow differed",
                None: "Main check shadow lacked artifacts",
            }[record["matches"]]
            subprocess.run(["lucidos", "events", "emit", EVENT, "--summary", summary,
                            "--payload", json.dumps(record)], check=True)
            recorded.add(run["run_id"])
        except (subprocess.CalledProcessError, ValueError, StopIteration):
            print(f"Main check shadow could not record run {run['run_id']}.", file=sys.stderr)
            clean = False
    return 0 if clean else 1


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--since", required=True, help="ISO 8601 start of the shadow window")
    parser.add_argument("--state-dir", type=Path, required=True)
    args = parser.parse_args()
    target = None
    event_type = os.environ.get("TRIGGER_EVENT_TYPE")
    if event_type is not None:
        if event_type != COMPLETION_EVENT:
            return 0
        envelope = json.loads(os.environ.get("TRIGGER_EVENT_PAYLOAD", "{}"))
        workflow = json.loads(subprocess.check_output(["bash", str(HELPER), "--workflow"]))
        target = completed_run(envelope, workflow)
        if target is None:
            return 0

    args.state_dir.mkdir(parents=True, exist_ok=True)
    with (args.state_dir / "reader.lock").open("a") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        return scan(args.since, args.state_dir, target)


if __name__ == "__main__":
    sys.exit(main())
