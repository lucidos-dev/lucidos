#!/usr/bin/env python3
"""Strip comments from tracked Rust, TypeScript, JavaScript and CSS files, with both proofs.

  strip_tree.py run [--write] [PATH...]     strip, prove, and report
  strip_tree.py verify --base REV [PATH...] prove the tree against a revision
  strip_tree.py blocks [PATH...]            print every comment block as JSON lines
  strip_tree.py count [PATH...]             count the comments each kept class keeps
  strip_tree.py regrowth --base REV         fail on a comment added since a revision

`run` changes no tracked file unless `--write` is given. It strips into a
scratch copy and proves each file twice there: once with the stripper's own
lexer, once with an independent parser. `--write` then copies back only the
files that passed both. A file that fails either proof stays as it was, and
the exit status is 1.

The independent parsers are the TypeScript compiler, `postcss`, and for Rust
the `comment_strip_verify` example of `lucidos-wire-codegen`.

`regrowth` reads the files changed since REV in a stripped language, untracked
ones included. It fails if the strip would remove a comment from one.
`STRIPPED_LANGUAGES` names the languages already stripped, and `run
--stripped-only` limits a run to them.

PATH narrows the run to tracked files under it. Vendored and generated
sources are never touched.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import strip_comments as sc  # noqa: E402

# Third-party source, and build outputs that codegen rewrites.
EXCLUDED_PREFIXES = ("crates/vendor/",)
EXCLUDED_SEGMENTS = ("/generated/",)
STRIPPED_LANGUAGES = ("ts", "css")
RUST_VERIFIER = ("cargo", "run", "--locked", "--quiet", "-p", "lucidos-wire-codegen", "--example", "comment_strip_verify", "--")


def git(root: Path, *args: str) -> str:
    return subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True, text=True).stdout


def in_scope(rel: str) -> bool:
    return (
        sc.language_of(rel) is not None
        and not rel.startswith(EXCLUDED_PREFIXES)
        and not any(seg in f"/{rel}" for seg in EXCLUDED_SEGMENTS)
    )


def tracked_files(root: Path, paths: list[str]) -> list[str]:
    listed = git(root, "ls-files", "-z", "--", *paths).split("\0")
    return sorted(rel for rel in listed if rel and in_scope(rel) and (root / rel).is_file())


def by_language(files: list[str]) -> dict[str, list[str]]:
    groups: dict[str, list[str]] = {}
    for rel in files:
        groups.setdefault(sc.language_of(rel), []).append(rel)
    return groups


def run_tool(cmd: list[str], files: list[str], cwd: Path) -> tuple[dict[str, str], str]:
    """Run one path-list tool. Returns `{path: reason}` for each file it rejects, and its output."""
    if not files:
        return {}, ""
    proc = subprocess.run(cmd, input="\n".join(files) + "\n", capture_output=True, text=True, cwd=cwd)
    failures = {}
    for line in proc.stdout.splitlines():
        if line.startswith("DIFF\t"):
            _, rel, why = line.split("\t", 2)
            failures[rel] = why
    if proc.returncode not in (0, 1) or (proc.returncode == 1 and not failures):
        # A verifier that could not run has proven nothing.
        sys.exit(f"tool failed ({proc.returncode}): {' '.join(cmd)}\n{proc.stderr}")
    return failures, proc.stdout


def run_verifier(cmd: list[str], files: list[str], cwd: Path) -> dict[str, str]:
    return run_tool(cmd, files, cwd)[0]


def proof_two(root: Path, orig_root: Path, new_root: Path, files: list[str], rust_verifier: list[str]) -> dict[str, str]:
    groups = by_language(files)
    roots = [str(orig_root), str(new_root)]
    rust = [*rust_verifier, "--kept-classes", str(sc.KEPT_CLASSES_PATH), *roots]
    failures = run_verifier(["node", str(HERE / "ts_proof.mjs"), "verify", *roots], groups.get("ts", []), root)
    failures |= run_verifier(["node", str(HERE / "css_proof.mjs"), "verify", *roots], groups.get("css", []), root)
    failures |= run_verifier(rust, groups.get("rust", []), root)
    return failures


def without_empty_braces(before: list, after: list) -> bool:
    """Whether `after` is `before` with some touching `{` `}` pairs removed."""
    pair = [("punct", "{"), ("punct", "}")]
    i = j = 0
    while i < len(before):
        if before[i : i + 2] == pair and after[j : j + 2] != pair:
            i += 2
        elif j < len(after) and before[i] == after[j]:
            i += 1
            j += 1
        else:
            return False
    return j == len(after)


def remove_emptied_jsx(root: Path, scratch: Path, files: list[str]) -> tuple[int, int, dict[str, str]]:
    """Run the JSX pass over the scratch copy. Returns wrappers removed, wrappers left as `{}`, failures."""
    tsx = [rel for rel in files if rel.endswith(sc.JSX_SUFFIXES)]
    before = {rel: (scratch / rel).read_text(encoding="utf-8") for rel in tsx}
    failures, output = run_tool(["node", str(HERE / "ts_proof.mjs"), "jsx", str(root), str(scratch)], tsx, root)
    removed, left = next(
        ((int(a), int(b)) for tag, a, b in (line.split("\t") for line in output.splitlines() if line.startswith("JSX\t"))),
        (0, 0),
    )
    for rel in tsx:
        after = (scratch / rel).read_text(encoding="utf-8")
        # The lexer's own check on the pass: it may only drop touching braces.
        if after != before[rel] and not without_empty_braces(
            sc.tokens_without_comments(before[rel], "ts"), sc.tokens_without_comments(after, "ts")
        ):
            failures[rel] = "the JSX pass changed more than empty braces"
    return removed, left, failures


def cmd_run(args) -> int:
    root = Path(args.root).resolve()
    kept_list = sc.load_kept_list(args.kept_list) if args.kept_list else frozenset()
    files = tracked_files(root, args.paths)
    if args.stripped_only:
        files = [rel for rel in files if sc.language_of(rel) in STRIPPED_LANGUAGES]
    scratch = Path(args.out).resolve() if args.out else Path(tempfile.mkdtemp(prefix="comment-strip."))
    if args.out and scratch.exists() and (not scratch.is_dir() or any(scratch.iterdir())):
        # A file already there could be the checkout's own, or a link into it.
        sys.exit("--out must be a new or empty directory")
    failures: dict[str, str] = {}
    changed: list[str] = []
    stats: Counter = Counter()
    kept: Counter = Counter()
    try:
        for rel in files:
            lang = sc.language_of(rel)
            src = sc.read_source(root / rel)
            res = sc.strip_file(src, lang, args.mode, kept_list)
            for block in sc.classify_blocks(src, lang):
                if block.kept_class:
                    kept[block.kept_class] += 1
            why = sc.proof_one(src, res, lang)
            if why:
                failures[rel] = f"proof one: {why}"
                continue
            if res.out == src:
                continue
            changed.append(rel)
            stats[f"{lang} files"] += 1
            stats[f"{lang} blocks"] += len(res.removed)
            stats[f"{lang} lines"] += src.count("\n") - res.out.count("\n")
            target = scratch / rel
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(res.out.encode("utf-8"))

        jsx_removed, jsx_left, jsx_failures = remove_emptied_jsx(root, scratch, changed)
        failures |= {rel: f"jsx pass: {why}" for rel, why in jsx_failures.items()}
        rust_verifier = [args.rust_verifier] if args.rust_verifier else list(RUST_VERIFIER)
        proven = [rel for rel in changed if rel not in failures]
        failures |= {rel: f"proof two: {why}" for rel, why in proof_two(root, root, scratch, proven, rust_verifier).items()}

        passed = [rel for rel in changed if rel not in failures]
        if args.write:
            for rel in passed:
                shutil.copyfile(scratch / rel, root / rel)
    finally:
        if not args.out:
            shutil.rmtree(scratch, ignore_errors=True)

    for lang in ("rust", "ts", "css"):
        print(f"{lang}: {stats[f'{lang} blocks']} blocks, {stats[f'{lang} lines']} lines, in {stats[f'{lang} files']} files")
    print(f"jsx wrappers: {jsx_removed} removed, {jsx_left} left as an empty expression")
    for name, n in sorted(kept.items()):
        print(f"kept {name}: {n} blocks")
    for rel, why in sorted(failures.items()):
        print(f"REFUSED {rel}: {why}")
    verb = "wrote" if args.write else "would write"
    print(f"{verb} {len(passed)} of {len(files)} files, refused {len(failures)}")
    return 1 if failures else 0


def cmd_verify(args) -> int:
    root = Path(args.root).resolve()
    def changed(*flags: str) -> list[str]:
        names = git(root, "diff", "--name-only", "--no-renames", "-z", *flags, args.base, "--", *args.paths).split("\0")
        return sorted(rel for rel in names if rel and in_scope(rel))

    files = changed()
    # A strip deletes no file, so a deletion is a failure, never a skip.
    failures: dict[str, str] = {rel: "deleted" for rel in changed("--diff-filter=D")}
    failures |= {rel: "deleted" for rel in files if not (root / rel).is_file()}
    with tempfile.TemporaryDirectory(prefix="comment-strip.") as tmp:
        orig = Path(tmp)
        present = []
        for rel in files:
            if rel in failures:
                continue
            shown = subprocess.run(["git", "-C", str(root), "show", f"{args.base}:{rel}"], capture_output=True)
            if shown.returncode != 0:
                failures[rel] = f"not in {args.base}"
                continue
            (orig / rel).parent.mkdir(parents=True, exist_ok=True)
            (orig / rel).write_bytes(shown.stdout)
            present.append(rel)
        rust_verifier = [args.rust_verifier] if args.rust_verifier else list(RUST_VERIFIER)
        failures |= proof_two(root, orig, root, present, rust_verifier)
        for rel in present:
            lang = sc.language_of(rel)
            if rel in failures:
                continue
            before, after = (orig / rel).read_text(encoding="utf-8"), (root / rel).read_text(encoding="utf-8")
            if not without_empty_braces(sc.tokens_without_comments(before, lang), sc.tokens_without_comments(after, lang)):
                failures[rel] = "proof one: a code token changed"
            elif sc.tool_read_comments(before, lang) != sc.tool_read_comments(after, lang):
                failures[rel] = "a comment a tool reads changed"
    for rel, why in sorted(failures.items()):
        print(f"DIFF {rel}: {why}")
    print(f"{len(files) - len(failures)} of {len(files)} changed files are comment-only against {args.base}")
    return 1 if failures else 0


def cmd_regrowth(args) -> int:
    root = Path(args.root).resolve()
    changed = git(root, "diff", "--name-only", "--no-renames", "--diff-filter=d", "-z", args.base, "--", *args.paths)
    untracked = git(root, "ls-files", "--others", "--exclude-standard", "-z", "--", *args.paths)
    names = set(changed.split("\0") + untracked.split("\0"))
    files = sorted(rel for rel in names if rel and in_scope(rel) and sc.language_of(rel) in STRIPPED_LANGUAGES)
    hits = 0
    for rel in files:
        lang = sc.language_of(rel)
        src = sc.read_source(root / rel)
        res = sc.strip_file(src, lang, "keep-invariants", frozenset())
        if res.refusal:
            print(f"REFUSED {rel}: {res.refusal}")
            hits += 1
        for text in res.removed:
            print(f"COMMENT {rel}: {text.strip().splitlines()[0][:100]}")
            hits += 1
    print(f"{len(files)} changed files checked against {args.base}, {hits} comments to remove")
    return 1 if hits else 0


def cmd_blocks(args) -> int:
    root = Path(args.root).resolve()
    for rel in tracked_files(root, args.paths):
        src = sc.read_source(root / rel)
        for block in sc.classify_blocks(src, sc.language_of(rel)):
            row = {
                "path": rel,
                "line": block.line,
                "hash": block.hash,
                "kept_class": block.kept_class,
                "directive": any(block.directive),
                "text": block.text,
            }
            print(json.dumps(row, ensure_ascii=False))
    return 0


def cmd_count(args) -> int:
    root = Path(args.root).resolve()
    counts: Counter = Counter()
    for rel in tracked_files(root, args.paths):
        lang = sc.language_of(rel)
        src = sc.read_source(root / rel)
        classes = [k for k in sc.load_kept_classes() if lang in k.langs and k.kind != "clap-derive"]
        for block in sc.classify_blocks(src, lang):
            if block.kept_class in sc.macro_read_class_names():
                counts[block.kept_class] += len(block.comments)
            for c, read in zip(block.comments, block.directive):
                text = src[c.start : c.end]
                names = [k.name for k in classes if k.reads(text) or k.reads_block(text) or (k.kind == "prose" and k.pattern.search(text))]
                in_tool_block = any(k.reads_block(src[b.start : b.end]) for k in classes for b in block.comments)
                if read and not in_tool_block and not any(k.reads(text) for k in classes):
                    names += [k.name for k in classes if k.kind == "typed-jsdoc"]
                counts.update(names)
    for k in sc.load_kept_classes():
        print(f"{k.name}\t{counts[k.name]}")
    return 0


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--root", default=str(HERE.parent.parent), help="the checkout to read (default: this one)")
    sub = ap.add_subparsers(dest="command", required=True)

    run = sub.add_parser("run", help="strip, prove, and report")
    run.add_argument("--mode", choices=sc.MODES, default="keep-invariants")
    run.add_argument("--kept-list", help="blocks to keep as invariants, by block hash")
    run.add_argument("--write", action="store_true", help="rewrite the files that passed both proofs")
    run.add_argument("--out", help="keep the stripped copies in this directory")
    run.add_argument("--stripped-only", action="store_true", help="skip files outside STRIPPED_LANGUAGES")
    run.set_defaults(func=cmd_run)

    verify = sub.add_parser("verify", help="prove the tree is a revision minus comments")
    verify.add_argument("--base", required=True, help="the revision to compare against")
    verify.set_defaults(func=cmd_verify)

    blocks = sub.add_parser("blocks", help="print every comment block as JSON lines")
    blocks.set_defaults(func=cmd_blocks)
    count = sub.add_parser("count", help="count the comments each kept class keeps")
    count.set_defaults(func=cmd_count)

    regrowth = sub.add_parser("regrowth", help="fail on a comment added since a revision")
    regrowth.add_argument("--base", required=True, help="the revision to compare against")
    regrowth.set_defaults(func=cmd_regrowth)

    for p in (run, verify):
        p.add_argument("--rust-verifier", help="a built comment_strip_verify binary (default: cargo run)")
    for p in (run, verify, blocks, count, regrowth):
        p.add_argument("paths", nargs="*", metavar="PATH")
    args = ap.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
