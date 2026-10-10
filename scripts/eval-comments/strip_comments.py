#!/usr/bin/env python3
"""Remove comments from Rust and TypeScript sources, for the comment-ablation eval.

Two modes:

  justifying  (arm C) removes every contiguous comment block whose text
              contains a keyword from JUSTIFYING_KEYWORDS.
  all         (arm B) removes every comment block.

Both keep tool directives (`@ts-expect-error`, `eslint-disable`, a triple-slash
`<reference>`), because deleting one changes what the compiler accepts.

A block is one of:

  * a run of full-line comments on consecutive lines, blank lines ending it;
  * a trailing comment plus the full-line comments aligned under it;
  * one inline block comment.

The lexer knows strings, raw strings, char literals and lifetimes (Rust),
template literals and regex literals (TypeScript). `tokens_without_comments`
exposes the same lexer so a caller can prove only comments changed.

Usage:
  strip_comments.py --mode justifying FILE...   rewrite files in place
  strip_comments.py --mode justifying --check FILE...   list blocks, change nothing
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass

JUSTIFYING_KEYWORDS = (
    "deliberately",
    "on purpose",
    "load-bearing",
    "load bearing",
    "intentional",
    "acceptable",
    "accepted",
    "harmless",
    "not worth",
    "good enough",
    "best-effort",
    "best effort",
    "for now",
    "workaround",
    "rare enough",
)

# A keyword must start at a word boundary, so "unacceptable" and
# "unintentional" do not match. The tail may run on: "intentionally" matches.
JUSTIFYING_RE = re.compile(
    r"\b(" + "|".join(re.escape(k) for k in JUSTIFYING_KEYWORDS) + r")",
    re.IGNORECASE,
)

DIRECTIVE_RE = re.compile(r"@ts-|eslint-|prettier-ignore|^///\s*<reference|istanbul ignore|c8 ignore")

RUST_SUFFIXES = (".rs",)
TS_SUFFIXES = (".ts", ".tsx", ".mts", ".cts")


@dataclass
class Comment:
    start: int  # offset of the first comment character
    end: int  # offset just past the comment (a line comment stops before "\n")
    kind: str  # "line" or "block"


@dataclass
class Token:
    kind: str
    text: str


def language_of(path: str) -> str | None:
    if path.endswith(RUST_SUFFIXES):
        return "rust"
    if path.endswith(TS_SUFFIXES):
        return "ts"
    return None


class Lexer:
    """One pass over a source file, yielding comments and code tokens."""

    def __init__(self, src: str, lang: str):
        self.src = src
        self.lang = lang
        self.n = len(src)
        self.comments: list[Comment] = []
        self.tokens: list[Token] = []

    def run(self) -> "Lexer":
        self._scan(0, stop_at_brace=False)
        return self

    # The scanner is recursive only for TypeScript template substitutions.
    def _scan(self, i: int, stop_at_brace: bool) -> int:
        src, n = self.src, self.n
        depth = 0
        while i < n:
            c = src[i]
            nxt = src[i + 1] if i + 1 < n else ""
            if c in " \t\r\n":
                i += 1
            elif c == "/" and nxt == "/" and not (self.lang == "ts" and i > 0 and src[i - 1] == ":"):
                # A TypeScript `://` is a URL in JSX text (`<code>https://…</code>`), not a comment.
                j = src.find("\n", i)
                j = n if j == -1 else j
                self.comments.append(Comment(i, j, "line"))
                i = j
            elif c == "/" and nxt == "*":
                i = self._block_comment(i)
            elif self.lang == "rust" and self._rust_string_start(i):
                i = self._rust_string(i)
            elif self.lang == "rust" and c == "'":
                i = self._rust_quote(i)
            elif c == '"' or (self.lang == "ts" and c == "'"):
                i = self._quoted(i, c)
            elif self.lang == "ts" and c == "`":
                i = self._template(i)
            elif self.lang == "ts" and c == "/" and self._regex_allowed():
                i = self._regex(i)
            elif stop_at_brace and c == "}" and depth == 0:
                return i
            else:
                if stop_at_brace and c == "{":
                    depth += 1
                elif stop_at_brace and c == "}":
                    depth -= 1
                i = self._word_or_punct(i)
        return i

    def _word_or_punct(self, i: int) -> int:
        src = self.src
        if src[i].isalnum() or src[i] == "_":
            j = i
            while j < self.n and (src[j].isalnum() or src[j] in "_$"):
                j += 1
            self.tokens.append(Token("word", src[i:j]))
            return j
        self.tokens.append(Token("punct", src[i]))
        return i + 1

    def _block_comment(self, i: int) -> int:
        src, n = self.src, self.n
        depth, j = 1, i + 2
        while j < n and depth:
            if src.startswith("*/", j):
                depth -= 1
                j += 2
            elif self.lang == "rust" and src.startswith("/*", j):
                depth += 1
                j += 2
            else:
                j += 1
        self.comments.append(Comment(i, j, "block"))
        return j

    def _rust_string_start(self, i: int) -> bool:
        src = self.src
        if i > 0 and (src[i - 1].isalnum() or src[i - 1] == "_"):
            return False
        return bool(re.match(r'(b|c)?r#*"|(b|c)"', src[i : i + 260]))

    def _rust_string(self, i: int) -> int:
        src = self.src
        m = re.match(r'(b|c)?(r)?(#*)"', src[i:])
        assert m
        if m.group(2):
            closing = '"' + m.group(3)
            j = src.find(closing, i + m.end())
            j = self.n if j == -1 else j + len(closing)
        else:
            j = self._skip_escaped(i + m.end(), '"')
        self.tokens.append(Token("string", src[i:j]))
        return j

    def _rust_quote(self, i: int) -> int:
        """A char literal ('a', '\\n', b'x' handled by the word path) or a lifetime."""
        src, n = self.src, self.n
        if i + 1 < n and src[i + 1] == "\\":
            j = self._skip_escaped(i + 1, "'")
            self.tokens.append(Token("char", src[i:j]))
            return j
        if i + 2 < n and src[i + 2] == "'":
            self.tokens.append(Token("char", src[i : i + 3]))
            return i + 3
        self.tokens.append(Token("punct", "'"))
        return i + 1

    def _quoted(self, i: int, quote: str) -> int:
        # A TypeScript string cannot span lines, so a stray apostrophe in JSX
        # text costs at most the rest of its own line.
        j = self._skip_escaped(i + 1, quote, stop_at_newline=self.lang == "ts")
        self.tokens.append(Token("string", self.src[i:j]))
        return j

    def _skip_escaped(self, j: int, quote: str, stop_at_newline: bool = False) -> int:
        src, n = self.src, self.n
        while j < n:
            if src[j] == "\\":
                j += 2
            elif src[j] == quote:
                return j + 1
            elif stop_at_newline and src[j] == "\n":
                return j
            else:
                j += 1
        return n

    def _template(self, i: int) -> int:
        src, n = self.src, self.n
        j = i + 1
        start = i
        while j < n:
            if src[j] == "\\":
                j += 2
            elif src[j] == "`":
                j += 1
                break
            elif src.startswith("${", j):
                self.tokens.append(Token("template", src[start:j]))
                j = self._scan(j + 2, stop_at_brace=True) + 1
                start = j
            else:
                j += 1
        self.tokens.append(Token("template", src[start:j]))
        return j

    def _regex_allowed(self) -> bool:
        """A `/` starts a regex after an operator, an opening bracket or a keyword."""
        if not self.tokens:
            return True
        last = self.tokens[-1]
        if last.kind == "punct":
            return last.text not in ")]}"
        if last.kind == "word":
            return last.text in {"return", "typeof", "case", "in", "of", "new", "delete", "void", "throw", "yield", "await", "else", "do"}
        return False

    def _regex(self, i: int) -> int:
        src, n = self.src, self.n
        j, in_class = i + 1, False
        while j < n and src[j] != "\n":
            c = src[j]
            if c == "\\":
                j += 2
                continue
            if c == "[":
                in_class = True
            elif c == "]":
                in_class = False
            elif c == "/" and not in_class:
                j += 1
                while j < n and src[j].isalpha():
                    j += 1
                self.tokens.append(Token("regex", src[i:j]))
                return j
            j += 1
        # No closing slash on this line: it was a division after all.
        self.tokens.append(Token("punct", "/"))
        return i + 1


def lex(src: str, lang: str) -> Lexer:
    return Lexer(src, lang).run()


def tokens_without_comments(src: str, lang: str) -> list[tuple[str, str]]:
    return [(t.kind, t.text) for t in lex(src, lang).tokens]


def _line_start(src: str, i: int) -> int:
    return src.rfind("\n", 0, i) + 1


def _line_end(src: str, i: int) -> int:
    j = src.find("\n", i)
    return len(src) if j == -1 else j


def _is_full_line(src: str, c: Comment) -> bool:
    before = src[_line_start(src, c.start) : c.start]
    after = src[c.end : _line_end(src, c.end)]
    return before.strip() == "" and after.strip() == ""


def _line_no(src: str, i: int) -> int:
    return src.count("\n", 0, i)


def comment_blocks(src: str, lang: str) -> list[list[Comment]]:
    """Group a file's comments into blocks (see the module doc)."""
    blocks: list[list[Comment]] = []
    prev: tuple[int, int, bool] | None = None  # (end line, column, full-line)
    for c in lex(src, lang).comments:
        full = _is_full_line(src, c)
        col = c.start - _line_start(src, c.start)
        joins = (
            prev is not None
            and full
            and c.kind == "line"
            and _line_no(src, c.start) == prev[0] + 1
            and (prev[2] or col == prev[1])
        )
        if joins:
            blocks[-1].append(c)
        else:
            blocks.append([c])
        prev = (_line_no(src, c.end), col, full)
    return blocks


def block_text(src: str, block: list[Comment]) -> str:
    return "\n".join(src[c.start : c.end] for c in block)


def is_directive(src: str, block: list[Comment]) -> bool:
    return any(DIRECTIVE_RE.search(src[c.start : c.end]) for c in block)


def selects(mode: str, text: str) -> bool:
    if mode == "all":
        return True
    if mode == "justifying":
        return bool(JUSTIFYING_RE.search(text))
    raise ValueError(f"unknown mode {mode!r}")


def strip(src: str, lang: str, mode: str) -> tuple[str, list[str]]:
    """Return the stripped source and the text of every removed block."""
    removed: list[str] = []
    cuts: list[tuple[int, int, str]] = []
    for block in comment_blocks(src, lang):
        text = block_text(src, block)
        if is_directive(src, block) or not selects(mode, text):
            continue
        removed.append(text)
        for c in block:
            cuts.append(_cut_for(src, c))
    out = src
    for start, end, repl in sorted(cuts, reverse=True):
        out = out[:start] + repl + out[end:]
    return out, removed


def _cut_for(src: str, c: Comment) -> tuple[int, int, str]:
    """The span to delete for one comment, and what replaces it."""
    ls, le = _line_start(src, c.start), _line_end(src, c.end)
    if _is_full_line(src, c):
        # Whole lines go, including the newline that ends the last one.
        return ls, min(le + 1, len(src)), ""
    after = src[c.end : le]
    if after.strip() == "":
        # A trailing comment: drop it and the whitespace before it.
        start = c.start
        while start > ls and src[start - 1] in " \t":
            start -= 1
        return start, c.end, ""
    # An inline block comment between tokens. Keep one space where two words
    # would otherwise fuse.
    left = src[c.start - 1] if c.start > 0 else " "
    right = src[c.end] if c.end < len(src) else " "
    fuse = (left.isalnum() or left == "_") and (right.isalnum() or right == "_")
    return c.start, c.end, " " if fuse else ""


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--mode", choices=["justifying", "all"], required=True)
    ap.add_argument("--check", action="store_true", help="report blocks, write nothing")
    ap.add_argument("--json", action="store_true", help="print a JSON summary")
    ap.add_argument("files", nargs="+")
    args = ap.parse_args(argv)

    summary: dict[str, list[str]] = {}
    for path in args.files:
        lang = language_of(path)
        if lang is None:
            continue
        with open(path, encoding="utf-8") as f:
            src = f.read()
        out, removed = strip(src, lang, args.mode)
        if not removed:
            continue
        if tokens_without_comments(src, lang) != tokens_without_comments(out, lang):
            print(f"REFUSED {path}: stripping changed a code token", file=sys.stderr)
            return 1
        summary[path] = removed
        if not args.check:
            with open(path, "w", encoding="utf-8") as f:
                f.write(out)

    if args.json:
        json.dump(summary, sys.stdout, indent=1)
        print()
    else:
        blocks = sum(len(v) for v in summary.values())
        print(f"{blocks} blocks in {len(summary)} files")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
