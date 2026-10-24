#!/usr/bin/env python3
"""Unit tests for strip_comments.py. Run: python3 scripts/eval-comments/strip_comments_test.py"""

import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from strip_comments import comment_blocks, strip, tokens_without_comments  # noqa: E402


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


class Grouping(unittest.TestCase):
    def test_block_count(self):
        src = "// a\n// b\n\n// c\nx(); // d\n"
        self.assertEqual(len(comment_blocks(src, "rust")), 3)


if __name__ == "__main__":
    unittest.main()
