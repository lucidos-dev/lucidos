# Postmortem: dry run 4 wrote shard 07 twice

**Status:** closed | **Severity:** SEV-3 | **Written:** 2026-07-16 |
**Reviewed:** 2026-07-17

| Field | Value |
|---|---|
| Incident id | INC-2026-0714-01 |
| Detected | 2026-07-14T03:46:41Z, by the `parity-check` step of run 4188 |
| Injected | 2026-07-14T02:11:49Z, when a second worker claimed shard 07 |
| Time to detect | 1h34m52s |
| Time to diagnose | 3h51m, from detection to the confirmed cause |
| Customer impact | None |
| Production impact | None |
| Schedule impact | Cutover moved from 2026-08-08 to 2026-08-29 |
| Owner | Ledger platform |
| Facilitator | Release captain |

## 1. Summary

During dry run 4 of the ledger migration, two backfill workers held a claim on
shard 07 at the same time. Both wrote the same range of source rows into the
shadow database. Shard 07 therefore ended the run with 29,946,091 duplicate
entries and every affected balance doubled.

The duplication was not detected by the backfill step, which reported success.
It was not detected by the sequence check, which did not run. It was detected
by the parity check, which is the last step of the pipeline.

No production system was involved. The source database was read through a
replica and was never written. The shadow database is disposable.

The defect is in the shard lease protocol and it is still present in
production tooling. It has been present since 2026-05-28.

## 2. Impact

### 2.1 What was affected

| Thing | Effect |
|---|---|
| Shadow database `ledger_shadow` | 29,946,091 duplicate rows, discarded |
| Dry run 4 | Failed after 2h47m48s |
| Dry run 5 | Cancelled, would have measured nothing |
| Cutover date | Moved back by three weeks |
| Source database `tally` | None, read only through `tally-ro-03` |
| Production `ledger` | None, not yet serving traffic |
| Customers | None |

### 2.2 What was not affected

No live ledger data exists yet. The new service is not in the request path,
and no merchant balance is served from it. This is the reason the incident is
a SEV-3 and not higher.

### 2.3 Cost

The direct cost is one wasted pipeline run and one wasted engineer day. The
indirect cost is the schedule, and that is the part worth taking seriously.

Three weeks of slip is not caused by a defect that takes an afternoon to fix.
It is caused by needing two clean dry runs before a cutover, and dry runs are
weekly.

## 3. Detection

The `parity-check` step compares every account balance in the source against
the same balance in the shadow database. It failed with 1,918,447 divergent
accounts, all on shard 07.

Every divergent target balance was exactly twice its source balance. The
doubling was visible in the first five rows the tool printed, so the shape of
the problem was clear immediately. The cause was not.

### 3.1 Why nothing detected it earlier

Three earlier steps could have caught it and none did.

| Step | Could have caught it by | Why it did not |
|---|---|---|
| `backfill` | Comparing planned rows against written rows | It printed both and compared neither |
| `migrate-up` | Keeping the unique index during the load | The index is dropped for load speed |
| `seq-check` | Rebuilding the index and counting duplicates | The step was gated off and skipped |

The unique index `entries_shard_seq_uniq` would have turned the first
duplicate row into an immediate failure. It is dropped before the backfill and
rebuilt afterwards, which is a normal bulk load pattern. The pattern is only
safe when the rebuild actually runs.

## 4. Timeline

All times UTC on 2026-07-14 unless stated otherwise.

| Time | Event |
|---|---|
| 01:00:04 | Run 4188 starts on `forge-runner-07` |
| 01:26:02 | `migrate-up` completes and drops the unique index |
| 01:26:42 | Eight backfill workers start, each claims one shard |
| 01:26:42 | `worker-7` claims shard 07, lease until 01:31:42 |
| 01:31:42 | First renewal round, all eight leases extended |
| 02:04:33 | Target cluster resets a pooled connection, `worker-4` retries a range |
| 02:11:00 | Target cluster begins a checkpoint |
| 02:11:42 | `worker-7` lease on shard 07 expires, renewal in flight |
| 02:11:42 | Commit latency on the target reaches 4870 ms |
| 02:11:47 | `worker-7` renewal completes after 4812 ms, logs a warning, continues |
| 02:11:49 | `worker-3` finishes shard 03, looks for work |
| 02:11:49 | `worker-3` reads the lease row, sees a deadline seven seconds in the past |
| 02:11:49 | `worker-3` claims shard 07 and resumes it from range 07/3 |
| 02:11:50 | `worker-7` continues shard 07 at range 07/3, unaware |
| 02:11:50 | Both workers are now writing the same ranges |
| 02:36:58 | `worker-7` reports shard 07 complete |
| 02:38:41 | `worker-3` reports shard 07 complete |
| 03:38:18 | Backfill reports 612,981,933 planned and 642,928,024 written |
| 03:38:19 | `backfill` step passes |
| 03:44:07 | `seq-check` skipped, `RUN_SEQ_CHECK` unset |
| 03:46:41 | `parity-check` reports 1,918,447 divergent accounts on shard 07 |
| 03:47:52 | Run 4188 fails, agent and shadow cluster held for triage |
| 07:20:00 | On-call engineer starts triage against the held cluster |
| 07:34:00 | Duplicate sequence numbers confirmed by direct query |
| 07:51:00 | `shard_lease_history` shows two overlapping claims |
| 08:30:00 | Triage call, five immediate actions agreed |
| 11:38:00 | Root cause confirmed by reproducing the race in a test |
| 2026-07-15 | Fix merged, lease renewal moved to a third of the lease lifetime |
| 2026-07-16 | Postmortem written |
| 2026-07-17 | Postmortem reviewed, seven action items accepted |

### 4.1 The seven seconds that matter

Between 02:11:42 and 02:11:49 the lease row on shard 07 was expired. It was
expired because the renewal statement was still waiting on a checkpoint.

`worker-7` believed it held the lease, because it had not yet been told
otherwise. `worker-3` read the row, saw an expired deadline, and took it.
Both were behaving exactly as written.

## 5. Root cause

The shard lease protocol renews a lease at the moment it expires, not before
it. A renewal that is slower than zero seconds therefore leaves a window in
which the lease is expired and the holder is still working.

### 5.1 How the lease is meant to work

Each backfill worker claims a shard by writing a row into `shard_lease`. The
row carries the worker id, the shard number and a deadline. A worker renews
its own row by pushing the deadline forward.

A worker looking for work reads the table and picks any shard whose lease is
absent or whose deadline is in the past. This is a normal design and it is
correct when the renewal interval is comfortably shorter than the lifetime.

### 5.2 What the configuration actually said

```
LEDGER_LEASE_TTL=5m
LEDGER_LEASE_RENEW_EVERY=5m
```

The two values are equal. A worker therefore renews its lease at the exact
moment the lease dies. There is no margin at all.

Under a fast target database the renewal completes in a few milliseconds and
the window is too small to lose a race in practice. Under a slow one the
window is as long as the statement takes. On 2026-07-14 that was 4812 ms.

### 5.3 Why the target was slow

The target cluster began a checkpoint at 02:11:00. Commit latency p99 rose
from 38 ms to 4870 ms and stayed high for about forty seconds.

This is ordinary PostgreSQL behaviour under a heavy write load. It is not a
fault. `checkpoint_completion_target` is 0.9 and `max_wal_size` is 16 GB, both
defaults we chose deliberately for bulk load throughput.

The lease protocol treated a slow commit as impossible. The database treated
it as a Tuesday.

### 5.4 The second failure: nothing stopped the write

A duplicate claim is bad. It only becomes duplicate data because nothing
between the claim and the disk rejects the second write.

`entries_shard_seq_uniq` is a unique index on `(shard_id, entry_seq)`. With
that index present, `worker-3` would have failed on its first insert. The
index is dropped by `migrate-up` before the load, and rebuilt by `seq-check`
after it.

`seq-check` runs only when `RUN_SEQ_CHECK` is set. The variable was introduced
on 2026-06-25 to skip a slow step during a rushed debug run. It was never set
again.

So the protection existed, and was disabled by an environment variable that
nobody remembered.

### 5.5 The third failure: the count was printed, not compared

The backfill step ends by logging two numbers.

```
entries planned:  612981933
entries written:  642928024
```

They differ by 29,946,091. Nothing compares them. The step exits zero because
every worker exited zero.

Every one of the three failures is enough on its own to cause the incident.
The fix has to address all three, not the most interesting one.

## 6. Five whys

1. **Why did parity fail?** Shard 07 held every entry twice.
2. **Why twice?** Two workers backfilled the same ranges of shard 07.
3. **Why two workers?** The lease on shard 07 looked expired to a second
   worker while the first was still working it.
4. **Why did it look expired?** Renewal starts when the lease expires, so any
   renewal latency is a window of visible expiry.
5. **Why was the window ever long?** A checkpoint pushed commit latency to
   4870 ms, and the lease protocol assumed a commit is fast.

A sixth why is worth writing down, because it is the one with the most
leverage. **Why did the run continue for 65 minutes after the duplication?**
Because the two checks that would have caught it were dropped for speed and
gated off for convenience.

## 7. Contributing factors

### 7.1 Equal TTL and renewal interval

The two values came from one commit on 2026-05-28 that added the lease
protocol. The commit message says "renew every TTL". That is the defect, and
it was reviewed and approved.

The review is not the interesting part. The interesting part is that nothing
in the codebase says the two values must differ, so a reader had nothing to
compare the setting against.

### 7.2 A unit test that could not fail

`TestLeaseRenewal` starts a worker, waits for a renewal, and asserts the
deadline moved forward. It uses an in-memory store, so the renewal takes
microseconds.

The test measures that renewal happens. It does not measure that renewal
happens in time. No test in the repository moves a clock.

### 7.3 A debug flag that outlived its debug session

`RUN_SEQ_CHECK` was added to make one run finish faster on 2026-06-25. The
pull request says "temporary, remove after we find the join bug". The join bug
was found on 2026-06-26. The flag stayed.

A flag that defaults to off is a check that defaults to off.

### 7.4 Green means green

Dry run 3, on 2026-07-07, passed clean in 2h04m. It ran the same code with the
same gated-off check. It passed because the target cluster happened not to
checkpoint during the backfill.

A passing run gave us confidence in a protocol that was already broken. This
is the ordinary shape of a latent race and it is why "it worked last week" is
weak evidence.

### 7.5 No shard-level row counts

The backfill logs a total. It does not log per-shard counts. A per-shard count
would have shown shard 07 at roughly 1.8 times its expected size, in a line
that a human reads every run.

## 8. What went well

- **The parity check did its job.** It is slow and it is last, but it found a
  defect that four faster things missed.
- **The agent and the shadow cluster were held.** Triage ran against the exact
  failing state, so no reproduction was needed for the first two hours.
- **`shard_lease_history` existed.** The overlapping claim was visible in one
  query because we keep an audit row per claim.
- **No production system was in the path.** The dry run is a dry run. This is
  the whole reason we do them.
- **Diagnosis took under four hours** including a reproduction.

## 9. What went badly

- **The failing step is the last step.** We paid 2h47m of pipeline time to
  learn something a check at 01:30 could have told us.
- **The log printed the evidence and nobody saw it.** The planned and written
  counts were 30 million apart on line 1,104 of the log.
- **The gated check was invisible.** `seq-check` logs `skipped` and exits
  zero. In the run summary it is indistinguishable from a step that passed.
- **No alert.** The run failed at 03:47 and a human read it at 07:20. That is
  fine for a dry run and would not be fine for a cutover.

## 10. Where we got lucky

Two pieces of luck are worth naming, because neither will hold next time.

The doubling was exact. Every divergent balance was precisely twice its
source, which made the shape obvious in five rows. Had the two workers
overlapped on a partial range, divergence would have been ragged and diagnosis
would have taken much longer.

And the race hit shard 07 rather than shard 00. Shard 00 carries the platform
settlement accounts, which are reconciled against the bank daily. A doubled
settlement balance in a dry run is harmless in itself. It would also have been
read by a finance dashboard that previews from the shadow database.

## 11. Action items

Seven items, accepted at the review on 2026-07-17. Each has one owner and a
date. Items 1 to 3 block dry run 5. Items 4 to 7 block the cutover.

| # | Action | Owner | Due | Status |
|---|---|---|---|---|
| 1 | Renew a lease at a third of its lifetime, and refuse to start if the interval is not shorter than the TTL | Ledger platform | 2026-07-15 | Done |
| 2 | Abort a worker whose renewal returns a lease it no longer owns | Ledger platform | 2026-07-15 | Done |
| 3 | Remove `RUN_SEQ_CHECK` and run `seq-check` unconditionally | Migration tooling | 2026-07-16 | Done |
| 4 | Fail the backfill step when planned and written counts differ | Migration tooling | 2026-07-21 | Done |
| 5 | Log per-shard row counts and compare each against its plan | Migration tooling | 2026-07-24 | Done |
| 6 | Rewrite the cutover runbook to gate on check output rather than on a pipeline result | Release captain | 2026-07-28 | Done |
| 7 | Add a clock-advancing test for lease expiry under a slow store | Ledger platform | 2026-08-04 | Open |

### 11.1 Notes on item 1

The fix has two halves and both matter.

The first half is the interval. Renewal now runs at `TTL / 3`, so a worker
gets two chances to renew before its lease dies. With a 5 minute TTL that is a
renewal every 100 seconds and a tolerance of about 200 seconds of latency.

The second half is a startup check. `ledgerd` now refuses to start when
`LEDGER_LEASE_RENEW_EVERY` is not strictly less than `LEDGER_LEASE_TTL`. A
configuration that cannot express the defect is better than a configuration
that documents it.

### 11.2 Notes on item 2

Renewal is now a conditional update. It writes the new deadline only when the
row still names this worker, and it returns the number of rows it changed.

Zero rows means the lease was taken while the renewal was in flight. The
worker stops immediately, rolls back its open transaction and exits non-zero.
Losing a lease is now a loud failure rather than a silent overlap.

This is the half that would have prevented the incident even with the bad
interval. `worker-7` would have discovered at 02:16:42 that it no longer owned
shard 07, and it had written nothing that a rollback could not undo.

### 11.3 Notes on item 3

`RUN_SEQ_CHECK` is gone from the pipeline, from the compose file and from the
two developer scripts that set it. `seq-check` now always runs.

It costs 5m38s on a full shadow database. That is 3.4% of a dry run and it is
the cheapest insurance in the pipeline.

The step also changed its reporting. A skipped step used to log `skipped` and
exit zero. There is no skip path any more, so the question does not arise.

### 11.4 Notes on item 4

The backfill step now ends with an explicit comparison.

```
entries planned:  612981933
entries written:  612981933
delta:            0
backfill: ok
```

Any non-zero delta fails the step. There is no tolerance and no warning band,
because there is no legitimate reason for the two numbers to differ.

### 11.5 Notes on item 5

Per-shard counts turn a 30 million row delta into a line that names the shard.
The new output is a table with one row per shard, and the step prints it
whether it passes or fails.

The point is not machine detection, which item 4 already covers. The point is
that a human reading a green log learns the shape of a normal run, and
notices when the shape changes.

### 11.6 Notes on item 6

The old runbook said "wait for the pipeline to go green". Dry run 4 shows what
that sentence is worth: the pipeline was green through eleven of twelve steps
while holding 30 million duplicate rows.

Revision 6 replaces the pipeline result with named checks and their expected
output. It is a separate document and it is the one the release captain reads
on the night. Read it there rather than here.

### 11.7 Notes on item 7

This is the only open item and it is the one most likely to be dropped.

The test needs an injectable clock in the lease package, which the package
does not have. Adding one is a small refactor with a wide blast radius,
because `time.Now` is called in eleven places.

The item stays open rather than being quietly closed. A race that cannot be
tested will come back.

## 12. The fix

### 12.1 Before

```go
func (w *Worker) holdLease(ctx context.Context, shard int) error {
	ticker := time.NewTicker(w.cfg.LeaseTTL)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-ticker.C:
			if err := w.renew(ctx, shard); err != nil {
				log.Warn("lease renewal failed", "shard", shard, "err", err)
				continue
			}
		}
	}
}

func (w *Worker) renew(ctx context.Context, shard int) error {
	_, err := w.db.ExecContext(ctx,
		`UPDATE shard_lease SET deadline = now() + $1 WHERE shard_id = $2`,
		w.cfg.LeaseTTL, shard)
	return err
}
```

Three defects are visible in eleven lines. The ticker fires at the TTL. A
failed renewal logs a warning and carries on. And the update names the shard
without naming the owner, so it will happily renew a lease that belongs to
somebody else.

### 12.2 After

```go
func (w *Worker) holdLease(ctx context.Context, shard int) error {
	ticker := time.NewTicker(w.cfg.LeaseRenewEvery)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-ticker.C:
			held, err := w.renew(ctx, shard)
			if err != nil {
				return fmt.Errorf("renew shard %d: %w", shard, err)
			}
			if !held {
				return fmt.Errorf("lost lease on shard %d", shard)
			}
		}
	}
}

func (w *Worker) renew(ctx context.Context, shard int) (bool, error) {
	res, err := w.db.ExecContext(ctx,
		`UPDATE shard_lease
		    SET deadline = now() + $1
		  WHERE shard_id = $2 AND owner = $3`,
		w.cfg.LeaseTTL, shard, w.id)
	if err != nil {
		return false, err
	}
	n, err := res.RowsAffected()
	return n == 1, err
}
```

`holdLease` runs under an errgroup with the shard's writer. A returned error
cancels the writer's context, so losing a lease stops the write within one
statement.

### 12.3 The startup check

```go
func (c *Config) Validate() error {
	if c.LeaseRenewEvery >= c.LeaseTTL {
		return fmt.Errorf(
			"LEDGER_LEASE_RENEW_EVERY (%s) must be shorter than "+
				"LEDGER_LEASE_TTL (%s)", c.LeaseRenewEvery, c.LeaseTTL)
	}
	if c.LeaseRenewEvery*3 > c.LeaseTTL {
		log.Warn("lease renewal has less than three attempts per lifetime",
			"renew_every", c.LeaseRenewEvery, "ttl", c.LeaseTTL)
	}
	return nil
}
```

The hard failure catches the exact defect. The warning catches the near miss,
where somebody sets renewal to four fifths of the TTL and believes they have
left margin.

### 12.4 The new defaults

| Variable | Was | Now |
|---|---|---|
| `LEDGER_LEASE_TTL` | 5m | 5m |
| `LEDGER_LEASE_RENEW_EVERY` | 5m | 100s |
| `LEDGER_LEASE_STEAL_AFTER` | not present | 30s past deadline |

`LEDGER_LEASE_STEAL_AFTER` is new. A worker looking for work now ignores a
lease that expired less than 30 seconds ago. That is a second line of defence
and it costs nothing, because a genuinely dead worker is not coming back in
30 seconds.

## Appendix A: the triage queries

These are the queries run against the held shadow cluster on 2026-07-14,
in the order they were run. They are recorded here because the next person
to triage a parity failure should start with the same four.

### A.1 Is the divergence real, or is it the comparison?

```sql
SELECT account_id, source_balance, target_balance,
       target_balance - source_balance AS delta
  FROM parity_divergence
 ORDER BY abs(target_balance - source_balance) DESC
 LIMIT 5;
```

```
     account_id      | source_balance | target_balance |    delta
---------------------+----------------+----------------+-------------
 acct_01J8QK4M2P0001 |     4182993317 |     8365986634 |  4182993317
 acct_01J8QK4M2P0007 |     3901772044 |     7803544088 |  3901772044
 acct_01J8QK4M2P0019 |     3855018802 |     7710037604 |  3855018802
 acct_01J8QK4M2P0031 |     3702990615 |     7405981230 |  3702990615
 acct_01J8QK4M2P0044 |     3611884471 |     7223768942 |  3611884471
(5 rows)
```

Every delta equals the source balance. The target is exactly double. That
rules out rounding, currency handling and a comparison bug in one query.

### A.2 Is it every shard, or one?

```sql
SELECT shard_id, count(*) AS divergent_accounts
  FROM parity_divergence
  JOIN accounts USING (account_id)
 GROUP BY shard_id
 ORDER BY shard_id;
```

```
 shard_id | divergent_accounts
----------+--------------------
        7 |            1918447
(1 row)
```

One shard. That points at the unit of work rather than at the transform,
because the transform is shared by all sixteen shards.

### A.3 Are the entries duplicated, or are the balances wrong?

```sql
SELECT entry_seq, count(*) AS copies
  FROM entries
 WHERE shard_id = 7
 GROUP BY entry_seq
HAVING count(*) > 1
 ORDER BY entry_seq
 LIMIT 10;
```

```
 entry_seq | copies
-----------+--------
   8556027 |      2
   8556028 |      2
   8556029 |      2
   8556030 |      2
   8556031 |      2
   8556032 |      2
   8556033 |      2
   8556034 |      2
   8556035 |      2
   8556036 |      2
(10 rows)
```

Contiguous from 8556027. That is a range boundary, not scattered corruption.

### A.4 Who wrote them?

```sql
SELECT shard_id, owner, claimed_at, released_at, deadline
  FROM shard_lease_history
 WHERE shard_id = 7
 ORDER BY claimed_at;
```

```
 shard_id |  owner   |       claimed_at       |      released_at       |        deadline
----------+----------+------------------------+------------------------+------------------------
        7 | worker-7 | 2026-07-14 01:26:42+00 | 2026-07-14 02:36:58+00 | 2026-07-14 02:36:42+00
        7 | worker-3 | 2026-07-14 02:11:49+00 | 2026-07-14 02:38:41+00 | 2026-07-14 02:41:49+00
(2 rows)
```

Two claims, overlapping by 25 minutes and 9 seconds. This row pair is the
whole incident and it took eleven minutes to find.

### A.5 The counts

```sql
SELECT
  count(*)                          AS rows_written,
  count(DISTINCT entry_seq)         AS distinct_entries,
  count(*) - count(DISTINCT entry_seq) AS duplicates
FROM entries WHERE shard_id = 7;
```

```
 rows_written | distinct_entries | duplicates
--------------+------------------+------------
     68448209 |         38502118 |   29946091
(1 row)
```

29,946,091 duplicates. That is exactly the delta the backfill step printed and
did not compare.

### A.6 Which ranges overlapped?

```sql
SELECT range_key, count(*) AS writes, min(written_at), max(written_at)
  FROM backfill_range_log
 WHERE shard_id = 7
 GROUP BY range_key
 ORDER BY range_key;
```

```
 range_key | writes | entries |            min             |            max
-----------+--------+---------+----------------------------+----------------------------
 07/1      |      1 | 4278014 | 2026-07-14 01:26:44.118+00 | 2026-07-14 01:44:12.771+00
 07/2      |      1 | 4278013 | 2026-07-14 01:44:12.884+00 | 2026-07-14 02:01:40.664+00
 07/3      |      2 | 4278013 | 2026-07-14 02:01:40.772+00 | 2026-07-14 02:14:08.919+00
 07/4      |      2 | 4278013 | 2026-07-14 02:12:55.038+00 | 2026-07-14 02:19:31.442+00
 07/5      |      2 | 4278013 | 2026-07-14 02:14:09.061+00 | 2026-07-14 02:24:47.880+00
 07/6      |      2 | 4278013 | 2026-07-14 02:19:31.588+00 | 2026-07-14 02:30:12.335+00
 07/7      |      2 | 4278013 | 2026-07-14 02:24:48.014+00 | 2026-07-14 02:33:29.706+00
 07/8      |      2 | 4278013 | 2026-07-14 02:30:12.470+00 | 2026-07-14 02:36:58.129+00
 07/9      |      2 | 4278013 | 2026-07-14 02:33:29.842+00 | 2026-07-14 02:38:41.615+00
(9 rows)
```

Ranges 07/1 and 07/2 were written once, before the race. Ranges 07/3 through
07/9 were written twice, which is seven of shard 07's nine ranges.

The arithmetic closes. Seven ranges at 4,278,013 entries each is 29,946,091,
which is the delta the backfill step printed. The two clean ranges hold
8,556,027 entries between them, which is where the duplicate sequence numbers
start.

## Appendix B: divergence by currency

The parity tool groups divergence by currency because the settlement team
reads it that way. It has no diagnostic value here and is recorded for
completeness.

| Currency | Divergent accounts | Sum of deltas (minor units) |
|---|---|---|
| NOK | 1,204,881 | 61,338,517,204 |
| EUR | 502,933 | 19,447,286,391 |
| SEK | 210,633 | 7,356,505,956 |
| **Total** | **1,918,447** | **88,142,309,551** |

The three currencies are the ones live on shard 07. Shard 07 holds Nordic
marketplace sellers, which is why NOK dominates.

## Appendix C: the log lines that mattered

Five excerpts out of 38,411 lines in the run log, all from `forge/4188`.

### C.1 The slow renewal

```
2026-07-14T02:11:47.882Z  worker-7: lease renew on shard 07 took 4812ms, deadline was 02:11:42Z
2026-07-14T02:11:47.883Z  worker-7: WARN renew completed after the deadline, continuing
```

This is the only warning the backfill step emitted in 2h12m. It names the
shard, the duration and the deadline it missed. On its own it is enough to
diagnose the incident.

Nothing alerts on it. It scrolled past inside a step that went green, and the
word `continuing` is the whole defect in one token.

### C.2 The second claim

```
2026-07-14T02:11:49.221Z  worker-3: shard 07 lease looks expired, deadline 02:11:42Z, now 02:11:49Z
2026-07-14T02:11:49.222Z  worker-3: claimed shard 07, lease until 02:16:49Z
2026-07-14T02:11:49.223Z  worker-3: resuming shard 07 from range 07/3
2026-07-14T02:11:50.884Z  worker-7: continuing shard 07 at range 07/3
```

Four lines, 1.7 seconds apart, and the incident is complete. The last two say
that two workers are on range 07/3, and they are adjacent in the log.

The lines read as routine because taking over an expired lease is what the
protocol is for. Nothing here is an error from the code's point of view.

### C.3 Two completions

```
2026-07-14T02:36:58.007Z  worker-7: shard 07 complete, 38502118 entries in 70m15s
2026-07-14T02:38:41.884Z  worker-3: shard 07 complete, 38502118 entries in 26m52s
```

Two "shard complete" lines for one shard, 103 seconds apart.

Both report 38,502,118 entries, because the line prints the shard's planned
total rather than what this worker wrote. `worker-3` did seven of nine ranges
in 26m52s and reports the whole shard. A line that printed rows written would
have shown 29,946,091 against a 70 minute sibling.

### C.4 The uncompared totals

```
2026-07-14T03:38:18.884Z  backfill: 612981933 entries planned, 642928024 rows written
2026-07-14T03:38:19.007Z  backfill: elapsed 2h12m17s, mean 77236/s, peak 81904/s
2026-07-14T03:38:19.114Z  forge: step backfill passed in 2h12m17s
```

Thirty million rows apart on one line, then `passed` on the next.

### C.5 The skip

```
2026-07-14T03:44:07.114Z  forge: step seq-check skipped
2026-07-14T03:44:07.115Z  forge: condition RUN_SEQ_CHECK == "1" evaluated to false
2026-07-14T03:44:07.116Z  forge: RUN_SEQ_CHECK is unset in this pipeline's environment
```

Forge records the step as skipped, and a skipped step does not fail a run.
The three lines even name the variable and say it is unset.

In the run summary this reads as `seq-check 0s skipped`. Nothing in that
string says the unique index is still absent from the database.

## Appendix D: the reproduction

The race was reproduced on 2026-07-14 at 11:38Z, against a local target with
an injected delay. The reproduction is a shell script rather than a Go test,
because the lease package has no injectable clock. That gap is action item 7.

```bash
#!/usr/bin/env bash
# Reproduce the dry run 4 lease race. Two workers, one shard, slow commits.
set -euo pipefail

docker compose -f compose.repro.yaml up -d target
export LEDGER_LEASE_TTL=6s
export LEDGER_LEASE_RENEW_EVERY=6s

# Delay every UPDATE on shard_lease by five seconds.
psql -h localhost -p 55432 -U ledger -d ledger_repro -f delay_lease_update.sql

./bin/ledgerd backfill --worker-id worker-a --shards 7 &
sleep 7
./bin/ledgerd backfill --worker-id worker-b --shards "" --steal &

wait
psql -h localhost -p 55432 -U ledger -d ledger_repro -c \
  "SELECT count(*) - count(DISTINCT entry_seq) AS dupes FROM entries WHERE shard_id = 7;"
```

The delay is a trigger on `shard_lease`:

```sql
CREATE OR REPLACE FUNCTION delay_lease_update() RETURNS trigger AS $$
BEGIN
  PERFORM pg_sleep(5);
  RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER lease_update_delay
  BEFORE UPDATE ON shard_lease
  FOR EACH ROW EXECUTE FUNCTION delay_lease_update();
```

Result on the unfixed binary:

```
 dupes
-------
 41822
(1 row)
```

Result on the fixed binary, same script:

```
lost lease on shard 7
worker-b exited 1
 dupes
-------
     0
(1 row)
```

The fixed binary still loses the lease, because the reproduction forces a five
second delay against a six second TTL. It refuses to write after losing it,
which is the property that matters.

## Appendix E: the review meeting

Held 2026-07-17, 50 minutes, nine people. These are the parts of the
discussion worth keeping. The action items in section 11 are the output.

### E.1 Rejected: keep a longer TTL

The first suggestion was to raise `LEDGER_LEASE_TTL` to 20 minutes and leave
the renewal interval alone. That does make the race much less likely, because
a renewal would have to stall for a third of an hour.

It was rejected for two reasons. A long TTL means a genuinely dead worker
holds its shard for the same third of an hour. That turns a crash into a long
stall inside a pipeline with a 3 hour budget. The second reason is that it
treats the symptom: the defect is that renewal has no margin at all.

The room agreed on the general form. **Make the window impossible rather than
narrow.**

### E.2 Rejected: advisory locks

A PostgreSQL advisory lock would give real mutual exclusion, held by the
session rather than by a timestamp. No lease, no renewal, no race.

Rejected because the backfill uses a connection pool. An advisory lock is
scoped to a session, and a pooled worker does not own a session for the
duration of a shard. Pinning a connection per worker for 70 minutes is
possible, and it trades a race for a connection-exhaustion problem.

Worth revisiting when the backfill stops using a pool, which is not planned.

### E.3 Rejected: detect duplicates in the transform

Somebody proposed making the writer idempotent, with an upsert on
`(shard_id, entry_seq)` instead of an insert.

This is attractive and it is probably right eventually. It was rejected for
two reasons. An upsert on 613 million rows is materially slower than an
insert. And it would have masked the lease defect rather than surfacing it.

A silent correct result from a broken protocol is how you get the same
incident later, in a place with no parity check.

Recorded as a design note for the post-migration service rather than as an
action item.

### E.4 Accepted: the unique index is not negotiable

The longest part of the discussion was whether to keep
`entries_shard_seq_uniq` during the load.

Keeping it costs roughly 18% on the backfill, measured on dry run 2. That is
about 24 minutes on a 2h12m step, and the pipeline has the budget.

The decision was to keep dropping it, but to make the rebuild unconditional
and to fail the run on any violation. The index rebuild is also the sequence
check. So one unconditional `seq-check` buys the constraint and the gap report
together, for a single cost.

Dissent was recorded. Two people argued the index should simply stay on. The
minority view is written here so it can be revisited if `seq-check` ever gets
skipped again.

### E.5 Accepted: the last step cannot be the only check

The parity check is 5m14s and it runs after everything. It is the right final
gate and it is the wrong only gate.

The principle the room settled on is that **each step verifies its own
output**. The backfill compares its counts. The sequence check rebuilds its
index. The parity check then confirms the whole, rather than discovering
something a step should have caught 65 minutes earlier.

### E.6 Discussed and left open: alerting on a dry run

The run failed at 03:47 and a human read it at 07:20. Nobody argued that this
is acceptable for a cutover night, and nobody wanted a pager for a dry run.

Left open deliberately. It becomes a real question when the cutover is
scheduled, and it belongs in the runbook rather than in this document.

## Appendix F: dry run history

| Run | Date | Result | Failed at | Elapsed |
|---|---|---|---|---|
| 1 | 2026-06-16 | Failed | `migrate-up`, missing extension | 0h11m |
| 2 | 2026-06-23 | Failed | `backfill`, worker OOM on shard 04 | 1h38m |
| 3 | 2026-07-07 | Passed | none | 2h04m |
| 4 | 2026-07-14 | Failed | `parity-check`, shard 07 doubled | 2h47m |
| 5 | 2026-07-21 | Passed | none | 2h31m |
| 6 | 2026-07-28 | Passed | none | 2h29m |

Dry run 3 is the interesting row. It ran the defective lease protocol with the
sequence check gated off, and it passed. The target cluster did not checkpoint
during the backfill window, so no renewal was slow enough to lose a lease.

Runs 5 and 6 carry the fix. Run 5 is 27 minutes longer than run 3, and the
difference is the unconditional sequence check plus the per-shard counting.

Two clean runs is the bar for scheduling a cutover. Runs 5 and 6 meet it.

## Appendix G: terms used here

| Term | Meaning in this document |
|---|---|
| Backfill | Copying historical entries from `tally` into the new ledger |
| Dry run | A full migration into a disposable shadow database |
| Entry | One immutable ledger line, the unit that is copied |
| `entry_seq` | Per-shard monotonic sequence on an entry |
| Lease | A timed claim on a shard, held by one backfill worker |
| Parity | Every account balance equal in source and target |
| Range | A contiguous slice of one shard, the unit of work |
| Shadow database | The disposable target a dry run writes into |
| Shard | One of sixteen partitions of the new ledger |
| Source | The legacy ledger `tally`, read through `tally-ro-03` |
| Target | The new ledger, `ledgerd` against PostgreSQL 16.3 |

## Appendix H: what this document does not cover

Three things came up during triage and belong elsewhere.

**The vendor replay window.** The drain step depends on how far back the
vendor lets us replay, and the cutover plan is sized against it. That figure
is in the vendor's specification, not here.

**The cutover procedure.** What the release captain does on the night, in what
order, and what has to pass before writes resume. That is the runbook, and it
was rewritten as action item 6.

**The field mapping.** Which source column becomes which target column, and
the three that are computed rather than copied. That is the field dictionary.

This document covers one incident and its causes. It is deliberately not a
place to look up how the migration works.

## Appendix I: dry run 5, with the fix in place

Run 4231, 2026-07-21, the first run carrying items 1 to 5. Recorded here
because a postmortem that does not show the fix working is half a document.

### I.1 The backfill summary

```
03:29:41.118Z backfill  INFO  per-shard counts
shard  planned    written    delta  elapsed
   00  38311204   38311204       0  62m11s
   01  38402887   38402887       0  63m40s
   02  38290551   38290551       0  61m58s
   03  38190447   38190447       0  62m24s
   04  38455013   38455013       0  71m02s
   05  38377620   38377620       0  64m19s
   06  38268930   38268930       0  60m55s
   07  38502118   38502118       0  65m37s
   08  38144265   38144265       0  59m48s
   09  38398702   38398702       0  63m11s
   10  38221489   38221489       0  61m40s
   11  38470336   38470336       0  64m52s
   12  38059874   38059874       0  59m03s
   13  38336051   38336051       0  62m47s
   14  38287613   38287613       0  63m28s
   15  38364833   38364833       0  64m01s
03:29:41.221Z backfill  INFO  entries planned: 612981933
03:29:41.221Z backfill  INFO  entries written: 612981933
03:29:41.222Z backfill  INFO  delta: 0
03:29:41.884Z backfill  INFO  backfill: ok (8 workers, 16 shards, 2h01m14s)
```

Every shard matches its plan. The table is the part a human reads, and the
delta line is the part the step fails on.

### I.2 The sequence check

```
03:29:42.007Z seq-check INFO  rebuilding entries_shard_seq_uniq
03:35:04.442Z seq-check INFO  index built, 0 violations
03:35:04.884Z seq-check INFO  gap scan over 16 shards
03:35:19.118Z seq-check INFO  seq_gap_view reports 0 gaps
03:35:19.221Z seq-check INFO  seq-check: ok (5m37s)
```

Five minutes and 37 seconds, and no way to skip it. On dry run 4 this step
would have failed at the first duplicate, 65 minutes before the parity check
noticed anything.

### I.3 The lease log

```
01:29:14.882Z worker-5  WARN  lease renewal slow shard=11 took=2204ms ttl=5m0s renew_every=100s
```

One slow renewal, at 2204 ms. Under the old configuration that is a 2.2 second
window of visible expiry. Under the new one it is 97.8 seconds of remaining
margin, and the warning is informational.

### I.4 Timing, run 3 against run 5

| Step | Run 3 (2026-07-07) | Run 5 (2026-07-21) | Difference |
|---|---|---|---|
| `migrate-up` | 25m12s | 25m58s | +46s |
| `backfill` | 1h58m41s | 2h01m14s | +2m33s |
| `seq-check` | skipped | 5m37s | +5m37s |
| `replay-drain` | 12m04s | 12m41s | +37s |
| `parity-check` | 5m02s | 5m14s | +12s |
| **Total** | **2h04m** | **2h31m** | **+27m** |

The backfill is 2m33s slower because of the per-shard counting, which is one
extra aggregate query per shard. The rest of the 27 minutes is the sequence
check that run 3 skipped.

Twenty-seven minutes buys a run that cannot silently write a shard twice. The
comparison to make is not against run 3, which was lucky. It is against run 4,
which cost 2h47m and produced nothing.

## Closing note

The defect is nine lines of Go and it has been in the tree since 2026-05-28.
It survived a code review, a unit test, and a clean dry run.

What found it was a check that compares the whole result against the whole
source, at the end, slowly. Keep that check. Add the fast ones in front of it,
and never let a run reach it carrying something an earlier step could have
refused.
