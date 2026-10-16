#!/usr/bin/env bash
#
# check-audit-scans.sh: fail if a workspace-audit scan pattern misses a line it
# must catch, or catches a line it must not.
#
#   ./scripts/check-audit-scans.sh
#
# Run by `/harden` Phase 4.5 for EVERY diff. It is whole-tree and costs
# milliseconds, like check-knowhow-refs.sh beside it.
#
# Each audit section under system-knowhow/workspace-audit/ carries `scan` lines
# in its `## Scan` block. The audit runs them through the root's `scan()`, which
# hands the pattern to `grep -rnE`. This script evaluates the same lines with a
# stub `scan()` that keeps the pattern, so it tests the exact string the audit
# greps with, bash escapes and all. Then it runs each FIXTURES row through
# `grep -E`.
#
# The scar: the db-defaults psql arm needed whitespace after the command name.
# An argv list puts a quote there (`["psql","-A",...]`), so a real audit missed
# two broken callers with zero hits.
#
# Rules it enforces:
#
#   - a `hit` row matches its scan's pattern, and a `miss` row does not
#   - every scan has at least one `hit` row
#   - every row names a scan that exists
#
# Exit status: 0 clean, 1 a rule broke OR the check could not run. A gate that
# cannot run must never read as clean.
#
# Targets bash 3.2, the macOS system shell.

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
    echo "ERROR: not inside a git checkout, so the audit-scan check cannot run." >&2
    exit 1
fi
cd "$REPO_ROOT" || exit 1

# One row per line: <scan id> <hit|miss> <line the pattern sees>.
FIXTURES=$(
    cat << 'EOF'
storage hit localStorage.setItem("k", v)
host-realm hit window.parent.postMessage(msg, "*")
engine-fetch hit fetch("/api/v1/threads")
engine-fetch hit const u = new URL(`api/v1/threads`, base)
engine-fetch hit img.src = apiUrl("files/a.png")
relative-fetch hit fetch('data/items.json')
relative-fetch hit fetch( "./config.json")
relative-fetch miss fetch("/api/v1/threads")
download-link hit <a href="report.csv" download>
media-capture hit navigator.mediaDevices.getUserMedia({ audio: true })
web-share hit navigator.share({ url })
url-mutation hit history.pushState({}, "", "#a")
url-mutation hit location.href = "/x"
url-mutation hit window.location = url
theme-rename hit <html data-theme="dark">
theme-rename hit lucidos.preferences.get('look')
theme-rename miss <html data-theme-mode="dark">
dashboard-rename hit html[data-widget-place="home"] .clock {
dashboard-rename hit html[data-widget-place=home] .clock {
dashboard-rename hit if (lucidos.widget.place() === 'home') {
dashboard-rename hit const tile = document.documentElement.dataset.widgetPlace == "home"
dashboard-rename hit lucidos.ui.navigate('home-board')
dashboard-rename miss html[data-widget-place="dashboard"] .clock {
dashboard-rename miss if (lucidos.widget.place() === 'inline') {
hand-rolled-ui hit alert("Saved")
hand-rolled-ui hit <div class="toast">Saved</div>
hand-rolled-ui hit <select name="unit">
hand-rolled-ui hit <button title="Close">
hand-rolled-ui hit .spinner {
hand-rolled-ui miss lucidos.ui.alert("Saved")
hand-rolled-ui miss <p subtitle="x">
attr-escape hit html += `<a title="${escapeHtml(name)}">`
ready-signal hit "reveal": "ready",
ready-signal hit lucidos.ui.ready()
app-icon hit "icon": "icon.svg",
widget-tile hit "sizes": ["1x1", "2x1"],
touch-hover hit .card:hover {
state-as-action hit btn.classList.toggle("action-btn-danger")
state-as-action hit <button class="action-btn-secondary">
tap-strings hit "tap": "modal",
tap-strings hit tap: 'none',
tap-strings hit tap: "open_app",
tap-strings hit 'tap': 'open_thread',
tap-strings hit tap: { kind: 'none' },
tap-strings hit tap: { kind: "none" },
tap-strings miss tap: { kind: 'modal' },
tap-strings hit curl -d "{\"tap\": \"modal\"}" "$url"
tap-strings hit body = "{\"tap\":\"none\"}"
tap-strings hit lucidos notify --title T --message M --tap none
tap-strings hit args += ["--tap", "open-thread"]
tap-strings miss lucidos notify --title T --message M --tap navigate
removed-flags hit lucidos spawn-thread --parent "$id" "do it"
removed-flags hit run_coding_agent(prompt=p, repo="x")
removed-flags hit lucidos threads list --has-diff
removed-flags hit subprocess.run(["lucidos", "threads", "count", "--has-diff"])
removed-flags hit body = "[\"lucidos\", \"threads\", \"list\", \"--has-diff\"]"
removed-flags miss lucidos threads list --parent "$id"
removed-fields hit if (t.coding_agent_has_diff) {
cred-env hit token = os.environ["CRED_GITHUB_TOKEN"]
auth-header hit headers: { Authorization: "Bearer " + t }
machine-path hit open("/Users/me/notes.txt")
machine-path miss open(os.path.expanduser("~/notes.txt"))
db-defaults hit out = subprocess.run(["psql","-A","-t","-c",sql])
db-defaults hit cmd = ["psql", "-X", "-At", "-c", sql]
db-defaults hit psql "$LUCIDOS_DATABASE_URL" -At -c "select 1"
db-defaults hit run('psql', args)
db-defaults hit rows=`psql -At -c "$q"`
db-defaults hit execFileSync(`psql`, args)
db-defaults hit argv = [psql, "-c", sql]
db-defaults hit pg_dump --dbname="$url" > out.sql
db-defaults hit subprocess.run(["pg_dump", "-Fc"])
db-defaults hit def psql(sql):
db-defaults hit function psql(sql) {
db-defaults hit conn = psycopg2.connect("")
db-defaults hit const c = new pg.Client()
db-defaults hit host = os.environ["PGHOST"]
db-defaults hit url = os.environ["DATABASE_URL"]
db-defaults miss python3 psql.py
db-defaults miss psql-wrapper -c "select 1"
db-defaults miss psql_foo(sql)
db-defaults miss pg_dump_helper(db)
db-defaults miss url = os.environ["LUCIDOS_DATABASE_URL"]
EOF
)

SECTIONS=(system-knowhow/workspace-audit/*.md)
if [ ! -f "${SECTIONS[0]}" ]; then
    echo "ERROR: no audit sections under system-knowhow/workspace-audit/, so the audit-scan check cannot run." >&2
    exit 1
fi

WORK="$(mktemp -d)" || exit 1
trap 'rm -rf "$WORK"' EXIT
mkdir "$WORK/patterns"

# The real scan() greps; this one keeps the pattern under its id. The audit's
# own variables are set so `set -u` passes, and globbing is off so an unquoted
# `--include=*.css` stays a word, as it does in the audit.
# shellcheck disable=SC2034 # read by the evaluated scan lines
ex="" inc="" all=""
scan() {
    if [ -e "$WORK/patterns/$1" ]; then
        echo "FAIL: scan id '$1' is defined twice." >&2
        BROKEN=1
    fi
    printf '%s' "$2" > "$WORK/patterns/$1"
}
BROKEN=0
SCAN_LINES=$(awk '/^```bash/ { fence = 1; next } /^```/ { fence = 0 } fence && /^scan / { print }' "${SECTIONS[@]}")
set -f
while IFS= read -r line; do
    if ! eval "$line"; then
        echo "FAIL: a scan line does not parse, so the audit loses it: $line"
        BROKEN=1
    fi
done <<< "$SCAN_LINES"
set +f

SCANS=$(cd "$WORK/patterns" && ls)
if [ -z "$SCANS" ]; then
    echo "ERROR: no scan lines found in the audit sections, so the audit-scan check cannot run." >&2
    exit 1
fi

FAILED=$BROKEN
ROWS=0
while read -r id want text; do
    ROWS=$((ROWS + 1))
    if [ ! -f "$WORK/patterns/$id" ]; then
        echo "FAIL: a fixture names scan '$id', which no audit section defines."
        FAILED=1
        continue
    fi
    printf '%s\n' "$text" | grep -qE -e "$(cat "$WORK/patterns/$id")"
    rc=$?
    if [ "$rc" -gt 1 ]; then
        echo "ERROR: grep rejected the '$id' pattern, so the audit-scan check cannot run." >&2
        exit 1
    fi
    got=miss
    [ "$rc" -eq 0 ] && got=hit
    if [ "$got" != "$want" ]; then
        echo "FAIL: $id should $want, got $got: $text"
        FAILED=1
    fi
done <<< "$FIXTURES"

for id in $SCANS; do
    if ! printf '%s\n' "$FIXTURES" | grep -q "^$id hit "; then
        echo "FAIL: scan '$id' has no hit fixture. Add one to FIXTURES in $0."
        FAILED=1
    fi
done

if [ "$FAILED" -ne 0 ]; then
    exit 1
fi
echo "OK: $ROWS fixtures across $(printf '%s\n' "$SCANS" | grep -c .) audit scans behave."
