#!/usr/bin/env python3
"""Unit tests for strip_comments.py. Run: python3 scripts/comment-strip/strip_comments_test.py"""

import os
import re
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import strip_comments  # noqa: E402
from strip_comments import (  # noqa: E402
    StripResult,
    block_hash,
    comment_blocks,
    load_kept_classes,
    load_kept_list,
    language_of,
    proof_one,
    strip,
    strip_file,
    tool_read_comments,
    tokens_without_comments,
)


def stripped(src: str, lang: str = "rust", mode: str = "justifying") -> str:
    out, _ = strip(src, lang, mode)
    assert tokens_without_comments(src, lang) == tokens_without_comments(out, lang)
    return out


class RustBlocks(unittest.TestCase):
    def test_a_matching_line_block_goes_whole(self):
        src = "fn a() {}\n// first line\n// deliberately so\nfn b() {}\n"
        self.assertEqual(stripped(src), "fn a() {}\nfn b() {}\n")

    def test_a_blank_line_splits_blocks(self):
        src = "// keep me\n\n// on purpose\nfn b() {}\n"
        self.assertEqual(stripped(src), "// keep me\n\nfn b() {}\n")

    def test_doc_and_inner_doc_comments_count(self):
        src = "//! Module is best-effort.\nuse x;\n/// Harmless.\npub fn f() {}\n"
        self.assertEqual(stripped(src), "use x;\npub fn f() {}\n")

    def test_a_non_matching_block_stays(self):
        src = "/// Returns the thing.\npub fn f() {}\n"
        self.assertEqual(stripped(src), src)

    def test_word_boundary_blocks_unacceptable(self):
        src = "// unacceptable and unintentional\nfn f() {}\n"
        self.assertEqual(stripped(src), src)

    def test_the_tail_of_a_keyword_may_run_on(self):
        src = "// intentionally left\nfn f() {}\n"
        self.assertEqual(stripped(src), "fn f() {}\n")

    def test_a_trailing_comment_leaves_its_code(self):
        src = "let x = 1; // good enough\nlet y = 2;\n"
        self.assertEqual(stripped(src), "let x = 1;\nlet y = 2;\n")

    def test_aligned_continuation_joins_a_trailing_comment(self):
        src = "let x = 1; // the first half\n           // is a workaround\nlet y = 2;\n"
        self.assertEqual(stripped(src), "let x = 1;\nlet y = 2;\n")

    def test_an_unaligned_full_line_starts_a_new_block(self):
        src = "let x = 1; // for now\n// separate note\nlet y = 2;\n"
        self.assertEqual(stripped(src), "let x = 1;\n// separate note\nlet y = 2;\n")

    def test_nested_block_comments(self):
        src = "/* outer /* inner */ still accepted */\nfn f() {}\n"
        self.assertEqual(stripped(src), "fn f() {}\n")

    def test_an_inline_block_comment_keeps_words_apart(self):
        src = "let a = b/* harmless */c;\nlet d = e /* harmless */ + f;\n"
        self.assertEqual(stripped(src), "let a = b c;\nlet d = e  + f;\n")

    def test_comment_markers_inside_strings_are_code(self):
        src = 'let u = "http://x // deliberately";\nlet v = 1;\n'
        self.assertEqual(stripped(src), src)

    def test_raw_strings_hide_comment_lines(self):
        src = 'let s = r#"\n// deliberately\n"#;\n// on purpose\nfn f() {}\n'
        self.assertEqual(stripped(src), 'let s = r#"\n// deliberately\n"#;\nfn f() {}\n')

    def test_byte_and_escaped_strings(self):
        src = 'let s = b"a\\"// harmless";\nlet c = \'"\';\n// harmless\n'
        self.assertEqual(stripped(src), 'let s = b"a\\"// harmless";\nlet c = \'"\';\n')

    def test_lifetimes_are_not_char_literals(self):
        src = "fn f<'a>(x: &'a str) -> &'a str { x } // accepted\nlet q = '\\'';\n"
        self.assertEqual(stripped(src), "fn f<'a>(x: &'a str) -> &'a str { x }\nlet q = '\\'';\n")

    def test_all_mode_removes_everything_but_directives(self):
        src = "// a\nfn f() {} // b\n/* c */\n"
        self.assertEqual(stripped(src, mode="all"), "fn f() {}\n")


class TypeScript(unittest.TestCase):
    def test_line_and_jsdoc_blocks(self):
        src = "/** Deliberately loose. */\nexport const a = 1;\n// fine\nexport const b = 2;\n"
        self.assertEqual(stripped(src, "ts"), "export const a = 1;\n// fine\nexport const b = 2;\n")

    def test_directives_survive_even_when_they_match(self):
        src = "// @ts-expect-error: accepted for now\nconst x: number = 'a';\n"
        self.assertEqual(stripped(src, "ts"), src)

    def test_template_literals_with_substitutions(self):
        src = "const u = `http://${host /* harmless */}/a // not a comment`;\n"
        self.assertEqual(stripped(src, "ts"), "const u = `http://${host }/a // not a comment`;\n")

    def test_regex_literals_are_not_comments(self):
        src = "const re = /^\\/\\/ accepted/;\nconst d = a / b; // on purpose\n"
        self.assertEqual(stripped(src, "ts"), "const re = /^\\/\\/ accepted/;\nconst d = a / b;\n")

    def test_a_url_in_jsx_text_is_not_a_comment(self):
        src = "const el = <code>https://example.com</code>;\n// on purpose\n"
        self.assertEqual(stripped(src, "ts", mode="all"), "const el = <code>https://example.com</code>;\n")

    def test_a_jsx_apostrophe_costs_at_most_its_line(self):
        src = "const el = <p>don't</p>;\n// deliberately\nconst x = 1;\n"
        self.assertEqual(stripped(src, "ts"), "const el = <p>don't</p>;\nconst x = 1;\n")


def kept(src: str, lang: str = "rust", mode: str = "keep-invariants", hashes=frozenset()) -> str:
    res = strip_file(src, lang, mode, hashes)
    assert proof_one(src, res, lang) is None, proof_one(src, res, lang)
    return res.out


class FileEnd(unittest.TestCase):
    def test_a_comment_at_the_end_leaves_no_blank_last_line(self):
        res = strip_comments.strip_file("const a = 1;\n\n// gone\n", "ts", "all")
        self.assertEqual(res.out, "const a = 1;\n")
        self.assertIsNone(strip_comments.proof_one("const a = 1;\n\n// gone\n", res, "ts"))

    def test_a_deprecation_tag_stays_for_the_language_service(self):
        src = "/** @deprecated Use b. */\nexport const a = 1;\n"
        self.assertEqual(strip_comments.strip_file(src, "ts", "all").out, src)


class KeptClasses(unittest.TestCase):
    def test_a_vitest_environment_line_survives(self):
        src = "// @vitest-environment jsdom\n// Covers the picker.\nimport x from 'y';\n"
        self.assertEqual(kept(src, "ts", "all"), "// @vitest-environment jsdom\nimport x from 'y';\n")

    def test_a_vitest_environment_docblock_survives(self):
        src = "/**\n * @vitest-environment happy-dom\n */\nimport x from 'y';\n"
        self.assertEqual(kept(src, "ts", "all"), src)

    def test_only_the_directive_line_of_a_block_stays(self):
        src = "// Why the cast is wrong.\n// @ts-expect-error bad cast\n// More prose.\nf(1 as string);\n"
        self.assertEqual(kept(src, "ts"), "// @ts-expect-error bad cast\nf(1 as string);\n")

    def test_a_tab_before_a_directive_keeps_it(self):
        src = "//\t@ts-expect-error bad cast\nf(1 as string);\n"
        self.assertEqual(kept(src, "ts"), src)

    def test_a_jsx_pragma_stays(self):
        src = "/* @jsxImportSource preact */\nconst a = 1;\n"
        self.assertEqual(kept(src, "ts"), src)

    def test_the_kept_list_prefix_never_lands_on_a_directive(self):
        src = "// @ts-expect-error the caller checks the cast\n// A wrong tag must throw.\nfoo(1);\n"
        block = "// @ts-expect-error the caller checks the cast\n// A wrong tag must throw."
        res = strip_file(src, "ts", "keep-invariants", {block_hash(block)})
        self.assertEqual(res.out, "// @ts-expect-error the caller checks the cast\n// INVARIANT: A wrong tag must throw.\nfoo(1);\n")
        self.assertIsNone(proof_one(src, res, "ts"))

    def test_a_doc_comment_inside_macro_input_stays(self):
        src = "tools! {\n    /// Reads a file.\n    read // gone\n}\n"
        want = "tools! {\n    /// Reads a file.\n    read\n}\n"
        for mode in ("all", "keep-invariants"):
            self.assertEqual(strip_file(src, "rust", mode).out, want)

    def test_a_forwarding_macro_loses_its_doc_comments(self):
        src = "thread_local! {\n    /// Doc.\n    static A: u8 = 0;\n}\n"
        self.assertEqual(strip_file(src, "rust", "all").out, "thread_local! {\n    static A: u8 = 0;\n}\n")
        named = "macro_rules! m { () => {\n    /// Doc.\n    fn f() {}\n} }\n"
        self.assertEqual(strip_file(named, "rust", "all").out, "macro_rules! m { () => {\n    fn f() {}\n} }\n")

    def test_an_eslint_block_form_no_class_covers_is_refused(self):
        for src in ("/* eslint no-console: 0 */\nf();\n", "/* global foo */\nf();\n", "/*\n  globals a, b\n*/\nf();\n"):
            res = strip_file(src, "ts", "all")
            self.assertIn("eslint-unread-form", res.refusal)
            self.assertEqual(res.out, src)
        kept_form = "/*\n eslint-disable no-console\n*/\nf();\n"
        self.assertEqual(kept(kept_form, "ts"), kept_form)
        self.assertEqual(kept("// global state, not a directive\nf();\n", "ts"), "f();\n")

    def test_the_kept_list_follows_a_moved_file(self):
        src = "// hold the lock across both writes\nlet a = 1;\n// gone\nlet b = 2;\n"
        kept_block = block_hash("// hold the lock across both writes")
        with tempfile.TemporaryDirectory() as d:
            listed = os.path.join(d, "kept-list.tsv")
            with open(listed, "w", encoding="utf-8") as f:
                f.write(f"# head\nold/place.rs\t{kept_block}\tkeep\twhy\nold/place.rs\t{'0' * 16}\tadr\twhy\n")
            kept_hashes = load_kept_list(listed)
        self.assertEqual(kept_hashes, frozenset({kept_block}))
        out = strip_file(src, "rust", "keep-invariants", kept_hashes).out
        self.assertEqual(out, "// INVARIANT: hold the lock across both writes\nlet a = 1;\nlet b = 2;\n")

    def test_an_empty_comment_on_the_kept_list_is_refused(self):
        for src in ("/**/\nf();\n", "//\nf();\n"):
            res = strip_file(src, "ts", "keep-invariants", {block_hash(src.split("\n")[0])})
            self.assertIn("the kept list names an empty comment", res.refusal)
            self.assertEqual(res.out, src)

    def test_a_bundler_annotation_stays(self):
        src = "const m = import(/* @vite-ignore */ url);\nconst x = /* #__PURE__ */ f();\n"
        self.assertEqual(kept(src, "ts"), src)

    def test_javascript_is_in_scope(self):
        for name in ("a.js", "a.mjs", "a.cjs", "a.jsx"):
            self.assertEqual(language_of(name), "ts")

    def test_a_hashbang_line_is_never_a_comment(self):
        src = "#!/usr/bin//node\n// gone\nexport const i = 1;\n"
        self.assertEqual(kept(src, "ts"), "#!/usr/bin//node\nexport const i = 1;\n")

    def test_a_doctest_stays_with_its_block(self):
        src = "// gone\n\n/// A handle.\n///\n/// ```compile_fail\n/// let x: u8 = 256;\n/// ```\nstruct S;\n"
        for mode in ("all", "keep-invariants"):
            self.assertEqual(strip_file(src, "rust", mode).out, src[len("// gone\n") :])
        lost = "struct S;\n"
        self.assertNotEqual(tool_read_comments(src, "rust"), tool_read_comments(lost, "rust"))
        prose = "/// ```text\n/// a diagram\n/// ```\nstruct S;\n"
        self.assertEqual(kept(prose, "rust"), "struct S;\n")

    def test_a_source_pragma_stays(self):
        for pragma in ("//# sourceURL=a.js", "//@ sourceURL=a.js", "//# sourceMappingURL=a.js.map", "//# debugId=0a1b", "/*# sourceMappingURL=a.css.map */"):
            src = f"f(); // gone\n{pragma}\n"
            self.assertEqual(kept(src, "ts"), f"f();\n{pragma}\n")

    def test_jsdoc_stays_in_a_file_the_compiler_checks(self):
        typed = '/** @type {number} */\nlet n = 1; // gone\n/* gone */\n'
        checked = "// @ts-check\n" + typed
        for mode in ("all", "keep-invariants"):
            self.assertEqual(strip_file(checked, "ts", mode).out, '// @ts-check\n/** @type {number} */\nlet n = 1;\n')
            self.assertEqual(strip_file(typed, "ts", mode).out, "let n = 1;\n")
        for opener in ("/// @ts-check", "//\t@ts-check"):
            self.assertEqual(strip_file(f"{opener}\n{typed}", "ts", "all").out, f'{opener}\n/** @type {{number}} */\nlet n = 1;\n')

    def test_a_tool_read_comment_carries_its_place(self):
        above_a = "// eslint-disable-next-line no-console\na();\nb();\n"
        above_b = "a();\n// eslint-disable-next-line no-console\nb();\n"
        self.assertNotEqual(tool_read_comments(above_a, "ts"), tool_read_comments(above_b, "ts"))
        own_line = "a();\n// eslint-disable-line\nb();\n"
        trailing = "a(); // eslint-disable-line\nb();\n"
        self.assertNotEqual(tool_read_comments(own_line, "ts"), tool_read_comments(trailing, "ts"))
        wrapped = "const e = <a>{/* gone */}\n{/* eslint-disable-next-line x */}\n<b /></a>;\n"
        unwrapped = "const e = <a>\n{/* eslint-disable-next-line x */}\n<b /></a>;\n"
        self.assertEqual(tool_read_comments(wrapped, "ts"), tool_read_comments(unwrapped, "ts"))

    def test_a_directive_stays_on_its_line_when_a_comment_before_it_goes(self):
        src = "a(); /* gone\n */ // eslint-disable-line\nb();\n"
        self.assertEqual(kept(src, "ts"), "a();\n// eslint-disable-line\nb();\n")

    def test_a_carriage_return_is_refused(self):
        src = "const a = 1; // c\r\nconst b = 2;\r\n"
        self.assertIn("carriage return", strip_file(src, "ts", "all").refusal)
        self.assertEqual(strip(src, "ts", "all"), (src, []))

    def test_a_refused_file_reports_no_removed_block(self):
        src = "/* gone? */\na/* c */b { color: red; }\n"
        self.assertEqual(strip(src, "css", "all"), (src, []))

    def test_prose_that_names_a_directive_goes(self):
        src = "// We avoid @ts-expect-error here.\nconst a = 1;\n"
        self.assertEqual(kept(src, "ts"), "const a = 1;\n")

    def test_a_safety_note_stays_whole(self):
        src = "// SAFETY: the pointer is valid\n// for the whole call.\nunsafe { f(p) }\n// gone\n"
        self.assertEqual(kept(src), "// SAFETY: the pointer is valid\n// for the whole call.\nunsafe { f(p) }\n")

    def test_a_registry_pointer_stays(self):
        src = "/* Polls until SSE covers lists (docs/temporary-measures.md). */\n.a { color: red; }\n/* gone */\n"
        self.assertEqual(kept(src, "css"), "/* Polls until SSE covers lists (docs/temporary-measures.md). */\n.a { color: red; }\n")

    def test_a_marked_invariant_stays(self):
        src = "// INVARIANT: callers hold the lock.\nfn f() {}\n"
        self.assertEqual(kept(src), src)

    def test_the_word_invariant_inside_prose_is_no_marker(self):
        src = "/** It rests on one\n *  INVARIANT: nothing sits between. */\nconst a = 1;\n"
        self.assertEqual(kept(src, "ts"), "const a = 1;\n")

    def test_prose_classes_go_in_all_mode(self):
        src = "// SAFETY: fine.\nunsafe { f() }\n"
        self.assertEqual(kept(src, mode="all"), "unsafe { f() }\n")

    def test_a_best_effort_reason_stays_whole_in_every_mode(self):
        src = (
            "// BEST-EFFORT: a toast is wrong for a keepalive,\n"
            "// and the next beat retries it. For now.\n"
            "void flush();\n"
            "// gone, for now\n"
        )
        want = src.removesuffix("// gone, for now\n")
        for mode in ("justifying", "all", "keep-invariants"):
            self.assertEqual(kept(src, "ts", mode=mode), want, mode)

    def test_a_best_effort_reason_stays_in_css_and_rust_too(self):
        self.assertEqual(kept("/* BEST-EFFORT: why. */\n.a { color: red; }\n", "css", mode="all"), "/* BEST-EFFORT: why. */\n.a { color: red; }\n")
        self.assertEqual(kept("// BEST-EFFORT: why.\nfn f() {}\n", mode="all"), "// BEST-EFFORT: why.\nfn f() {}\n")

    def test_a_best_effort_reason_outranks_a_prose_class_in_the_same_block(self):
        src = "// BEST-EFFORT: see docs/temporary-measures.md.\nfn f() {}\n"
        for mode in ("justifying", "all"):
            self.assertEqual(kept(src, mode=mode), src, mode)

    def test_the_word_best_effort_inside_prose_is_no_marker(self):
        src = "/** A best-effort write\n *  BEST-EFFORT: not at the start. */\nvoid flush();\n"
        self.assertEqual(kept(src, "ts", mode="all"), "void flush();\n")

    def test_the_rule_states_the_marker_the_kept_class_reads(self):
        row = next(k for k in load_kept_classes() if k.name == "best-effort-reason")
        marker = re.search(r"([A-Z]+(?:-[A-Z]+)*:)", row.pattern.pattern).group(1)
        rule = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", ".claude", "rules", "frontend.md")
        with open(rule, encoding="utf-8") as f:
            self.assertIn(marker, f.read())

    def test_every_class_names_a_known_kind_and_language(self):
        for k in load_kept_classes():
            self.assertTrue(k.langs <= {"rust", "ts", "css"}, k.name)

    def test_patterns_stay_valid_for_grep(self):
        for k in load_kept_classes():
            self.assertNotRegex(k.pattern.pattern, r"\\[A-Za-z]", k.name)

    def test_no_second_list_of_kept_classes(self):
        here = os.path.dirname(os.path.abspath(__file__))
        literals = ("@ts-", "vitest-environment", "eslint-", "SAFETY", "BEST-EFFORT", "temporary-measures", "ValueEnum", "Subcommand")
        for name in sorted(os.listdir(here)):
            if name == "kept-classes.tsv" or "test" in name or not os.path.isfile(os.path.join(here, name)):
                continue
            with open(os.path.join(here, name), encoding="utf-8") as f:
                body = f.read()
            for lit in literals:
                self.assertNotIn(lit, body, f"{name} restates a kept class")


class ClapHelp(unittest.TestCase):
    SRC = (
        "use clap::Parser;\n"
        "// plain above\n"
        "/// The tool.\n"
        "#[derive(Parser, Debug)]\n"
        "#[command(name = \"x\")]\n"
        "struct Cli {\n"
        "    // plain inside\n"
        "    /// Path to read.\n"
        "    #[arg(long)]\n"
        "    path: String, // trailing\n"
        "}\n"
        "/// Not help text.\n"
        "struct Other;\n"
    )

    def test_doc_comments_on_a_clap_item_stay(self):
        want = (
            "use clap::Parser;\n"
            "/// The tool.\n"
            "#[derive(Parser, Debug)]\n"
            "#[command(name = \"x\")]\n"
            "struct Cli {\n"
            "    /// Path to read.\n"
            "    #[arg(long)]\n"
            "    path: String,\n"
            "}\n"
            "struct Other;\n"
        )
        self.assertEqual(kept(self.SRC), want)
        self.assertEqual(kept(self.SRC, mode="all"), want)

    def test_enum_variants_and_qualified_derives(self):
        src = "#[derive(clap::Subcommand)]\nenum Cmd {\n    /// Run it.\n    Run { /// How often.\n        n: u8 },\n}\n/// gone\nfn f() {}\n"
        self.assertEqual(kept(src), src.replace("/// gone\n", ""))

    def test_a_tuple_struct_ends_at_its_semicolon(self):
        src = "#[derive(Args)]\nstruct A(\n    /// Help.\n    String,\n);\n/// gone\nstruct B;\n"
        self.assertEqual(kept(src), src.replace("/// gone\n", ""))

    def test_another_derive_keeps_nothing(self):
        src = "/// gone\n#[derive(Debug)]\nstruct A {\n    /// gone\n    a: u8,\n}\n"
        self.assertEqual(kept(src), "#[derive(Debug)]\nstruct A {\n    a: u8,\n}\n")

    def test_a_const_generic_default_does_not_end_the_item(self):
        src = "/// Top.\n#[derive(Args)]\nstruct Cli<const N: usize = { 3 }> {\n    /// Path.\n    path: String,\n}\n"
        self.assertEqual(strip_file(src, "rust", "keep-invariants").out, src)

    def test_the_kept_list_never_marks_help_text(self):
        res = strip_file(self.SRC, "rust", "keep-invariants", {block_hash("/// The tool.")})
        self.assertIn("/// The tool.\n", res.out)


class KeptList(unittest.TestCase):
    def test_a_listed_block_stays_and_gains_the_marker(self):
        src = "// Callers hold the lock.\n// Always.\nfn f() {}\n// gone\n"
        out = kept(src, hashes={block_hash("// Callers hold the lock.\n// Always.")})
        self.assertEqual(out, "// INVARIANT: Callers hold the lock.\n// Always.\nfn f() {}\n")
        self.assertEqual(kept(out), out)

    def test_the_marker_fits_each_opener(self):
        for src, want in (
            ("/// Doc.\nfn f() {}\n", "/// INVARIANT: Doc.\nfn f() {}\n"),
            ("/**\n * Doc.\n */\nfn f() {}\n", "/** INVARIANT:\n * Doc.\n */\nfn f() {}\n"),
            ("//tight\nfn f() {}\n", "// INVARIANT: tight\nfn f() {}\n"),
        ):
            block = src[: src.index("\nfn")]
            self.assertEqual(kept(src, hashes={block_hash(block)}), want)

    def test_the_list_is_ignored_outside_the_commissioned_mode(self):
        src = "// Callers hold the lock.\nfn f() {}\n"
        self.assertEqual(kept(src, mode="all", hashes={block_hash("// Callers hold the lock.")}), "fn f() {}\n")


class TokenSeparation(unittest.TestCase):
    """A removed comment never joins two tokens, and never moves a line terminator."""

    ASI = (
        "function f() {\n  return /* a\n  b */ 1;\n}\n",
        "function f() {\n  throw /* a\n  b */ e;\n}\n",
        "for (;;) {\n  break /* a\n  b */ label;\n}\n",
        "for (;;) {\n  continue /* a\n  b */ label;\n}\n",
        "function* g() {\n  yield /* a\n  b */ 1;\n}\n",
        "a /* a\n b */ ++\nb;\n",
        "a /* a\n b */ --\nb;\n",
        "const a = b /* a\n b */ (c);\n",
        "const a = b /* a\n b */ [c];\n",
        "const a = b /* a\n b */ `c`;\n",
    )

    def test_a_line_terminator_inside_a_comment_survives(self):
        for src in self.ASI:
            out = kept(src, "ts")
            self.assertNotIn("/*", out)
            head = src[: src.index("/*")].rstrip()
            self.assertIn(head + "\n", out, src)

    def test_the_proof_refuses_a_lost_line_terminator(self):
        for src in self.ASI:
            joined = re.sub(r" ?/\*.*?\*/ ?", " ", src, flags=re.S)
            res = StripResult(joined)
            self.assertIn("separation", proof_one(src, res, "ts") or "", src)

    def test_the_proof_refuses_an_added_line_terminator(self):
        res = StripResult("return\n1;\n")
        self.assertIn("separation", proof_one("return /* a */ 1;\n", res, "ts"))

    def test_touching_punctuation_stays_apart(self):
        self.assertEqual(kept("let a = b &/* c */& d;\n"), "let a = b & & d;\n")
        self.assertEqual(kept("const a = b +/* c */+ d;\n", "ts"), "const a = b + + d;\n")

    def test_the_proof_refuses_joined_punctuation(self):
        self.assertIn("separation", proof_one("a &/* c */& b;\n", StripResult("a && b;\n"), "rust"))

    def test_the_proof_refuses_a_lost_kept_comment(self):
        src = "// SAFETY: fine.\nunsafe { f() }\n"
        res = strip_file(src, "rust", "keep-invariants")
        res.out = "unsafe { f() }\n"
        self.assertIn("surviving", proof_one(src, res, "rust"))


class Css(unittest.TestCase):
    def test_comments_go_and_rules_stay(self):
        src = "/* Header. */\n.a {\n  color: red; /* why */\n  margin: 0 /* x */ auto;\n}\n"
        self.assertEqual(kept(src, "css"), ".a {\n  color: red;\n  margin: 0  auto;\n}\n")

    def test_strings_and_urls_hide_comment_markers(self):
        src = ".a { content: '/* no */'; background: url(http://x/*.png); }\n.b { background: url( \"/*y*/\" ); }\n"
        self.assertEqual(kept(src, "css"), src)

    def test_a_double_slash_is_not_a_comment(self):
        src = ".a { background: url(//cdn.example.com/a.png); }\n/* gone */\n"
        self.assertEqual(kept(src, "css"), ".a { background: url(//cdn.example.com/a.png); }\n")

    def test_a_comment_between_touching_tokens_is_refused(self):
        res = strip_file(".a/* c */.b { color: red; }\n", "css", "keep-invariants")
        self.assertIn("both sides", res.refusal)
        self.assertIn("both sides", proof_one(".a/* c */.b {}\n", res, "css"))

    def test_the_proof_refuses_a_lost_space(self):
        self.assertIn("separation", proof_one(".a /* c */ .b {}\n", StripResult(".a.b {}\n"), "css"))

    def test_a_newline_inside_a_comment_is_not_whitespace(self):
        self.assertIn("separation", proof_one(".a/* \n */.b {}\n", StripResult(".a\n.b {}\n"), "css"))


class JsxWrappers(unittest.TestCase):
    def test_the_lexer_stage_leaves_the_braces(self):
        src = "const el = (\n  <div>\n    {/* note */}\n    <b />\n  </div>\n);\n"
        self.assertEqual(kept(src, "ts"), "const el = (\n  <div>\n    { }\n    <b />\n  </div>\n);\n")


class Grouping(unittest.TestCase):
    def test_rust_doc_and_plain_comments_are_separate_blocks(self):
        self.assertEqual(len(comment_blocks("// a\n/// b\nfn f() {}\n", "rust")), 2)
        self.assertEqual(len(comment_blocks("// a\n/// b\nconst f = 1;\n", "ts")), 1)

    def test_block_count(self):
        src = "// a\n// b\n\n// c\nx(); // d\n"
        self.assertEqual(len(comment_blocks(src, "rust")), 3)


if __name__ == "__main__":
    unittest.main()
