#!/usr/bin/env python3
"""The module dependency graph inside one engine crate.

Guides the engine crate split (ADR 0392). It counts `crate::` references
between module units in production code. A unit is a top-level module, with
`engine`, `core` and `api` split one level deeper. Test files and inline
`#[cfg(test)]` modules add lines but no edges, and `super::` paths are not
followed, so the cycles it reports are a lower bound.

Usage:
  scripts/engine-module-graph.py [--src DIR] [--layers] [--json FILE]
  scripts/engine-module-graph.py [--src DIR] --inbound PREFIX
  scripts/engine-module-graph.py [--src DIR] --cut agents|core [--widened FILE]

  scripts/engine-module-graph.py --layout FILE [--cut CRATE]

  --layers         Also list references that point up the planned tiers.
  --json FILE      Write lines, test lines and edges as JSON.
  --inbound PREFIX List references from units outside PREFIX into it, and exit
                   1 if there are any. Phase 3 is done when `--inbound api`
                   exits 0.
  --cut agents     Inventory lower-to-agent dependency candidates by edge kind.
  --cut core       Check resolved dependencies after the split, syntax before it.
  --layout FILE    Compare crate sizes, dependency inversions, and churn.
                   Use --cut CRATE for a boundary inventory.
  --widened FILE   With --cut, check a tab-separated list of lower
                   source paths and widened symbols for references above the cut.
"""

import argparse
import collections
from fnmatch import fnmatchcase
import json
import os
from pathlib import Path
import re
import subprocess
import sys

SPLIT = {"engine", "core", "api"}
REF_RE = re.compile(r"crate::([a-z_][a-z0-9_]*)(?:::([a-z_][a-z0-9_]*))?")
GROUP_RE = re.compile(r"use\s+crate::\{(.*?)\};", re.S)
ITEM_RE = re.compile(r"([a-z_][a-z0-9_]*)(?:::([a-z_][a-z0-9_]*))?")
INLINE_TESTS_RE = re.compile(r"#\[cfg\(test\)\]\s*mod\s+\w+\s*\{")
TEST_SUFFIXES = ("_unit_tests", "_integration_tests", "_tests")

# The planned tiers, lowest first (ADR 0392). A reference from a unit to a
# unit in a higher tier is one the split must move or invert.
TIER = {
    "paths": 0, "net_config": 0, "gateway_auth": 0, "boot_report": 0,
    "boot_failure": 0, "core": 0, "llm": 0, "capability_manifest": 0,
    "memory": 1, "mcp": 1, "runtime": 1, "voice": 1, "triggers": 1,
    "scheduler": 1, "engine": 1,
    "api": 2,
    "root": 3, "bin": 3,
}
FOUNDATION_ENGINE_UNITS = {
    "engine::thread_events", "engine::event_bus", "engine::thread_lifecycle",
    "engine::thread_state", "engine::consumption_claim", "engine::continuation_reason",
    "engine::form_requests", "engine::handoff_sql", "engine::provider_rejection",
    "engine::question_park_sql", "engine::session_scope", "engine::standing_instruction",
    "engine::switch_recovery_sql", "engine::trigger_context", "engine::pending_handoff",
    "engine::thread_family", "engine::question_parsing", "engine::widget_embed",
}
CORE_ROOT_UNITS = {
    "paths", "net_config", "gateway_auth", "boot_report", "boot_failure",
    "core", "llm", "capability_manifest", "diagnostic_log", "internal_faults",
    "engine_runtime", "gateway_users", "test_support", "diagnostic_bundles",
}
CORE_ENGINE_PREFIXES = ("event_bus_", "thread_lifecycle_")

AGENT_ENGINE_UNITS = {
    "agent_session", "tools", "chat", "agentic_loop", "agent_recovery",
    "turn_end", "successor", "claude_code",
}
MOVE_UP_CANDIDATES = {
    "thread_triage", "archive_request", "http", "draft_submission", "question_card_gate",
}

CUT_PATH_RE = re.compile(r"\b(?:crate|(?:super::)*super|self)(?:::\w+)*::(?:\{|\*|\w+)")
REEXPORT_RE = re.compile(r"\bpub(?:\([^)]*\))?\s+use\s+([^;]+);", re.S)
ENGINE_IMPL_RE = re.compile(r"\bimpl\s+(?:crate::engine::|(?:super::)+|self::)?LucidosEngine\s*\{")
METHOD_RE = re.compile(r"\bfn\s+(\w+)\s*[<(]")
CALL_RE = re.compile(
    r"(?:(?P<host>\.\s*agent_host\s*\(\s*\)\s*)?\.|\b(?:Self|LucidosEngine)::)"
    r"\s*(?P<method>\w+)\s*(?:\(|::<)"
)
NON_CODE_RE = re.compile(
    r'//[^\n]*|/\*.*?\*/|r(?P<hashes>\#*)".*?"(?P=hashes)|"(?:\\.|[^"\\])*"'
    r"|'(?:\\.|[^'\\\n])'", re.S
)
BRACE_RE = re.compile(r"[{}]")


def agent_unit(unit):
    top, _, sub = unit.partition("::")
    return top in ("api", "api_support") or (
        top == "engine" and any(sub == name or sub.startswith(name + "_")
                                for name in AGENT_ENGINE_UNITS | MOVE_UP_CANDIDATES)
    )


def core_unit(unit):
    top, _, sub = unit.partition("::")
    return (top in CORE_ROOT_UNITS or unit in FOUNDATION_ENGINE_UNITS
            or (top == "engine" and sub.startswith(CORE_ENGINE_PREFIXES)))



def blank(match):
    return re.sub(r"[^\n]", " ", match[0])


def closing_brace(text, opening):
    depth = 0
    for brace in BRACE_RE.finditer(text, opening):
        depth += 1 if brace[0] == "{" else -1
        if depth == 0:
            return brace.start()
    return len(text)


def without_inline_tests(text):
    offset = 0
    for test in INLINE_TESTS_RE.finditer(text):
        if test.start() >= offset:
            yield text[offset:test.start()], False
            offset = closing_brace(text, test.end() - 1) + 1
            yield text[test.start():offset], True
    yield text[offset:], False


def module_path(rel):
    parts = rel[:-3].split("/")
    if parts[-1] in ("lib", "mod"):
        parts.pop()
    return parts


def resolve_path(path, module):
    parts = path.split("::")
    if parts[0] == "crate":
        return parts[1:]
    if parts[0] == "self":
        return module + parts[1:]
    depth = 0
    while depth < len(parts) and parts[depth] == "super":
        depth += 1
    return module[:max(0, len(module) - depth)] + parts[depth:]


def path_unit(path, module):
    parts = resolve_path(path, module) + ["", ""]
    return target_of(parts[0], parts[1])


def use_paths(path):
    """Expand nested use trees while keeping the leading module path."""
    prefix, brace, group = path.partition("{")
    if not brace:
        yield prefix.split(" as ")[0].strip()
        return
    for item in group_items(group.rsplit("}", 1)[0]):
        if item:
            yield from use_paths(prefix + item)


def path_uses(text):
    for match in CUT_PATH_RE.finditer(text):
        path = match[0]
        if path.endswith("{"):
            path += text[match.end():closing_brace(text, match.end() - 1) + 1]
        yield from use_paths(path)


def engine_methods(text):
    for impl in ENGINE_IMPL_RE.finditer(text):
        body = text[impl.end():closing_brace(text, impl.end() - 1)]
        offset = 0
        for method in METHOD_RE.finditer(body):
            if method.start() < offset:
                continue
            yield method[1]
            opening = body.find("{", method.end())
            if opening >= 0:
                offset = closing_brace(body, opening) + 1


def report_unreferenced_widenings(sources, list_path, upper_unit, cut):
    references = set()
    signatures = collections.defaultdict(set)
    for rel, code, lower_root in sources:
        if not lower_root or upper_unit(unit_of(rel)):
            references.update(re.findall(r"\b\w+\b", code))
            continue
        # The compiler can require visibility for a type whose name callers
        # never spell, such as an inferred return value or an alias's target.
        for signature in re.finditer(
            r"\bpub\s+(?:(?:async\s+)?fn\s+|(?:const|static)\s+)?"
            r"(\w+)(?:<[^;{}]*?>)?\s*"
            r"(?:\([^;{}]*\)\s*(?:->\s*[^;{}]+)?|:\s*[^=;{}\n]+)",
            code,
        ):
            signatures[signature[1]].update(re.findall(r"\b\w+\b", signature[0]))
        for export in REEXPORT_RE.finditer(code):
            for target, alias in re.findall(r"\b(\w+)\s+as\s+(\w+)\b", export[1]):
                signatures[alias].add(target)
        for enum in re.finditer(r"\bpub\s+enum\s+(\w+)[^;{]*\{", code):
            # Variant payloads are part of the enum's public signature.
            opening = enum.end() - 1
            payloads = code[opening + 1:closing_brace(code, opening)]
            signatures[enum[1]].update(re.findall(r"\b\w+\b", payloads))

    pending = list(references & signatures.keys())
    while pending:
        for name in signatures[pending.pop()] - references:
            references.add(name)
            if name in signatures:
                pending.append(name)

    missing = []
    with open(list_path, encoding="utf-8") as handle:
        for line in handle:
            path, symbol = line.rstrip("\n").split("\t")
            name = re.split(r"::|\.", symbol)[-1]
            if name not in references:
                missing.append((path, symbol))
    for path, symbol in missing:
        print(f"  widened without a reference above cut {cut}: {path} -> {symbol}")
    return bool(missing)


def check_core_dependencies(core_src):
    """Once split, Cargo resolves cross-crate edges, including renamed dependencies."""
    manifest = os.path.join(core_src, "..", "Cargo.toml")
    result = subprocess.run(
        ["cargo", "metadata", "--locked", "--offline", "--format-version", "1",
         "--manifest-path", manifest],
        check=True, capture_output=True, text=True,
    )
    graph = json.loads(result.stdout)
    packages = {package["id"]: package for package in graph["packages"]}
    core_id = next(package["id"] for package in packages.values()
                   if package["name"] == "lucidos-core" and package["source"] is None)
    dependencies = {node["id"]: node["dependencies"] for node in graph["resolve"]["nodes"]}
    upper_names = {"lucidos-engine", "lucidos-agents-api", "lucidos-engine-bin"}
    visited = {core_id}
    pending = [core_id]
    upward = set()
    while pending:
        for dependency in dependencies[pending.pop()]:
            if dependency in visited:
                continue
            visited.add(dependency)
            pending.append(dependency)
            if packages[dependency]["name"] in upper_names:
                upward.add(packages[dependency]["name"])
    for name in sorted(upward):
        print(f"  lucidos-core depends on upper crate {name}")
    if not upward:
        print("Zero resolved upward dependencies for cut core")
    return bool(upward)


def cut_inventory(src, cut, widened=None):
    """Inventory syntactic candidates. Rust resolves the final types and imports."""
    upper_unit = agent_unit if cut == "agents" else lambda unit: not core_unit(unit)
    roots = [src]
    agents_src = os.path.join(src, "..", "..", "lucidos-agents-api", "src")
    if os.path.isdir(agents_src):
        roots.append(agents_src)
    if cut == "core":
        core_src = os.path.join(src, "..", "..", "lucidos-core", "src")
        if os.path.isdir(core_src):
            try:
                upward = check_core_dependencies(core_src)
            except subprocess.CalledProcessError as error:
                print(error.stderr, file=sys.stderr, end="")
                return 1
            if not widened:
                return int(upward)
            sources = read_cut_sources([core_src, *roots], core_src)
            missing = report_unreferenced_widenings(sources, widened, lambda _: False, cut)
            return int(upward or missing)
    sources = list(read_cut_sources(roots, src))
    methods = set()
    for rel, code, lower_root in sources:
        if (upper_unit(unit_of(rel)) or (cut == "core" and not lower_root)) and not is_test_path(rel):
            for segment, test in without_inline_tests(code):
                if not test:
                    methods.update(engine_methods(segment))

    counts = collections.Counter()
    rows = collections.Counter()
    lower_candidates = collections.Counter()
    for rel, code, lower_root in sources:
        if not lower_root or upper_unit(unit_of(rel)):
            continue
        module = module_path(rel)
        for segment, inline_test in without_inline_tests(code):
            scope = "test" if inline_test or is_test_path(rel) else "production"
            for export in REEXPORT_RE.finditer(segment):
                for path in use_paths(export[1]):
                    if upper_unit(path_unit(path, module)):
                        rows[scope, "re-export", rel, path] += 1
            for path in path_uses(REEXPORT_RE.sub(blank, segment)):
                if upper_unit(path_unit(path, module)):
                    rows[scope, path.split("::", 1)[0], rel, path] += 1
            for call in CALL_RE.finditer(segment):
                if not call["host"] and call["method"] in methods:
                    rows[scope, "method candidate", rel, call["method"]] += 1

    for (scope, kind, rel, target), count in sorted(rows.items()):
        print(f"  {scope:10s} {kind:16s} {count:3d}  {rel} -> {target}")
        counts[scope, kind] += count
        if kind != "method candidate":
            target_unit = path_unit(target, module_path(rel))
            if cut == "agents" and target_unit.partition("::")[2] in MOVE_UP_CANDIDATES:
                lower_candidates[scope, target_unit] += count
    for (scope, kind), count in sorted(counts.items()):
        print(f"{count:5d} {scope} {kind} references")
    for (scope, unit), count in sorted(lower_candidates.items()):
        print(f"{count:5d} {scope} references into move-up candidate {unit}")
    if not rows:
        print(f"Zero syntactic upward references for cut {cut}")
    missing = report_unreferenced_widenings(sources, widened, upper_unit, cut) if widened else False
    return int(bool(rows) or missing)


def read_cut_sources(roots, lower_root):
    for root in roots:
        for dirpath, _, files in os.walk(root):
            for name in sorted(files):
                if not name.endswith(".rs"):
                    continue
                path = os.path.join(dirpath, name)
                with open(path, encoding="utf-8") as handle:
                    code = NON_CODE_RE.sub(blank, handle.read())
                yield os.path.relpath(path, root), code, root == lower_root


def unit_of(rel, splits=SPLIT):
    parts = rel[:-3].split("/")
    if parts[-1] == "mod":
        parts = parts[:-1]
    top = parts[0]
    if top in ("lib", "main"):
        return "root"
    if top in splits and len(parts) > 1:
        sub = parts[1]
        for suffix in TEST_SUFFIXES:
            if sub.endswith(suffix):
                sub = sub[: -len(suffix)]
                break
        return f"{top}::{sub}"
    return top


def is_test_path(rel):
    """A test file or directory by name, never a substring match: a module
    such as `push_test_log.rs` is production code."""
    parts = rel[:-3].split("/")
    return any(part in ("tests", "test_support") or part.endswith(TEST_SUFFIXES + ("_test",))
               for part in parts)


def target_of(top, sub):
    return f"{top}::{sub}" if top in SPLIT and sub else top


def group_items(body):
    depth, token, items = 0, "", []
    for ch in body:
        depth += ch == "{"
        depth -= ch == "}"
        if ch == "," and depth == 0:
            items.append(token)
            token = ""
        else:
            token += ch
    items.append(token)
    return [item.strip() for item in items]


def build_graph(src):
    tops = {name[:-3] if name.endswith(".rs") else name for name in os.listdir(src)}
    lines, test_lines = collections.Counter(), collections.Counter()
    edges = collections.defaultdict(collections.Counter)
    for dirpath, _, files in os.walk(src):
        for name in files:
            if not name.endswith(".rs"):
                continue
            path = os.path.join(dirpath, name)
            rel = os.path.relpath(path, src)
            with open(path, encoding="utf-8", errors="ignore") as handle:
                text = handle.read()
            unit = unit_of(rel)
            lines[unit] += text.count("\n")
            inline = INLINE_TESTS_RE.search(text)
            if is_test_path(rel):
                test_lines[unit] += text.count("\n")
                continue
            if inline:
                test_lines[unit] += text[inline.start():].count("\n")
                text = text[: inline.start()]
            for top, sub in REF_RE.findall(text):
                if top in tops:
                    edges[unit][target_of(top, sub)] += 1
            for body in GROUP_RE.findall(text):
                for item in group_items(body):
                    match = ITEM_RE.match(item)
                    if match and match.group(1) in tops:
                        edges[unit][target_of(match.group(1), match.group(2))] += 1
    for unit in list(edges):
        edges[unit].pop(unit, None)
        for target in [t for t in edges[unit] if t not in lines]:
            edges[unit].pop(target)
    return lines, test_lines, edges


def strongly_connected(units, edges):
    index, low, stack, on_stack, out = {}, {}, [], set(), []
    counter = [0]
    sys.setrecursionlimit(max(10000, 4 * len(units)))

    def visit(v):
        index[v] = low[v] = counter[0]
        counter[0] += 1
        stack.append(v)
        on_stack.add(v)
        for w in edges.get(v, {}):
            if w not in index:
                visit(w)
                low[v] = min(low[v], low[w])
            elif w in on_stack:
                low[v] = min(low[v], index[w])
        if low[v] == index[v]:
            component = []
            while True:
                w = stack.pop()
                on_stack.discard(w)
                component.append(w)
                if w == v:
                    break
            out.append(component)

    for unit in units:
        if unit not in index:
            visit(unit)
    return out


def tier_of(unit):
    if unit in FOUNDATION_ENGINE_UNITS:
        return TIER["core"]
    return TIER.get(unit.split("::")[0], TIER["engine"])


def report(lines, test_lines, edges, layers):
    units = sorted(lines, key=lambda u: -lines[u])
    components = strongly_connected(units, edges)
    cycles = sorted((c for c in components if len(c) > 1),
                    key=lambda c: -sum(lines[u] for u in c))
    print(f"units: {len(units)}, lines: {sum(lines.values())}, "
          f"test lines: {sum(test_lines.values())}")
    print(f"cycles (strongly connected components larger than one unit): {len(cycles)}")
    for component in cycles:
        members = sorted(component, key=lambda u: -lines[u])
        print(f"  {len(component)} units, {sum(lines[u] for u in component)} lines: "
              f"{', '.join(members)}")
    in_cycle = {u for c in cycles for u in c}
    print("\nunits outside any cycle (lines, test lines, outgoing references):")
    for unit in units:
        if unit not in in_cycle:
            print(f"  {lines[unit]:7d} {test_lines[unit]:7d}  {unit} -> "
                  f"{', '.join(sorted(edges.get(unit, {}))) or '-'}")
    if layers:
        print("\nreferences pointing up the planned tiers (count, from, to):")
        upward = [(c, u, v) for u in edges for v, c in edges[u].items()
                  if tier_of(v) > tier_of(u)]
        for count, source, target in sorted(upward, reverse=True):
            print(f"  {count:5d}  {source} -> {target}")


def inbound(edges, prefix):
    def inside(unit):
        return unit == prefix or unit.startswith(prefix + "::")

    found = sorted((c, u, v) for u in edges if not inside(u)
                   for v, c in edges[u].items() if inside(v))
    for count, source, target in sorted(found, reverse=True):
        print(f"  {count:5d}  {source} -> {target}")
    print(f"{sum(c for c, _, _ in found)} references into {prefix} from outside it")
    return 1 if found else 0


LAYOUT_SPLITS = SPLIT | {"llm", "runtime"}
LAYOUT_PATH = re.compile(r"\b(?:crate|lucidos_\w+)::(?:\w+::)*(?:\{|\*|\w+)")
BINARY_ROOT = "lucidos-engine-bin"


def layout_rows(path):
    for number, line in enumerate(path.read_text().splitlines(), 1):
        if line.strip() and not line.startswith("#"):
            try:
                name, tier, *attrs = line.split("\t")
                yield name, int(tier), attrs
            except ValueError:
                raise ValueError(f"{path}:{number}: expected a crate and tier") from None


def layout_paths(code):
    for match in LAYOUT_PATH.finditer(code):
        path = match[0]
        if path.endswith("{"):
            path += code[match.end():closing_brace(code, match.end() - 1) + 1]
        for resolved in use_paths(path):
            prefix, _, module = resolved.partition("::")
            top, _, rest = module.partition("::")
            sub = rest.split("::", 1)[0]
            yield prefix, f"{top}::{sub}" if top in LAYOUT_SPLITS and sub else top


def layout_metrics(sizes, edges, changes):
    # A production edit rebuilds its crate and the transitive users of it.
    users = collections.defaultdict(set)
    for name, dependencies in edges.items():
        for dependency in dependencies:
            users[dependency].add(name)
    closure = {}

    def reach(name):
        if name not in closure:
            closure[name] = {name}.union(*(reach(user) for user in users[name]))
        return closure[name]

    def critical_path(rebuilt):
        length = {}

        def span(name):
            if name not in length:
                length[name] = sizes[name] + max(
                    (span(dep) for dep in edges[name] & rebuilt), default=0)
            return length[name]

        return max(map(span, rebuilt), default=0)

    lines = critical = 0
    for change in changes:
        rebuilt = set(change).union(*(reach(name) for name, prod in change.items() if prod))
        lines += sum(sizes[name] for name in rebuilt)
        critical += critical_path(rebuilt)
    divisor = max(1, len(changes))
    return (
        (round(lines / divisor), round(critical / divisor), critical_path(set(sizes))),
        {name: sum(sizes[user] for user in reach(name) - {name}) for name in sizes},
    )


def layout_report(path, cut):
    repo = Path(__file__).resolve().parents[1]
    tiers, rules = {}, []
    for name, tier, attrs in layout_rows(Path(path)):
        if name in tiers or len(attrs) != 1 or not attrs[0]:
            raise ValueError(f"{path}: expected a unique crate, tier and unit patterns for {name}")
        tiers[name] = tier
        if attrs[0] != "(remainder)":
            rules.extend((pattern, name) for pattern in attrs[0].split())
    if cut and cut not in tiers:
        raise ValueError(f"unknown crate in {path}: {cut}")
    installed = {name: tier for name, tier, role in layout_rows(repo / "crates/crate-tiers.tsv")
                 if role in (["engine"], ["leaf"])}
    sources = {}
    units, owners = collections.defaultdict(set), collections.defaultdict(set)
    for crate in installed:
        for dir_name in ("src", "tests"):
            directory = repo / "crates" / crate / dir_name
            for file in sorted(directory.rglob("*.rs")):
                rel = file.relative_to(directory).as_posix()
                unit = unit_of(rel, LAYOUT_SPLITS)
                test = dir_name == "tests" or is_test_path(rel)
                dest = crate if crate == BINARY_ROOT else next(
                    (name for pattern, name in rules if fnmatchcase(unit, pattern)), crate)
                sources[file.relative_to(repo).as_posix()] = (crate, dest, unit, test)
                if not test:
                    units[crate, unit].add(dest)
                    owners[unit].add(dest)

    before_sizes, sizes, production = (collections.Counter() for _ in range(3))
    before_edges, edges = collections.defaultdict(set), collections.defaultdict(set)
    inversions = collections.Counter()
    crossing = collections.defaultdict(collections.Counter)
    for file, (crate, source, unit, test) in sources.items():
        text = (repo / file).read_text(encoding="utf-8")
        lines = text.count("\n") + int(bool(text) and not text.endswith("\n"))
        sizes[source] += lines
        before_sizes[crate] += lines
        if test:
            continue
        chunks = list(without_inline_tests(NON_CODE_RE.sub(blank, text)))
        production[source] += max(0, lines - sum(
            chunk.count("\n") + 1 for chunk, test in chunks if test))
        for chunk, test in chunks:
            if test:
                continue
            for prefix, target_unit in layout_paths(chunk):
                facade = crate if prefix == "crate" else prefix.replace("_", "-")
                if facade not in installed:
                    continue
                if installed[facade] < installed[crate]:
                    before_edges[crate].add(facade)
                targets = units.get((facade, target_unit), owners[target_unit])
                if len(targets) != 1:
                    continue
                target = next(iter(targets))
                if target == source:
                    continue
                rank = tiers.get(target, installed.get(target, 6))
                if rank >= tiers.get(source, installed.get(source, 6)) or (
                        rank == 7 and source != BINARY_ROOT):
                    inversions[source, target] += 1
                else:
                    edges[source].add(target)
                if cut in (source, target):
                    crossing[source, target][unit, target_unit] += 1
    if BINARY_ROOT in sizes:
        edges[BINARY_ROOT].update(name for name in sizes if tiers.get(name) == 7)

    # A pure crate move preserves src-relative paths. Attribute pre-move
    # commits to today's owner when that relative path has only one match.
    prior_paths = collections.defaultdict(set)
    for file, (_, dest, unit, test) in sources.items():
        prior_paths[file.split("/", 2)[2]].add((dest, unit, test))
    history = subprocess.check_output([
        "git", "-C", str(repo), "log", "--since=30.days", "--no-merges",
        "--format=%x00%H", "--name-only", "--no-renames", "--", "crates"
    ], text=True)
    before_changes, changes = [], []
    commits, prod_commits = collections.Counter(), collections.Counter()
    for entry in history.split("\0"):
        before, planned = {}, {}
        for file in entry.splitlines()[1:]:
            if file in sources:
                crate, dest, _, test = sources[file]
            elif file.startswith("crates/") and len(file.split("/", 2)) == 3:
                _, crate, rel = file.split("/", 2)
                candidates = prior_paths[rel]
                if len(candidates) != 1:
                    continue
                dest, _, test = next(iter(candidates))
            else:
                continue
            if crate in installed:
                before[crate] = before.get(crate, False) or not test
            else:
                continue
            planned[dest] = planned.get(dest, False) or not test
        if planned:
            before_changes.append(before)
            changes.append(planned)
            commits.update(planned.keys())
            prod_commits.update(name for name, prod in planned.items() if prod)

    current, _ = layout_metrics(before_sizes, before_edges, before_changes)
    planned, blast = layout_metrics(sizes, edges, changes)
    print("30-day history of current paths and unambiguous crate moves; reference counts are lower bounds.")
    print("Projections use permitted references and root wiring; inversions need redesign.")
    print("Proxy (physical lines)              Current   Planned")
    for metric, old, new in zip(("Recompiled per commit", "Critical path per commit",
                                 "Cold critical path"), current, planned):
        print(f"{metric:30} {old:12,d} {new:9,d}")
    print("\nCrate                             Total    Production   Commits   Prod commits  Lines above")
    quartile = max(1, (len(sizes) + 3) // 4)
    hot = {name for name, _ in commits.most_common(quartile)}
    high = set(sorted(blast, key=blast.get, reverse=True)[:quartile])
    for name in sorted(sizes, key=lambda n: (tiers.get(n, installed.get(n, 6)), n)):
        flag = "  review" if name in hot & high else ""
        print(f"{name:32} {sizes[name]:8,d} {production[name]:13,d} {commits[name]:9,d}"
              f" {prod_commits[name]:14,d} {blast[name]:12,d}{flag}")
    print("\nReferences to invert")
    for (source, target), count in sorted(inversions.items()):
        if cut is None or cut in (source, target):
            print(f"{source} -> {target}: {count:,}")
    if cut:
        print(f"\nReferences crossing {cut}, including permitted edges")
        for (source, target), refs in sorted(crossing.items()):
            for (src_unit, dest_unit), count in refs.most_common():
                print(f"{source}:{src_unit} -> {target}:{dest_unit}: {count:,}")
    return 0


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--src", default=os.path.join(
        os.path.dirname(os.path.abspath(__file__)), "..", "crates", "lucidos-engine", "src"))
    parser.add_argument("--layers", action="store_true")
    parser.add_argument("--json")
    parser.add_argument("--inbound")
    parser.add_argument("--layout")
    parser.add_argument("--cut")
    parser.add_argument("--widened")
    args = parser.parse_args()

    if args.layout:
        if args.widened or args.layers or args.inbound or args.json:
            parser.error("--layout supports --cut only")
        try:
            return layout_report(args.layout, args.cut)
        except (OSError, ValueError, subprocess.CalledProcessError) as error:
            parser.error(str(error))
    if args.cut:
        if args.cut not in ("agents", "core"):
            parser.error("use --layout FILE for a named crate cut")
        return cut_inventory(args.src, args.cut, args.widened)
    if args.widened:
        parser.error("--widened requires --cut")
    lines, test_lines, edges = build_graph(args.src)
    if args.json:
        with open(args.json, "w") as handle:
            json.dump({"lines": lines, "test_lines": test_lines,
                       "edges": {u: dict(c) for u, c in edges.items()}}, handle, indent=1)
    if args.inbound:
        return inbound(edges, args.inbound)
    report(lines, test_lines, edges, args.layers)
    return 0


if __name__ == "__main__":
    sys.exit(main())
