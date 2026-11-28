#!/usr/bin/env python3
"""Tests for the comment-ablation harness. None calls a model or builds anything.

    python3 scripts/eval-comments/harness_test.py
"""

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import harness  # noqa: E402


def judge_reply(models, result='{"score": 4}'):
    return json.dumps({"result": result, "total_cost_usd": 0.01, "modelUsage": {m: {} for m in models}})


def row(arm, passed, score):
    return {"arm": arm, "pass": passed, "score": score}


class JudgeModel(unittest.TestCase):
    def test_judge_model_differs_from_the_agents(self):
        self.assertFalse(harness.shares_agent_model([harness.JUDGE_MODEL]))

    def test_judge_model_is_not_refused(self):
        self.assertIsNone(harness.REFUSED_MODEL_RE.search(harness.JUDGE_MODEL))

    def test_judge_argv_runs_the_judge_model(self):
        with mock.patch.object(harness, "claude_bin", return_value="claude"):
            argv = harness.judge_argv()
        self.assertEqual(argv[argv.index("--model") + 1], harness.JUDGE_MODEL)

    def test_every_model_has_a_pinned_vertex_region(self):
        # Unpinned, a model falls back to CLOUD_ML_REGION, where it may have no quota.
        for model in (harness.MODEL, harness.JUDGE_MODEL):
            family, *version = model.split("[")[0].removeprefix("claude-").split("-")
            var = f"VERTEX_REGION_CLAUDE_{'_'.join(version)}_{family.upper()}"
            self.assertIn(var, harness.VERTEX_DEFAULTS, model)

    def test_agent_model_is_recognised_with_vertex_suffixes(self):
        agent = harness.MODEL.split("[")[0]
        for reported in (harness.MODEL, agent, f"{agent}@20261101"):
            self.assertTrue(harness.shares_agent_model([reported]), reported)


class ParseJudgeOutput(unittest.TestCase):
    def test_records_the_models_that_answered(self):
        verdict = harness.parse_judge_output(judge_reply(["claude-sonnet-5-5"]))
        self.assertEqual(verdict["score"], 4)
        self.assertEqual(verdict["judge_models"], ["claude-sonnet-5-5"])

    def test_refused_model_is_an_error(self):
        verdict = harness.parse_judge_output(judge_reply(["claude-fable-5-2"]))
        self.assertIn("error", verdict)

    def test_api_error_raises_even_when_its_text_holds_json(self):
        reply = json.dumps({"is_error": True, "modelUsage": {}, "total_cost_usd": 0,
                            "result": 'API Error: Request rejected (429) · [{"error":{"code":429}}]'})
        with self.assertRaises(harness.JudgeCallFailed):
            harness.parse_judge_output(reply)

    def test_reply_without_a_verdict_raises_for_a_retry(self):
        with self.assertRaises(AttributeError):
            harness.parse_judge_output(judge_reply(["claude-sonnet-5-5"], result="no json here"))


class PassRule(unittest.TestCase):
    def rows(self, a_passes, b_passes, a_score, b_score):
        return ([dict(row("A", i < a_passes, a_score), task=f"T{i}") for i in range(8)]
                + [dict(row("B", i < b_passes, b_score), task=f"T{i}") for i in range(8)])

    def test_a_duplicated_task_cannot_hide_a_missing_one(self):
        rows = self.rows(6, 6, 4.0, 4.0)
        rows[-1]["task"] = "T0"
        text = harness.pass_rule(rows)
        self.assertIn("arm B ran T0 2 times", text)
        self.assertIn("arms A and B cover different tasks", text)

    def test_small_drops_pass(self):
        text = harness.pass_rule(self.rows(6, 4, 4.0, 3.1))
        self.assertTrue(text.endswith("arm B PASSES."), text)

    def test_three_fewer_withheld_passes_fail(self):
        text = harness.pass_rule(self.rows(6, 3, 4.0, 4.0))
        self.assertIn("FAILS", text)
        self.assertIn("withheld passes 3 below", text)

    def test_judge_mean_one_point_lower_fails(self):
        text = harness.pass_rule(self.rows(6, 6, 4.0, 3.0))
        self.assertIn("judge mean 1.00 below", text)

    def test_missing_judge_scores_fail(self):
        text = harness.pass_rule(self.rows(6, 6, None, 3.0))
        self.assertIn("a judge mean is missing", text)

    def test_missing_runs_fail(self):
        rows = [r for r in self.rows(5, 3, 4.0, 4.0) if not (r["arm"] == "B" and not r["pass"])]
        text = harness.pass_rule(rows)
        self.assertIn("FAILS", text)
        self.assertIn(f"arm B has 3 of {harness.PASS_RULE_RUNS} graded runs", text)


    def test_runs_without_a_judge_score_fail(self):
        rows = self.rows(6, 6, 4.0, 4.0)
        rows[-1]["score"] = None
        self.assertIn("arm B has 1 runs without a judge score", harness.pass_rule(rows))


class VerifyTsc(unittest.TestCase):
    def test_a_tsc_failure_without_diagnostics_is_broken(self):
        result = {"clippy_B": 0, "clippy_B_files": [], "suite_B": {"exit": 0, "failed": []},
                  "fmt_B": 0, "fmt_B_files": []}
        tsc = mock.MagicMock(side_effect=[(124, "timed out"), (0, "")])
        with tempfile.TemporaryDirectory() as tmp, \
                mock.patch.multiple(harness, task_root=lambda _: Path(tmp), reset_prep=mock.DEFAULT,
                                    tsc_check=tsc, log=mock.DEFAULT):
            def verify():
                return harness.verify_arm({"id": "T"}, "B", {}, result, lambda: None, Path(tmp) / "p.log")[0]

            self.assertIn("tsc_B exited 124 without a diagnostic", verify())
            self.assertEqual(verify(), [], "the next run retries the undiagnosed failure")


class VerifyRestore(unittest.TestCase):
    def verify(self, green_on_call):
        calls = []

        def verify_arm(*_):
            calls.append(1)
            return ([], {}) if len(calls) == green_on_call else (["clippy_B"], {"src/a.rs": "clippy"})

        def read_json(path, default=None):
            return {"base": "fail", "fix": "pass"} if path.name == "verify.json" else {"src/a.rs": "x"}

        with tempfile.TemporaryDirectory() as tmp, \
                mock.patch.multiple(harness, verify_arm=verify_arm, read_json=read_json,
                                    task_root=lambda _: Path(tmp), restore_files=mock.DEFAULT,
                                    write_json=mock.DEFAULT, drop_target=mock.DEFAULT, log=mock.DEFAULT):
            harness.verify_task({"id": "T"}, ["B"], {}, Path(tmp) / "prepare.log")
        return len(calls)

    def test_the_last_restore_is_verified(self):
        self.assertEqual(self.verify(harness.RESTORE_ROUNDS + 1), harness.RESTORE_ROUNDS + 1)

    def test_still_broken_after_every_restore_exits(self):
        with self.assertRaises(SystemExit):
            self.verify(green_on_call=0)


class Permutation(unittest.TestCase):
    def test_compares_the_arms_it_is_given(self):
        rows = [dict(row("A", True, 5), task=t) for t in ("T1", "T2", "T3")]
        rows += [dict(row("B", False, 1), task=t) for t in ("T1", "T2", "T3")]
        p = harness.permutation_p(rows, lambda r: r["score"], ("A", "B"))
        self.assertAlmostEqual(p, 2 / 8)


CLIPPY_OUT = """\
    Checking lucidos-engine v0.1.0
error: this `if` statement can be collapsed
   --> crates/lucidos-engine/src/engine/recovery.rs:300:25
    |
300 | /     if is_branch(br) {
error: aborting due to 1 previous error
   --> crates/not-a-lint.rs:1:1
error: could not compile `lucidos-engine` (lib) due to 1 previous error
"""


class GateParsers(unittest.TestCase):
    def test_clippy_names_the_file_under_each_error(self):
        self.assertEqual(harness.files_flagged_by_clippy(CLIPPY_OUT), {"crates/lucidos-engine/src/engine/recovery.rs"})

    def test_fmt_names_files_relative_to_the_tree(self):
        tree = Path("/home/user/prep-B")
        out = (f"Diff in {tree}/crates/app/src/lib.rs:698:\n"
               f"Diff in {tree}/crates/app/src/lib.rs:751:\n"
               f"Diff in {tree}/crates/app/src/panel.rs:1:\n")
        self.assertEqual(harness.files_flagged_by_fmt(out, tree), {"crates/app/src/lib.rs", "crates/app/src/panel.rs"})

    def test_tsc_errors_drop_the_position(self):
        out = ("crates/app/src/triggers.ts(47,38): error TS2345: Argument is not assignable.\n"
               "../../lucidos/crates/app/src/tap.ts(1,62): error TS2307: Cannot find module.\n"
               "  continuation line\n")
        self.assertEqual(harness.tsc_errors(out), {
            "crates/app/src/triggers.ts: error TS2345: Argument is not assignable.",
            "../../lucidos/crates/app/src/tap.ts: error TS2307: Cannot find module.",
        })

    def test_failing_tests_reads_the_libtest_lines(self):
        out = "test a::b ... ok\ntest a::c ... FAILED\ntest d ... FAILED\n"
        self.assertEqual(harness.failing_tests(out), {"a::c", "d"})


class SuiteArgv(unittest.TestCase):
    def test_skips_reach_the_test_binary_and_filter_nothing_else(self):
        # test-engine.sh appends what follows its own `--` to `cargo test`.
        self.assertEqual(harness.SUITE_ARGV[:3], ["./scripts/test-engine.sh", "--", "--"])
        self.assertEqual(harness.SUITE_ARGV[3::2], ["--skip"] * len(harness.SUITE_SKIP))
        self.assertEqual(tuple(harness.SUITE_ARGV[4::2]), harness.SUITE_SKIP)


class ForgetArmResults(unittest.TestCase):
    def test_drops_only_that_arms_gate_results(self):
        result = {"base": "fail", "fix": "pass", "clippy_B": 1, "clippy_B_files": [], "suite_B": {}, "suite_B_rerun": {},
                  "tsc_A": [], "tsc_B": [], "fmt_B": 0, "fmt_B_files": [], "clippy_C": 0, "suite_A": {}}
        harness.forget_arm_results(result, "B")
        self.assertEqual(sorted(result), ["base", "clippy_C", "fix", "suite_A", "tsc_A"])


def git_tree(files: dict[str, str]) -> tempfile.TemporaryDirectory:
    tmp = tempfile.TemporaryDirectory()
    for rel, text in files.items():
        (Path(tmp.name) / rel).parent.mkdir(parents=True, exist_ok=True)
        (Path(tmp.name) / rel).write_text(text)
    subprocess.run(["git", "init", "-q"], cwd=tmp.name, check=True)
    subprocess.run(["git", "add", "-A"], cwd=tmp.name, check=True)
    return tmp


class GeneratorsIn(unittest.TestCase):
    def test_lists_each_generator_once(self):
        src = "fn generate_api_types_file() {}\nfn generate_api_types_file() {}\nfn generate_sdk_file() {}\n"
        with git_tree({"crates/lucidos-engine/src/gen.rs": src}) as tmp:
            self.assertEqual(harness.generators_in(Path(tmp)), ["generate_api_types_file", "generate_sdk_file"])

    def test_a_tree_without_generators_has_none(self):
        with git_tree({"crates/lucidos-engine/src/lib.rs": "fn main() {}\n"}) as tmp:
            self.assertEqual(harness.generators_in(Path(tmp)), [])


class LinkedNodeModules(unittest.TestCase):
    def test_links_inside_the_block_and_unlinks_after_an_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            link = Path(tmp) / "node_modules"
            with self.assertRaises(RuntimeError):
                with harness.linked_node_modules(Path(tmp)):
                    self.assertTrue(link.is_symlink())
                    raise RuntimeError("codegen failed")
            self.assertFalse(link.exists() or link.is_symlink())


class FilesReadByTests(unittest.TestCase):
    def test_finds_the_defining_file_and_the_files_it_reads(self):
        scan = ('const DOC: &str = include_str!("../../docs/table.md");\n'
                'fn phase_banners_stay() { let p = src_root().join("engine/flow.rs"); }\n')
        tree = {"crates/eng/src/api/scan_tests.rs": scan, "crates/eng/src/api/other.rs": "fn unrelated() {}\n"}
        with git_tree(tree) as tmp:
            files = harness.files_read_by_tests(["api::scan_tests::phase_banners_stay"], Path(tmp))
        self.assertEqual(files, {"crates/eng/src/api/scan_tests.rs", "crates/eng/docs/table.md", "crates/eng/src/engine/flow.rs"})


if __name__ == "__main__":
    unittest.main()
