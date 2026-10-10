#!/usr/bin/env bash
# check-e2e-workflow.sh: fail if .github/workflows/e2e.yml broke one of its
# limits (ADR 0382). /harden Phase 4.5 runs it on every change.
#
#   ./scripts/check-e2e-workflow.sh [<e2e.yml> [<workflows-dir>]]
#
# The limits:
#   1. No secret is referenced, and the only permission is contents: read.
#   2. It triggers only on push to e2e/** and main.
#   3. Every job checks it runs on the mirror itself.
#   4. Only the seed-cache job saves a cache.
#   5. Every artifact lives one day.
#   6. mobile-webkit runs on macOS: the macos job runs on macos-15 and takes
#      the plan's macOS matrix.
#   7. No other workflow triggers on e2e/**.
#
# Stdlib Python reading lines, because the runners' YAML library is not on
# every Mac. Exit 0 clean, 1 on any broken limit.

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
workflow="${1:-$PROJECT_DIR/.github/workflows/e2e.yml}"
workflows_dir="${2:-$(dirname "$workflow")}"

# The repo slug and branch prefix come from the driver, so the two agree.
# shellcheck source=scripts/lib/e2e_github.sh
source "$SCRIPT_DIR/lib/e2e_github.sh"

python3 - "$workflow" "$workflows_dir" "$E2E_GITHUB_REPO" "$E2E_GITHUB_BRANCH_PREFIX" <<'PY'
import os, re, sys

path, wf_dir, repo, prefix = sys.argv[1:5]
lines = open(path).read().splitlines()
errors = []

def top_block(name):
    out, inside = [], False
    for line in lines:
        if re.match(r"^[A-Za-z]", line):
            inside = line.split(":")[0].strip() == name
            continue
        if inside and line.strip() and not line.lstrip().startswith("#"):
            out.append(line.rstrip())
    return out

def jobs():
    found, name = {}, None
    for line in top_block("jobs"):
        m = re.match(r"^  ([A-Za-z0-9_-]+):\s*$", line)
        if m:
            name = m.group(1)
            found[name] = []
        elif name:
            found[name].append(line)
    return found

code = [l for l in lines if not l.lstrip().startswith("#")]

if any("secrets." in l for l in code):
    errors.append("references a secret; the e2e workflow holds none")
perms = [l for l in code if re.match(r"^\s*permissions:", l)]
if len(perms) != 1 or perms[0] != "permissions:" or top_block("permissions") != ["  contents: read"]:
    errors.append("permissions must be one top-level block granting only contents: read")

on = top_block("on")
if on != ["  push:", f"    branches: ['{prefix}**', 'main']"]:
    errors.append(f"must trigger only on push to {prefix}** and main, found: " + " / ".join(x.strip() for x in on))

all_jobs = jobs()
for name, body in all_jobs.items():
    ifs = [l for l in body if re.match(r"^    if:", l)]
    if not ifs or f"github.repository == '{repo}'" not in ifs[0]:
        errors.append(f"job {name} does not check github.repository == '{repo}'")
    saves = [m.group(1) for m in (re.search(r"save-if:\s*(\S+)", l) for l in body) if m]
    if name != "seed-cache" and any(v != "false" for v in saves):
        errors.append(f"job {name} may save a cache; only seed-cache saves")
    if any("actions/cache@" in l for l in body):
        errors.append(f"job {name} uses actions/cache, which saves by default")

uploads = sum(1 for l in code if "actions/upload-artifact@" in l)
retentions = [l.split(":", 1)[1].strip() for l in code if re.match(r"^\s*retention-days:", l)]
if len(retentions) != uploads or any(r != "1" for r in retentions):
    errors.append(f"every artifact must set retention-days: 1 ({uploads} uploads, retentions {retentions})")

mac = all_jobs.get("macos", [])
if "    runs-on: macos-15" not in mac or not any("fromJSON(needs.plan.outputs.macos)" in l for l in mac):
    errors.append("the macos job must run on macos-15 and take the plan's macOS matrix")
linux = all_jobs.get("linux", [])
if any("outputs.macos" in l for l in linux):
    errors.append("the linux job must not take the macOS matrix")

for other in sorted(os.listdir(wf_dir)):
    full = os.path.join(wf_dir, other)
    if full == path or not other.endswith((".yml", ".yaml")):
        continue
    if any(re.search(r"(^|['\"\s\[,])" + re.escape(prefix), l.strip()) and not l.lstrip().startswith("#")
           for l in open(full).read().splitlines() if "branches" in l or l.strip().startswith("- ")):
        errors.append(f"{other} names an {prefix} branch; only e2e.yml may trigger on it")

for e in errors:
    print(f"✗ {os.path.basename(path)}: {e}")
if errors:
    sys.exit(1)
print(f"✓ {os.path.basename(path)} keeps its limits")
PY
