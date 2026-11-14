# 0444: A webhook delivery's ledger row commits in its event's own transaction, so no claim can strand

- **Status**: Accepted
- **Date**: 2026-10-12
- **Amends**: [0122: A webhook claims a delivery before it emits](0122-a-webhook-claims-a-delivery-before-it-emits.md)

## Context

ADR 0122 dedupes a webhook resend with a ledger row per delivery. The row was a
*claim*, committed on its own before the domain event was emitted:

1. Claim the key. A row with no event id means "in flight".
2. Emit the event.
3. Record the event id on the claim, or release the claim if the emit failed.

Step 3 ran only if the handler survived step 2. A client that hung up (the
gateway times a delivery out at 30 seconds) dropped the handler future. A
restart killed it. Either way the claim stayed "in flight" with no event, and
every resend got a 503 until the dedupe window passed: an hour by default, up
to seven days. The sender gave up, and the delivery was lost.

A race audit filed it under "a marker written before the thing it records".

## Decision

The ledger row is written inside the domain event's own transaction, after the
bus opens it and before it inserts the event. The row stores the event id
from the start.

- A fresh key, or one past its window, takes the row and the event commits with
  it. The answer is 202.
- A key inside its window is a resend. The transaction rolls back and the
  answer is 200 with the first event's id.
- A second copy arriving while the first is still in its transaction blocks on
  the uncommitted key. It wakes to a duplicate if the first commits, and wins if
  the first rolls back.

The in-flight state, the 503, `release`, the claim token and the claim retry
loop are deleted. `event_id` is `NOT NULL`.

## Rationale

**A dropped future and a restart roll back.** The row and the event exist
together or not at all, so there is nothing left to strand and nothing to
release. The window is gone rather than covered.

**Waiting beats a 503.** The holder's transaction is milliseconds long. Making
the second copy wait for it gives the sender a real answer at once, where the
503 cost a retry at best.

**The bus owns the transaction, the ledger owns the SQL.** The insert is a
`DeliveryLedger` function that takes the transaction, so raw SQL on the table
stays in its owner (`core/announced_surfaces.rs`).

## Consequences

- The sender sees the same contract as before, minus the 503: 202 for a first
  copy, 200 with `duplicate: true` and the first event id for a resend.
- A waiting copy holds its in-flight append lock while it waits. A committed
  horizon read can wait behind it for as long as the holder's transaction runs.
- Rows written before this change with no event id are dropped by the
  migration, which frees their keys. Most were stranded claims. One could also
  be a delivery that emitted and then failed to record its event id. Its
  resend inside the window emits once more, which is the old scheme's own
  failure surfacing once at upgrade.

## Alternatives considered

- **Keep the claim and add a lease.** A claim older than a few seconds counts
  as abandoned. Rejected: it is a timer guessing at a race, and a slow emit past
  the lease emits twice.
- **Release the claim from a drop guard.** Rejected: a restart runs no guard,
  and the guard's own write can fail.
