#!/usr/bin/env python3
"""Classify comment blocks for the kept list: strip, keep, or ADR candidate.

    classify.py select --out pop.json            the blocks a model must read
    classify.py run --pop pop.json --model M --out answers.jsonl
    classify.py kept-list --answers run1.jsonl run2.jsonl --out kept-list.tsv
    classify.py audit --answers run1.jsonl run2.jsonl --pop pop.json --out audit.md

`select` leaves out what needs no judgment: a block a kept class keeps, a
block of tool directives only, a block that cites an ADR, and test code.
Test code is a test path, a `#![cfg(test)]` file or an inline Rust test module. `run` resumes: a block with an answer
in `--out` is not asked again. Each call goes through the Claude CLI with no
tools, and its reported cost is summed.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile
import threading
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import strip_comments as sc  # noqa: E402
import strip_tree  # noqa: E402

VERDICTS = ("strip", "keep", "adr")
SCORES = (1, 2, 3, 4, 5)
TEST_PATH_RE = re.compile(
    r"(^|/)(tests?|__tests__|e2e|fixtures|contract-tests|benches|[a-z0-9_]+_tests?)/"
    r"|\.test\.|\.spec\.|_tests?\.rs$|(^|/)tests?\.rs$|^crates/lucidos-e2e/"
)
ADR_CITATION_RE = re.compile(r"\bADR[ -]?\d{3,4}\b")
TEXT_LIMIT = 1600
AFTER_LINES = 6
AFTER_LIMIT = 400

PROMPT = """You label source-code comments in a large Rust and TypeScript codebase.
Almost every comment will be deleted. Decide which few must stay.

For each block answer one verdict:

keep  - The comment states something a future editor MUST NOT BREAK here.
        Typical: an ordering requirement, a lock or concurrency hazard, a
        security reason, a platform or browser quirk behind odd code, a
        coupling to a distant site that must stay in step, a deliberate
        non-obvious choice whose "obvious fix" would reintroduce a bug.
adr   - The comment records a design decision or a rejected alternative at the
        level of a module or subsystem ("why this is one unit", "why not X"),
        not a constraint on these lines.
strip - Everything else.

A reason is not a constraint. Most comments that give a reason are still
strip. Answer keep only when all three hold:

1. It names a specific hazard or requirement, not a description.
2. An editor changing these lines could violate it and nothing would tell
   them: no compiler or type error, no visibly broken feature.
3. The code below does not already show it through a name, a type, an
   assertion or a helper that exists for it.

These are strip, even when they sound important:

- what the code does, case by case, or what a name, field or parameter means;
- a function's contract: what it accepts, returns, or does on failure;
- why a call or check is here, when removing it would plainly break the feature;
- how two values relate when one is computed from the other;
- where something else lives, what mirrors what, or which rule or doc says so;
- history, section banners, and narration of the next lines.

Also score each block from 1 to 5: how likely is a careful editor to introduce
a bug here once the comment is gone?

1  no risk: the comment describes or labels
2  low: a reason the code already makes plain
3  some: a real reason, but breaking it would be noticed quickly
4  high: a specific hazard that nothing else flags
5  certain: the comment is the only guard against a known bug

About one block in four deserves keep, at score 4 or 5.

Reply with one JSON object per line and nothing else, one per block, in order:
{"id": <id>, "s": <1-5>, "v": "strip" | "keep" | "adr", "why": "<at most 14 plain words>"}

The blocks follow as JSON. "text" is the comment, "after" is the code below it.
"""


def _test_only(pred: list[str]) -> bool:
    """Whether a cfg predicate holds only in a test build: `test`, or an `all(...)` naming it.

    `any`, `not` or another shape can hold outside tests, so it is production.
    """
    return pred == ["test"] or (pred[:1] == ["all"] and "test" in pred and not {"any", "not"} & set(pred))


def rust_test_spans(src: str) -> list[tuple[int, int]]:
    """Offset ranges of the inline Rust test code in `src`.

    A top-level `#![cfg(test)]` makes the whole file one range. Otherwise each
    inline module under a test-only cfg is one, from its attribute to its
    closing brace. A `mod name;` declaration is production: its code lives in
    another file.
    """
    toks = sc.lex(src, "rust").tokens
    texts = [t.text for t in toks]
    spans = []
    depth = 0
    for i in range(len(toks) - 4):
        depth += {"{": 1, "}": -1}.get(texts[i], 0) if toks[i].kind == "punct" else 0
        inner = texts[i + 1] == "!"
        at = i + 2 if inner else i + 1
        if texts[i] != "#" or texts[at : at + 3] != ["[", "cfg", "("]:
            continue
        if not _test_only(texts[at + 3 : sc._matching(toks, at + 2)]):
            continue
        if inner:
            if depth == 0:
                return [(0, len(src))]
            continue
        j = sc._matching(toks, at) + 1
        while texts[j : j + 2] == ["#", "["]:
            j = sc._matching(toks, j + 1) + 1
        if texts[j : j + 1] == ["pub"]:
            j += 1
            if texts[j : j + 1] == ["("]:
                j = sc._matching(toks, j) + 1
        if texts[j : j + 1] == ["mod"] and texts[j + 2 : j + 3] == ["{"]:
            spans.append((toks[i].start, toks[sc._matching(toks, j + 2)].end))
    return spans


def is_test_code(root: Path, rel: str, offset: int, spans: dict[str, list[tuple[int, int]]]) -> bool:
    """Whether the comment starting at `offset` in `rel` is test code."""
    if TEST_PATH_RE.search(rel):
        return True
    if not rel.endswith(sc.RUST_SUFFIXES):
        return False
    if rel not in spans:
        spans[rel] = rust_test_spans(sc.read_source(root / rel))
    return any(first <= offset < last for first, last in spans[rel])


def after_context(src: str, block: sc.Block, comments: list[sc.Comment]) -> str:
    """The code below `block`, stopping before the next comment.

    The next comment belongs to other code, and the model must not judge
    `block` by it.
    """
    end = block.comments[-1].end
    stop = next((c.start for c in comments if c.start >= end), len(src))
    code = src[end:stop].lstrip("\n").split("\n")[:AFTER_LINES]
    return "\n".join(code).rstrip()[:AFTER_LIMIT]


def cmd_select(args) -> int:
    root = Path(args.root).resolve()
    spans: dict[str, list[tuple[int, int]]] = {}
    seen: set[tuple[str, str]] = set()
    out = []
    for rel in strip_tree.tracked_files(root, args.paths):
        lang = sc.language_of(rel)
        src = sc.read_source(root / rel)
        lx = sc.lex(src, lang)
        for block in sc.classify_blocks(src, lang, lx):
            if block.kept_class or all(block.directive) or ADR_CITATION_RE.search(block.text):
                continue
            if (rel, block.hash) in seen or is_test_code(root, rel, block.comments[0].start, spans):
                continue
            seen.add((rel, block.hash))
            after = after_context(src, block, lx.comments)
            out.append({"path": rel, "line": block.line, "hash": block.hash, "text": block.text, "after": after})
    Path(args.out).write_text(json.dumps(out), encoding="utf-8")
    print(f"{len(out)} blocks, {sum(len(b['text']) for b in out)} comment characters")
    return 0


def ask(model: str, batch: list[dict], cwd: str) -> tuple[dict[int, dict], float]:
    """One model call. Returns the answers by id and the call's cost in USD."""
    items = [{"id": i, "path": b["path"], "text": b["text"][:TEXT_LIMIT], "after": b["after"]} for i, b in enumerate(batch)]
    argv = [shutil.which("claude") or "claude", "-p", "--model", model, "--output-format", "json",
            "--no-session-persistence", "--setting-sources", "", "--tools", ""]
    p = subprocess.run(argv, cwd=cwd, input=PROMPT + json.dumps(items, ensure_ascii=False),
                       text=True, capture_output=True, timeout=600)
    outer = json.loads(p.stdout)
    answers: dict[int, dict] = {}
    for line in (outer.get("result") or "").splitlines():
        m = re.search(r"\{.*\}", line)
        if not m:
            continue
        try:
            row = json.loads(m.group(0))
        except json.JSONDecodeError:
            continue
        if isinstance(row.get("id"), int) and row.get("v") in VERDICTS and row.get("s") in SCORES:
            answers[row["id"]] = row
    return answers, float(outer.get("total_cost_usd") or 0.0)


def cmd_run(args) -> int:
    pop = json.loads(Path(args.pop).read_text(encoding="utf-8"))
    out_path = Path(args.out)
    done = set()
    if out_path.exists():
        # A run killed mid-write leaves a cut line. Replace the file without it, so new rows start on
        # their own line. The replace is atomic, so a kill here loses no paid answer.
        text = out_path.read_bytes().decode("utf-8", errors="replace")
        rows = []
        for line in text.splitlines():
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            rows.append(line)
            done.add((row["path"], row["hash"]))
        kept = "".join(r + "\n" for r in rows)
        if kept != text:
            tmp = out_path.with_name(out_path.name + ".tmp")
            tmp.write_text(kept, encoding="utf-8")
            os.replace(tmp, out_path)
    todo = [b for b in pop if (b["path"], b["hash"]) not in done]
    if args.limit:
        todo = todo[: args.limit]
    batches = [todo[i : i + args.batch] for i in range(0, len(todo), args.batch)]
    lock = threading.Lock()
    cost = [0.0, 0, 0]
    cwd = tempfile.mkdtemp(prefix="comment-classify-")

    def work(batch: list[dict]) -> None:
        answers: dict[int, dict] = {}
        spent = 0.0
        pending = list(range(len(batch)))
        for _ in range(3):
            if not pending:
                break
            try:
                got, c = ask(args.model, [batch[i] for i in pending], cwd)
            except (subprocess.TimeoutExpired, json.JSONDecodeError, OSError):
                continue
            spent += c
            for local, row in got.items():
                if 0 <= local < len(pending):
                    answers[pending[local]] = row
            pending = [i for i in pending if i not in answers]
        with lock, out_path.open("a", encoding="utf-8") as f:
            for i, row in sorted(answers.items()):
                b = batch[i]
                why = " ".join(str(row.get("why", "")).split())
                f.write(json.dumps({"path": b["path"], "line": b["line"], "hash": b["hash"],
                                    "verdict": row["v"], "score": row["s"], "why": why}, ensure_ascii=False) + "\n")
            cost[0] += spent
            cost[1] += len(answers)
            cost[2] += len(pending)
            if cost[1] % (args.batch * 20) < args.batch:
                print(f"answered {cost[1]} unanswered {cost[2]} cost ${cost[0]:.2f}", flush=True)

    with concurrent.futures.ThreadPoolExecutor(args.jobs) as pool:
        list(pool.map(work, batches))
    print(f"answered {cost[1]}, unanswered {cost[2]}, cost ${cost[0]:.4f}")
    return 0 if cost[2] == 0 else 1


def merged_answers(paths: list[str]) -> dict[tuple[str, str], dict]:
    """One answer per block across runs. A block is kept when any run kept it:
    one run misses too many. `keep` outranks `adr`, and `adr` outranks `strip`."""
    rank = {"strip": 0, "adr": 1, "keep": 2}
    rows: dict[tuple[str, str], dict] = {}
    for answers in paths:
        for line in Path(answers).read_text(encoding="utf-8").splitlines():
            row = json.loads(line)
            key = (row["path"], row["hash"])
            if key not in rows or rank[row["verdict"]] > rank[rows[key]["verdict"]]:
                rows[key] = row
    return rows


AUDIT_LABELS = {"strip": "STRIP", "keep": "KEEP", "adr": "ADR candidate"}
FENCE = "`" * 3


def cmd_audit(args) -> int:
    """A random sample of answers as markdown, for a reader on a phone."""
    rows = merged_answers(args.answers)
    pop = {(b["path"], b["hash"]): b for b in json.loads(Path(args.pop).read_text(encoding="utf-8"))}
    unanswered = len(pop.keys() - rows.keys())
    if unanswered:
        print(f"{unanswered} blocks have no answer: finish `run` before the audit", file=sys.stderr)
        return 1
    keys = sorted(pop)
    picked = random.Random(args.seed).sample(keys, min(args.count, len(keys)))
    out = [f"# Comment strip: {len(picked)} classifier answers to audit\n",
           "Each entry is one comment, the verdict, and one line of why.",
           "KEEP stays in the code. STRIP and ADR candidates are deleted; an ADR may record a candidate first.\n"]
    for n, key in enumerate(picked, 1):
        row, block = rows[key], pop[key]
        text = block["text"] if len(block["text"]) <= args.chars else block["text"][: args.chars] + " …"
        out += [f"## {n}. {AUDIT_LABELS[row['verdict']]}\n", f"`{row['path']}:{block['line']}`\n",
                FENCE, text.replace(FENCE, "'" * 3), FENCE + "\n", f"Why: {row['why']}\n"]
    Path(args.out).write_text("\n".join(out), encoding="utf-8")
    counts = {v: sum(rows[k]["verdict"] == v for k in picked) for v in VERDICTS}
    print(f"{len(picked)} answers: {counts}")
    return 0


def cmd_kept_list(args) -> int:
    rows = {k: r for k, r in merged_answers(args.answers).items() if r["verdict"] != "strip"}
    head = ("# The kept list: the comment blocks a classifier judged worth keeping.\n"
            "# Columns, tab-separated: path when classified, block hash, verdict, why.\n"
            "# The block hash is the key. The path is a hint for a reader, so a moved file keeps its rows.\n"
            "# Generated by scripts/comment-strip/classify.py. A block the list does not name is stripped.\n")
    body = "".join(f"{r['path']}\t{r['hash']}\t{r['verdict']}\t{r['why'].replace(chr(9), ' ')}\n"
                   for _, r in sorted(rows.items()))
    Path(args.out).write_text(head + body, encoding="utf-8")
    print(f"{len(rows)} rows")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    select = sub.add_parser("select", help="write the blocks a model must read")
    select.add_argument("--root", default=".")
    select.add_argument("--out", required=True)
    select.add_argument("paths", nargs="*", metavar="PATH")
    run = sub.add_parser("run", help="ask the model, resuming from --out")
    run.add_argument("--pop", required=True)
    run.add_argument("--model", required=True)
    run.add_argument("--out", required=True)
    run.add_argument("--batch", type=int, default=25)
    run.add_argument("--jobs", type=int, default=6)
    run.add_argument("--limit", type=int, default=0)
    kept = sub.add_parser("kept-list", help="write the kept list from the answers")
    kept.add_argument("--answers", required=True, nargs="+", help="one answers file per run")
    kept.add_argument("--out", required=True)
    audit = sub.add_parser("audit", help="write a random sample of answers as markdown")
    audit.add_argument("--answers", required=True, nargs="+")
    audit.add_argument("--pop", required=True)
    audit.add_argument("--out", required=True)
    audit.add_argument("--count", type=int, default=100)
    audit.add_argument("--seed", type=int, default=1)
    audit.add_argument("--chars", type=int, default=600)
    args = ap.parse_args()
    return {"select": cmd_select, "run": cmd_run, "kept-list": cmd_kept_list, "audit": cmd_audit}[args.cmd](args)


if __name__ == "__main__":
    sys.exit(main())
