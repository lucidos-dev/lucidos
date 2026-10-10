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

  --layers         Also list references that point up the planned tiers.
  --json FILE      Write lines, test lines and edges as JSON.
  --inbound PREFIX List references from units outside PREFIX into it, and exit
                   1 if there are any. Phase 3 is done when `--inbound api`
                   exits 0.
"""

import argparse
import collections
import json
import os
import re
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
    "engine::thread_state",
}


def unit_of(rel):
    parts = rel[:-3].split("/")
    if parts[-1] == "mod":
        parts = parts[:-1]
    top = parts[0]
    if top in ("lib", "main"):
        return "root"
    if top in SPLIT and len(parts) > 1:
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


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--src", default=os.path.join(
        os.path.dirname(os.path.abspath(__file__)), "..", "crates", "lucidos-engine", "src"))
    parser.add_argument("--layers", action="store_true")
    parser.add_argument("--json")
    parser.add_argument("--inbound")
    args = parser.parse_args()

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
