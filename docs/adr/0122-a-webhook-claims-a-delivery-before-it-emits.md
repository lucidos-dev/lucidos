# 0122: A webhook claims a delivery before it emits, so a resend fires the event once

- **Status**: Accepted
- **Date**: 2026-08-24

## Context

A verified delivery emits the webhook's pinned domain event. Nothing recorded
that it had happened, so a sender resending the same delivery emitted the event
again.

Senders resend as a matter of course. GitHub retries a slow or non-2xx response
and offers a manual Redeliver button. Stripe retries for days. The engine could
not tell a retry from a new event: the two are byte-identical requests.

The cost is higher here than in a point-to-point agent runtime. The event goes on
the bus, so one duplicate is multiplied by every subscriber, and it lands on the
append-only log where nothing removes it.

ADR 0097 built the public surface this arrives on. The webhooks plan beside it
recorded two adjacent non-goals, and this reverses half of one.

## Decision

A `webhook_deliveries` table, keyed on `(webhook_id, delivery_key)`. Before
emitting, a delivery claims its key in one statement:

```sql
INSERT INTO webhook_deliveries (webhook_id, delivery_key)
VALUES ($1, $2)
ON CONFLICT (webhook_id, delivery_key) DO UPDATE
    SET created = NOW(), event_id = NULL
    WHERE webhook_deliveries.created < NOW() - make_interval(secs => $3)
RETURNING webhook_id
```

A row comes back to the caller that owns the delivery, whether the key was fresh
or its previous claim had aged out. No row means a live claim holds it, so the
delivery is a resend. The caller then emits and records the event id against its
own claim token, and a resend answers 200 with that id and `duplicate: true`.

**A resend whose predecessor has not emitted yet answers 503 instead**, so the
sender keeps retrying until there is a real answer either way.

**Deduping is opt-in and off by default.** A hook dedupes only when its `dedupe`
block names a window, and `window_secs: 0` switches it back off.

**The key is named as data**, the shape `hmac` already uses: `dedupe.header`
says which header carries the sender's delivery id. With no header configured,
or none present on a request, the key is a digest of the body.

**A failed emit releases the claim** and answers 500. A claim that could not be
taken at all also answers 500, without emitting.

**The event payload becomes `{summary, headers, payload}`.** The sender's body
moves under `payload`, and a per-hook allow-list of request headers lands under
`headers`.

## Rationale

**The claim is one statement because two copies of one delivery can be in
flight.** Read-then-decide-then-write has no lock to take, and an in-memory
cache cannot serialise across them at all. The primary key does it for free, and
survives the restart that most often provokes the retry.

**Every write after the claim carries its token.** An emit slower than its own
window can be superseded mid-flight. A keyed-only update would then stamp the
loser's event onto the winner's row, or delete it, and the token makes both
no-ops instead. A key found held by nobody is likewise re-claimed rather than
called a duplicate: that state only arises when the holder released a failed
emit, so the key is genuinely free.

**Off by default is the interesting default, not the timid one.** With no
dedupe, every arrival is an event, so the log answers "how often does this sender
resend". Turning dedupe on is choosing not to see that. The two things a user
might want are one setting apart, and neither needs a delivery-count column
nobody can read.

**The fallback to a body digest is safe because the key authenticates nothing.**
It runs strictly after verification and decides only whether this delivery has
been seen. Both forms are digested and prefixed apart. So a public caller's
header value is never stored raw, and a body cannot key the same claim as a
header.

**Failing closed is the cheap side.** The sender owns retrying (see below), so a
refused delivery costs one retry. Guessing that an untaken claim was won costs a
duplicate event on the permanent log.

**Wrapping the payload makes a collision impossible rather than adjudicated.** A
sender with its own `headers` field is simply `payload.headers`. The alternative
was a rule about who wins, which is a special case someone has to remember. It
also removes the older `body` special case, so a delivery has one shape whether
or not the body parses as JSON. ADR 0119's field paths are what make the wrapped
form addressable, and webhooks are unreleased, so no stored condition breaks.

**Headers are allow-listed, never filtered.** `Authorization` and the hook's own
signature header arrive on every delivery. The events table is append-only, so a
carried secret is a permanent one, and a refusal at create reaches whoever
configured the hook. A deny-list would go stale; an allow-list cannot.

**`Authorization` is also refused as a dedupe key.** The bearer token never
changes, so every delivery would resolve to one key and only the first would ever
emit.

## Reversing a recorded non-goal

`docs/plans/2026-08-19-webhooks-and-engines-off-the-network.md` ruled out three
things. One is reversed, two stand.

- **"A `last_called_at` column would be an unannounced write on the hot path."**
  Reversed. That column was refused because it bought nothing for its cost. Here
  the write IS the mechanism: with no durable claim there is no way to recognise
  a retry.
- **"A webhook delivery log."** Still refused. This table holds no payload, is
  listed nowhere, and its only reader is the next delivery. It is a nonce
  ledger. The emitted domain event remains the record of what arrived.
- **"Retries and redelivery. The sender owns that."** Unchanged, and this
  depends on it. The sender still owns retrying; the engine only stops counting
  one delivery twice.

## Consequences

- The write path gains one round trip per delivery, and only for a hook that
  opted in.
- A daily sweep drops claims past `MAX_WINDOW_SECS`. That constant is also the
  cap on a configurable window, so the sweep cannot drop a claim still deciding
  a duplicate.
- Deleting a webhook takes its claims with it, through the foreign key.
- A resend arriving while the first copy is still emitting answers **503**, not
  200. The holder can still fail and release. "Already handled" would then be a
  promise nobody kept, and the sender would stop retrying a delivery that
  emitted nothing. One extra request beats losing it.
- A consumer can still dedupe for itself: allow-list the delivery-id header and
  it reaches the payload.

## Alternatives considered

**Dedupe in the trigger.** Rejected. It is per-subscriber, so the timeline still
shows two events and a second trigger still fires twice. A script trigger has no
atomic primitive, only a state file two concurrent deliveries both miss. An
intent trigger is a prompt, so exactly-once bookkeeping would be procedure in
knowhow, executed by a model. It also needs the delivery id in the payload,
which is an engine change either way.

**An in-memory cache**, as the nearest comparable agent runtime uses. Rejected.
It loses the window on restart, which is exactly when a sender is retrying, and
it cannot serialise two concurrent copies of one delivery.

**On by default, with a body-digest fallback.** Rejected. It closes the gap on
existing hooks with no action from anyone. But it silently collapses two
genuinely distinct deliveries whose bodies match, so a hook fed something like
`{"status":"ok"}` loses a real event.

**A per-route rate limit**, which the comparable runtime also has. Not rejected,
just separate. The hook socket already carries a body cap, a request timeout and
a concurrency ceiling (ADR 0097), and rate limiting answers a different failure.

**A `hits` counter on the claim row**, so a duplicate rate is readable with
dedupe on. Rejected as state with no surface to read it. Leaving dedupe off
answers the same question with the event log.
