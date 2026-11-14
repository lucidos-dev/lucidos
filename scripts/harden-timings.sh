#!/usr/bin/env bash
# harden-timings.sh: how long a /harden round takes, and where the time goes
# (docs/plans/2026-11-14-harden-in-minutes.md, Verification).
#
# Read-only. Suite timings come from HardenRunCompleted events, lane and main
# check times from their events, and GitHub build times from this host's e2e
# timings log. psql reads LUCIDOS_DATABASE_URL, so no URL appears in argv.
#
#   ./scripts/harden-timings.sh            # the last 7 days
#   ./scripts/harden-timings.sh --days 30
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=lib/e2e_github.sh
source "$SCRIPT_DIR/lib/e2e_github.sh"

DAYS=7
while [ $# -gt 0 ]; do
    case "$1" in
        --days)
            DAYS="${2:-}"
            shift 2
            ;;
        -h | --help)
            sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "ERROR: unknown argument '$1'. Usage: $0 [--days N]" >&2
            exit 2
            ;;
    esac
done
case "$DAYS" in
    '' | *[!0-9]*)
        echo "ERROR: --days takes a whole number of days, got '$DAYS'." >&2
        exit 2
        ;;
esac
if [ -z "${LUCIDOS_DATABASE_URL:-}" ]; then
    echo "ERROR: LUCIDOS_DATABASE_URL is not set. Run this from a Lucidos agent shell." >&2
    exit 2
fi

WINDOW="now() - interval '$DAYS days'"

section() { printf '\n== %s (last %s days)\n' "$1" "$DAYS"; }
query() { psql "$LUCIDOS_DATABASE_URL" -X -q -P pager=off -v ON_ERROR_STOP=1 -c "$1"; }

# One row per suite per round. A handoff re-emits a round's evidence, so rounds
# are de-duplicated on thread, head and the suite record itself. An exit of 72
# (memory gate VOID) or 75 (busy e2e slot SKIP) is not a failure.
SUITES="
with r as (
    select distinct on (payload->>'thread_id', payload->>'head_sha', payload->'suites'->'suites')
           md5(concat(payload->>'thread_id', payload->>'head_sha', payload->'suites'->>'suites')) id,
           payload->'suites'->>'round' round, payload->'suites'->'suites' s
    from events
    where event_type = 'HardenRunCompleted' and created > $WINDOW
      and jsonb_typeof(payload->'suites'->'suites') = 'object'
    order by payload->>'thread_id', payload->>'head_sha', payload->'suites'->'suites', created),
x as (
    select r.id, r.round, j.k suite,
           (j.v->>'queued')::bigint q, (j.v->>'start')::bigint st, (j.v->>'end')::bigint en,
           (j.v->>'exit') not in ('0', '72', '75') failed
    from r, jsonb_each(r.s) j(k, v)
    where j.v->>'end' is not null and j.v->>'exit' <> '')"

# pct <fraction> <seconds expression>: a percentile in minutes, one decimal.
pct() { printf "round((percentile_cont(%s) within group (order by (%s) / 60.0))::numeric, 1)" "$1" "$2"; }

section "Round span by shape, full rounds (first queued to last end, minutes)"
query "$SUITES,
rounds as (
    select id,
           case when bool_or(suite like 'e2e-touched%') then 'browser leg'
                when bool_or(suite in ('engine', 'engine-touched', 'engine-filtered', 'lint', 'lint-rust',
                                       'eval', 'cli', 'crates', 'confinement')) then 'cargo lane'
                else 'no cargo lane' end shape,
           max(en) - min(q) span, bool_or(failed) failed,
           max(en) - min(en) filter (where failed) fail_to_verdict
    from x where round = 'full' group by id)
select shape, count(*) rounds, $(pct 0.5 span) p50, $(pct 0.9 span) p90,
       count(*) filter (where failed) failing,
       round((avg(fail_to_verdict) / 60.0)::numeric, 1) first_fail_to_verdict_avg
from rounds group by shape order by p50 desc;"

section "Per suite, full rounds (minutes)"
query "$SUITES
select suite, count(*) runs, count(*) filter (where failed) failed,
       $(pct 0.5 'en - st') run_p50, $(pct 0.9 'en - st') run_p90,
       $(pct 0.5 'st - q') wait_p50, $(pct 0.9 'st - q') wait_p90
from x where round = 'full' group by suite order by run_p50 desc;"

section "Main check: start to verdict (minutes)"
query "
with s as (select payload->'data'->>'check_id' id, min(created) t0 from events
           where event_type = 'MainCheckStarted' and created > $WINDOW group by 1),
     v as (select payload->'data'->>'check_id' id, event_type outcome, min(created) t1 from events
           where event_type in ('MainCheckPassed', 'MainCheckFailed') and created > $WINDOW group by 1, 2)
select v.outcome, count(*) checks,
       $(pct 0.5 'extract(epoch from v.t1 - s.t0)') p50, $(pct 0.9 'extract(epoch from v.t1 - s.t0)') p90
from s join v using (id) group by 1 order by 1;"

section "Lane batches: harden rounds, and first passing round to landing (minutes)"
query "
with b as (select aggregate_id id, min(created) t0 from events
           where event_type = 'LaneBatchStarted' and created > $WINDOW group by 1),
     e as (select aggregate_id id, event_type outcome, min(created) t1 from events
           where event_type in ('LaneBatchLanded', 'LaneBatchFailed') and created > $WINDOW group by 1, 2),
     h as (select b.id,
                  count(distinct h.payload->>'head_sha') rounds,
                  min(h.created) filter (where h.payload->>'verdict' = 'pass') first_pass
           from b join events h on h.event_type = 'HardenRunCompleted'
                               and h.payload->>'branch' = 'lane/' || b.id and h.created > b.t0
           group by b.id)
select coalesce(e.outcome, 'open') outcome, count(*) batches,
       round(avg(coalesce(h.rounds, 0)), 1) rounds_avg,
       $(pct 0.5 'extract(epoch from e.t1 - h.first_pass)') pass_to_land_p50,
       $(pct 0.9 'extract(epoch from e.t1 - h.first_pass)') pass_to_land_p90
from b left join e using (id) left join h using (id) group by 1 order by 1;"

section "GitHub build jobs on this host (minutes, from $(e2e_github_timings_log))"
python3 -I - "$(e2e_github_timings_log)" "$DAYS" <<'PY'
import json, statistics, sys, time
from datetime import datetime

log, days = sys.argv[1], int(sys.argv[2])
since = time.time() - days * 86400
runs = {}
try:
    with open(log) as f:
        for line in f:
            try:
                rec = json.loads(line)
                pushed = datetime.fromisoformat(rec["pushed_at"]).timestamp()
            except (ValueError, KeyError, TypeError):
                continue
            if pushed < since:
                continue
            for job in rec.get("jobs", []):
                if job.get("name", "").startswith("build-") and job.get("conclusion") == "success":
                    runs.setdefault(job["name"], []).append(job["run_secs"] / 60)
except FileNotFoundError:
    print("no timings log on this host")
    sys.exit(0)


def p90(xs):
    return sorted(xs)[int(0.9 * (len(xs) - 1))]


print(f"{'job':<14} {'builds':>6} {'p50':>6} {'p90':>6}")
for name, xs in sorted(runs.items()):
    print(f"{name:<14} {len(xs):>6} {statistics.median(xs):>6.1f} {p90(xs):>6.1f}")
PY
