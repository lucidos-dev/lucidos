#!/usr/bin/env python3
"""Remove comments from Rust, TypeScript and CSS sources.

Three modes:

  justifying       (eval arm C) removes every comment block whose text contains
                   a keyword from JUSTIFYING_KEYWORDS.
  all              (eval arm B) removes every comment that no tool reads.
  keep-invariants  (the commissioned strip) also keeps the prose classes, and
                   every block the kept list names.

`kept-classes.tsv`, beside this file, is the one definition of what stays.

A block is one of:

  * a run of full-line comments on consecutive lines, blank lines ending it;
  * a trailing comment plus the full-line comments aligned under it;
  * one inline block comment.

A Rust block also ends where doc comments meet plain ones.

The lexer knows strings, raw strings, char literals and lifetimes (Rust),
template literals and regex literals (TypeScript), and unquoted `url()` (CSS).
`proof_one` uses the same lexer to prove that only comments changed. It
compares the tokens and what separates each pair of them. A removed comment
must never join two tokens. It must never add or remove a line terminator
between two either, because JavaScript inserts a semicolon at one.

Usage:
  strip_comments.py --mode keep-invariants FILE...          rewrite files in place
  strip_comments.py --mode keep-invariants --check FILE...  list blocks, change nothing

`strip_tree.py` runs the whole pipeline, with the independent second proof.
"""

from __future__ import annotations

import argparse
import bisect
import functools
import hashlib
import itertools
import json
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path

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

KEPT_CLASSES_PATH = Path(__file__).with_name("kept-classes.tsv")
KEPT_CLASS_KINDS = ("directive", "tool-block", "prose", "clap-derive", "macro-doc", "unsupported", "typed-jsdoc")
# A class of these kinds keeps a doc comment that a macro reads, in every mode.
MACRO_READ_KINDS = ("clap-derive", "macro-doc")
MODES = ("justifying", "all", "keep-invariants")
# The stripper writes this in front of a block the kept list names. The
# `site-invariant` kept class then recognises the block on every later run.
INVARIANT_PREFIX = "INVARIANT:"
COMMENT_OPENER_RE = re.compile(r"//[/!]?|/\*[*!]?")
EMPTY_COMMENT_RE = re.compile(r"//[/!]?\s*|/\*[*!\s]*\*/")

RUST_SUFFIXES = (".rs",)
TS_SUFFIXES = (".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs")
JSX_SUFFIXES = (".tsx", ".jsx")
CSS_SUFFIXES = (".css",)


@dataclass(frozen=True)
class KeptClass:
    name: str
    kind: str
    langs: frozenset
    pattern: re.Pattern
    why: str = ""

    def reads(self, comment: str) -> bool:
        """Whether a directive class matches this comment. A whitespace run counts as one space."""
        return self.kind == "directive" and self._matches_collapsed(comment)

    def reads_block(self, comment: str) -> bool:
        """Whether a tool-block class matches this comment. A tool then reads its whole block."""
        return self.kind == "tool-block" and bool(self.pattern.search(comment))

    def refuses(self, comment: str) -> bool:
        """Whether an unsupported class matches this comment: a form no proof covers."""
        return self.kind == "unsupported" and self._matches_collapsed(comment)

    def _matches_collapsed(self, comment: str) -> bool:
        return bool(self.pattern.search(" ".join(comment.split())))


@functools.cache
def load_kept_classes(path: Path = KEPT_CLASSES_PATH) -> tuple[KeptClass, ...]:
    classes = []
    for line in path.read_text(encoding="utf-8").splitlines():
        if not line or line.startswith("#"):
            continue
        name, kind, langs, pattern, why = line.split("\t")
        if kind not in KEPT_CLASS_KINDS:
            raise ValueError(f"{path}: unknown kind {kind!r} for {name}")
        classes.append(KeptClass(name, kind, frozenset(langs.split(",")), re.compile(pattern), why))
    return tuple(classes)


def load_kept_list(path: str) -> frozenset[str]:
    """Read `path<TAB>block hash<TAB>verdict` rows, and return the hashes of the `keep` rows.

    The hash is the whole key. The path column is a hint for a reader, so a
    moved or renamed file keeps its rows.
    """
    kept = set()
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        if not line or line.startswith("#"):
            continue
        _hint, block, verdict = line.split("\t")[:3]
        if verdict == "keep":
            kept.add(block)
    return frozenset(kept)


def read_source(path) -> str:
    """The file's text with its line ends as written, so a carriage return is seen."""
    return Path(path).read_bytes().decode("utf-8")


@dataclass
class Comment:
    start: int  # offset of the first comment character
    end: int  # offset just past the comment (a line comment stops before "\n")
    kind: str  # "line" or "block"


@dataclass
class Token:
    kind: str
    text: str
    start: int
    # What separates this token from the one before: "" nothing, "c" comments
    # only, "w" whitespace, "n" a line terminator.
    gap: str

    @property
    def end(self) -> int:
        return self.start + len(self.text)


def language_of(path: str) -> str | None:
    if path.endswith(RUST_SUFFIXES):
        return "rust"
    if path.endswith(TS_SUFFIXES):
        return "ts"
    if path.endswith(CSS_SUFFIXES):
        return "css"
    return None


class Lexer:
    """One pass over a source file, yielding comments and code tokens."""

    def __init__(self, src: str, lang: str):
        self.src = src
        self.lang = lang
        self.n = len(src)
        self.comments: list[Comment] = []
        self.tokens: list[Token] = []
        self._prev_end = 0
        self._prev_comments = 0

    def run(self) -> "Lexer":
        start = 0
        if self.lang == "ts" and self.src.startswith("#!"):
            # The hashbang line is one token: a `//` in its path opens no comment.
            start = _line_end(self.src, 0)
            self._emit("hashbang", 0, start)
        self._scan(start, stop_at_brace=False)
        return self

    def _emit(self, kind: str, start: int, end: int) -> None:
        between = self.src[self._prev_end : start]
        outside, pos = [], self._prev_end
        for c in self.comments[self._prev_comments :]:
            outside.append(self.src[pos : c.start])
            pos = c.end
        outside.append(self.src[pos:start])
        blank = "".join(outside)
        # A newline inside a comment ends a JavaScript line. It is not CSS whitespace.
        if "\n" in (blank if self.lang == "css" else between) or not self.tokens:
            gap = "n"
        elif blank:
            gap = "w"
        elif between:
            gap = "c"
        else:
            gap = ""
        self.tokens.append(Token(kind, self.src[start:end], start, gap))
        self._prev_end = end
        self._prev_comments = len(self.comments)

    # The scanner is recursive only for TypeScript template substitutions.
    def _scan(self, i: int, stop_at_brace: bool) -> int:
        src, n = self.src, self.n
        depth = 0
        while i < n:
            c = src[i]
            nxt = src[i + 1] if i + 1 < n else ""
            if c in " \t\r\n":
                i += 1
            elif c == "/" and nxt == "/" and self._line_comment_allowed(i):
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
            elif c == '"' or (self.lang != "rust" and c == "'"):
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

    def _line_comment_allowed(self, i: int) -> bool:
        if self.lang == "css":
            return False
        # A TypeScript `://` is a URL in JSX text (`<code>https://…</code>`), not a comment.
        return not (self.lang == "ts" and i > 0 and self.src[i - 1] == ":")

    def _word_or_punct(self, i: int) -> int:
        src = self.src
        if src[i].isalnum() or src[i] == "_":
            j = i
            while j < self.n and (src[j].isalnum() or src[j] in "_$"):
                j += 1
            if self.lang == "css" and src[i:j].lower() == "url" and src.startswith("(", j):
                k = j + 1
                while k < self.n and src[k] in " \t\r\n":
                    k += 1
                if k < self.n and src[k] not in "\"'":
                    # An unquoted URL may hold `/*`, which is not a comment there.
                    close = src.find(")", k)
                    j = self.n if close == -1 else close + 1
            self._emit("word", i, j)
            return j
        self._emit("punct", i, i + 1)
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
        self._emit("string", i, j)
        return j

    def _rust_quote(self, i: int) -> int:
        """A char literal ('a', '\\n', b'x' handled by the word path) or a lifetime."""
        src, n = self.src, self.n
        if i + 1 < n and src[i + 1] == "\\":
            j = self._skip_escaped(i + 1, "'")
            self._emit("char", i, j)
            return j
        if i + 2 < n and src[i + 2] == "'":
            self._emit("char", i, i + 3)
            return i + 3
        self._emit("punct", i, i + 1)
        return i + 1

    def _quoted(self, i: int, quote: str) -> int:
        # A TypeScript or CSS string cannot span lines, so a stray apostrophe
        # in JSX text costs at most the rest of its own line.
        j = self._skip_escaped(i + 1, quote, stop_at_newline=self.lang != "rust")
        self._emit("string", i, j)
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
                self._emit("template", start, j)
                j = self._scan(j + 2, stop_at_brace=True) + 1
                start = j
            else:
                j += 1
        self._emit("template", start, min(j, n))
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
                self._emit("regex", i, j)
                return j
            j += 1
        # No closing slash on this line: it was a division after all.
        self._emit("punct", i, i + 1)
        return i + 1


def lex(src: str, lang: str) -> Lexer:
    return Lexer(src, lang).run()


def tokens_without_comments(src: str, lang: str) -> list[tuple[str, str]]:
    return [(t.kind, t.text) for t in lex(src, lang).tokens]


def tool_read_comments(src: str, lang: str, lx: Lexer | None = None) -> list[tuple[str, int, bool, bool]]:
    """Every comment a tool reads, with its place. A strip changes none of them.

    The place is the count of code tokens before it, and whether a line
    terminator separates it from the token on each side. A touching `{` `}`
    pair is not counted, since the JSX pass removes an emptied one.
    """
    lx = lx or lex(src, lang)
    classes = [k for k in load_kept_classes() if lang in k.langs]
    toks = lx.tokens
    paired = [False] * len(toks)
    for i in range(len(toks) - 1):
        if toks[i].text == "{" and toks[i + 1].text == "}":
            paired[i] = paired[i + 1] = True
    counted = list(itertools.accumulate((not p for p in paired), initial=0))
    starts = [t.start for t in toks]
    out = []
    for block in classify_blocks(src, lang, lx):
        for c, read in zip(block.comments, block.directive):
            text = src[c.start : c.end]
            if not (read or any(k.refuses(text) for k in classes)):
                continue
            n = bisect.bisect_left(starts, c.start)
            before = src[toks[n - 1].end if n else 0 : c.start]
            after = src[c.end : toks[n].start if n < len(toks) else len(src)]
            out.append((text, counted[n], n == 0 or "\n" in before, "\n" in after))
    return out


def token_signature(lx: Lexer) -> list[tuple[str, str, bool, bool]]:
    """Each token with whether something separates it from the one before, and
    whether a line terminator does."""
    return [(t.kind, t.text, t.gap != "", t.gap == "n") for t in lx.tokens]


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


def _doc_kind(src: str, c: Comment, lang: str) -> str:
    """`outer` and `inner` are Rust doc comments, which the compiler reads as attributes."""
    if lang != "rust":
        return "plain"
    text = src[c.start : c.end]
    if (text.startswith("///") and not text.startswith("////")) or (
        text.startswith("/**") and not text.startswith("/***") and text != "/**/"
    ):
        return "outer"
    if text.startswith(("//!", "/*!")):
        return "inner"
    return "plain"


def _group(src: str, lang: str, comments: list[Comment]) -> list[list[Comment]]:
    blocks: list[list[Comment]] = []
    prev: tuple[int, int, bool, str] | None = None  # (end line, column, full-line, doc kind)
    for c in comments:
        full = _is_full_line(src, c)
        col = c.start - _line_start(src, c.start)
        doc = _doc_kind(src, c, lang)
        joins = (
            prev is not None
            and full
            and c.kind == "line"
            and _line_no(src, c.start) == prev[0] + 1
            and (prev[2] or col == prev[1])
            and doc == prev[3]
        )
        if joins:
            blocks[-1].append(c)
        else:
            blocks.append([c])
        prev = (_line_no(src, c.end), col, full, doc)
    return blocks


def comment_blocks(src: str, lang: str) -> list[list[Comment]]:
    """Group a file's comments into blocks (see the module doc)."""
    return _group(src, lang, lex(src, lang).comments)


def block_text(src: str, block: list[Comment]) -> str:
    return "\n".join(src[c.start : c.end] for c in block)


def block_hash(text: str) -> str:
    """The kept list's key for a block."""
    return hashlib.sha256(text.encode("utf-8")).hexdigest()[:16]


def _matching(tokens: list[Token], i: int) -> int:
    """The index of the bracket closing the one at `i`, or the last token."""
    pairs = {"(": ")", "[": "]", "{": "}"}
    stack = []
    for j in range(i, len(tokens)):
        t = tokens[j]
        if t.kind != "punct":
            continue
        if t.text in pairs:
            stack.append(pairs[t.text])
        elif stack and t.text == stack[-1]:
            stack.pop()
            if not stack:
                return j
    return len(tokens) - 1


def clap_spans(lx: Lexer) -> list[tuple[int, int]]:
    """Source spans of Rust items that derive a `clap-derive` class.

    A span starts after the token before the item's attributes, so it holds
    the doc comment written above `#[derive(...)]`.
    """
    names = [k.pattern for k in load_kept_classes() if k.kind == "clap-derive" and lx.lang in k.langs]
    spans: list[tuple[int, int]] = []
    if not names:
        return spans
    toks, n, i = lx.tokens, len(lx.tokens), 0

    def is_attr(k: int) -> bool:
        return k + 1 < n and toks[k].text == "#" and toks[k + 1].text == "["

    while i < n:
        if not is_attr(i):
            i += 1
            continue
        first, derives = i, False
        while is_attr(i):
            close = _matching(toks, i + 1)
            words = [t.text for t in toks[i + 2 : close] if t.kind == "word"]
            if "derive" in words and any(p.search(w) for p in names for w in words):
                derives = True
            i = close + 1
        if not derives:
            continue
        end, angles = n - 1, 0
        while i < n:
            t = toks[i]
            if t.kind == "punct" and t.text == "<":
                angles += 1
            elif t.kind == "punct" and t.text == ">" and angles and toks[i - 1].text != "-":
                angles -= 1
            elif t.kind == "punct" and (t.text in "([" or (t.text == "{" and angles)):
                # A brace inside generics is a const default, not the item's body.
                i = _matching(toks, i)
            elif t.kind == "punct" and t.text == "{":
                end = _matching(toks, i)
                break
            elif t.kind == "punct" and t.text == ";":
                end = i
                break
            i += 1
        spans.append((toks[first - 1].end if first else 0, toks[end].end))
        i = end + 1
    return spans


def macro_doc_spans(lx: Lexer) -> list[tuple[int, int]]:
    """Source spans of macro inputs where a doc comment stays.

    A macro can read a doc comment in its input, so one stays unless a
    `macro-doc` class names the macro as one that only forwards it. The token
    before `!` names the macro. `if !(a)` reads as one too, which only keeps more.
    """
    forwarding = [k.pattern for k in load_kept_classes() if k.kind == "macro-doc" and lx.lang in k.langs]
    spans: list[tuple[int, int]] = []
    if not forwarding:
        return spans
    toks, n = lx.tokens, len(lx.tokens)
    for i in range(1, n - 1):
        if toks[i].text != "!" or toks[i - 1].kind != "word":
            continue
        j = i + 2 if toks[i + 1].kind == "word" else i + 1
        opens = j < n and toks[j].kind == "punct" and toks[j].text in ("(", "[", "{")
        if opens and not any(p.search(toks[i - 1].text) for p in forwarding):
            spans.append((toks[j].start, toks[_matching(toks, j)].end))
    return spans


@dataclass
class Block:
    comments: list[Comment]
    text: str
    line: int  # 1-based line of the first comment
    kept_class: str | None  # the class that keeps the whole block, if any
    directive: list[bool]  # per comment: a tool reads it

    @property
    def hash(self) -> str:
        return block_hash(self.text)


def classify_blocks(src: str, lang: str, lx: Lexer | None = None) -> list[Block]:
    """Every block, with what the kept classes keep of it. The kept list is not applied."""
    lx = lx or lex(src, lang)
    classes = [k for k in load_kept_classes() if lang in k.langs]
    macro_read = {k.kind: k.name for k in classes if k.kind in MACRO_READ_KINDS}
    spans = clap_spans(lx) if "clap-derive" in macro_read else []
    macro_spans = macro_doc_spans(lx) if "macro-doc" in macro_read else []
    inside = lambda c, among: any(a <= c.start < b for a, b in among)  # noqa: E731
    checked = any(k.kind == "typed-jsdoc" and k._matches_collapsed(src[c.start : c.end]) for k in classes for c in lx.comments)
    out = []
    for comments in _group(src, lang, lx.comments):
        texts = [src[c.start : c.end] for c in comments]
        directive, whole = [False] * len(comments), None
        for idx, (c, text) in enumerate(zip(comments, texts)):
            for k in classes:
                if k.kind == "prose" and k.pattern.search(text):
                    whole = whole or k.name
                elif k.reads(text) or (checked and text.startswith("/**")):
                    directive[idx] = True
            doc = _doc_kind(src, c, lang)
            if doc == "outer" and inside(c, spans):
                whole = macro_read["clap-derive"]
            elif doc != "plain" and inside(c, macro_spans):
                whole = macro_read["macro-doc"]
        if any(k.reads_block(text) for k in classes for text in texts):
            directive = [True] * len(comments)
        out.append(Block(comments, "\n".join(texts), _line_no(src, comments[0].start) + 1, whole, directive))
    return out


def selects(mode: str, text: str) -> bool:
    if mode in ("all", "keep-invariants"):
        return True
    if mode == "justifying":
        return bool(JUSTIFYING_RE.search(text))
    raise ValueError(f"unknown mode {mode!r}")


@dataclass
class StripResult:
    out: str
    removed: list[str] = field(default_factory=list)  # the text of every removed block
    kept: list[str] = field(default_factory=list)  # every comment `out` must hold, in order
    refusal: str | None = None


def strip_file(src: str, lang: str, mode: str, kept_hashes: set[str] | frozenset = frozenset()) -> StripResult:
    lx = lex(src, lang)
    res = StripResult(src)
    if "\r" in src:
        res.refusal = "a carriage return: the lexer reads LF line ends only"
        return res
    classes = [k for k in load_kept_classes() if lang in k.langs]
    for c in lx.comments:
        text = src[c.start : c.end]
        unread = next((k for k in classes if k.refuses(text)), None)
        if unread and not any(k.reads(text) for k in classes):
            res.refusal = f"line {_line_no(src, c.start) + 1}: {unread.name}: {unread.why}"
            return res
    cuts: list[tuple[int, int, str]] = []
    for block in classify_blocks(src, lang, lx):
        # A prose class survives only the commissioned strip: no tool reads one.
        whole = block.kept_class is not None and (mode == "keep-invariants" or block.kept_class in macro_read_class_names())
        keep = [True] * len(block.comments) if whole else list(block.directive)
        prefix = marked = None
        if mode == "keep-invariants" and block.kept_class is None and block.hash in kept_hashes:
            # A prefix in front of a directive would hide it from its tool.
            marked = next((c for c, d in zip(block.comments, block.directive) if not d), None)
            keep = [True] * len(keep)
            if marked and EMPTY_COMMENT_RE.fullmatch(src[marked.start : marked.end]):
                res.refusal = f"line {block.line}: the kept list names an empty comment, which cannot take the prefix"
                return res
            prefix = _prefix_cut(src, marked) if marked else None
        going = [c for c, k in zip(block.comments, keep) if not k]
        going_text = block_text(src, going)
        if not going or not selects(mode, going_text):
            keep = [True] * len(keep)
            going = []
        for c, k in zip(block.comments, keep):
            text = src[c.start : c.end]
            if k:
                res.kept.append(prefix[2] + text[prefix[1] - c.start :] if c is marked else text)
        if prefix:
            cuts.append(prefix)
        if not going:
            continue
        res.removed.append(going_text)
        for c in going:
            cut = _cut_for(src, c, lang)
            if cut is None:
                res.refusal = f"line {_line_no(src, c.start) + 1}: a comment with code on both sides and no space"
                return res
            cuts.append(cut)
    out = src
    for start, end, repl in sorted(cuts, reverse=True):
        out = out[:start] + repl + out[end:]
    if out.endswith("\n\n") and not src.endswith("\n\n"):
        out = out.rstrip("\n") + "\n"
    res.out = out
    return res


def strip(src: str, lang: str, mode: str) -> tuple[str, list[str]]:
    """Return the stripped source and the text of every removed block."""
    res = strip_file(src, lang, mode)
    return res.out, ([] if res.refusal else res.removed)


def proof_one(src: str, res: StripResult, lang: str) -> str | None:
    """Why `res.out` is not `src` minus comments, or None when it is."""
    if res.refusal:
        return res.refusal
    before, after = lex(src, lang), lex(res.out, lang)
    a, b = token_signature(before), token_signature(after)
    if a != b:
        at = next((i for i, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))
        tok = after.tokens[at] if at < len(after.tokens) else None
        where = f"line {_line_no(res.out, tok.start) + 1}" if tok else "end of file"
        if at < len(a) and at < len(b) and a[at][:2] == b[at][:2]:
            return f"{where}: the separation before `{b[at][1][:40]}` changed"
        return f"{where}: a code token changed"
    if tool_read_comments(src, lang, before) != tool_read_comments(res.out, lang, after):
        return "a comment a tool reads moved"
    survivors = [res.out[c.start : c.end] for c in after.comments]
    if survivors != res.kept:
        return "the surviving comments are not the kept ones"
    return None


@functools.cache
def macro_read_class_names() -> frozenset[str]:
    return frozenset(k.name for k in load_kept_classes() if k.kind in MACRO_READ_KINDS)


def _prefix_cut(src: str, c: Comment) -> tuple[int, int, str]:
    opener = COMMENT_OPENER_RE.match(src, c.start).group()
    rest = src[c.start + len(opener) : c.end]
    sep = "" if rest[:1] in ("", " ", "\n") else " "
    return c.start, c.start + len(opener), f"{opener} {INVARIANT_PREFIX}{sep}"


def _cut_for(src: str, c: Comment, lang: str) -> tuple[int, int, str] | None:
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
    # An inline block comment with code after it on its last line.
    left = src[c.start - 1] if c.start > ls else " "
    right = src[c.end]
    touching = not left.isspace() and not right.isspace()
    if lang == "css":
        # CSS reads whitespace as a combinator, and a comment is not whitespace.
        # Neither a space nor nothing is right between two touching tokens.
        return None if touching else (c.start, c.end, "")
    if "\n" in src[c.start : c.end] and src[ls : c.start].strip():
        # The comment held the only line terminator between two tokens.
        start, end = c.start, c.end
        while src[start - 1] in " \t":
            start -= 1
        while src[end] in " \t":
            end += 1
        indent = src[ls : ls + len(src[ls:start]) - len(src[ls:start].lstrip())]
        return start, end, "\n" + indent
    return c.start, c.end, " " if touching else ""


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--mode", choices=MODES, required=True)
    ap.add_argument("--check", action="store_true", help="report blocks, write nothing")
    ap.add_argument("--json", action="store_true", help="print a JSON summary")
    ap.add_argument("--kept-list", help="the kept list, for --mode keep-invariants")
    ap.add_argument("files", nargs="+")
    args = ap.parse_args(argv)
    kept_list = load_kept_list(args.kept_list) if args.kept_list else frozenset()

    summary: dict[str, list[str]] = {}
    for path in args.files:
        lang = language_of(path)
        if lang is None:
            continue
        src = read_source(path)
        res = strip_file(src, lang, args.mode, kept_list)
        if res.out == src and not res.refusal:
            continue
        why = proof_one(src, res, lang)
        if why:
            print(f"REFUSED {path}: {why}", file=sys.stderr)
            return 1
        summary[path] = res.removed
        if not args.check:
            Path(path).write_bytes(res.out.encode("utf-8"))

    if args.json:
        json.dump(summary, sys.stdout, indent=1)
        print()
    else:
        blocks = sum(len(v) for v in summary.values())
        print(f"{blocks} blocks in {len(summary)} files")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
