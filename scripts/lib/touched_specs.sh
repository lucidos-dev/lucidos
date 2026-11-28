#!/usr/bin/env bash
#
# touched_specs.sh: the browser specs a branch's diff can break, for the
# `/harden` e2e-touched suite (docs/adr/0502). Sourced by harden_suites.sh and
# tested by touched_specs_test.sh. Defines functions only.
#
# A spec is touched when any holds:
#   - the branch changes the spec file itself;
#   - the coverage map records a file the branch changes in the spec's run,
#     unless nearly every spec runs that file (app startup);
#   - a removed line of crates/lucidos-app/src held a string literal that no
#     longer appears anywhere under that directory, and the spec names it,
#     directly or through an e2e helper export that does.
#
# A literal with spaces also counts word by word, since a class list loses one
# name at a time. A literal that still appears in src/ is never vanished, so a
# rename that keeps a longer name sharing the old one as a prefix goes
# unseen: the selector is conservative about what it selects, not about what it
# misses. The main check stays the backstop.

TS_APP_DIR="crates/lucidos-app"

# ts_coverage_map_path <root>: where the main check keeps the coverage map. In
# the git common dir, so every worktree reads the one map.
ts_coverage_map_path() {
    local common
    common="$(git -C "$1" rev-parse --path-format=absolute --git-common-dir 2>/dev/null)" || return 1
    echo "$common/lucidos/e2e-coverage-map.json"
}

# ts_touched_specs <root> <base> <paths-file> [<coverage-map>]: spec file names,
# sorted, one per line. <paths-file> lists the branch's changed paths, untracked
# ones included. With a map, a spec whose recorded files meet the diff is added
# too. A map that is missing or unreadable adds nothing, and says so on stderr.
ts_touched_specs() {
    python3 -I - "$1" "$2" "$3" "$TS_APP_DIR" "${4:-}" <<'PY'
import json, os, re, subprocess, sys
from collections import Counter

root, base, paths_file, app, coverage_map = sys.argv[1:6]
e2e = f"{app}/e2e"
src = f"{app}/src"


def git(*args):
    return subprocess.run(
        ["git", "-C", root, *args], capture_output=True, text=True, check=True
    ).stdout


if not os.path.isdir(os.path.join(root, e2e)):
    sys.exit(0)


def read(path):
    try:
        with open(os.path.join(root, path), encoding="utf-8", errors="replace") as f:
            return f.read()
    except OSError:
        return ""


specs = {}
for name in sorted(os.listdir(os.path.join(root, e2e))):
    if name.endswith(".spec.ts"):
        specs[name] = read(f"{e2e}/{name}")

touched = set()
changed = {line.strip() for line in open(paths_file, encoding="utf-8") if line.strip()}
for path in sorted(changed):
    if path.startswith(f"{e2e}/") and path.endswith(".spec.ts") and "/" not in path[len(e2e) + 1:]:
        if path[len(e2e) + 1:] in specs:
            touched.add(path[len(e2e) + 1:])

LITERAL = re.compile(r"""'([^'\\\n]+)'|"([^"\\\n]+)"|`([^`\\$\n]+)`""")
WORD = re.compile(r"[A-Za-z0-9_-]{3,}")
MIN_LEN = 3


def removed_literals():
    out = set()
    diff = git("diff", "-U0", "--no-color", "--no-renames", base, "--", src)
    for line in diff.splitlines():
        if not line.startswith("-") or line.startswith("---"):
            continue
        for match in LITERAL.finditer(line[1:]):
            lit = next(g for g in match.groups() if g is not None).strip()
            if len(lit) < MIN_LEN or lit.startswith((".", "@", "/")) and "/" in lit:
                continue
            out.add(lit)
            if " " in lit:
                out.update(WORD.findall(lit))
    return out


def source_text():
    chunks = []
    for here, _dirs, files in os.walk(os.path.join(root, src)):
        for name in files:
            chunks.append(read(os.path.relpath(os.path.join(here, name), root)))
    return "\n".join(chunks)


def vanished(literals):
    if not literals:
        return set()
    text = source_text()
    return {lit for lit in literals if lit not in text}


EXPORT = re.compile(r"^export\s+(?:async\s+)?(?:function|const|let|class)\s+(\w+)")


def helper_blocks():
    """Map each e2e helper file to its exports: name -> the text that export owns."""
    blocks = {}
    for name in sorted(os.listdir(os.path.join(root, e2e))):
        path = f"{e2e}/{name}"
        if not name.endswith(".ts") or name.endswith((".spec.ts", ".test.ts")):
            continue
        current, exports = None, {}
        for line in read(path).splitlines():
            m = EXPORT.match(line)
            if m:
                current = m.group(1)
                exports[current] = []
            if current:
                exports[current].append(line)
        blocks[path] = {k: "\n".join(v) for k, v in exports.items()}
    return blocks


def tainted_names(gone, blocks):
    names = set()
    for exports in blocks.values():
        for name, body in exports.items():
            if any(lit in body for lit in gone):
                names.add(name)
    # An export that calls a tainted one is tainted too.
    while True:
        grown = {
            name
            for exports in blocks.values()
            for name, body in exports.items()
            if name not in names and any(re.search(rf"\b{re.escape(t)}\b", body) for t in names)
        }
        if not grown:
            return names
        names |= grown


gone = vanished(removed_literals())
if gone:
    names = tainted_names(gone, helper_blocks())
    for name, text in specs.items():
        if any(lit in text for lit in gone) or any(
            re.search(rf"\b{re.escape(n)}\b", text) for n in names
        ):
            touched.add(name)



# A file this share of specs runs is app startup: a change to it would select
# nearly every spec, so the coverage map ignores it (ADR 0502, as amended).
SHARED_FILE_SHARE = 0.9


def covered(map_path):
    """Specs whose recorded source files meet the diff, by the coverage map."""
    fallback = "selecting by changed specs and vanished literals alone"
    try:
        with open(map_path, encoding="utf-8") as f:
            recorded = json.load(f)
        by_spec = recorded["specs"]
        if not isinstance(by_spec, dict):
            raise TypeError("specs is not an object")
        runs = {}
        for spec, files in by_spec.items():
            if not isinstance(files, list) or not all(isinstance(f, str) for f in files):
                raise TypeError(f"{spec} does not list file paths")
            runs[spec] = set(files)
    except FileNotFoundError:
        print(f"coverage map: none yet at {map_path}; {fallback}", file=sys.stderr)
        return set()
    except (OSError, ValueError, KeyError, TypeError) as err:
        print(f"coverage map: unreadable ({err}); {fallback}", file=sys.stderr)
        return set()
    recording = [files for files in runs.values() if files]
    counts = Counter(f for files in recording for f in files)
    shared = {f for f, n in counts.items() if n >= SHARED_FILE_SHARE * len(recording)}
    meets = changed - shared
    picked = {spec for spec, files in runs.items() if spec in specs and meets & files}
    print(
        f"coverage map: from {recorded.get('commit', 'an unknown commit')}, {len(picked)} spec(s) meet the diff"
        f" ({len(changed & shared)} changed startup file(s) ignored)",
        file=sys.stderr,
    )
    return picked


if coverage_map:
    touched |= covered(coverage_map)

if touched:
    print("\n".join(sorted(touched)))
PY
}
