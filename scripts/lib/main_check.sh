#!/usr/bin/env bash
# main_check.sh: the pure half of scripts/main-check.sh (ADR 0464).
#
# The main check runs `e2e-github.sh main-check --commit <sha>` and reads its
# log. These functions turn that log into the protocol the engine reads:
#   main-check run <url> <branch>  as soon as the workflow run is found
#   main-check verdict <json>      always the last line
#
# Sourced by scripts/main-check.sh. Tested by scripts/lib/main_check_test.sh.

# shellcheck source=scripts/lib/e2e_flaky.sh
source "$(dirname "${BASH_SOURCE[0]}")/e2e_flaky.sh"
# shellcheck source=scripts/lib/e2e_github.sh
source "$(dirname "${BASH_SOURCE[0]}")/e2e_github.sh"

MAIN_CHECK_RUN_PREFIX="main-check run "
MAIN_CHECK_VERDICT_PREFIX="main-check verdict "
# How much failure output the verdict carries. The full log stays on disk.
MAIN_CHECK_TAIL_LINES_PER_PHASE=25
MAIN_CHECK_TAIL_MAX_CHARS=4000
# What the verdict says of each failed test. Names are never cut. The error
# heads share one budget, so a red with many failures still fits a prompt.
MAIN_CHECK_NAME_MAX_CHARS=200
MAIN_CHECK_DETAIL_LINES=16
MAIN_CHECK_DETAIL_BUDGET_CHARS=8000

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
# run line the moment the driver names its run. It ignores SIGTERM and SIGUSR2
# and reads until the driver closes its output: engine shutdown signals the
# whole group, and the driver's cleanup still writes to this pipe.
main_check_follow() {
    local log="$1" line
    trap '' TERM USR2
    while IFS= read -r line; do
        printf '%s\n' "$line" >>"$log"
        case "$line" in
            "$E2E_GITHUB_LINKED_MARK"*) printf '%s%s\n' "$MAIN_CHECK_RUN_PREFIX" "${line#"$E2E_GITHUB_LINKED_MARK"}" ;;
        esac
    done
}

# main_check_verdict_line <verdict> <driver-exit> <log>: the last line of
# main-check.sh. The run URL, summary and tail come from the log.
main_check_verdict_line() {
    python3 - "$1" "$2" "$3" "$E2E_GITHUB_LINKED_MARK" \
        "$MAIN_CHECK_TAIL_LINES_PER_PHASE" "$MAIN_CHECK_TAIL_MAX_CHARS" "$MAIN_CHECK_VERDICT_PREFIX" \
        "$MAIN_CHECK_NAME_MAX_CHARS" "$MAIN_CHECK_DETAIL_LINES" "$MAIN_CHECK_DETAIL_BUDGET_CHARS" \
        "$E2E_FLAKY_ENGINE_MARK" "$E2E_FLAKY_END_MARK" <<'PY'
import json, re, sys

verdict, rc, log_path, linked, per_phase, max_chars, prefix, name_max, detail_lines, detail_budget, flaky_mark, flaky_end = sys.argv[1:13]
per_phase, max_chars = int(per_phase), int(max_chars)
name_max, detail_lines, detail_budget = int(name_max), int(detail_lines), int(detail_budget)
ANSI = re.compile(r"\x1b\[[0-9;]*m")
try:
    lines = open(log_path, errors="replace").read().splitlines()
except OSError:
    lines = []

run_url = None
for line in lines:
    if line.startswith(linked):
        run_url = (line[len(linked):].split() or [None])[0]

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

def block_head(rows):
    """The first lines of an error block, without its blank lines."""
    kept = [r.strip() for r in rows if r.strip()]
    return "\n".join(kept[:detail_lines])

def cargo_blocks(body, first):
    """The panic block of each test in a `cargo test` section, by test name.
    A test run twice keeps its first block when `first`, else its last."""
    blocks, current = {}, None
    for line in body:
        m = re.fullmatch(r"---- (\S+) stdout ----", line.strip())
        if m:
            current = None if first and m.group(1) in blocks else blocks.setdefault(m.group(1), [])
            if not first:
                current[:] = []
        elif line.strip() == "failures:" or line.startswith("test result:"):
            current = None
        elif current is not None:
            current.append(line)
    return blocks

def cargo_failures(body):
    """Failed tests of a `cargo test` section, with each one's panic head."""
    names = []
    for i, line in enumerate(body):
        if line.strip() == "failures:":
            names = []
            for row in body[i + 1:]:
                m = re.fullmatch(r"    (\S+)", row)
                if not m:
                    if names or row.strip():
                        break
                    continue
                names.append(m.group(1))
    blocks = cargo_blocks(body, first=False)
    return [(name, block_head(blocks.get(name, []))) for name in names]

def engine_flaky(body):
    """Tests a shard's rerun passed, each with its first run's panic head."""
    names, listing = [], False
    for line in body:
        if line.strip() == flaky_mark:
            listing = True
        elif listing and line.strip() == flaky_end:
            listing = False
        elif listing and line.strip():
            names.append(line.strip())
    blocks = cargo_blocks(body, first=True)
    return [(name, block_head(blocks.get(name, []))) for name in names]

PW_HEADER = re.compile(r"  \d+\) (\[.+)")
PW_SUMMARY = re.compile(r"  (\d+) (failed|flaky|skipped|did not run|interrupted)")
PW_BLOCK_END = re.compile(r"\s+(attachment #|Error Context:|─{10,})")

def playwright_tests(body, group_name, last_attempt):
    """Tests a Playwright section lists under "N <group_name>", each with the
    error head of its last attempt, or of its first when not `last_attempt`."""
    listed, group = [], None
    for line in body:
        m = PW_SUMMARY.fullmatch(line.rstrip())
        if m:
            group = m.group(2)
        elif group == group_name and re.match(r"    \[", line):
            listed.append(line.strip())
        elif group and not line.startswith("    "):
            group = None
    blocks, current = {}, None
    for line in body:
        m = PW_HEADER.fullmatch(line.rstrip())
        if m:
            current = blocks[m.group(1).strip()] = []
        elif PW_SUMMARY.fullmatch(line.rstrip()):
            current = None
        elif current is not None:
            current.append(line)
    out = []
    for name in dict.fromkeys(listed):
        attempts = [[]]
        for line in blocks.get(name, []):
            if re.match(r"\s+Retry #\d+ ─", line):
                attempts.append([])
            else:
                attempts[-1].append(line)
        text = []
        for line in attempts[-1 if last_attempt else 0]:
            if PW_BLOCK_END.match(line):
                break
            text.append(line)
        out.append((re.sub(r"^\[[^\]]+\] › ", "", name), block_head(text)))
    return out

def shrink(found):
    """Entries as the verdict carries them: every name, the details within one budget."""
    cap = detail_budget // max(len(found), 1)
    return [
        {"suite": phase, "name": test[:name_max], "detail": detail[:cap]}
        for phase, test, detail in found
    ]

def phase_of(title):
    return title.replace("browser: ", "browser ")

def failed_tests(sections, failed_phases):
    found = []
    for name, body in sections:
        phase = phase_of(name)
        if phase not in failed_phases:
            continue
        body = [ANSI.sub("", row) for row in body]
        parse = (lambda b: playwright_tests(b, "failed", True)) if phase.startswith("browser ") else cargo_failures
        found += [(phase, test, detail) for test, detail in parse(body)]
    return shrink(found)

def flaky_tests(sections):
    found = []
    for name, body in sections:
        phase = phase_of(name)
        body = [ANSI.sub("", row) for row in body]
        parse = (lambda b: playwright_tests(b, "flaky", False)) if phase.startswith("browser ") else engine_flaky
        found += [(phase, test, detail) for test, detail in parse(body)]
    return shrink(found)

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

failures = failed_tests(sections, failed) if verdict == "fail" else []
flaky = flaky_tests(sections) if verdict != "no-verdict" else []
tail = "\n".join(tail_rows)
if len(tail) > max_chars:
    tail = tail[-max_chars:]
print(prefix + json.dumps({
    "verdict": verdict,
    "run_url": run_url,
    "summary": summary,
    "tail": tail,
    "failures": failures,
    "flaky": flaky,
    "log": log_path,
}))
PY
}
