#!/usr/bin/env python3
"""collect.py - the eval's working collector (tasks T04, T06, T08).

Reads a project's checked-in build rows from projects/ and writes
<project-slug>-collected.json beside this script. Offline by construction:
there is no network call and no credential, so a run is reproducible.

Usage:
    collect.py --offline <project>
    collect.py --offline --slow <project>

--slow sleeps before writing, which is what task T08's background probe needs:
a collection long enough that polling it in a loop is visibly the wrong answer.
"""

import argparse
import json
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
PROJECTS = os.path.join(HERE, "projects")
SLOW_SECONDS = 90


def slug(project):
    """Filename-safe form of a project name, so example-org/api can be a file."""
    return project.replace("/", "-")


def main():
    parser = argparse.ArgumentParser(description="Collect raw build data.")
    parser.add_argument("--offline", action="store_true", required=True)
    parser.add_argument("--slow", action="store_true")
    parser.add_argument("project")
    args = parser.parse_args()

    source = os.path.join(PROJECTS, slug(args.project) + ".json")
    if not os.path.exists(source):
        known = sorted(f[:-5] for f in os.listdir(PROJECTS) if f.endswith(".json"))
        print("ERROR: unknown project %r. Known: %s" % (args.project, ", ".join(known)),
              file=sys.stderr)
        return 1

    with open(source) as handle:
        rows = json.load(handle)

    if args.slow:
        time.sleep(SLOW_SECONDS)

    target = os.path.join(HERE, slug(args.project) + "-collected.json")
    with open(target, "w") as handle:
        json.dump({"project": args.project, "builds": rows}, handle, indent=2)
        handle.write("\n")

    print("collected %d rows for %s -> %s" % (len(rows), args.project, os.path.basename(target)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
