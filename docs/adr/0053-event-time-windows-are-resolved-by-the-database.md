# 0053: A time window over an event-store column is resolved by the database, never by the caller's clock

- **Status**: Accepted
- **Date**: 2026-08-07

## Context

`events.created` is written by Postgres: every `EventBus` insert stamps it with
SQL `NOW()`. Several queries then ask "what happened in the last N", and the
natural way to write one is to compute the cutoff in Rust:

```rust
let since = Utc::now() - Duration::seconds(ARMING_LOOKBACK_SECS);
// ... WHERE created >= $2
```

That single `>=` has the host clock on one side and the database clock on the
other. The two are the same machine in a packaged install, but not in
development: a dev workspace and the test suite both run Postgres in a Docker
VM, whose clock drifts from the host's and is then resynced. While it is
drifting, the comparison is nonsense.

On 2026-08-07 a `./scripts/test-engine.sh --full` run failed exactly seven
tests, all of them the arming-lookback tests that assert a NON-empty result, all
of them reporting zero matches. The window was three minutes wide, the container
clock was behind by more than that, and every row in the window failed
`created >= $since`. The three lookback tests that assert an EMPTY result passed
in the same run, which is why the module looked half-broken rather than
mis-clocked. Reproduced deterministically as a predicate against the test
container: with the caller's clock 200 seconds ahead, a row written a moment
earlier is matched by `created >= now() - make_interval(secs => 180)` and missed
by `created >= $host_cutoff`.

The engine runs the same comparison in production, in `Engine::arming_lookback`.
Its failure mode there is worse than a red test, because it is silent: the
lookback reports nothing, the model is told nothing already happened, and it
misses the event. That miss is the entire reason the arming lookback exists
(see `docs/plans/2026-08-06-await-event-covers-the-observe-then-arm-gap.md`).

This is the second sighting. `pg_now` in
`core/changes_projection_tests/helpers.rs` was added for the same drift, in one
test module's cutoffs, and its doc already names the cause: "Mixing Rust
`Utc::now()` with Postgres NOW() flakes under clock drift between host and the
Postgres container (notably after laptop sleep/wake on macOS Docker)." Fixing
one call site did not stop the next one being written.

## Decision

A query that bounds an event-store timestamp column by "the last N" takes the
window as a **duration** and resolves it in SQL:

```sql
WHERE created >= now() - make_interval(secs => $n)
```

Never as a cutoff instant computed by the caller. Anything derived from the same
column for the caller to read, an age most of all, is computed by the same
statement: `EXTRACT(EPOCH FROM now() - created)::bigint`, not by subtracting the
row's `created` from the engine's `Utc::now()`.

Applies to `arming_lookback_matches` and `delivered_event_ids` today.

## Rationale

The column and the boundary have to come from one clock, and only one of the two
clocks is available to both. `created` is written by the server and cannot be
rewritten to the host's clock without either trusting whatever process emitted
the event or breaking event ordering. So the boundary moves to the server, which
is one line of SQL.

Passing a duration rather than an instant is what makes the rule hold. A
`since: DateTime<Utc>` parameter can be satisfied by `Utc::now() - window`, and
that expression is shorter, obvious, and wrong; a reviewer has to notice the
provenance of an argument that is correctly typed. With `window_secs: i64` there
is no timestamp for a caller to supply, so the mistake is not expressible rather
than merely discouraged. That is the same move as making impossible states
impossible everywhere else in this codebase.

The failure this prevents is silent in both directions that matter. In the
tests, an emptied window reports "0 matches found", which reads as a query bug
and sends the reader into the scan logic; the seven tests that failed took a
full investigation to place. In production nothing is reported at all.

## Consequences

- Each query resolves its own `now()`, so two queries in one logical operation
  have boundaries a round trip apart. Where that ordering matters it is stated
  at the call site: `Engine::arming_lookback` reads the delivered-event
  exclusion set FIRST, so its window is the wider one and cannot fail to cover
  an event the scan is willing to report.
- A paging scan re-resolves the boundary per page, so it creeps later by a round
  trip across the scan. Microseconds against a window of minutes, and only ever
  at the far edge of a report that is advisory by design.
- Tests that place a row relative to the window back-date it in server time
  (`UPDATE events SET created = now() - make_interval(...)`). This is what
  `the_lookback_window_is_measured_by_the_database_clock` does, and it is why
  that test pins the boundary rather than the agreement of two machines.
- **Two sites in the same class are knowingly left alone.**
  `ChangesProjection::requires_restart_since` and `client_update_since` compare
  `resolved_at` against a caller-supplied instant (`state.started_at`, a chat
  turn's `result_started_at`). Their windows are minutes to hours rather than
  three minutes, and a drift costs a missed restart nudge rather than a missed
  event, so they were out of scope for the change that wrote this ADR. They are
  named here so the next person finds them rather than rediscovering the class.
- The `changes_projection` tests that still hold a `Utc::now() - 1 second`
  cutoff assert the negative, so drift cannot make them fail. Left as they are.

## Alternatives considered

**Keep the instant parameter and feed it from `SELECT NOW()`.** What `pg_now`
already does for one test module. Correct, and it preserves a single shared
boundary across the two queries for free. Rejected as the general rule because
it costs a round trip and, more importantly, leaves the wrong call spellable:
the parameter type still accepts `Utc::now() - window`, so the rule survives
only as long as everyone remembers it. That is precisely what did not happen
between the `pg_now` fix and this one.

**A newtype over `DateTime<Utc>` constructible only from the database clock.**
Gets the type-level guarantee and keeps one shared boundary. Rejected as more
machinery than the problem needs: a duration parameter already makes the mistake
unexpressible, and the newtype would still need a doc comment explaining why it
exists. It also leaves a door open, since nothing stops a later
`from_host_clock` constructor.

**Synchronise the container clock instead.** Treats the environment as the bug.
Rejected: it does not survive a laptop sleeping, it cannot be enforced on a
contributor's machine, and it leaves a production query correct only by luck.
Drift between two machines is a normal condition, not a misconfiguration, and a
query that needs two clocks to agree is the thing that is wrong.

**Widen the window until drift stops mattering.** Would have made the seven
tests pass. Rejected outright: `ARMING_LOOKBACK_SECS` is sized to the gap it
covers (a model deciding to wait, then taking 84 seconds to get to the call),
and widening it starts surfacing work the model did earlier in the same turn as
though it were missed. Tuning a semantic constant to paper over a clock bug
would also have left production silently broken.
