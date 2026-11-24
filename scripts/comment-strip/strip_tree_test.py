#!/usr/bin/env python3
"""Tests for strip_tree.py. Run: python3 scripts/comment-strip/strip_tree_test.py

Each test builds a throwaway git repository. The TypeScript and CSS proofs are
the real ones. The Rust proof is a stand-in script, so no test compiles
anything: its own tests run under `cargo test -p lucidos-wire-codegen`.
"""

import contextlib
import io
import os
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import strip_tree  # noqa: E402

FILES = {
    "src/a.rs": "// gone\nfn f() {}\n",
    "src/b.ts": "// gone\nexport const b = 1;\n",
    "src/c.tsx": "export const c = (\n  <div>\n    {/* gone */}\n    <b />\n  </div>\n);\n",
    "src/d.css": "/* gone */\n.a { color: red; }\n",
    "src/generated/e.ts": "// stays\nexport const e = 1;\n",
    "crates/vendor/f.rs": "// stays\nfn f() {}\n",
    "src/g.sql": "-- stays\nselect 1;\n",
    "src/h.css": ".a/* touching */.b { color: red; }\n",
}


class Pipeline(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name) / "repo"
        for rel, body in FILES.items():
            path = self.root / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body, encoding="utf-8")
        subprocess.run(["git", "init", "-q", str(self.root)], check=True)
        subprocess.run(["git", "-C", str(self.root), "add", "-A"], check=True)
        commit = ["-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "-q", "-m", "base"]
        subprocess.run(["git", "-C", str(self.root), *commit], check=True)
        self.addCleanup(self.tmp.cleanup)

    def verifier(self, body: str) -> str:
        path = Path(self.tmp.name) / "verifier"
        path.write_text("#!/bin/sh\n" + body, encoding="utf-8")
        path.chmod(path.stat().st_mode | stat.S_IXUSR)
        return str(path)

    def run_tool(self, *args: str, verifier_body: str = "cat > /dev/null\n") -> tuple[int, str]:
        out = io.StringIO()
        argv = ["--root", str(self.root), *args]
        if args[0] in ("run", "verify"):
            argv += ["--rust-verifier", self.verifier(verifier_body)]
        with contextlib.redirect_stdout(out):
            code = strip_tree.main(argv)
        return code, out.getvalue()

    def read(self, rel: str) -> str:
        return (self.root / rel).read_text(encoding="utf-8")

    def test_a_run_without_write_changes_nothing(self):
        code, out = self.run_tool("run", "src/a.rs", "src/b.ts", "src/c.tsx", "src/d.css")
        self.assertEqual(code, 0, out)
        self.assertIn("would write 4 of 4 files", out)
        for rel, body in FILES.items():
            self.assertEqual(self.read(rel), body)

    def test_write_strips_every_language_and_removes_the_jsx_wrapper(self):
        code, out = self.run_tool("run", "--write", "src/a.rs", "src/b.ts", "src/c.tsx", "src/d.css")
        self.assertEqual(code, 0, out)
        self.assertEqual(self.read("src/a.rs"), "fn f() {}\n")
        self.assertEqual(self.read("src/b.ts"), "export const b = 1;\n")
        self.assertEqual(self.read("src/c.tsx"), "export const c = (\n  <div>\n    <b />\n  </div>\n);\n")
        self.assertEqual(self.read("src/d.css"), ".a { color: red; }\n")
        self.assertIn("jsx wrappers: 1 removed, 0 left", out)

    def test_vendored_generated_and_sql_files_are_out_of_scope(self):
        self.run_tool("run", "--write")
        for rel in ("src/generated/e.ts", "crates/vendor/f.rs", "src/g.sql"):
            self.assertEqual(self.read(rel), FILES[rel])

    def test_a_refused_file_stays_and_fails_the_run(self):
        code, out = self.run_tool("run", "--write")
        self.assertEqual(code, 1)
        self.assertIn("REFUSED src/h.css: proof one", out)
        self.assertEqual(self.read("src/h.css"), FILES["src/h.css"])
        self.assertEqual(self.read("src/d.css"), ".a { color: red; }\n")

    def test_a_file_the_second_proof_rejects_is_not_written(self):
        body = "cat > /dev/null\nprintf 'DIFF\\tsrc/a.rs\\ttoken 1 changed\\n'\nexit 1\n"
        code, out = self.run_tool("run", "--write", "src/a.rs", "src/b.ts", verifier_body=body)
        self.assertEqual(code, 1)
        self.assertIn("REFUSED src/a.rs: proof two: token 1 changed", out)
        self.assertEqual(self.read("src/a.rs"), FILES["src/a.rs"])
        self.assertEqual(self.read("src/b.ts"), "export const b = 1;\n")

    def test_a_verifier_that_cannot_run_stops_the_run(self):
        with self.assertRaises(SystemExit):
            self.run_tool("run", "--write", "src/a.rs", verifier_body="exit 2\n")
        self.assertEqual(self.read("src/a.rs"), FILES["src/a.rs"])

    def test_stripped_only_leaves_an_unstripped_language_alone(self):
        code, out = self.run_tool("run", "--write", "--stripped-only", "src/a.rs", "src/b.ts")
        self.assertEqual(code, 0, out)
        self.assertEqual(self.read("src/a.rs"), FILES["src/a.rs"])
        self.assertEqual(self.read("src/b.ts"), "export const b = 1;\n")

    def test_verify_accepts_a_comment_only_tree_and_rejects_a_code_edit(self):
        self.run_tool("run", "--write", "src/b.ts", "src/c.tsx", "src/d.css")
        code, out = self.run_tool("verify", "--base", "HEAD")
        self.assertEqual(code, 0, out)
        self.assertIn("3 of 3 changed files", out)
        (self.root / "src/b.ts").write_text("export const b = 2;\n", encoding="utf-8")
        code, out = self.run_tool("verify", "--base", "HEAD")
        self.assertEqual(code, 1)
        self.assertIn("DIFF src/b.ts", out)

    def commit_b(self, body: str) -> None:
        (self.root / "src/b.ts").write_text(body, encoding="utf-8")
        subprocess.run(["git", "-C", str(self.root), "-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "-qam", "directive"], check=True)

    def test_verify_rejects_a_removed_directive_comment(self):
        for body in ("// eslint-disable-next-line no-console\nexport const b = 1;\n", "/* global a */\nexport const b = 1;\n"):
            self.commit_b(body)
            (self.root / "src/b.ts").write_text("export const b = 1;\n", encoding="utf-8")
            code, out = self.run_tool("verify", "--base", "HEAD")
            self.assertEqual(code, 1, out)
            self.assertIn("DIFF src/b.ts: a comment a tool reads changed", out)

    def test_verify_rejects_a_moved_directive_comment(self):
        self.commit_b("// eslint-disable-next-line no-console\nexport const b = 1;\nexport const c = 2;\n")
        (self.root / "src/b.ts").write_text("export const b = 1;\n// eslint-disable-next-line no-console\nexport const c = 2;\n", encoding="utf-8")
        code, out = self.run_tool("verify", "--base", "HEAD")
        self.assertEqual(code, 1, out)
        self.assertIn("DIFF src/b.ts: a comment a tool reads changed", out)

    def test_javascript_files_are_stripped_and_proven(self):
        for rel, body in {"src/i.mjs": "#!/usr/bin/env node\n// gone\nexport const i = 1;\n", "src/j.cjs": "/* gone */\nmodule.exports = 1;\n"}.items():
            (self.root / rel).write_text(body, encoding="utf-8")
        subprocess.run(["git", "-C", str(self.root), "add", "-A"], check=True)
        code, out = self.run_tool("run", "--write", "src/i.mjs", "src/j.cjs")
        self.assertEqual(code, 0, out)
        self.assertEqual((self.root / "src/i.mjs").read_text(encoding="utf-8"), "#!/usr/bin/env node\nexport const i = 1;\n")
        self.assertEqual((self.root / "src/j.cjs").read_text(encoding="utf-8"), "module.exports = 1;\n")

    def test_verify_fails_on_a_deleted_file(self):
        (self.root / "src/b.ts").unlink()
        code, out = self.run_tool("verify", "--base", "HEAD")
        self.assertEqual(code, 1)
        self.assertIn("DIFF src/b.ts: deleted", out)

    def test_verify_fails_on_a_staged_deletion_with_an_untracked_copy(self):
        subprocess.run(["git", "-C", str(self.root), "rm", "-q", "--cached", "src/b.ts"], check=True)
        code, out = self.run_tool("verify", "--base", "HEAD")
        self.assertEqual(code, 1)
        self.assertIn("DIFF src/b.ts: deleted", out)

    def test_a_scratch_directory_that_is_the_checkout_is_refused(self):
        with self.assertRaises(SystemExit) as raised:
            self.run_tool("run", "--out", str(self.root))
        self.assertIn("--out must be a new or empty directory", str(raised.exception))
        self.assertEqual((self.root / "src/b.ts").read_text(encoding="utf-8"), FILES["src/b.ts"])

    def test_blocks_and_count_read_the_kept_classes(self):
        (self.root / "src/b.ts").write_text("// @ts-expect-error x\nf();\n// prose\n", encoding="utf-8")
        _, out = self.run_tool("blocks", "src/b.ts")
        self.assertEqual(out.count("\n"), 2)
        self.assertIn('"directive": true', out)
        _, out = self.run_tool("count", "src/b.ts")
        self.assertIn("ts-directive\t1\n", out)
        (self.root / "src/b.ts").write_text("//\t@ts-expect-error x\nf();\n", encoding="utf-8")
        _, out = self.run_tool("count", "src/b.ts")
        self.assertIn("ts-directive\t1\n", out)
        (self.root / "src/b.ts").write_text("// BEST-EFFORT: why.\n// more.\nf();\n", encoding="utf-8")
        _, out = self.run_tool("count", "src/b.ts")
        self.assertIn("best-effort-reason\t1\n", out)
        _, out = self.run_tool("run", "--mode", "all", "src/b.ts")
        self.assertIn("ts: 0 blocks", out)
        self.assertIn("would write 0 of 1 files", out)
        (self.root / "src/b.ts").write_text("// @ts-check\n/** @type {number} */\nlet n = 1;\n/* plain */\n", encoding="utf-8")
        _, out = self.run_tool("count", "src/b.ts")
        self.assertIn("checked-js-types\t1\n", out)

    def test_brace_pairs_align_past_a_following_brace(self):
        pair = [("punct", "{"), ("punct", "}")]
        rest = [("punct", "{"), ("word", "a"), ("punct", "}")]
        self.assertTrue(strip_tree.without_empty_braces(pair + rest, rest))
        self.assertTrue(strip_tree.without_empty_braces(pair + rest, pair + rest))
        self.assertFalse(strip_tree.without_empty_braces(rest, pair + rest))
        self.assertFalse(strip_tree.without_empty_braces(pair + rest, rest[:1]))


class Regrowth(Pipeline):
    def edit(self, rel: str, body: str) -> None:
        (self.root / rel).write_text(body, encoding="utf-8")

    def test_an_added_comment_in_a_stripped_language_fails(self):
        self.edit("src/b.ts", "// back again\nexport const b = 2;\n")
        code, out = self.run_tool("regrowth", "--base", "HEAD")
        self.assertEqual(code, 1, out)
        self.assertIn("COMMENT src/b.ts: // back again", out)

    def test_a_kept_class_and_an_unstripped_language_pass(self):
        self.edit("src/b.ts", "// @ts-expect-error a tool reads this\nexport const b: number = 'x';\n")
        self.edit("src/a.rs", "// rust is not stripped yet\nfn g() {}\n")
        self.edit("src/generated/e.ts", "// generated\nexport const e = 2;\n")
        code, out = self.run_tool("regrowth", "--base", "HEAD")
        self.assertEqual(code, 0, out)
        self.assertIn("1 changed files checked", out)

    def test_a_new_untracked_file_with_a_comment_fails(self):
        self.edit("src/new.ts", "// explains things\nexport const n = 1;\n")
        code, out = self.run_tool("regrowth", "--base", "HEAD")
        self.assertEqual(code, 1, out)
        self.assertIn("COMMENT src/new.ts: // explains things", out)

    def test_an_unchanged_file_with_a_comment_is_not_read(self):
        code, out = self.run_tool("regrowth", "--base", "HEAD")
        self.assertEqual(code, 0, out)
        self.assertIn("0 changed files checked", out)


if __name__ == "__main__":
    unittest.main()
