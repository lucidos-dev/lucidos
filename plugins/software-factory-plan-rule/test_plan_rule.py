import json
from pathlib import Path
import subprocess
import sys
import unittest

from scripts.plan_rule import verdict


class PlanRuleTest(unittest.TestCase):
    def test_marker_and_landing_cases(self):
        plan = ["docs/plans/example.md"]
        code = ["src/example.rs"]
        cases = [
            ("missing", None, plan, None, "missing"),
            ("changed", None, code, None, "changed"),
            ("unreadable", None, code, None, "unreadable"),
            ("present", "proposed", plan, None, "awaiting_approval"),
            ("present", "proposed", code, None, "awaiting_approval"),
            ("present", "plan_only", plan, None, None),
            ("present", "plan_only", [], None, "plan_only_code"),
            ("present", "plan_only", None, None, "plan_only_code"),
            ("present", "plan_only", plan + code, None, "plan_only_code"),
            ("present", "planned", code, None, None),
            ("present", "acknowledged_simple", code, None, None),
            ("present", "bounded_security_fix", plan + code, code, None),
            ("present", "bounded_security_fix", code, [], "outside_bound"),
            ("present", "bounded_security_fix", None, code, "unreadable"),
        ]
        for state, kind, files, bound, expected in cases:
            with self.subTest(state=state, kind=kind, files=files, bound=bound):
                self.assertEqual(verdict({
                    "coding_agent_kind": "lucidos",
                    "plan_marker": {"state": state, "kind": kind, "file_bound": bound},
                    "landing_files": files,
                }), expected)

    def test_other_repositories_are_exempt(self):
        for kind in ("external", "app"):
            self.assertIsNone(verdict({"coding_agent_kind": kind}))

    def test_both_hook_events_return_a_reason_for_each_hold(self):
        script = Path(__file__).parent / "scripts" / "plan_rule.py"
        for event in ("ChangeProposalChecked", "ChangeApplyChecked"):
            for state, kind, files in (
                ("missing", None, []),
                ("changed", None, []),
                ("unreadable", None, []),
                ("present", "proposed", []),
                ("present", "plan_only", ["src/example.rs"]),
                ("present", "bounded_security_fix", ["src/example.rs"]),
            ):
                with self.subTest(event=event, state=state, kind=kind):
                    result = subprocess.run(
                        [sys.executable, str(script)],
                        input=json.dumps({
                            "event_type": event,
                            "payload": {"change_context": {
                                "coding_agent_kind": "lucidos",
                                "plan_marker": {
                                    "state": state, "kind": kind, "file_bound": [],
                                },
                                "landing_files": files,
                            }},
                        }),
                        text=True, check=True, capture_output=True,
                    )
                    answer = json.loads(result.stdout)
                    self.assertEqual(answer["answer"], "refuse")
                    self.assertTrue(answer["reason"])
