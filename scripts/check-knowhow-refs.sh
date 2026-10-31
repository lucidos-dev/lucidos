#!/usr/bin/env bash
#
# check-knowhow-refs.sh: fail if system-knowhow/ points at something that is
# not there, or names an event the engine does not have.
#
#   ./scripts/check-knowhow-refs.sh
# Checks references and reverse event, CLI and SDK documentation coverage.
# Reads docs/generated/agent-surface.json without Cargo. Missing inputs fail.
# Domain examples declare `<!-- domain-event: Name: reason -->` in their file.
# Semantic currency still needs the review in .claude/rules/system-knowhow.md.

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
import json
import pathlib
import shlex
import urllib.parse

import os
import re
import sys

sys.excepthook = lambda kind, value, trace: sys.stderr.write("ERROR: knowhow references: %s\n" % value)

SURFACE = "docs/generated/agent-surface.json"
SETTINGS = "crates/lucidos-app/src/components/search/searchIndex.ts"
AUDIT = "system-knowhow/workspace-audit.md"
LEARNING = "system-knowhow/workspace-learning.md"
AUDIT_SECTIONS_GLOB = "system-knowhow/workspace-audit/**/*.md"

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

problems = []


def fatal(message):
    sys.stderr.write("ERROR: %s, so the knowhow-reference check cannot run.\n" % message)
    sys.exit(1)


def read(path):
    if not os.path.isfile(path):
        fatal("%s is missing" % path)
    with open(path, encoding="utf-8") as handle:
        return handle.read()


def names(value, label):
    if not isinstance(value, list) or not value or any(not isinstance(v, str) or not v for v in value):
        fatal("%s is empty or malformed in %s" % (label, SURFACE))
    if len(value) != len(set(value)):
        fatal("%s has duplicate names" % label)
    return set(value)


def cli_tree(node):
    if not isinstance(node, dict) or not node.get("name") or not isinstance(node.get("hidden"), bool):
        fatal("CLI command is malformed")
    for field in ("flags", "aliases", "commands"):
        if not isinstance(node.get(field), list):
            fatal("CLI %s is missing" % field)
    if any(not isinstance(flag, str) or not flag for flag in node["flags"] + node["aliases"]):
        fatal("CLI flags or aliases are malformed")
    for child in node["commands"]:
        cli_tree(child)


def legacy_qualified(line):
    return re.search(r"\b(alias(?:es)?|legacy|retired)\b", line, re.I) is not None


def cli_reference(snippet, line):
    try:
        tokens = shlex.split(snippet.replace("\\\n", " "), comments=True)
    except ValueError:
        # Documentation placeholders can contain unmatched quotes.
        tokens = snippet.split()
    if not tokens or tokens[0] != "lucidos" or (len(tokens) > 1 and tokens[1].endswith(":")):
        return
    if tokens[1:3] == ["events", "emit"] and len(tokens) > 3:
        name = tokens[3]
        if name in legacy_events and not legacy_qualified(line):
            yield "event %s is legacy; qualify it on this line" % name
        if re.fullmatch(r"[A-Z][A-Za-z0-9]+", name) and name not in known_events and name not in domain_events:
            yield "event %s is not live, legacy or declared" % name
    node = surface["cli"]
    for token in tokens[1:]:
        if token in ("--", "|", ";", "&&", ">", "2>"):
            break
        if token.startswith(("<", "$", "[", "{", "…")):
            continue
        if token.startswith("--"):
            flag = token[2:].split("=", 1)[0].rstrip(".,;:")
            if flag and flag not in node["flags"]:
                yield "CLI flag --%s is not on lucidos %s" % (flag, node["name"])
            continue
        if token.startswith("-"):
            continue
        children = {name: child for child in node["commands"] for name in [child["name"]] + child["aliases"]}
        if token in children:
            node = children[token]
        elif children:
            yield "CLI subcommand %s does not exist below %s" % (token, node["name"])
            break


def negative_reference(line, reference):
    """A qualifier must describe this span, not another clause on its line."""
    quoted = re.search(r"`" + re.escape(reference) + r"`", line)
    if not quoted:
        return False
    before, after = line[:quoted.start()], line[quoted.end():]
    words = r"(?:no|not|nonexistent|invalid|removed|retired|deleted|refused|misspelled|gone)"
    noun = r"(?:(?:events?|tools?|commands?|types?|names?)\s+)?"
    return (re.search(r"\b" + words + r"\s+" + noun + r"$", before, re.I) is not None
            or re.search(r"\bno\s+(?:`[^`]+`,?\s*(?:or\s+)?)+$", before, re.I) is not None
            or re.match(r"\s*" + noun + r"(?:(?:is|was|are|were)\s+)?(?:not\s+(?:a|an|offered|live)|" + words + r")\b", after, re.I) is not None)


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

surface = json.loads(read(SURFACE))
if surface.get("schema_version") != 1:
    fatal("agent surface schema_version is unsupported")
cli_tree(surface.get("cli"))
if not surface["cli"]["commands"]:
    fatal("CLI tree is empty")
tools = surface.get("tools", {})
offered = names(tools.get("offered"), "offered tools")
retired = names(tools.get("retired"), "retired tools")
aliases = tools.get("aliases")
if not isinstance(aliases, dict) or not aliases or any(not k or v not in offered for k, v in aliases.items()):
    fatal("tool aliases are empty or malformed")
if offered & (retired | set(aliases)) or retired & set(aliases):
    fatal("tool classifications overlap")
sdk_exports = names(surface.get("sdk_exports"), "SDK exports")
legacy_events = names(surface.get("events", {}).get("legacy"), "legacy events")
known_events = set()
for kind in ("thread", "system", "legacy"):
    known_events |= names(surface.get("events", {}).get(kind), kind + " events")
thread_events = set(surface["events"]["thread"])
event_suffixes = {re.findall(r"[A-Z][a-z0-9]*", name)[-1] for name in known_events}
settings_text = read(SETTINGS)
settings_chains = []
for entry in re.findall(r"\{[^{}]*\}", settings_text):
    label = re.search(r"\blabel:\s*(['\"])(.*?)\1", entry)
    path = re.search(r"\bpath:\s*(['\"])(.*?)\1", entry)
    if label and path:
        settings_chains.append((path[2] + " → " + label[2]).split(" → "))
if not settings_chains:
    fatal("Settings search index parsed to nothing")
read(LEARNING)

# The audit's sections carry its checks, so they share its event and severity
# rules. None found means the split went missing, which must not read as clean.
audit_sections = sorted(glob.glob(AUDIT_SECTIONS_GLOB, recursive=True))
if not audit_sections:
    fatal("no audit sections under %s" % AUDIT_SECTIONS_GLOB)

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

# --- forward names and links across the whole corpus -----------------------

for path in knowhow_files:
    text = read(path)
    domain_events = {}
    for declaration in re.findall(r"<!--\s*domain-event:(.*?)-->", text):
        match = re.fullmatch(r"\s*([A-Z][A-Za-z0-9]+):\s*(\S.*?)\s*", declaration)
        if not match or match[1] in known_events:
            problems.append("%s: invalid domain-event declaration: %s" % (path, declaration))
        else:
            domain_events[match[1]] = match[2]
    fenced = False
    lines = text.splitlines()
    for number, line in enumerate(lines, 1):
        if line.strip().startswith(("```", "~~~")):
            fenced = not fenced
        spans = re.findall(r"`([^`]+)`", line)
        snippets = [span for span in spans if span.startswith("lucidos ")]
        if re.match(r"^\s*(?:\$ )?lucidos\s+", line):
            snippets.append(re.sub(r"^\s*(?:\$ )?", "", line))
        for snippet in snippets:
            reference = snippet
            cursor = number
            while snippet.rstrip().endswith("\\") and cursor < len(lines):
                snippet = snippet.rstrip()[:-1] + " " + lines[cursor].strip()
                cursor += 1
            if negative_reference(line, reference):
                continue
            for problem in cli_reference(snippet, line):
                problems.append("%s:%d: %s" % (path, number, problem))
        for span in spans:
            symbol = re.fullmatch(r"([^\s]+\.rs)::([A-Za-z_][A-Za-z0-9_:]*)(?:\(\))?", span)
            if symbol and not is_placeholder(symbol[1]):
                if not os.path.isfile(symbol[1]):
                    problems.append("%s:%d: symbol source does not exist: %s" % (path, number, symbol[1]))
                    continue
                source = read(symbol[1])
                for name in symbol[2].split("::"):
                    if not re.search(r"\b" + re.escape(name) + r"\b", source):
                        problems.append("%s:%d: symbol %s is absent from %s" % (path, number, name, symbol[1]))
            tool = re.fullmatch(r"([a-z][a-z0-9_]*)(?:\([^`]*\))?", span)
            if tool:
                reference = span
                span = tool[1]
                tool_context = re.search(r"(?:tools?\s+(?:named\s+)?`" + re.escape(span) + r"`|`" + re.escape(reference) + r"`\s+tools?\b)", line, re.I)
                if span in aliases or span in retired:
                    if not legacy_qualified(line):
                        problems.append("%s:%d: tool %s is an alias or retired; qualify it on this line" % (path, number, span))
                elif (tool_context or ("(" in reference and ("_" in span or "action=" in reference))) and span not in offered and not negative_reference(line, reference):
                    problems.append("%s:%d: tool %s is not offered" % (path, number, span))
            if re.fullmatch(r"[A-Z][a-z]+(?:[A-Z][A-Za-z0-9]+)+", span):
                event_form = (path in [AUDIT, LEARNING] + audit_sections or re.findall(r"[A-Z][a-z0-9]*", span)[-1] in event_suffixes
                              or re.search(r"\bevent(?:s| type)?\s+(?:named\s+)?`" + re.escape(span), line, re.I))
                negative = negative_reference(line, span)
                if span in legacy_events and not legacy_qualified(line):
                    problems.append("%s:%d: event %s is legacy; qualify it on this line" % (path, number, span))
                if span not in known_events and span not in domain_events and event_form and not negative and not re.search(r"(?:browser type\s+`" + re.escape(span) + r"`|`" + re.escape(span) + r"`\s+browser type)", line):
                    problems.append("%s:%d: %s is not a live event, legacy alias or declared domain example" % (path, number, span))
        for name in re.findall(r"\b(?:on_event|event_type)[\"']?\s*[:=]\s*[\"']?([A-Z][A-Za-z0-9]+)", line):
            if name in legacy_events and not legacy_qualified(line):
                problems.append("%s:%d: event %s is legacy; qualify it on this line" % (path, number, name))
            if name not in known_events and name not in domain_events:
                problems.append("%s:%d: event %s is not live, legacy or declared" % (path, number, name))
        link_line = re.sub(r"`[^`]*`", "", line)
        if fenced:
            continue
        for target in re.findall(r"!?\[[^\]]*\]\(([^\s)]+)(?:\s+[^)]*)?\)", link_line):
            target = target.strip("<>")
            if re.match(r"[A-Za-z][A-Za-z0-9+.-]*:", target) or target.startswith(("#", "//")) or is_placeholder(target):
                continue
            target = urllib.parse.unquote(target.split("#", 1)[0].split("?", 1)[0])
            if target and not (pathlib.Path(path).parent / target).exists():
                problems.append("%s:%d: relative markdown link does not resolve: %s" % (path, number, target))

    for match in re.finditer(r"Settings[ \t]*[→>][ \t]*((?:(?!\n[ \t]*\n)[^`*|.,;:()<])+)", text):
        number = text.count("\n", 0, match.start()) + 1
        prefix = text[:match.start()].rsplit("\n", 1)[-1]
        if re.search(r"(?:System|iOS|macOS)\s*(?:\*\*)?$", prefix):
            continue
        chain = ["Settings"] + [re.sub(r"\s+", " ", part.replace("//", " ")).strip().rstrip('"\'] ') for part in re.split(r"[→>]", match[1])]
        def matches(candidate):
            if len(chain) > len(candidate) or chain[:-1] != candidate[:len(chain)-1]:
                return False
            label = candidate[len(chain)-1]
            return chain[-1] == label or re.match(re.escape(label) + r"(?:\s+[a-z]|\s+/)", chain[-1]) is not None
        if not any(matches(candidate) for candidate in settings_chains):
            problems.append("%s:%d: Settings chain does not resolve: %s" % (path, number, " → ".join(chain)))


# --- arm 4: the audit's severity vocabulary is closed ----------------------

legend = set()
used = {}
for path in [AUDIT] + audit_sections:
    text = read(path)
    for line in text.splitlines():
        cells = table_cells(line)
        for index, cell in enumerate(cells):
            bold = re.match(r"^\*\*([a-z]+)\*\*", cell)
            if not bold:
                continue
            if index == 0 and cell == bold.group(0) and path == AUDIT:
                legend.add(bold.group(1))
            elif index > 0:
                used.setdefault(bold.group(1), path)
    for word in re.findall(r"[Ss]everity: \*\*([a-z]+)\*\*", text):
        used.setdefault(word, path)

# A missing legend is a finding, not an inability to run: reporting it as a
# problem keeps arms 1 to 3 visible in the same pass.
if not legend:
    problems.append("%s: no severity legend table, so no severity is defined" % AUDIT)
for word in sorted(set(used) - legend):
    problems.append(
        "%s: severity `%s` is used but not in %s's legend" % (used[word], word, AUDIT)
    )

# --- arm 5: every public surface has documentation -------------------------

def markdown_lines(path):
    """Examples and comments cannot stand in for reference documentation."""
    text = re.sub(r"<!--.*?(?:-->|\Z)", "", read(path), flags=re.S)
    fence = None
    for line in text.splitlines():
        marker = re.match(r"^\s{0,3}(`{3,}|~{3,})", line)
        if marker:
            if fence is None:
                fence = marker[1]
            elif (marker[1][0] == fence[0] and len(marker[1]) >= len(fence)
                  and not line[marker.end():].strip()):
                fence = None
            continue
        if fence is None and not re.match(r"^(?: {4}|\t)", line):
            yield line


event_doc = "system-knowhow/thread-events.md"
documented_events = set()
for line in markdown_lines(event_doc):
    cells = table_cells(line)
    if len(cells) >= 3 and re.fullmatch(r"`[A-Z][A-Za-z0-9]+`", cells[0]):
        documented_events.add(cells[0].strip("`"))
for name in sorted(thread_events - documented_events):
    problems.append("%s: live ThreadEvent %s has no event table row" % (event_doc, name))


def command_paths(node, prefix=()):
    if node["hidden"]:
        return
    path = prefix + (node["name"],)
    if node["name"] == "help":
        yield (surface["cli"]["name"], "help")
        return
    yield path
    # Clap mirrors the command tree below every generated help command.
    # One help section explains that syntax; the targets need their own sections.
    for child in node["commands"]:
        yield from command_paths(child, path)


def cli_heading_paths(snippet, parent=()):
    """Expand shorthand alternatives, ignoring pipes inside usage arguments."""
    depth = 0
    cleaned = ""
    for char in snippet:
        if char in "[<({":
            if depth == 0:
                cleaned += " "
            depth += 1
        elif char in "]>)}":
            depth = max(0, depth - 1)
        elif depth == 0:
            cleaned += char
    for alternative in re.split(r"\s*[|/]\s*", cleaned):
        words = []
        for word in alternative.split():
            if not re.fullmatch(r"[a-z][a-z0-9-]*", word):
                break
            words.append(word)
        words = tuple(words)
        if not words:
            continue
        path = words if words[0] == surface["cli"]["name"] else parent + words
        if words[0] == surface["cli"]["name"] or not parent:
            parent = path[:-1]
        yield path


cli_doc = "system-knowhow/lucidos-cli.md"
documented_commands = set()
for line in markdown_lines(cli_doc):
    if re.match(r"^#{2,6}\s", line):
        parent = ()
        previous_end = 0
        for snippet in re.finditer(r"`([^`]+)`", line):
            if not re.fullmatch(r"\s*[|/]\s*", line[previous_end:snippet.start()]):
                parent = ()
            paths = list(cli_heading_paths(snippet[1], parent))
            previous_end = snippet.end()
            if paths:
                parent = paths[0][:-1]
            for path in paths:
                for length in range(1, len(path) + 1):
                    documented_commands.add(path[:length])
for path in sorted(set(command_paths(surface["cli"])) - documented_commands):
    problems.append("%s: visible CLI command %s has no section" % (cli_doc, " ".join(path)))

sdk_doc = "system-knowhow/js-sdk.md"
documented_exports = set()
for line in markdown_lines(sdk_doc):
    heading = re.match(r"^## lucidos\.([A-Za-z][A-Za-z0-9_]*)(?=[:\s]|$)", line)
    if heading:
        documented_exports.add(heading[1])
for name in sorted(sdk_exports - documented_exports):
    problems.append("%s: SDK lucidos.%s has no ## section" % (sdk_doc, name))


# --- report ----------------------------------------------------------------

if not problems:
    print(
        "✓ %d system-knowhow files: references resolve and public surfaces have documentation"
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
