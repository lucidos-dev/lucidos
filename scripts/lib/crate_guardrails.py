"""Cargo workspace graph and target size checks (offline, without a build)."""

import argparse
from graphlib import CycleError, TopologicalSorter
import json
import os
from pathlib import Path
import re
import subprocess
import sys

CAP = 50_000
# A ceiling may rise to this much above its crate's largest target, so feature
# work goes on until the crate is split (ADR 0738).
HEADROOM_PERCENT = 10
DEPTH_CAP = 6
NEW_CRATE_RADIUS_CAP = 150_000
CHURN_DAYS = 30
TIER_MAP = Path("crates/crate-tiers.tsv")
CEILINGS = Path("crates/crate-size-ceilings.tsv")
GRAPH_BASELINE = Path("crates/crate-graph-baseline.tsv")
ROOT_CRATE = "lucidos-engine-bin"
FRONT_DOOR = {"lucidos-gateway", "lucidos-cli"}
FILE_MODULE = re.compile(
    r'(?m)^[ \t]*(?P<attrs>(?:#\[[^\n]*\][ \t\r\n]*)*)'
    r'(?:pub(?:\([^)]*\))?[ \t]+)?mod[ \t]+(?P<name>\w+)[ \t]*;'
)
MODULE_PATH = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')
INCLUDED_RUST = re.compile(r'\binclude!\(\s*"([^"]+\.rs)"\s*\)')


def command(*args):
    return subprocess.check_output(args, text=True)


def workspace_packages():
    metadata = json.loads(command("cargo", "metadata", "--format-version", "1",
                                  "--no-deps", "--offline", "--locked"))
    members = set(metadata["workspace_members"])
    return {p["name"]: p for p in metadata["packages"] if p["id"] in members}


def read_map(text, path, roles=()):
    rows = {}
    for number, line in enumerate(text.splitlines(), 1):
        if not line or line.startswith("#"):
            continue
        fields = line.split("\t")
        if len(fields) < 2:
            raise ValueError(f"{path}:{number}: expected a tab-separated row")
        name, value, *role = fields
        digits = value.removeprefix("-") if roles else value
        valid_role = not role or len(role) == 1 and role[0] in roles
        if (name in rows or not re.fullmatch(r"[A-Za-z0-9_-]+", name) or not valid_role
                or not digits.isascii() or not digits.isdecimal()):
            raise ValueError(f"{path}:{number}: invalid or duplicate row: {line}")
        rows[name] = (int(value), role[0] if role else "")
    return rows


def workspace_dependencies(package, packages):
    for dependency in package["dependencies"]:
        dest = dependency["name"]
        path = dependency.get("path")
        # A registry package with the same name is not a workspace edge.
        if (dest in packages and path is not None
                and Path(path) == Path(packages[dest]["manifest_path"]).parent):
            yield dest, dependency["kind"] or "normal"


def tier_errors(packages, tiers):
    for name in sorted(packages.keys() - tiers.keys()):
        yield f"{name}: missing tier in {TIER_MAP}"
    for name in sorted(tiers.keys() - packages.keys()):
        yield f"{name}: stale tier in {TIER_MAP}"
    for name in sorted(packages.keys() & tiers.keys()):
        source_tier, role = tiers[name]
        if Path(packages[name]["manifest_path"]).parent.name != name:
            yield f"{name}: expected manifest in a directory named {name}"
        if source_tier > 8 or source_tier < 0 and role:
            yield f"{name}: invalid tier {source_tier} for its role"
        if (source_tier == 7 and role != "leaf") or (role == "leaf" and source_tier != 7):
            yield f"{name}: tier 7 requires the leaf role"
        for dest, kind in workspace_dependencies(packages[name], packages):
            if dest not in tiers:
                continue
            dest_tier, dest_role = tiers[dest]
            edge = f"{name} -> {dest} ({kind})"
            if dest_tier >= source_tier:
                yield f"{edge}: tier {source_tier} must depend on a lower tier than itself"
            if dest_role == "leaf" and name != ROOT_CRATE:
                yield f"{edge}: only {ROOT_CRATE} may depend on a leaf"
            if dest_role in ("engine", "leaf") and (
                    name in FRONT_DOOR or role == "" and source_tier < 8):
                yield f"{edge}: non-engine support crates must stay outside the engine"


def physical_lines(path):
    with path.open("rb") as source:
        return sum(1 for _ in source)


def standalone_sources(entry, tracked):
    # Count potential cfg variants together. Libraries use the entire src tree
    # by policy. A standalone target follows file modules and literal includes.
    seen = set()
    pending = [(entry, entry.parent)]
    while pending:
        path, module_dir = pending.pop()
        if path not in tracked or path in seen:
            continue
        seen.add(path)
        text = path.read_text(encoding="utf-8")
        for declaration in FILE_MODULE.finditer(text):
            attrs, name = declaration.group("attrs", "name")
            explicit = MODULE_PATH.search(attrs)
            if explicit:
                resolved = (path.parent / explicit[1]).resolve()
                pending.append((resolved, resolved.parent))
            else:
                pending.append((module_dir / f"{name}.rs", module_dir / name))
                pending.append((module_dir / name / "mod.rs", module_dir / name))
        pending.extend(((path.parent / source).resolve(), module_dir)
                       for source in INCLUDED_RUST.findall(text))
    return seen


def target_sizes(package, tracked):
    root = Path(package["manifest_path"]).parent
    src = root / "src"
    targets = package["targets"]
    libraries = {"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"}
    shared_src = [p for p in tracked if p.is_relative_to(src)
                  and not p.is_relative_to(src / "bin")]
    for target in targets:
        entry = Path(target["src_path"])
        kind = set(target["kind"])
        if kind & libraries or (entry == src / "main.rs" and not any(
                set(other["kind"]) & libraries for other in targets)):
            files = shared_src
        else:
            files = standalone_sources(entry, tracked)
        yield target["name"], ", ".join(target["kind"]), sum(map(physical_lines, files))


def tracked_sources(root):
    return {root / p for p in command("git", "ls-files", "-z", "--", "*.rs").split("\0")
            if p and (root / p).is_file()}


def read_graph_baseline(text):
    lines = [line for line in text.splitlines() if line and not line.startswith("#")]
    label, _, depth = lines[0].partition("\t") if lines else ("", "", "")
    if label != "depth" or not depth.isascii() or not depth.isdecimal():
        raise ValueError(f"{GRAPH_BASELINE}: expected a depth row")
    rows = {}
    for line in lines[1:]:
        name, tab, value = line.partition("\t")
        dependents = value.split(",") if value else []
        if (not tab or name in rows or not re.fullmatch(r"[A-Za-z0-9_-]+", name)
                or not all(re.fullmatch(r"[A-Za-z0-9_-]+", dep) for dep in dependents)):
            raise ValueError(f"{GRAPH_BASELINE}: invalid or duplicate row: {line}")
        rows[name] = set(dependents)
    return int(depth), rows


def main_baseline(path):
    committed = subprocess.run(["git", "show", f"main:{path}"], text=True,
                               capture_output=True)
    if committed.returncode == 0:
        return committed.stdout
    if (subprocess.run(["git", "rev-parse", "--verify", "main"],
                       capture_output=True).returncode
            or command("git", "ls-tree", "main", "--", str(path))):
        raise ValueError(f"cannot read {path} from main: {committed.stderr.strip()}")
    return None


def graph_snapshot(packages, root):
    tiers = read_map(TIER_MAP.read_text(), TIER_MAP, ("engine", "leaf"))
    missing_tiers = packages.keys() - tiers.keys()
    if missing_tiers:
        raise ValueError(f"missing workspace tiers: {', '.join(sorted(missing_tiers))}")
    edges = sorted({(name, dest, kind) for name, package in packages.items()
                    for dest, kind in workspace_dependencies(package, packages)})
    dependencies = {name: set() for name in packages}
    propagated = {name: set() for name in packages}
    direct_dev = {name: set() for name in packages}
    for name, dest, kind in edges:
        dependencies[name].add(dest)
        if kind == "dev":
            direct_dev[dest].add(name)
        else:
            propagated[name].add(dest)
    try:
        order = list(TopologicalSorter(dependencies).static_order())
    except CycleError as error:
        raise ValueError("workspace dependency cycle: " + " -> ".join(error.args[1])) from error

    paths = {}
    for name in order:
        downstream = (paths[dest] for dest in sorted(dependencies[name]))
        paths[name] = (name,) + max(downstream, key=len, default=())
    longest = max((paths[name] for name in sorted(paths)), key=len, default=())

    normal_dependents = {name: set() for name in packages}
    for name in reversed(order):
        for dest in propagated[name]:
            normal_dependents[dest].add(name)
            normal_dependents[dest].update(normal_dependents[name])

    tracked = tracked_sources(root)
    sizes = {name: sum(count for _, _, count in target_sizes(package, tracked))
             for name, package in packages.items()}
    crates = {}
    for name, downstream in sorted(normal_dependents.items()):
        # A dev edge terminates propagation at the crate that owns its tests.
        dependents = downstream.union(*(direct_dev[dep] for dep in downstream | {name}))
        crates[name] = {
            "tier": tiers[name][0],
            "target_lines": sizes[name],
            "blast_radius": sum(sizes[dependent] for dependent in dependents),
            "dependents": sorted(dependents),
        }
    return {
        "depth": max(0, len(longest) - 1),
        "depth_cap": (read_graph_baseline(GRAPH_BASELINE.read_text())[0]
                      if GRAPH_BASELINE.exists() else DEPTH_CAP),
        "longest_path": longest,
        "crates": crates,
        "edges": [{"from": name, "to": dest, "kind": kind}
                  for name, dest, kind in edges],
    }


def graph_baseline(snapshot):
    return ("# Workspace dependency depth in edges and the transitive dependents of each crate\n"
            f"depth\t{snapshot['depth']}\n" +
            "".join(f"{name}\t{','.join(crate['dependents'])}\n"
                    for name, crate in snapshot["crates"].items()))


def add_change_frequency(snapshot, packages, root):
    directories = {Path(package["manifest_path"]).parent.relative_to(root): name
                   for name, package in packages.items()}
    history = command("git", "log", f"--since={CHURN_DAYS}.days", "--no-merges",
                      "--format=%x00", "--name-only", "--no-renames", "-z", "--", "*.rs")
    counts = dict.fromkeys(packages, 0)
    for commit in history.split("\0\0"):
        touched = set()
        for file in commit.lstrip("\0\n").split("\0"):
            for directory in Path(file).parents:
                if directory in directories:
                    touched.add(directories[directory])
                    break
        for name in touched:
            counts[name] += 1
    snapshot["change_window_days"] = CHURN_DAYS
    for name, crate in snapshot["crates"].items():
        crate["change_commits"] = counts[name]


def graph_errors(snapshot, packages, root):
    current_depth, current = read_graph_baseline(GRAPH_BASELINE.read_text())
    prior = main_baseline(GRAPH_BASELINE)
    prior_depth, previous = (DEPTH_CAP, {}) if prior is None else read_graph_baseline(prior)
    main_paths = set(command("git", "ls-tree", "-rz", "--name-only", "main").split("\0"))
    existing = previous.keys() | {
        name for name, package in packages.items()
        if Path(package["manifest_path"]).relative_to(root).as_posix() in main_paths}

    depth_limit = min(DEPTH_CAP, prior_depth)
    depth = snapshot["depth"]
    if max(depth, current_depth) > depth_limit:
        path = " -> ".join(snapshot["longest_path"])
        yield f"dependency depth rose from {depth_limit} to {max(depth, current_depth)}: {path}"
    if current_depth != depth:
        yield f"dependency depth: record {depth} instead of {current_depth}"

    crates = snapshot["crates"]
    for name in sorted(current.keys() - crates.keys()):
        yield f"{name}: stale dependents baseline"
    for name, crate in crates.items():
        dependents = set(crate["dependents"])
        recorded = current.get(name)
        if recorded is None:
            yield f"{name}: missing dependents baseline"
        elif recorded != dependents:
            yield f"{name}: dependents differ from the baseline, record them with --baseline"
        if name in previous:
            gained = sorted(((dependents | (recorded or set())) - previous[name]) & existing)
            if gained:
                yield f"{name}: gained dependents {', '.join(gained)}"
        elif name not in existing and crate["blast_radius"] > NEW_CRATE_RADIUS_CAP:
            yield (f"{name}: new crate exceeds {NEW_CRATE_RADIUS_CAP} dependent lines "
                   f"({crate['blast_radius']})")


def with_headroom(peak):
    return (peak * (100 + HEADROOM_PERCENT) + 99) // 100


def size_check(packages, root, reset=None):
    committed = main_baseline(CEILINGS)
    prior = read_map(committed or "", CEILINGS)
    current = read_map(CEILINGS.read_text(), CEILINGS) if CEILINGS.exists() else {}
    tracked = tracked_sources(root)
    sizes = {name: list(target_sizes(package, tracked))
             for name, package in packages.items()}

    if reset:
        if reset not in sizes:
            raise ValueError(f"unknown package: {reset}")
        peak = max(count for _, _, count in sizes[reset])
        if peak <= CAP:
            current.pop(reset, None)
        else:
            if committed is not None and reset not in prior:
                raise ValueError(f"{reset}: reset would raise the {CAP} line cap")
            current[reset] = (with_headroom(peak), "")
        CEILINGS.write_text("# Package and maximum physical lines per compilation target\n" +
                            "".join(f"{name}\t{value}\n"
                                    for name, (value, _) in sorted(current.items())))
        return []

    errors = []
    for name, (ceiling, _) in current.items():
        if name not in sizes or ceiling <= CAP:
            errors.append(f"{name}: invalid ceiling {ceiling}")
        if committed is not None and name not in prior:
            errors.append(f"{name}: new ceiling {ceiling} would raise the {CAP} line cap")
        elif name in prior and ceiling > prior[name][0] and name in sizes:
            allowed = with_headroom(max(count for _, _, count in sizes[name]))
            if ceiling > allowed:
                errors.append(f"{name}: ceiling rose from {prior[name][0]} to {ceiling}, "
                              f"past {allowed} ({HEADROOM_PERCENT}% over the largest target)")
    for name, targets in sorted(sizes.items()):
        limit = current.get(name, (CAP, ""))[0]
        for target, kind, count in targets:
            if count > limit:
                errors.append(f"{name} {target} ({kind}): {count} lines exceeds {limit}")
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("check", choices=("tiers", "size", "graph", "engine-packages"))
    parser.add_argument("--reset", metavar="PACKAGE")
    parser.add_argument("--libraries", action="store_true")
    output = parser.add_mutually_exclusive_group()
    output.add_argument("--json", action="store_true")
    output.add_argument("--baseline", action="store_true")
    args = parser.parse_args()
    if args.reset and args.check != "size":
        parser.error("--reset requires the size check")
    if (args.json or args.baseline) and args.check != "graph":
        parser.error("--json and --baseline require the graph check")
    try:
        root = Path(command("git", "rev-parse", "--show-toplevel").strip())
        # Run Cargo and git relative to the same checkout, including in fixtures.
        os.chdir(root)
        packages = workspace_packages()
        if args.check == "engine-packages":
            tiers = read_map(TIER_MAP.read_text(), TIER_MAP, ("engine", "leaf"))
            for name, (_, role) in tiers.items():
                if not role:
                    continue
                if name not in packages:
                    raise ValueError(f"{name}: no workspace package for its tier row")
                if not args.libraries or any(
                        "lib" in target["kind"] for target in packages[name]["targets"]):
                    print(name)
            return 0
        if args.check == "tiers":
            tiers = read_map(TIER_MAP.read_text(), TIER_MAP, ("engine", "leaf"))
            errors = list(tier_errors(packages, tiers))
        elif args.check == "graph":
            snapshot = graph_snapshot(packages, root)
            if args.json:
                revision, date, subject = command(
                    "git", "log", "-1", "--format=%H%x00%aI%x00%s").strip().split("\0")
                snapshot.update(source=subject, source_date=date, revision=revision,
                                dirty=bool(command("git", "status", "--porcelain").strip()))
                add_change_frequency(snapshot, packages, root)
                print(json.dumps(snapshot, indent=2))
                return 0
            if args.baseline:
                print(graph_baseline(snapshot), end="")
                return 0
            errors = list(graph_errors(snapshot, packages, root))
        else:
            errors = size_check(packages, root, args.reset)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(error, file=sys.stderr)
        return 1
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(f"Crate {args.check} check passed ({len(packages)} packages)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
