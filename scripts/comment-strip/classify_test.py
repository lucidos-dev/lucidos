#!/usr/bin/env python3
"""Tests for classify.py. No test calls a model."""

import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import classify  # noqa: E402
import strip_comments as sc  # noqa: E402


class Select(unittest.TestCase):
    def test_test_paths_are_left_out(self):
        root = Path("/nonexistent")
        for rel in ("a/tests/x.rs", "a/x_tests/y.rs", "a/x.test.ts", "a/e2e/x.ts", "a/x_test.rs", "a/__tests__/x.tsx"):
            self.assertTrue(classify.is_test_code(root, rel, 1, {}), rel)
        self.assertFalse(classify.is_test_code(root, "a/latest.ts", 1, {}))

    def answers(self, src: str, marks: tuple[str, ...]) -> dict[str, bool]:
        """Whether the comment holding each mark counts as test code."""
        with tempfile.TemporaryDirectory() as d:
            Path(d, "a.rs").write_text(src, encoding="utf-8")
            spans: dict = {}
            return {m: classify.is_test_code(Path(d), "a.rs", src.index(m), spans) for m in marks}

    def test_only_an_inline_rust_test_module_is_test_code(self):
        src = (
            "#[cfg(test)]\nmod stub;\n// declared\nfn f() {}\n\n"
            "#[cfg(test)]\n#[allow(dead_code)]\npub(crate) mod tests {\n    // inside \"}\"\n    fn t() {}\n} // same line\n"
            "// below\nfn g() {}\n"
        )
        self.assertEqual(self.answers(src, ("// declared", "// inside", "// same line", "// below")),
                         {"// declared": False, "// inside": True, "// same line": False, "// below": False})

    def test_a_cfg_naming_test_marks_a_module_unless_it_negates_it(self):
        src = "#[cfg(all(test, unix))]\nmod a {\n    // gated\n}\n#[cfg(not(test))]\nmod b {\n    // negated\n}\n"
        self.assertEqual(self.answers(src, ("// gated", "// negated")), {"// gated": True, "// negated": False})

    def test_an_inner_cfg_test_marks_the_whole_file(self):
        self.assertEqual(self.answers("// top\n#![cfg(test)]\nfn f() {}\n", ("// top",)), {"// top": True})

    def test_an_inner_cfg_test_inside_a_module_leaves_the_file_production(self):
        src = "mod tests {\n    #![cfg(test)]\n}\n// after\nfn f() {}\n"
        self.assertEqual(self.answers(src, ("// after",)), {"// after": False})

    def test_a_cfg_that_also_holds_outside_tests_is_production(self):
        src = "#[cfg(any(test, unix))]\nmod a {\n    // either\n}\n"
        self.assertEqual(self.answers(src, ("// either",)), {"// either": False})

    def test_after_context_stops_before_the_next_comment(self):
        src = "// --- Section ---\n\nlet a = 1;\n// The toggle races the dismiss.\nlet b = 2;\n"
        block = sc.classify_blocks(src, "typescript")[0]
        self.assertEqual(classify.after_context(src, block, sc.lex(src, "typescript").comments), "let a = 1;")

    def test_a_block_mixing_a_directive_and_prose_is_asked(self):
        src = "// The order matters: the probe reads it first.\n// eslint-disable-next-line no-console\nconsole.log(x);\n"
        with tempfile.TemporaryDirectory() as d:
            subprocess.run(["git", "init", "-q", d], check=True)
            Path(d, "a.ts").write_text(src, encoding="utf-8")
            subprocess.run(["git", "-C", d, "add", "a.ts"], check=True)
            out = Path(d, "pop.json")
            with contextlib.redirect_stdout(io.StringIO()):
                classify.cmd_select(SimpleNamespace(root=d, out=str(out), paths=[]))
            self.assertEqual(len(json.loads(out.read_text(encoding="utf-8"))), 1)


class Run(unittest.TestCase):
    def test_a_resume_drops_a_line_cut_mid_write(self):
        row = {"path": "a.rs", "line": 1, "hash": "h1", "verdict": "strip", "score": 1, "why": "w"}
        with tempfile.TemporaryDirectory() as d:
            pop, out = Path(d, "pop.json"), Path(d, "answers.jsonl")
            pop.write_text(json.dumps([{"path": "a.rs", "hash": "h1"}]), encoding="utf-8")
            out.write_bytes((json.dumps(row) + "\n" + '{"why": "\u00e9').encode("utf-8")[:-1])
            args = SimpleNamespace(pop=str(pop), out=str(out), model="m", batch=25, jobs=1, limit=0)
            with contextlib.redirect_stdout(io.StringIO()):
                self.assertEqual(classify.cmd_run(args), 0)
            self.assertEqual(out.read_text(encoding="utf-8"), json.dumps(row) + "\n")


class Audit(unittest.TestCase):
    def test_an_unanswered_block_refuses_the_audit(self):
        row = {"path": "a.rs", "line": 1, "hash": "h1", "verdict": "keep", "score": 4, "why": "w"}
        with tempfile.TemporaryDirectory() as d:
            pop, answers = Path(d, "pop.json"), Path(d, "answers.jsonl")
            blocks = [{"path": "a.rs", "line": 1, "hash": h, "text": "// x"} for h in ("h1", "h2")]
            pop.write_text(json.dumps(blocks), encoding="utf-8")
            answers.write_text(json.dumps(row) + "\n", encoding="utf-8")
            args = SimpleNamespace(answers=[str(answers)], pop=str(pop), out=str(Path(d, "a.md")), count=5, seed=1, chars=60)
            with contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(classify.cmd_audit(args), 1)


class KeptList(unittest.TestCase):
    def write(self, d: str, name: str, rows: list[dict]) -> str:
        path = os.path.join(d, name)
        Path(path).write_text("".join(json.dumps(r) + "\n" for r in rows), encoding="utf-8")
        return path

    def test_a_block_is_kept_when_any_run_kept_it(self):
        row = lambda h, v: {"path": "a.rs", "line": 1, "hash": h, "verdict": v, "score": 3, "why": "w\ty"}  # noqa: E731
        with tempfile.TemporaryDirectory() as d:
            one = self.write(d, "1.jsonl", [row("h1", "strip"), row("h2", "adr"), row("h3", "strip")])
            two = self.write(d, "2.jsonl", [row("h1", "keep"), row("h2", "keep"), row("h3", "strip")])
            out = os.path.join(d, "kept-list.tsv")

            class Args:
                answers = [one, two]

            Args.out = out
            classify.cmd_kept_list(Args)
            rows = [line.split("\t") for line in Path(out).read_text(encoding="utf-8").splitlines() if not line.startswith("#")]
        self.assertEqual([r[:3] for r in rows], [["a.rs", "h1", "keep"], ["a.rs", "h2", "keep"]])
        self.assertEqual(rows[0][3], "w y")


if __name__ == "__main__":
    unittest.main()
