# 0365: A refusal run alarms only when its refusals span the clock, and an unsigned run does not blame the secret

- **Status**: Accepted; amends
  [ADR 0235](0235-a-refused-delivery-is-an-outage.md) § "Both a count and a
  clock". Everything else in that ADR stands.
- **Date**: 2026-10-05

## Context

On 2026-10-04 and again on 2026-10-05 the refusal bar said: "3 deliveries have
arrived over 37 minutes, and none of them verified: no readable signature (3).
The public path is fine, so this is the secret or the signature config."

The secret was fine both times. A GitHub ping returned 202 right after. The three
refusals were unsigned POSTs from a local repair script, and all three landed
inside 61 seconds (05:52:52Z to 05:53:53Z). Nothing came in after them.

Every user with a public hook URL can get this. Internet scanners, health checks
and a user's own `curl` all send unsigned POSTs. Two faults sat behind it.

**The clock measured the wrong span.** ADR 0235 conjoined a count (3) and a
clock (30 minutes). The clock read `run_secs`, the age from the first refusal to
the check. Three refusals inside one minute, then silence, pass it half an hour
later. A burst followed by silence is not an outage.

**The words blamed the secret for evidence that does not point at it.** A sender
with a secret set sends the signature header on every delivery. GitHub does, and
so do most senders. So a run made only of `signature-missing` refusals says the
requests probably did not come from the sender. Or the sender has no secret set.
Rotating the secret is the one step that can break a working hook.

## Decision

**For a hook that is on, the refusals themselves must span the clock**, first
refusal to last. The count floor is unchanged. A switched-off hook keeps the old
clock, the run's age to now, and still declares on one refusal.

**A run of nothing but `signature-missing` refusals is reported in its own
words.** The bar and the Webhooks row say unsigned requests reached the hook,
possibly not from the sender. They tell the reader not to rotate the secret. A
run with any other reason in it keeps the verification words.

## Rationale

### The span is already on the row

`run_secs` is `now() - refusal_run_since` and `quiet_secs` is `now() -
last_refused_at`. Postgres measures both in one statement against one `now()`.
So `run_secs - quiet_secs` is exactly `last_refused_at - refusal_run_since`, and
`judge` still reads no host clock (ADR 0053). No column and no migration.

`last_refused_at` is always the run's newest refusal. One statement writes it and
the run together, and the probe skip in `api::webhooks` skips both.

### The rule covers every verification run, mismatch included

A burst is not an outage whatever its reasons. Three mismatches in one minute
and nothing after is one bad payload, or one stranger, and the next real delivery
settles it either way. A spread-out mismatch run still alarms, which is the
2026-09-18 shape ADR 0235 was written against: 42 deliveries over 27 hours.

The shapes ADR 0235 named still work. A busy broken hook keeps refusing until its
refusals span the clock. A quiet hook spans the clock between deliveries and
waits for the count.

### The disabled path cannot use the span

A switched-off hook declares on one refusal, because nothing was read and the
loss is certain. One refusal spans nothing. The run's age still keeps a straggler
arriving right after the user's own click from paging them.

### Unsigned is a wording, not a stored cause

`RefusalCause` stays two-valued. It is stored on the run, and a refusal of the
other cause restarts the run. If "unsigned" were a stored cause, one stray probe
would restart a 42-delivery mismatch run. The per-reason tally exists to stop
exactly that, so the wording reads the tally instead
(`utils/webhookRefusalNotice.ts`).

The wire `cause` stays `verification`, which is still true: the request reached
the verifier and failed it. A trigger can tell the unsigned shape apart from the
payload's `reasons`.

## Consequences

- **The two false alarms do not recur.** Pinned by
  `a_burst_of_refusals_followed_by_silence_declares_nothing` in
  `core/webhook_refusal_tests.rs`, built from the real shape.
- **A real outage whose only traffic is one burst is reported later.** It now
  waits for a delivery at least 30 minutes after the first refusal. Nothing on
  the row can tell that burst from a stranger's, so the next delivery is the
  first honest evidence.
- **A declaration standing on a burst run retracts once**, on the first cycle
  after upgrade. With no acceptance and no flag move, `resolution` names it
  `reconfigured`.
- **A steady unsigned stream still alarms**, for example a health check every
  ten minutes against a hook nothing else delivers to. It now says what that
  shape means instead of pointing at the secret.
- No wire value, event field, column or route changes.

## Alternatives considered

- **Keep the run's age as the clock and add a minimum gap between refusals.**
  Rejected: a second number to tune, and it still passes a burst once the gap
  rule is met by one late straggler. The span is one reading the row already
  has.
- **Apply the span only to unsigned runs, leaving mismatch on the old clock.**
  Rejected as a special case. A burst of mismatches is no more an outage than a
  burst of missing headers, and one rule is easier to state and to test.
- **Store a third cause, `unsigned`.** Rejected on the run's homogeneity, above.
- **Suppress unsigned-only runs entirely.** Rejected. A sender with no secret set
  produces exactly this run, and that is a real outage the owner needs to see.
- **Store the last refusal's span as a new column.** Rejected as a copy of what
  two existing ages already give, with a migration and a second writer to keep
  in step.

Plan: `docs/plans/2026-10-05-a-burst-is-not-an-outage.md`.
