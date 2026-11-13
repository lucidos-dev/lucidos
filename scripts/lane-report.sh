#!/usr/bin/env bash
# lane-report.sh: measure whether the landing lane pays off, from the events
# store alone (docs/plans/2026-10-12-landing-lane.md, Verification).
#
# Read-only: every statement is a SELECT. psql takes the connection from
# LUCIDOS_DATABASE_URL, the read-only handle the engine sets in every agent
# subprocess (ADR 0481), so no URL appears in argv.
#
#   ./scripts/lane-report.sh            # the last 21 days
#   ./scripts/lane-report.sh --days 7
set -euo pipefail

DAYS=21
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

WINDOW="now() - interval '$DAYS days'"

section() { printf '\n== %s (last %s days)\n' "$1" "$DAYS"; }
if [ -z "${LUCIDOS_DATABASE_URL:-}" ]; then
    echo "ERROR: LUCIDOS_DATABASE_URL is not set. Run this from a Lucidos agent shell." >&2
    exit 2
fi

query() { psql "$LUCIDOS_DATABASE_URL" -X -q -P pager=off -v ON_ERROR_STOP=1 -c "$1"; }

section "Lane batches: time from open to landed or failed"
query "
with s as (select aggregate_id id, min(created) t0, max(jsonb_array_length(payload->'data'->'members')) members
           from events where event_type = 'LaneBatchStarted' and created > $WINDOW group by 1),
     e as (select aggregate_id id, event_type outcome, min(created) t1
           from events where event_type in ('LaneBatchLanded', 'LaneBatchFailed') and created > $WINDOW
           group by 1, 2)
select coalesce(e.outcome, 'open') outcome, count(*) batches, sum(s.members) members,
       round(percentile_cont(0.5) within group (order by extract(epoch from e.t1 - s.t0) / 60)::numeric, 1) median_min,
       round(percentile_cont(0.9) within group (order by extract(epoch from e.t1 - s.t0) / 60)::numeric, 1) p90_min,
       round((sum(extract(epoch from e.t1 - s.t0)) / 3600)::numeric, 1) hours
from s left join e using (id) group by 1 order by 1;"

section "Per-change conflict resolutions outside the lane (app changes, and history)"
query "
with d as (select payload->>'change_id' cid, min(created) t0 from events
           where event_type = 'MergeConflictDetected' and created > $WINDOW group by 1),
     r as (select d.cid, d.t0,
                  (select min(x.created) from events x
                   where x.event_type in ('ChangeApplied', 'ChangeApplyFailed', 'MergeResolutionCleared')
                     and x.payload->>'change_id' = d.cid and x.created > d.t0) t1 from d)
select count(*) changes,
       round(percentile_cont(0.5) within group (order by extract(epoch from t1 - t0) / 60)::numeric, 1) median_min,
       round((sum(least(extract(epoch from t1 - t0), 3 * 3600)) / 3600)::numeric, 1) hours_capped_3h
from r where t1 is not null;"

section "Failures because main moved (target: 0)"
query "
select count(*) failures from events
where event_type = 'ChangeApplyFailed' and created > $WINDOW
  and payload->>'error' like '%concurrent main%';"

section "Queued to landed, per change (a clean change should take seconds)"
query "
with q as (select payload->>'change_id' cid, min(created) t0 from events
           where event_type = 'ChangeQueued' and created > $WINDOW group by 1),
     a as (select payload->>'change_id' cid, min(created) t1 from events
           where event_type = 'ChangeApplied' and created > $WINDOW group by 1)
select count(*) landed,
       round(percentile_cont(0.5) within group (order by extract(epoch from a.t1 - q.t0))::numeric, 1) median_s,
       round(percentile_cont(0.9) within group (order by extract(epoch from a.t1 - q.t0))::numeric, 1) p90_s
from q join a using (cid) where a.t1 >= q.t0;"

section "Main checks"
query "
select event_type, count(*) from events
where aggregate = 'main_check' and created > $WINDOW group by 1 order by 1;"

section "/harden runs per day: peak concurrent, count, median minutes"
query "
with q as (select thread_id, created t from events
           where event_type = 'CodingAgentToolCalled' and created > $WINDOW
             and payload::text like '%hardened query%'),
     m as (select thread_id, created t from events
           where event_type = 'CodingAgentToolCalled' and created > $WINDOW
             and payload::text like '%hardened mark%'),
     p as (select m.t t1, (select max(q.t) from q where q.thread_id = m.thread_id
                           and q.t < m.t and q.t > m.t - interval '4 hours') t0 from m),
     g as (select to_char(h, 'MM-DD') d, count(*) n
           from generate_series($WINDOW, now(), interval '5 min') h
           join p on p.t0 <= h and p.t1 > h group by h),
     pk as (select d, max(n) peak from g group by d),
     st as (select to_char(t1, 'MM-DD') d, count(*) runs,
                   round(percentile_cont(0.5) within group (order by extract(epoch from t1 - t0) / 60)::numeric) median_min
            from p where t0 is not null group by 1)
select st.d as day, coalesce(pk.peak, 0) peak_concurrent, st.runs, st.median_min
from st left join pk using (d) order by 1;"

section "Agent-side merge conflicts (an agent's own git merge)"
query "
select count(*) failed_merges from events
where event_type = 'CodingAgentToolResult' and created > $WINDOW
  and payload->>'result' like '%Automatic merge failed%';"
