#!/usr/bin/env bash
# main_check.sh: the pure half of scripts/main-check.sh (ADR 0464).
#
# The main check runs `e2e-github.sh main-check --commit <sha>` and reads its
# log. These functions turn that log into the protocol the engine reads:
#   main-check run <url>        as soon as the workflow run has started
#   main-check verdict <json>   always the last line
#
# Sourced by scripts/main-check.sh. Tested by scripts/lib/main_check_test.sh.

MAIN_CHECK_RUN_PREFIX="main-check run "
MAIN_CHECK_VERDICT_PREFIX="main-check verdict "
# The line e2e_github_wait prints when it starts watching a run.
MAIN_CHECK_WATCHING_MARK="[e2e-github] watching "
# How much failure output the verdict carries. The full log stays on disk.
MAIN_CHECK_TAIL_LINES_PER_PHASE=25
MAIN_CHECK_TAIL_MAX_CHARS=4000

# main_check_verdict <driver-exit>: pass, fail or no-verdict.
main_check_verdict() {
    case "$1" in
        0) echo pass ;;
        "$E2E_GITHUB_NO_VERDICT_EXIT") echo no-verdict ;;
        *) echo fail ;;
    esac
}

# main_check_exit <verdict>: the exit code main-check.sh ends with.
main_check_exit() {
    case "$1" in
        pass) echo 0 ;;
        fail) echo 1 ;;
        *) echo "$E2E_GITHUB_NO_VERDICT_EXIT" ;;
    esac
}

# main_check_follow <log>: copy the driver's output into <log>, and print the
# run line the moment the driver names its run. It ignores SIGTERM and reads
# until the driver closes its output: engine shutdown signals the whole group,
# and the driver's cleanup (cancel the run, delete the branch) still writes to
# this pipe.
main_check_follow() {
    local log="$1" line
    trap '' TERM
    while IFS= read -r line; do
        printf '%s\n' "$line" >>"$log"
        case "$line" in
            "$MAIN_CHECK_WATCHING_MARK"*) printf '%s%s\n' "$MAIN_CHECK_RUN_PREFIX" "${line#"$MAIN_CHECK_WATCHING_MARK"}" ;;
        esac
    done
}

# main_check_verdict_line <verdict> <driver-exit> <log>: the last line of
# main-check.sh. The run URL, summary and tail come from the log.
main_check_verdict_line() {
    python3 - "$1" "$2" "$3" "$MAIN_CHECK_WATCHING_MARK" \
        "$MAIN_CHECK_TAIL_LINES_PER_PHASE" "$MAIN_CHECK_TAIL_MAX_CHARS" "$MAIN_CHECK_VERDICT_PREFIX" <<'PY'
import json, re, sys

verdict, rc, log_path, watching, per_phase, max_chars, prefix = sys.argv[1:8]
per_phase, max_chars = int(per_phase), int(max_chars)
try:
    lines = open(log_path, errors="replace").read().splitlines()
except OSError:
    lines = []

run_url = None
for line in lines:
    if line.startswith(watching):
        run_url = line[len(watching):].strip() or None

# Each phase of the report opens with "══ <title> ══".
sections, title = [], None
for line in lines:
    m = re.fullmatch(r"══ (.+) ══", line.strip())
    if m:
        title = m.group(1).replace(" (GitHub)", "")
        sections.append((title, []))
    elif title is not None:
        sections[-1][1].append(line)

failed = []
for line in lines:
    m = re.fullmatch(r"── (.+) exit code: (\d+) ──", line.strip())
    if m and m.group(2) != "0":
        failed.append(m.group(1))
in_projects = False
for line in lines:
    if line.strip() == "── Per-project exit codes ──":
        in_projects = True
        continue
    if in_projects:
        m = re.fullmatch(r"\s+(\S+): (\d+|UNKNOWN.*)", line)
        if not m:
            in_projects = False
        elif m.group(2) != "0":
            failed.append("browser " + m.group(1))

def last(n, rows):
    return rows[-n:] if n else []

if verdict == "pass":
    summary, tail_rows = "Every suite passed.", []
elif verdict == "fail":
    summary = ("Failed: " + ", ".join(failed) + ".") if failed else f"The run failed (exit {rc})."
    tail_rows = []
    for name, body in sections:
        phase = name.replace("browser: ", "browser ")
        if phase in failed:
            tail_rows += [f"══ {name} ══"] + last(per_phase, body)
    if not tail_rows:
        tail_rows = last(2 * per_phase, lines)
else:
    errors = [l.strip()[len("ERROR:"):].strip() for l in lines if l.strip().startswith("ERROR:")]
    summary = errors[-1] if errors else f"The run produced no verdict (exit {rc})."
    tail_rows = last(per_phase, lines)

tail = "\n".join(tail_rows)
if len(tail) > max_chars:
    tail = tail[-max_chars:]
print(prefix + json.dumps({
    "verdict": verdict,
    "run_url": run_url,
    "summary": summary,
    "tail": tail,
    "log": log_path,
}))
PY
}
