#!/usr/bin/env bash
# check-e2e-workflow.sh: fail if .github/workflows/e2e.yml broke one of its
# limits (ADRs 0382, 0464). /harden Phase 4.5 runs it on every change.
#
#   ./scripts/check-e2e-workflow.sh [<e2e.yml> [<workflows-dir>]]
#
# The limits:
#   1. No secret is referenced, and the only permissions are contents: read
#      and actions: read, which finds and downloads an earlier run's engine.
#   2. It triggers only on push to e2e/** and main.
#   3. Every job checks it runs on the mirror itself.
#   4. Only the seed-cache job saves a cache, by save-if or actions/cache/save.
#   5. Every artifact lives one day, except what a later run reuses: an
#      engine and its manifest live seven days, target/ state two.
#   6. mobile-webkit runs on macOS: the macos job runs on macos-15 and takes
#      the plan's macOS matrix.
#   7. No other workflow triggers on e2e/**.
#   8. The plan job runs only on an e2e/** push, and seed-cache only on main.
#      Every other job needs the plan job, so only a run's request starts it.
#      The engine job also runs only when the plan asks for it.
#   9. Every action is pinned by a full commit sha.
#  10. A test job runs only after its build job succeeded or was skipped. Its
#      steps unpack or assemble an engine for those two results alone, so any
#      other result (cancelled, failed) leaves it without an engine.
#
# Tested by scripts/lib/check_e2e_workflow_test.sh.
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
if len(perms) != 1 or perms[0] != "permissions:" or top_block("permissions") != ["  contents: read", "  actions: read"]:
    errors.append("permissions must be one top-level block granting only contents: read and actions: read")

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
    if name != "seed-cache" and any("actions/cache/save@" in l for l in body):
        errors.append(f"job {name} saves a cache with actions/cache/save; only seed-cache saves")

# The artifacts a later run reuses, by name prefix, and how many days each
# lives. Every other artifact lives one day.
REUSED = {"e2e-engine-": "7", "e2e-manifest-": "7", "e2e-target-": "2"}
for i, l in enumerate(code):
    if "actions/upload-artifact@" not in l:
        continue
    indent = len(l) - len(l.lstrip(" -"))
    step = []
    for nxt in code[i + 1:]:
        if nxt.strip() and (nxt.lstrip().startswith("- ") and len(nxt) - len(nxt.lstrip()) < indent
                            or len(nxt) - len(nxt.lstrip()) < indent - 2):
            break
        step.append(nxt.strip())
    name = next((x.split(":", 1)[1].strip() for x in step if x.startswith("name:")), "")
    days = next((x.split(":", 1)[1].strip() for x in step if x.startswith("retention-days:")), None)
    want = next((d for p, d in REUSED.items() if name.startswith(p)), "1")
    if days != want:
        errors.append(f"artifact {name or '(unnamed)'} must set retention-days: {want}, found {days}")

def job_if(name):
    ifs = [l for l in all_jobs.get(name, []) if re.match(r"^    if:", l)]
    return ifs[0] if ifs else ""

if f"startsWith(github.ref, 'refs/heads/{prefix}')" not in job_if("plan"):
    errors.append(f"the plan job must run only on an {prefix} branch")
if "github.ref == 'refs/heads/main'" not in job_if("seed-cache"):
    errors.append("the seed-cache job must run only on main")
for name, body in all_jobs.items():
    if name in ("plan", "seed-cache"):
        continue
    needs = [l for l in body if re.match(r"^    needs:", l)]
    if not needs or not re.search(r"(:\s*|[\[,]\s*)plan\s*($|[\],])", needs[0]):
        errors.append(f"job {name} does not need the plan job, so it could run on a push to main")

if "engine" in all_jobs and "needs.plan.outputs.has_engine == 'true'" not in job_if("engine"):
    errors.append("the engine job must run only when the plan asks for it (has_engine)")

for l in code:
    m = re.search(r"uses:\s*(\S+)", l)
    if m and not re.fullmatch(r"[^@\s]+@[0-9a-f]{40}", m.group(1)):
        errors.append(f"action not pinned by commit: {m.group(1)}")

for test_job, build_job in (("linux", "build-linux"), ("macos", "build-macos")):
    want = f"(needs.{build_job}.result == 'success' || needs.{build_job}.result == 'skipped')"
    if want not in job_if(test_job):
        errors.append(f"the {test_job} job must not run after a cancelled or failed build: gate it on {want}")

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
