import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import reader

from reader import linked_runs, observation, pages

URL = "https://github.com/lucidos-dev/lucidos/actions/runs/42"
START = {"check_id": "check", "from_sha": "a", "to_sha": "b"}
LINK = {"check_id": "check", "branch": "e2e/run", "run_url": URL}
RESULT = {"check_id": "check", "flaky": [{"name": "flaky test"}]}


class ShadowReaderTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = json.loads(subprocess.check_output(["bash", str(reader.HELPER), "--workflow"]))

    def completion(self, **run_overrides):
        return {"payload": {
            "action": "completed",
            "repository": {"full_name": "lucidos-dev/lucidos"},
            "workflow_run": {
                "id": 42, "head_branch": "e2e/run", "event": "push",
                "path": ".github/workflows/e2e.yml", "status": "completed",
                "head_repository": {"full_name": "lucidos-dev/lucidos"},
                **run_overrides,
            },
        }}

    def test_filters_completion_envelope(self):
        valid = self.completion()
        self.assertEqual(reader.completed_run(valid, self.workflow), (URL, LINK["branch"]))
        invalid = [
            valid["payload"],
            {"payload": []},
            {"payload": {**valid["payload"], "action": "requested"}},
            {"payload": {**valid["payload"], "repository": {"full_name": "other/repo"}}},
            self.completion(path=".github/workflows/release.yml"),
            self.completion(head_branch="main"),
            self.completion(head_repository={"full_name": "fork/lucidos"}),
            self.completion(event="pull_request"),
            self.completion(status="in_progress"),
            self.completion(id=True),
            self.completion(id=0),
        ]
        for delivery in invalid:
            with self.subTest(delivery=delivery):
                self.assertIsNone(reader.completed_run(delivery, self.workflow))

    def test_main_routes_schedule_completion_and_unrelated_events(self):
        with tempfile.TemporaryDirectory() as scratch:
            state_dir = Path(scratch)
            argv = ["reader.py", "--since", "start", "--state-dir", scratch]
            deliveries = [
                ({}, None),
                ({"TRIGGER_EVENT_TYPE": "UnrelatedEvent"}, None),
                ({
                    "TRIGGER_EVENT_TYPE": reader.COMPLETION_EVENT,
                    "TRIGGER_EVENT_PAYLOAD": json.dumps(self.completion()),
                }, (URL, LINK["branch"])),
            ]
            base_env = {key: value for key, value in os.environ.items()
                        if key not in {"TRIGGER_EVENT_TYPE", "TRIGGER_EVENT_PAYLOAD"}}
            for env, target in deliveries:
                with (
                    self.subTest(event=env.get("TRIGGER_EVENT_TYPE")),
                    patch.dict(os.environ, {**base_env, **env}, clear=True),
                    patch.object(reader.sys, "argv", argv),
                    patch.object(reader, "scan", return_value=0) as scan,
                ):
                    self.assertEqual(reader.main(), 0)
                    if env.get("TRIGGER_EVENT_TYPE") == "UnrelatedEvent":
                        scan.assert_not_called()
                    else:
                        scan.assert_called_once_with("start", state_dir, target)

    def test_completion_before_settlement_and_out_of_order_catch_up(self):
        newer = {**LINK, "check_id": "newer", "run_url": URL.replace("/42", "/43")}
        snapshot = {
            "MainCheckStarted": [{**START, "check_id": "newer"}, START],
            "MainCheckRunLinked": [newer, LINK],
            "MainCheckPassed": [{**RESULT, "check_id": "newer"}],
            reader.EVENT: [],
        }
        target = reader.completed_run(self.completion(), self.workflow)

        def emit(args, *, check):
            self.assertTrue(check)
            snapshot[reader.EVENT].append(json.loads(args[-1]))

        with (
            tempfile.TemporaryDirectory() as scratch,
            patch.object(reader, "event_payloads", side_effect=lambda kind, _: snapshot.get(kind, [])),
            patch.object(reader, "read_report", return_value={"verdict": "pass"}) as read,
            patch.object(reader.subprocess, "run", side_effect=emit),
        ):
            state_dir = Path(scratch)
            reader.scan("start", state_dir, target)
            read.assert_not_called()
            reader.scan("start", state_dir)
            snapshot["MainCheckPassed"].append(RESULT)
            reader.scan("start", state_dir, (URL, "e2e/different"))
            reader.scan("start", state_dir, target)
            reader.scan("start", state_dir, target)
            reader.scan("start", state_dir)
            self.assertEqual(read.call_count, 2)

        self.assertEqual(
            [(row["run_id"], row["source"]) for row in snapshot[reader.EVENT]],
            [("43", "schedule"), ("42", "webhook")],
        )

    def test_pages_keep_the_boundary(self):
        calls = []

        def query(cursor):
            calls.append(cursor)
            return [{"id": "new"}, {"id": "old"}] if not cursor else [{"id": "first"}]

        self.assertEqual([r["id"] for r in pages(query, size=2)], ["new", "old", "first"])
        self.assertEqual(calls, [None, "old"])

    def test_waits_for_engine_settlement_and_skips_receipts(self):
        events = {"MainCheckStarted": [START], "MainCheckRunLinked": [LINK]}
        self.assertEqual(list(linked_runs(events)), [])
        events["MainCheckPassed"] = [RESULT]
        self.assertEqual(list(linked_runs(events))[0]["run_id"], "42")
        events["FactoryMainCheckObserved"] = [{"run_id": "42"}]
        self.assertEqual(list(linked_runs(events)), [])

    def test_check_started_before_window_does_not_block_later_checks(self):
        earlier = {**LINK, "check_id": "earlier", "run_url": URL.replace("/42", "/41")}
        events = {
            "MainCheckStarted": [START],
            "MainCheckRunLinked": [LINK, earlier],
            "MainCheckPassed": [RESULT, {**RESULT, "check_id": "earlier"}],
        }
        self.assertEqual([run["run_id"] for run in linked_runs(events)], ["42"])

    def test_observation_reports_flaky_disagreement(self):
        run = {**START, "run_id": "42", "engine_verdict": "pass", "engine_findings": RESULT["flaky"]}
        self.assertTrue(observation(run, {"verdict": "pass", "flaky": RESULT["flaky"]})["matches"])
        self.assertFalse(observation(run, {"verdict": "pass", "flaky": []})["matches"])

    def test_failure_compares_the_shared_report_field(self):
        failures = [{"suite": "engine", "name": "failed test", "detail": ""}]
        run = {**START, "engine_verdict": "fail", "engine_findings": failures}
        record = observation(run, {"verdict": "fail", "failures": failures})
        self.assertEqual(record["findings"], failures)
        self.assertTrue(record["matches"])

    def test_no_verdict_compares_with_engine_abandonment(self):
        events = {
            "MainCheckStarted": [START],
            "MainCheckRunLinked": [LINK],
            "MainCheckAbandoned": [{"check_id": "check"}],
        }
        run, = linked_runs(events)
        self.assertTrue(observation(run, {"verdict": "no-verdict"})["matches"])
        self.assertFalse(observation(run, {"verdict": "pass"})["matches"])
        self.assertIsNone(observation(run, {"verdict": "unavailable"})["matches"])

    def test_unrelated_link_is_rejected_before_github_access(self):
        with tempfile.TemporaryDirectory() as scratch:
            result = self.run_helper(scratch, URL.replace("lucidos-dev", "elsewhere"))
        self.assertEqual(result.returncode, 1)
        self.assertIn("invalid run link", result.stderr)

    def test_missing_artifacts_are_unavailable_but_failed_download_is_retryable(self):
        with tempfile.TemporaryDirectory() as scratch:
            result = self.run_helper(scratch, URL)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn('"verdict": "unavailable"', result.stdout)
            self.assertIn("No shard artifacts remain", result.stdout)
            result = self.run_helper(scratch, URL, available="1")
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn("main-check verdict", result.stdout)

    def test_pending_run_is_retried_without_a_failed_scan(self):
        snapshot = {
            "MainCheckStarted": [START],
            "MainCheckRunLinked": [LINK],
            "MainCheckSuperseded": [{"check_id": "check"}],
        }
        with tempfile.TemporaryDirectory() as scratch:
            pending = self.run_helper(scratch, URL, status="in_progress")
            self.assertEqual(pending.returncode, 0, pending.stderr)
            completed = self.run_helper(scratch, URL)
            self.assertEqual(completed.returncode, 0, completed.stderr)
            with (
                patch.object(reader, "event_payloads", side_effect=lambda kind, _: snapshot.get(kind, [])),
                patch.object(reader.subprocess, "check_output", side_effect=[pending.stdout, completed.stdout]),
                patch.object(reader.subprocess, "run") as emit,
            ):
                self.assertEqual(reader.scan("start", Path(scratch)), 0)
                emit.assert_not_called()
                self.assertEqual(reader.scan("start", Path(scratch)), 0)
                emit.assert_called_once()

    def test_pending_run_with_wrong_identity_fails(self):
        with tempfile.TemporaryDirectory() as scratch:
            result = self.run_helper(scratch, URL, status="in_progress", head_branch="e2e/other")
            self.assertEqual(result.returncode, 1)
            self.assertIn("could not verify", result.stderr)

    def test_new_attempt_while_reading_report_fails(self):
        with tempfile.TemporaryDirectory() as scratch:
            result = self.run_helper(scratch, URL, latest="in_progress\t2")
            self.assertEqual(result.returncode, 1)
            self.assertIn("saw a new attempt", result.stderr)
            self.assertNotIn("main-check verdict", result.stdout)

    def run_helper(self, scratch, url, available="", latest="completed\t1", **overrides):
        run = {
            "head_branch": LINK["branch"],
            "repository": {"full_name": self.workflow["repo"]},
            "head_repository": {"full_name": self.workflow["repo"]},
            "event": "push",
            "path": self.workflow["path"],
            "status": "completed",
            "run_attempt": 1,
            "conclusion": "cancelled",
            **overrides,
        }
        gh = Path(scratch, "gh")
        gh.write_text("""#!/bin/sh
case "$1:$2" in
    'auth:token')
        test "$*" = 'auth token -h github.com -u fixture' && printf '%s\\n' fixture-token
        ;;
    api:*/artifacts) printf '%s\\n' "$FIXTURE_ARTIFACTS" ;;
    api:*)
        if [ "$#" -gt 2 ]; then
            printf '%s\\n' "$FIXTURE_LATEST"
        else
            printf '%s\\n' "$FIXTURE_RUN"
        fi
        ;;
    'run:download') false ;;
    *) false ;;
esac
""")
        gh.chmod(0o700)
        env = {**os.environ, "PATH": f"{scratch}:{os.environ['PATH']}",
               "GH_ACCOUNT_GITHUB_COM": "fixture", "FIXTURE_ARTIFACTS": available,
               "FIXTURE_LATEST": latest, "FIXTURE_RUN": json.dumps(run)}
        helper = Path(__file__).with_name("read-run.sh")
        return subprocess.run(
            ["bash", str(helper), url, "e2e/run", scratch],
            capture_output=True, text=True, env=env,
        )


if __name__ == "__main__":
    unittest.main()
