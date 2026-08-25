#!/usr/bin/env bash
#
# check-knowhow-refs.sh: fail if system-knowhow/ points at something that is
# not there, or names an event the engine does not have.
#
#   ./scripts/check-knowhow-refs.sh
#
# Run by `/harden` Phase 4.5 for EVERY diff, not only ones touching
# system-knowhow/. It is a whole-tree consistency check costing milliseconds,
# and the drift it finds is usually introduced by the OTHER side: a module that
# became a directory, a heading that got renamed, an event that got retired.
# The knowhow file is then stale without anybody having edited it.
#
# system-knowhow/ ships to every install and the engine LLM treats it as fact,
# so a pointer into nothing is not a broken link. It is guidance the model acts
# on. Four arms:
#
#   1. a backtick-quoted repo path that does not exist
#   2. a sibling knowhow file or id that does not resolve
#   3. an event name in workspace-audit.md / workspace-learning.md that is
#      neither a live event, a retired alias, nor one of the two the recipes
#      emit themselves
#   4. a severity word in workspace-audit.md outside its own legend
#
# Arm 3 is the one with a scar. workspace-audit.md told a workspace that
# ContextAssembled and ContextTokensMeasured were retired event renames. They
# are frontend-only legacy names and the engine has never listed them, so the
# audit's own worked example sent workspaces re-pointing live triggers at an
# event that was never the answer. Nothing caught it for months.
#
# What it CANNOT see: a check that has gone semantically stale while every name
# in it still resolves. `.claude/rules/system-knowhow.md` owns that half, and it
# needs a human.
#
# Exit status: 0 clean, 1 problems found OR the check could not run. A gate that
# cannot run must never read as clean, so every input is sanity-checked first.
#
# Targets bash 3.2, the macOS system shell. The scan itself is python3, which
# `build-dmg.sh` and `status.sh` already depend on.

set -uo pipefail

while [ $# -gt 0 ]; do
    case "$1" in
        -h | --help)
            awk 'NR == 1 { next } /^#/ { sub(/^# ?/, ""); print; next } { exit }' "${BASH_SOURCE[0]}"
            exit 0
            ;;
        *)
            echo "ERROR: unknown argument: $1" >&2
            exit 1
            ;;
    esac
done

REPO_ROOT="$(git rev-parse --show-toplevel 2> /dev/null)"
if [ -z "$REPO_ROOT" ]; then
    echo "ERROR: not inside a git checkout, so the knowhow-reference check cannot run." >&2
    exit 1
fi
cd "$REPO_ROOT" || exit 1

if ! command -v python3 > /dev/null 2>&1; then
    echo "ERROR: python3 is missing, so the knowhow-reference check cannot run." >&2
    exit 1
fi

python3 - <<'PYTHON'
import glob
import os
import re
import sys

EVENT_ENUM = "crates/lucidos-engine/src/engine/thread_events/event.rs"
EVENT_IMPL = "crates/lucidos-engine/src/engine/thread_events/event_impl.rs"
SYSTEM_ENUM = "crates/lucidos-engine/src/engine/event_bus_system_event.rs"
AUDIT = "system-knowhow/workspace-audit.md"
LEARNING = "system-knowhow/workspace-learning.md"

# Prefixes that name a real tracked path. A backtick span starting with
# anything else is prose, a data/ path inside a workspace, or a shell snippet.
PREFIXES = (
    "crates/",
    "docs/",
    "scripts/",
    "packages/",
    ".claude/",
    "system-knowhow/",
)

# PascalCase names in the two recipes that are correctly absent from every
# engine enum: a Rust type they talk about, and the domain events they emit
# themselves. Arm 3 flags anything else, so a recipe that starts naming another
# genuine type belongs here. A name the recipe calls an EVENT never does.
NOT_AN_ENGINE_EVENT = {
    "ThreadEvent",
    "WorkspaceAuditCompleted",
    "WorkspaceLearningCompleted",
}

problems = []


def fatal(message):
    sys.stderr.write("ERROR: %s, so the knowhow-reference check cannot run.\n" % message)
    sys.exit(1)


def read(path):
    if not os.path.isfile(path):
        fatal("%s is missing" % path)
    with open(path, encoding="utf-8") as handle:
        return handle.read()


def enum_variants(path, name):
    """Variant names of a `pub enum <name>`, by brace matching."""
    text = read(path)
    marker = "pub enum %s" % name
    if marker not in text:
        fatal("%s has no `%s`" % (path, marker))
    start = text.index("{", text.index(marker))
    depth = 0
    cursor = start
    while cursor < len(text):
        if text[cursor] == "{":
            depth += 1
        elif text[cursor] == "}":
            depth -= 1
            if depth == 0:
                break
        cursor += 1
    else:
        fatal("%s's `%s` body is unbalanced" % (path, marker))
    body = text[start:cursor]
    return set(re.findall(r"^\s{4}([A-Z][A-Za-z0-9]*)\s*(?:\{|,|\()", body, re.M))


def legacy_aliases():
    text = read(EVENT_IMPL)
    marker = "LEGACY_TYPE_NAME_ALIASES"
    if marker not in text:
        fatal("%s has no `%s`" % (EVENT_IMPL, marker))
    block = text[text.index(marker):]
    if "];" not in block:
        fatal("%s's `%s` list is unterminated" % (EVENT_IMPL, marker))
    return set(re.findall(r'"([A-Za-z0-9]+)"', block[: block.index("];")]))


def is_placeholder(path):
    """A deliberately generic path, which resolves to nothing by design."""
    if any(char in path for char in "<>*{}|…") or "..." in path:
        return True
    # A final segment written in capitals stands in for a name: `system-knowhow/X`.
    return re.match(r"^[A-Z][A-Z_]*$", path.rsplit("/", 1)[-1]) is not None


def table_cells(line):
    stripped = line.strip()
    if not stripped.startswith("|"):
        return []
    return [cell.strip() for cell in stripped.strip("|").split("|")]


# --- inputs ----------------------------------------------------------------

knowhow_files = sorted(glob.glob("system-knowhow/**/*.md", recursive=True))
if len(knowhow_files) < 20:
    fatal("only %d system-knowhow markdown files found" % len(knowhow_files))

# Each source carries its own floor. A union floor would not notice one enum
# parsing to nothing, because the other two comfortably clear it on their own,
# and every name in the silent enum would then be reported as invented.
known_events = set(NOT_AN_ENGINE_EVENT)
for source, count in (
    (enum_variants(EVENT_ENUM, "ThreadEvent"), 50),
    (enum_variants(SYSTEM_ENUM, "SystemEvent"), 50),
    (legacy_aliases(), 10),
):
    if len(source) < count:
        fatal("only %d names parsed where at least %d were expected" % (len(source), count))
    known_events |= source

# --- arms 1 and 2: every path and knowhow id resolves ----------------------

for path in knowhow_files:
    with open(path, encoding="utf-8") as handle:
        for number, line in enumerate(handle, 1):
            for match in re.finditer(r"`([^`\s]+)`", line):
                target = match.group(1)
                if not target.startswith(PREFIXES) or is_placeholder(target):
                    continue
                # Trailing punctuation, and the `path.rs::function` form.
                target = target.split("::", 1)[0].rstrip(".,;:")
                if os.path.exists(target):
                    continue
                # A `system-knowhow/<name>` with no suffix is a knowhow id.
                if target.startswith("system-knowhow/") and os.path.exists(target + ".md"):
                    continue
                problems.append("%s:%d: `%s` does not exist" % (path, number, target))

# --- arm 3: every event name the two recipes cite is a real one ------------

for path in (AUDIT, LEARNING):
    # read() fails closed on a missing recipe. Opening it directly would raise a
    # traceback instead, which reads as the gate crashing rather than the recipe
    # having been renamed or deleted.
    for number, line in enumerate(read(path).splitlines(), 1):
        for match in re.finditer(r"`([A-Z][a-z]+(?:[A-Z][A-Za-z0-9]+)+)`", line):
            name = match.group(1)
            if name in known_events:
                continue
            problems.append(
                "%s:%d: `%s` is not a live event, a retired alias, or a "
                "recipe's own event" % (path, number, name)
            )

# --- arm 4: the audit's severity vocabulary is closed ----------------------

audit = read(AUDIT)
legend = set()
used = set()
for line in audit.splitlines():
    cells = table_cells(line)
    for index, cell in enumerate(cells):
        bold = re.match(r"^\*\*([a-z]+)\*\*", cell)
        if not bold:
            continue
        if index == 0 and cell == bold.group(0):
            legend.add(bold.group(1))
        elif index > 0:
            used.add(bold.group(1))
used |= set(re.findall(r"[Ss]everity: \*\*([a-z]+)\*\*", audit))

# A missing legend is a finding, not an inability to run: reporting it as a
# problem keeps arms 1 to 3 visible in the same pass.
if not legend:
    problems.append("%s: no severity legend table, so no severity is defined" % AUDIT)
for word in sorted(used - legend):
    problems.append("%s: severity `%s` is used but not in the legend" % (AUDIT, word))

# --- report ----------------------------------------------------------------

if not problems:
    print(
        "✓ %d system-knowhow files: references, event names and severities resolve"
        % len(knowhow_files)
    )
    sys.exit(0)

sys.stderr.write("\n✗ BLOCKED: %d problem(s) in system-knowhow/.\n\n" % len(problems))
for problem in problems:
    sys.stderr.write("  %s\n" % problem)
sys.stderr.write(
    "\nsystem-knowhow/ ships to every install and the engine LLM reads it as\n"
    "fact. Fix the pointer, or the name, rather than the check. The rule is\n"
    ".claude/rules/system-knowhow.md.\n\n"
)
sys.exit(1)
PYTHON
