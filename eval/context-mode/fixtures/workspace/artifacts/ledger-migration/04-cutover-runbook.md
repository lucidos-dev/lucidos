# Ledger cutover runbook

**Revision 6** | **Effective 2026-07-28** | **Owner: release captain**

This is the document the release captain reads on the night. It replaces
revision 5, which said "wait for the pipeline to go green". Dry run 4 showed
what that sentence is worth, so revision 6 gates on named checks and their
output instead.

| Field | Value |
|---|---|
| Scheduled | 2026-08-29, starting 01:00 UTC |
| Source | `tally`, PostgreSQL 11.22 |
| Target | `ledger`, PostgreSQL 16.3, 16 shards |
| Vendor | Kestrel Clearing, replay API v3.2.4 |
| Expected total | 3h10m from freeze to full service |
| Decision point | T+45m, the write release gate |
| Rollback deadline | T+2h30m |

## 1. The one thing to know

**The cutover holds writes for 45 minutes. Writes resume only after the
balance parity check and the sequence-gap check both pass.**

Nothing else in this document changes that sentence. There is no partial
release, no release on one check, and no release on a timer. If the hold has
to run longer than 45 minutes, it runs longer and the comms template in
section 12 covers it.

Reads stay available throughout. Only writes are held.

## 2. Scope

### 2.1 In scope

- Freezing writes on `tally`.
- Draining outstanding vendor records through the replay API.
- Promoting `ledger` to the write path.
- Verifying parity and sequence integrity before writes resume.
- Rolling back to `tally` if either check fails.

### 2.2 Out of scope

- The backfill. It runs in the week before the cutover and is finished before
  T-0. A cutover that starts with an unfinished backfill is aborted at the
  preconditions, not managed in flight.
- Decommissioning `tally`. That is a separate change, no earlier than 30 days
  after a successful cutover.
- The reporting pipeline, which reads a nightly export and is unaffected until
  the following morning.

## 3. Roles

Five roles. One person per role, and the release captain does not hold a
second role.

| Role | Owns |
|---|---|
| Release captain | The decision to proceed, hold, or roll back |
| Ledger operator | Runs every command in section 8 |
| Database operator | Postgres on both sides, replication, promotion |
| Verifier | Runs the checks in section 9 and reads their output aloud |
| Comms | Status page, the internal channel, partner mail |

The verifier is deliberately not the ledger operator. The person who ran the
migration should not be the only person reading whether it worked.

## 4. Timeline

All times relative to T-0, which is the write freeze.

| Time | Phase | Owner |
|---|---|---|
| T-90m | Preconditions checked, go or no-go call | Release captain |
| T-60m | Final incremental backfill starts | Ledger operator |
| T-20m | Partner notice sent, status page set to scheduled | Comms |
| T-10m | Final incremental backfill confirmed complete | Ledger operator |
| T-5m | Connection pools drained to read only on `tally` | Database operator |
| **T-0** | **Write freeze begins** | Database operator |
| T+2m | Freeze confirmed, zero writes observed for 60 seconds | Verifier |
| T+3m | Vendor replay drain starts | Ledger operator |
| T+16m | Drain complete, gap closed | Ledger operator |
| T+18m | `ledger` promoted to primary write target | Database operator |
| T+20m | Balance parity check starts | Verifier |
| T+26m | Sequence-gap check starts | Verifier |
| T+34m | Both checks complete | Verifier |
| T+40m | Results read aloud, release decision | Release captain |
| **T+45m** | **Writes released, or the hold continues** | Database operator |
| T+50m | First write confirmed on `ledger` | Verifier |
| T+65m | Smoke suite complete | Verifier |
| T+90m | Status page cleared | Comms |
| T+2h30m | Rollback deadline passes | Release captain |
| T+3h10m | Full service declared | Release captain |

The checks finish at T+34m and the release is at T+45m. The eleven minutes in
between are not slack for the checks. They are there so the release captain
can read the output, ask questions, and decide without a clock pressing.

### 4.1 Why 45 and not less

Three numbers add up to it, measured on dry runs 5 and 6.

| Piece | Measured | Budgeted |
|---|---|---|
| Vendor replay drain | 12m41s | 16m |
| Promotion | 1m18s | 2m |
| Balance parity check | 5m14s | 8m |
| Sequence-gap check | 5m37s | 8m |
| Decision | not measurable | 11m |

The two checks are 11 minutes of real work and 16 minutes of budget. The drain
is the largest single piece and it is the one that varies, because it depends
on how much the vendor has to replay.

### 4.2 Why not less than the checks take

The obvious saving is to release writes while the checks run, and to roll back
if a check fails. That was rejected.

Once writes land on `ledger`, a rollback has to replay them into `tally`. That
turns a clean rollback into a merge, at the worst possible moment. The hold
exists so that rollback stays a switch rather than a reconciliation.

## 5. Preconditions

Checked at T-90m. Every line is a hard gate. A single unchecked line is a
no-go, and there is no discretion here.

### 5.1 Migration state

- [ ] Two consecutive dry runs passed clean, the most recent within 14 days.
- [ ] The backfill on the production shadow is complete and its per-shard
      deltas are all zero.
- [ ] `seq-check` passed on the production shadow, 0 violations and 0 gaps.
- [ ] `entries_shard_seq_uniq` exists on every shard. Confirmed by query, not
      by pipeline result.
- [ ] No migration is pending. `ledger-migrate status` reports `up to date`.

### 5.2 Vendor state

- [ ] Certification suite passed against the vendor's certification host
      within the last 7 days.
- [ ] Production credentials tested against a read-only endpoint today.
- [ ] The gap we intend to drain is inside the vendor's documented replay
      window. Check the figure in the vendor specification, do not assume it.
- [ ] No vendor maintenance is scheduled inside the cutover window.

### 5.3 Infrastructure

- [ ] Target cluster has at least 40% free disk on every node.
- [ ] Replication lag on `tally-ro-03` under 5 seconds.
- [ ] No checkpoint tuning changed on the target in the last 48 hours.
- [ ] Both clusters have a fresh base backup, taken today.
- [ ] Rollback DNS records exist with a 60 second TTL, already lowered.

### 5.4 People

- [ ] All five roles are filled and each person has confirmed by voice.
- [ ] Release captain has not been on call in the preceding 12 hours.
- [ ] The escalation path in appendix D has been read aloud.

## 6. The go or no-go call

At T-90m the release captain reads section 5 aloud, line by line, and each
owner answers `checked` or `not checked`. There is no `mostly`.

A no-go costs a rescheduled night. A go on an unchecked line costs a rollback
at three in the morning, or worse, a divergence discovered a week later. The
asymmetry is the whole reason this call exists.

Once the call is `go`, the release captain says so on the channel and the
timeline in section 4 starts.

## 7. Freeze

### 7.1 Draining the pools

At T-5m the database operator moves `tally`'s connection pools to read only.
This is a PgBouncer configuration reload, not a restart.

```
$ pgbouncer-admin -h tally-pool-01 -c "SET default_pool_mode = 'transaction'"
$ pgbouncer-admin -h tally-pool-01 -c "RELOAD"
$ ledger-freeze prepare --source tally --confirm
freeze: 14 write pools identified
freeze: 3 long transactions running, longest 4.2s
freeze: prepared, not yet applied
```

Long transactions matter. A transaction that is still open at T-0 holds a
write that the drain will not see. Anything over 30 seconds is killed here
rather than at the freeze.

### 7.2 The freeze itself

At T-0 exactly.

```
$ ledger-freeze apply --source tally --confirm
freeze: applied at 2026-08-29T01:00:00Z
freeze: 14 pools now reject writes
freeze: last committed transaction xid 4188920117
freeze: high water mark recorded
```

Record the high water mark on the channel. Every later step refers to it, and
the drain uses it as its starting point.

### 7.3 Confirming the freeze

The verifier watches for 60 seconds and confirms zero writes.

```
$ watch -n5 'psql -h tally-ro-03 -Atc "SELECT count(*) FROM entries WHERE created_at > now() - interval '"'"'1 minute'"'"'"'
0
0
0
0
```

Four consecutive zeros is the bar. A non-zero count means a write path we do
not know about, and that is an abort, not a puzzle to solve in the window.

## 8. Drain and promote

### 8.1 The vendor replay drain

The drain pulls every record the vendor accepted after our high water mark and
writes it into `ledger`. It starts at T+3m.

```
$ ledger-migrate drain --vendor kestrel --env production \
    --from-hwm 4188920117 --confirm
drain: target host api.kestrel-clearing.example
drain: obtaining token, scope replay.read replay.write
drain: token acquired
drain: gap starts at the tally high water mark
drain: requesting a replay session for the gap
drain: session accepted, id rps_01J9F2K8Q4
drain: batching within the documented cap
```

Two things to watch.

**The session must be accepted.** If the vendor rejects the session because
the gap is older than its replay window, stop. That is an abort and the
rollback in section 11 is the answer. The window is documented in the vendor
specification, and the precondition at 5.2 exists so this never happens live.

**The batch size must be inside the vendor's cap.** The tool reads the cap
from its configuration and will refuse a larger value. Do not override it to
speed the drain up. A rejected batch costs more than a smaller one.

Expected output at the end:

```
drain: 47 batches acknowledged
drain: 11284 records applied
drain: gap closed, 0 records outstanding
drain: elapsed 12m41s
```

`0 records outstanding` is the gate. A non-zero figure means the drain did not
finish, and promotion must not start.

### 8.2 Promotion

At T+18m, after the drain reports the gap closed.

```
$ ledger-promote --target ledger --confirm
promote: 16 shards healthy
promote: write path switched to ledger
promote: tally now read only and will stay so
promote: elapsed 1m18s
```

Promotion switches the write path but does not open it. Writes are still held
by the freeze. This is the step people misread, so it is worth saying plainly:
**promotion is not release.**

## 9. The two checks

Both checks run against production data with writes still held. That is the
point of the hold: the two sides cannot move while they are compared.

### 9.1 Balance parity check

```
$ ledger-parity compare --source tally --target ledger --confirm
parity: loading exclusions from parity_exclusions
parity: 412 accounts excluded, all suspense and clearing
parity: comparing balances as of 2026-08-29T01:20:04Z
parity: 41882306 accounts in scope
parity: shard 00   2618442 accounts        0 divergent
parity: shard 01   2612118 accounts        0 divergent
...
parity: shard 15   2617009 accounts        0 divergent
parity: 41882306 compared, 0 divergent
parity: ok (5m14s)
```

**Pass condition: `0 divergent`.** Not "a small number". Not "only suspense
accounts". Zero, on every shard, with the exclusion list unchanged from the
one committed in the repository.

The verifier reads the last two lines aloud. If any shard is non-zero, the
verifier says the shard number and the count, and the release captain moves to
section 11.

### 9.2 Sequence-gap check

```
$ ledger-migrate seq-check --target ledger --confirm
seq-check: verifying entries_shard_seq_uniq on 16 shards
seq-check: index present on all shards, 0 violations
seq-check: gap scan over 16 shards
seq-check: seq_gap_view reports 0 gaps
seq-check: highest entry_seq per shard recorded
seq-check: ok (5m37s)
```

**Pass condition: `0 violations` and `0 gaps`.** Both lines, not one.

A violation means the same `(shard_id, entry_seq)` exists twice, which is the
dry run 4 failure. A gap means a sequence number is missing, which is the
opposite failure and means the drain lost a record.

### 9.3 Why both, and why these two

The two checks look at different things and neither implies the other.

| Failure | Parity catches it | Sequence gap catches it |
|---|---|---|
| An entry written twice | Yes, balance doubles | Yes, index violation |
| An entry lost in the drain | Yes, balance short | Yes, gap in the sequence |
| Two entries that cancel out | No, balance matches | Yes, both are visible |
| A balance computed wrongly | Yes | No, the entries are fine |
| A duplicate with a fresh `entry_seq` | Yes | No, no violation |

Rows three and five are the reason for two checks. A pair of offsetting errors
leaves the balance correct and the sequence broken. A duplicate that got a new
sequence number leaves the sequence clean and the balance wrong.

Neither check alone covers the table. Both together cover every row.

### 9.4 What the verifier reads aloud

Exactly four lines, in this order, at T+40m.

```
parity: 41882306 compared, 0 divergent
parity: ok
seq-check: index present on all shards, 0 violations
seq-check: seq_gap_view reports 0 gaps
```

Reading them aloud is not ceremony. It is the moment a second person confirms
the numbers, and it is cheap.

## 10. The release gate

At T+45m the release captain makes one of three calls.

### 10.1 Release

Both checks passed. The captain says `release` and the database operator runs:

```
$ ledger-freeze release --confirm
freeze: released at 2026-08-29T01:45:00Z
freeze: 14 pools accepting writes against ledger
```

The verifier confirms the first write within 5 minutes:

```
$ psql -h ledger-01 -Atc "SELECT count(*) FROM entries WHERE created_at > now() - interval '2 minutes'"
1847
```

### 10.2 Hold

A check has not finished, or its output needs investigating, and the captain
believes the answer is minutes away. The hold continues and comms sends the
extended-window notice from section 12.2.

A hold is bounded by the rollback deadline at T+2h30m. There is no third
option after that, only release or roll back.

### 10.3 Roll back

Either check failed, or the captain is not satisfied. Go to section 11.

The captain does not need a reason that survives review. "I am not satisfied"
is sufficient at 02:00, and a rollback costs one rescheduled night.

## 11. Rollback

Rollback is a switch, not a reconciliation, and it stays that way only while
writes are held. This is the reason for the hold and it is worth restating
here, where the cost lands.

### 11.1 The decision

The release captain calls `roll back`. Nobody else does, and nobody starts
before the call.

### 11.2 The procedure

```
$ ledger-promote --target tally --rollback --confirm
promote: write path switched back to tally
promote: ledger marked read only
promote: elapsed 0m52s

$ ledger-freeze release --source tally --confirm
freeze: released at 2026-08-29T01:52:00Z
freeze: 14 pools accepting writes against tally
```

Then the verifier confirms writes are landing on `tally`:

```
$ psql -h tally-01 -Atc "SELECT count(*) FROM entries WHERE created_at > now() - interval '2 minutes'"
1913
```

### 11.3 What rollback does not undo

The drain wrote vendor records into `ledger`. Those records also exist in the
vendor's system and will be drained again at the next attempt.

`ledger` is left read only and is not wiped. The next cutover attempt starts
from a fresh backfill, because a partially drained target is harder to reason
about than an empty one.

### 11.4 After a rollback

- Comms sends the rollback notice, section 12.3.
- The release captain writes an incident note before going to bed. Not a full
  postmortem, just what failed and what the output said.
- The postmortem happens within three working days.
- The next attempt is scheduled no sooner than 14 days out, so that two fresh
  dry runs fit inside the gap.

### 11.5 The rollback deadline

T+2h30m. After that point, rolling back is more dangerous than fixing
forward. A hold that long has queued enough partner traffic that releasing to
either side becomes a large event in itself.

If T+2h30m arrives with the checks still unresolved, the captain releases to
whichever side the verifier can prove correct. The night then becomes an
incident rather than a cutover.

## 12. Communications

### 12.1 Scheduled window notice, sent at T-20m

> Ledger maintenance begins at 01:00 UTC. Payments and balance reads stay
> available. New ledger writes are held for up to an hour while we verify the
> migration. Nothing is lost during the hold: writes queue and are applied
> when the window closes.

Note the phrase "up to an hour". The plan is 45 minutes and the notice is
deliberately looser. A hold of a few extra minutes is then not itself an event
that needs explaining.

### 12.2 Extended window notice, sent when a hold is called

> The ledger maintenance window is running longer than planned. Reads are
> unaffected. We are holding writes while a verification check completes, and
> we will update in 20 minutes.

Send it once and then keep the 20 minute cadence. A quiet channel during a
hold is read as a bigger problem than the hold.

### 12.3 Rollback notice

> Ledger maintenance is complete and no changes were made. Writes have
> resumed against the existing system. Queued writes are being applied now.
> We will schedule a new window and give at least 5 days notice.

A rollback notice never says "failed". It says what is true: no changes were
made and service is normal.

### 12.4 Completion notice

> Ledger maintenance is complete. All services are normal. Thank you for your
> patience.

## 13. After the release

### 13.1 The smoke suite

Runs at T+50m, takes about 15 minutes.

| Check | Expected |
|---|---|
| Write a test entry and read it back | Round trip under 40 ms |
| Balance read for 20 sampled accounts | Matches the pre-freeze snapshot plus new writes |
| Vendor callback received and applied | One callback within 5 minutes |
| Nightly export dry run | Completes, row count within 0.1% of yesterday |
| Replica lag on every shard | Under 2 seconds |
| Error rate on the ledger API | Under 0.05% over 10 minutes |

### 13.2 The first hour

The release captain stays on the channel until T+3h10m and declares full
service. Until then the window is still open, in the sense that everybody
stays reachable.

### 13.3 The first week

- Parity runs nightly for 7 nights, against the frozen `tally` snapshot.
- `seq-check` runs nightly for 7 nights.
- The daily settlement reconciliation is read by a human every morning, rather
  than only alerting on a threshold.
- `tally` stays read only and running, untouched, for 30 days.

### 13.4 Decommission

Not before 30 days, and a separate change with its own runbook. Do not fold it
into the cutover night because the window happens to be quiet.

## Appendix A: the command list

Every command the ledger operator and database operator run, in order. Copy
this into the channel at T-90m so the sequence is visible to everybody.

```
# T-5m
pgbouncer-admin -h tally-pool-01 -c "SET default_pool_mode = 'transaction'"
pgbouncer-admin -h tally-pool-01 -c "RELOAD"
ledger-freeze prepare --source tally --confirm

# T-0
ledger-freeze apply --source tally --confirm

# T+3m
ledger-migrate drain --vendor kestrel --env production --from-hwm <HWM> --confirm

# T+18m
ledger-promote --target ledger --confirm

# T+20m
ledger-parity compare --source tally --target ledger --confirm

# T+26m
ledger-migrate seq-check --target ledger --confirm

# T+45m, on the release call only
ledger-freeze release --confirm

# T+45m, on the rollback call only
ledger-promote --target tally --rollback --confirm
ledger-freeze release --source tally --confirm
```

Nine commands on the happy path. Every one takes `--confirm`, and none of them
does anything without it.

## Appendix B: expected output, side by side

The verifier keeps this open. The left column is what a healthy run prints and
the right column is what the same line looks like when it is wrong.

| Step | Healthy | Wrong |
|---|---|---|
| Freeze confirm | four consecutive `0` | any non-zero count |
| Drain | `gap closed, 0 records outstanding` | any outstanding count |
| Drain session | `session accepted` | `replay_window_exceeded` |
| Promote | `16 shards healthy` | fewer than 16 |
| Parity | `0 divergent` | any divergent count |
| Sequence index | `0 violations` | any violation count |
| Sequence gaps | `0 gaps` | any gap count |
| First write | a count above zero | zero after 5 minutes |

Seven of the eight rows want a zero. The exception is the last one, and it is
the only place where a zero is the failure.

## Appendix C: abort criteria

An abort is a rollback called before promotion. It is cheaper than a rollback
after promotion and it is always the right call when a line below is true.

| Observation | Action |
|---|---|
| Writes still landing on `tally` after the freeze | Abort |
| A long transaction cannot be killed | Abort |
| The vendor rejects the replay session | Abort |
| The drain reports outstanding records after two attempts | Abort |
| Fewer than 16 shards report healthy at promotion | Abort |
| Replication lag over 60 seconds at T-0 | No-go, before the freeze |
| Any precondition unchecked at T-90m | No-go |
| A person in one of the five roles becomes unreachable | Hold, then no-go |

Note the last row. A cutover with four people is not a cutover with five and
one gap, it is a different and worse plan.

## Appendix D: escalation

Read aloud at T-90m so that nobody looks it up at 02:00.

| Situation | Who decides |
|---|---|
| A check fails | Release captain, alone |
| A check is ambiguous | Release captain, after the verifier reads it aloud |
| A vendor endpoint is down | Release captain, with the vendor's on-call |
| Postgres will not promote | Database operator recommends, captain decides |
| The hold passes T+2h | Release captain informs the engineering director |
| The rollback deadline passes | Engineering director joins the call |

The vendor's support contact and the internal escalation numbers are in the
sealed card in the operations channel. They are not in this document, because
this document is checked into a repository.

## Appendix E: rehearsal, dry run 6

Dry run 6 on 2026-07-28 was run as a rehearsal against the shadow database,
with the full timeline and all five roles. It is the reason the numbers in
section 4.1 are measurements and not estimates.

```
T-90m  00:00  preconditions read aloud, 21 of 21 checked, go
T-60m  00:30  final incremental backfill starts
T-20m  01:10  partner notice would be sent, drafted and reviewed
T-10m  01:20  incremental backfill complete, 1284 entries
T-5m   01:25  pools drained, 2 long transactions killed
T-0    01:30  freeze applied, hwm 4188920117
T+2m   01:32  freeze confirmed, four zeros
T+3m   01:33  drain starts
T+15m  01:45  drain complete, 12m41s, 0 outstanding
T+18m  01:48  promoted, 1m18s
T+20m  01:50  parity starts
T+25m  01:55  parity complete, 0 divergent, 5m14s
T+26m  01:56  seq-check starts
T+32m  02:02  seq-check complete, 0 violations, 0 gaps, 5m37s
T+40m  02:10  four lines read aloud
T+45m  02:15  release called, writes released
T+47m  02:17  first write confirmed
T+62m  02:32  smoke suite complete, 6 of 6
```

Two things came out of the rehearsal.

**The drain is the variable.** It took 12m41s against a gap of 11,284 records.
A cutover on a busier night, or after a longer freeze preparation, will drain
more. The 16 minute budget holds up to roughly 15,000 records at the observed
rate.

**The checks are steady.** Parity and the sequence check varied by under 20
seconds across dry runs 5 and 6. They are the predictable part of the window,
which is why the release gate is anchored to them.

## Appendix F: questions from the rehearsal

These were asked on the rehearsal call. They are recorded because the same
questions will be asked again on the night.

**Can we release writes as soon as parity passes, and let the sequence check
run afterwards?**

No. The hold is 45 minutes and writes resume only after both the balance
parity check and the sequence-gap check have passed. Section 9.3 has the
table showing what each check misses on its own. Releasing on parity alone
would have passed dry run 4's failure if the duplicates had been offsetting.

**What if the checks finish early?**

They usually will. The release still happens at T+45m, because the eleven
minutes after the checks are for the decision, and because the partner notice
described a window. Releasing early is not free: it surprises partners who
scheduled around the notice.

**What if a check finishes late?**

The hold continues past 45 minutes. The gate is the two checks passing, not
the clock. Section 10.2 covers the comms.

**Who can override the gate?**

Nobody. There is no override path and no flag. `ledger-freeze release` is a
manual command and only the release captain calls for it. The captain calls
for it only when both checks have passed.

**What happens to writes during the hold?**

Clients receive a retryable error and the SDK queues. The queue is bounded at
one hour of traffic, which is why the rollback deadline is where it is.

**Is the vendor drain restartable?**

Yes. A replay session can be resumed by its session id, and the tool records
the id in its state file. A restarted drain does not re-apply records it
already acknowledged.

**Why is the verifier not the person who ran the migration?**

Because dry run 4 was read by the person who ran it, and the two uncompared
numbers were in front of them for 65 minutes. A second reader is the cheapest
control we have.

## Appendix G: what changed in revision 6

Revision 5 was written before dry run 4 and it is not safe. If a copy is still
open somewhere, close it.

| Revision 5 said | Revision 6 says |
|---|---|
| Wait for the pipeline to go green | Read the output of two named checks |
| Release writes after promotion | Hold writes for 45 minutes, release on two passes |
| Parity check optional if the backfill was clean | Parity check is mandatory |
| No sequence check | Sequence-gap check is mandatory |
| Roles: captain and operator | Five roles, verifier separated from operator |
| Rollback "if needed" | Rollback deadline at T+2h30m, with a procedure |
| No abort criteria | Appendix C |

The single change that matters is the first one. A pipeline result is a claim
about steps. The two checks are a claim about data, and only the second kind
is worth holding writes for.

## Appendix H: how the freeze works

Worth understanding, because the freeze is the only mechanism holding the
45 minute window open.

### H.1 Where it is enforced

At the connection pool, not in the application and not in the database.

```
tally-pool-01 .. tally-pool-14
  default_pool_mode = transaction
  server_reset_query = DISCARD ALL
  ledger_freeze = on        <- set by ledger-freeze apply
```

With `ledger_freeze = on`, the pool rejects any transaction that issues a
write statement. The connection stays open, the read path is untouched, and
the rejection is immediate rather than a timeout.

### H.2 Why not a read-only transaction default

Setting `default_transaction_read_only = on` on the database would be simpler,
and it was the plan in revision 3. It was dropped for two reasons.

It applies to superusers and to the migration tooling, which needs to write
during the drain. And it is a cluster-wide setting whose reversal needs a
config reload, which is a worse thing to depend on at T+45m.

### H.3 What the pool returns

```
ERROR:  writes are held during scheduled maintenance
HINT:   retry after the maintenance window
SQLSTATE: 57014
```

`57014` is `query_canceled`, which every client library already treats as
retryable. This was chosen over a custom code precisely because it needs no
client change.

### H.4 Reversal

`ledger-freeze release` flips the flag and reloads all 14 pools in parallel.
Measured at 0.9 seconds across the fleet on dry run 6.

The reversal is idempotent. Running it twice is harmless, and the second run
prints `already released`.

## Appendix I: what clients see during the hold

The hold is user-visible and the shape of that visibility was designed rather
than inherited. Write it down so support can answer without asking us.

| Client | During the hold | After release |
|---|---|---|
| Payment API | Accepts and queues, 202 with a receipt id | Queue applied in order |
| Balance read | Normal, served from the replica | Normal |
| Statement export | Normal, data is frozen so exports are stable |Normal |
| Partner webhook | Delivered as usual | Delivered as usual |
| Admin console writes | Blocked, banner explains the window | Normal |
| Vendor callbacks | Accepted and stored, not applied | Applied within 5 minutes |

### I.1 The queue

Queued writes are held in the payment service, not in the ledger. The queue is
durable, ordered per account, and bounded at one hour of peak traffic.

At the observed peak of 1,847 writes per minute, one hour is about 110,000
entries. The queue drains at roughly 6,000 per minute after release, so a
45 minute hold drains in about 14 minutes.

### I.2 What breaks if the hold runs long

The queue bound is the constraint, and it is why the rollback deadline is
T+2h30m rather than later. Past that point the queue is close to full and the
drain after release becomes its own event.

Nothing silently drops. A full queue returns a 503 and the client retries,
which is worse for partners but is not data loss.

## Appendix J: the parity exclusion list

412 accounts are excluded from the balance parity check. Every exclusion is
committed in `parity_exclusions` and reviewed, because an exclusion list is
the easiest place to hide a failure.

| Category | Count | Why excluded |
|---|---|---|
| Suspense accounts | 188 | Balances are recomputed on read, not stored |
| Clearing accounts | 141 | Net to zero at settlement, transiently non-zero |
| Migration test accounts | 47 | Created by dry runs, not real |
| Closed with residue | 36 | Sub-minor-unit residue from the 2024 rounding change |

### J.1 The rule

An account may be excluded only when its balance is not derivable from its
entries. That is the whole test, and it is what makes the four categories
above legitimate.

An account is never excluded because it fails parity. If a real account
diverges, that is the check working.

### J.2 Reviewing it on the night

The verifier confirms two things before the parity check runs.

```
$ psql -h ledger-01 -Atc "SELECT count(*) FROM parity_exclusions"
412

$ git -C /srv/ledger log -1 --format=%cd -- migrations/0060_parity_exclusions.sql
Fri Jun 26 09:14:22 2026 +0000
```

The count must be 412 and the file must be unchanged since 2026-06-26. A
changed exclusion list on cutover night is an abort.

## Appendix K: revision history

| Rev | Date | Change |
|---|---|---|
| 1 | 2026-05-04 | First draft, before any dry run |
| 2 | 2026-06-17 | Added the incremental backfill, after dry run 1 |
| 3 | 2026-06-24 | Added pool draining, after dry run 2 |
| 4 | 2026-07-08 | Timings from dry run 3 |
| 5 | 2026-07-09 | Editorial, unchanged in substance |
| 6 | 2026-07-28 | Rewritten to gate on checks, after dry run 4 |

## Appendix L: terms

| Term | Meaning here |
|---|---|
| Abort | A rollback called before promotion |
| Drain | Pulling vendor records written after the freeze |
| Freeze | Rejecting writes on `tally`, reads unaffected |
| Gap | The records the vendor holds that we have not applied |
| Hold | The period between the freeze and the release |
| High water mark | The last committed transaction id before the freeze |
| Promotion | Switching the write path to `ledger`, without opening it |
| Release | Opening writes again, on one side or the other |
| Rollback | Returning the write path to `tally` after promotion |

## Appendix M: the one-page checklist

Print this. It is the whole night in tick boxes and it names the section to
read when a box will not tick.

**T-90m, go or no-go** (section 5)

- [ ] Two dry runs clean, most recent within 14 days
- [ ] Backfill complete, per-shard deltas zero
- [ ] `seq-check` clean on the shadow
- [ ] Unique index present on every shard, confirmed by query
- [ ] No pending migration
- [ ] Vendor certification within 7 days
- [ ] Production credentials tested today
- [ ] Gap inside the vendor's documented replay window
- [ ] No vendor maintenance in the window
- [ ] Disk, lag, backups, DNS TTL
- [ ] Five roles filled and confirmed by voice
- [ ] Escalation path read aloud
- [ ] Command list pasted into the channel

**T-5m to T-0, freeze** (section 7)

- [ ] Pools drained, long transactions killed
- [ ] `ledger-freeze prepare` clean
- [ ] `ledger-freeze apply` at T-0 exactly
- [ ] High water mark posted to the channel
- [ ] Four consecutive zeros observed

**T+3m to T+18m, drain and promote** (section 8)

- [ ] Replay session accepted
- [ ] `gap closed, 0 records outstanding`
- [ ] `16 shards healthy`
- [ ] Promotion complete, writes still held

**T+20m to T+40m, the two checks** (section 9)

- [ ] Exclusion count is 412 and the file is unchanged
- [ ] Parity: `0 divergent`
- [ ] Sequence check: `0 violations`
- [ ] Sequence check: `0 gaps`
- [ ] Four lines read aloud by the verifier

**T+45m, the gate** (section 10)

- [ ] Release captain calls release, hold, or roll back
- [ ] On release: `ledger-freeze release`
- [ ] First write confirmed within 5 minutes

**T+50m onward** (section 13)

- [ ] Smoke suite, 6 of 6
- [ ] Status page cleared at T+90m
- [ ] Full service declared at T+3h10m
- [ ] Nightly parity and sequence checks scheduled for 7 nights

## Appendix N: known risks

Six risks, each with what we did about it. This table is reviewed at T-90m
and it is short on purpose.

| Risk | Likelihood | Mitigation |
|---|---|---|
| Drain runs long | Medium | 16 minute budget against a 12m41s measurement |
| Vendor endpoint degraded | Low | Session is resumable, drain restarts by id |
| A check finds real divergence | Low | Rollback, and the hold makes it a switch |
| Promotion fails on a shard | Low | Abort before the write path moves |
| Queue fills during a long hold | Low | Rollback deadline at T+2h30m |
| A role becomes unreachable | Low | No-go before the freeze, hold after it |

### N.1 The risk not on the table

The risk we cannot mitigate on the night is a defect in the backfill that both
checks miss. Section 9.3 shows what each check covers, and the union is
everything we know how to look for.

Something outside that union would show up in the nightly parity runs during
the first week. That is why section 13.3 keeps them running for seven nights
against a frozen snapshot.

### N.2 What we decided not to do

**A canary release**, sending 1% of writes to `ledger` before the full switch.
It sounds safer and it is not, because it splits the ledger across two systems
for the duration. A split ledger is exactly the state the hold exists to
avoid.

**A shadow write period**, writing to both systems for a week. Rejected on
cost and on the reconciliation burden, and because it does not answer the
question the parity check answers.

## Appendix O: the incremental backfill

Section 4 puts an incremental backfill at T-60m and does not explain it. It is
small, it is easy to skip, and skipping it makes the drain much larger.

### O.1 What it is

The main backfill runs in the week before the cutover and copies history up to
its own start point. Everything written to `tally` after that point is not in
`ledger` yet.

The incremental backfill copies that tail. It runs continuously from the main
backfill's completion, and the run at T-60m is simply the last one before the
freeze.

```
$ ledger-migrate incremental --source tally-ro-03 --target ledger --confirm
incremental: last synced xid 4188903244
incremental: 1284 entries to copy
incremental: 16 shards, 1284 entries, 0 conflicts
incremental: complete in 11.4s
incremental: last synced xid 4188919802
```

### O.2 What to watch

**Conflicts must be zero.** A conflict means an entry already exists with that
`(shard_id, entry_seq)`, which is the dry run 4 shape appearing in miniature.
Any non-zero conflict count is a no-go.

**The entry count should be small.** A few thousand is normal at T-60m. Tens
of thousands means the incremental has not been running. The go call then
waits for a clean run rather than proceeding on a large one.

**The elapsed time should be seconds.** The T-60m run took 11.4 seconds on
dry run 6. A run that takes minutes is copying more than a tail, and it is
worth asking why before the freeze rather than after it.

### O.3 Why it is not the drain

They look similar and they are not the same thing.

| | Incremental backfill | Vendor replay drain |
|---|---|---|
| Source | `tally`, our own database | The vendor's replay API |
| Runs | Before the freeze | After the freeze |
| Covers | Writes we accepted | Records the vendor accepted |
| Gap it closes | Our own tail | Records in flight at the freeze |

A write can be in the vendor's system and not in ours, which is the whole
reason the drain exists. The incremental backfill cannot see those, because it
only reads `tally`.

### O.4 After the freeze

The incremental backfill stops at T-0 and does not run again. `tally` is read
only from that moment, so there is nothing left for it to copy.

If a rollback happens, the incremental is restarted only after the next
backfill, never against a partially drained target.

### O.5 The one failure mode worth naming

The incremental reads `tally-ro-03`, a replica. If replication lags, the
incremental copies less than it thinks and reports a clean run.

The precondition at 5.3 caps lag at 5 seconds for exactly this reason. The
freeze then waits for the replica to catch up before the drain starts, which
is the 2 minutes between T-0 and the freeze confirmation.

## Closing note

The hold is the expensive part of this plan and it is the part most likely to
be argued down. Somebody will point out that the checks finish in eleven
minutes and ask why writes are held for 45.

The answer is that a hold is a switch and a release is a commitment. Every
minute of the hold buys the ability to roll back without merging two ledgers
at two in the morning. That is a good trade and it is not worth renegotiating
on the night.
