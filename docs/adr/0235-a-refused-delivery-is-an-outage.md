# 0235: A refused delivery is an outage, and a switched-off hook says so itself

- **Status**: Accepted; the verification-layer sibling of
  [ADR 0143](0143-webhook-ingress-probed-per-address-family.md), and it changes
  nothing that ADR decided.
- **Date**: 2026-09-21

## Context

A webhook that refuses 100% of its deliveries reported perfectly healthy.

On 2026-09-02 04:41:06Z the "GitHub workflow runs" webhook was disabled by hand
from a browser. It stayed off for 18 days. GitHub kept delivering and every
delivery got a 401. The panel row stayed green, the ingress probe stayed green,
and nothing alarmed. A workspace audit re-enabled it on 2026-09-20 06:46:34Z and
the first 202 in three weeks arrived 22 seconds later.

A 23-minute transport wedge on 2026-09-21 raised the ingress bar inside two
probe cycles. That asymmetry is the bug.

**The ingress probe measures transport, and nothing else.** It dials the public
funnel and reads a 401 as the healthy answer, because an unsigned probe is
exactly what a live verifier turns away. So a hook that completes every
handshake and then refuses every body passes it perfectly, by design.

The engine already held the answer. `webhooks.last_accepted_at`,
`last_refused_at` and `last_refusal_reason` were populated and correct
throughout, and nothing read them.

A second incident sits beside it and wants different words. For about 27 hours,
2026-09-18 04:57Z to 2026-09-19 08:43Z, 42 consecutive deliveries arrived with a
well-formed `X-Hub-Signature-256` and were all answered 401. That one really was
a verification failure. The disabled one was not, and the two are
indistinguishable from the sender's side.

## Decision

**A second reading, on the same 15-minute cadence, over the rows the delivery
path already writes.** It makes no network request. Where the ingress check asks
whether a sender can reach the workspace, this asks what happened to a delivery
that did.

Three parts.

**A refusal run on the `webhooks` row.** `refusal_run_count`,
`refusal_run_since`, `refusal_run_cause` and a per-reason `refusal_run_reasons`
tally. An acceptance ends a run, and every refusal of the same cause appends to
one. Every age it is judged on is measured by Postgres, so `judge` reads no
host clock at all (ADR 0053).

**A typed cause, split by one predicate.** `DeliveryRefusal::Disabled` joins the
enum, and `DeliveryRefusal::examined_the_delivery()` is the mirror of
`Stage::measured_the_ingress` from [ADR 0172](0172-a-blocked-port-is-not-a-dead-ingress.md).
One arm answers no. `RefusalCause` is derived from that predicate alone, so
`disabled` and `verification` cannot be reported in the same words.

**One event pair, edge-triggered.** `WebhookDeliveriesRefused` declares and
`WebhookDeliveriesRecovered` retracts, per webhook, read back off the events
table every cycle exactly as the ingress declaration is.

## Rationale

### The 18-day case is the one the existing prober refuses to run for

This is the load-bearing reason the check is its own scheduler job rather than a
step inside the ingress cycle. That cycle stops on three gates, one being *no
webhook is enabled*, and a closed gate retracts whatever stands.

In a workspace with one hook, switching that hook off IS that condition. So the
prober cannot see a disabled hook still taking deliveries, by construction, and
adding a step inside it would have inherited the same blindness. The new cycle
judges every hook, enabled or not.

### A tally, not a history, and not a single stamp

`last_refusal_reason` holds the LAST refusal. That tells a dead hook from a
quiet one, which is what ADR 0143 added it for. It is not enough to survive
being read.

**A diagnostic probe overwrites exactly the field you came to read**, and it did
during this very investigation: an unsigned `curl` replaced 42 `disabled`
refusals with one `signature-missing`.

Three fixes were available. The tally wins on cost against what it buys.

| Shape | Cost | What it survives |
|---|---|---|
| a bare counter | one column | the count, but not which reason |
| a **per-reason tally** | three columns, one `UPDATE` | the count, the start, and the breakdown |
| a bounded history | a row per refusal, plus a sweep | the above, plus per-delivery timing |

A history is the only one that answers "when did each arrive", and nothing here
asks that. The engine already refused a delivery log once
([ADR 0122](0122-a-webhook-claims-a-delivery-before-it-emits.md)), for the
reason that still holds: a public endpoint takes thousands of unsigned probes,
and each one would be a stored row.

The tally is O(1) in storage, needs no sweep, and is written in the same
statement as the stamps it sits beside. A stray probe then adds one to its own
reason and leaves the forty-two beside it standing.

**A run carries its cause, and a refusal of the other cause restarts it.** That
is what makes the count, the start and the tally describe ONE fault. Without
it, a hook switched off after an hour of signature failures reports forty-one
deliveries "thrown away before they were read". It would tell its owner the
secret is fine, while forty of them had failed exactly that check.

**The engine's own probe never reaches the tally at all.** It presents a bearer
this engine minted for the cycle, and `api::webhooks::is_probe_delivery` skips
the stamp on a match. That mechanism already existed for the same reason
(`core/webhook_probe_token.rs`), so the run inherits it rather than growing a
second one.

### The disabled state is distinct because nothing was verified

`deliver` checks `enabled` before it reads the body, looks up the credential, or
verifies anything. So a disabled hook's 401 is a configuration fact and says
nothing about the signature or the secret.

Reporting it in the verification words is not a cosmetic slip. It is the exact
wrong turn this fault already caused: a long 2026-09-21 investigation into HMAC
for a hook somebody had simply turned off. The knowhow file warns a human about
it in prose; this puts the distinction in a type.

It is also why `DeliveryRefusal::Disabled` had to be an enum arm rather than the
hand-written string it was. A reason outside the enum cannot be classified,
cannot key a tally, and drifts from `reason()` the first time either is
reworded. `BodyNotUtf8` was the second such string and joined for the same
reason.

### Both a count and a clock, because either alone fails a real hook

This hook's traffic has two shapes at once. One workflow run delivers three
times in quick succession (`requested`, `in_progress`, `completed`), and the
hook can then be quiet for days: only `install-smoke` carries a `schedule:`
block, so a silent week is its expected state.

| Rule | How it fails |
|---|---|
| time alone ("any refusal in the last 30 minutes") | false-negative on the quiet hook, which has nothing inside any useful window |
| count alone ("3 consecutive refusals") | cannot tell three refusals in four seconds, one bad payload, from three over three days |

So the floors are conjoined, and each handles the shape the other misses. A busy
hook reaches the count in seconds and then waits out the clock. A quiet hook
passes the clock at once and waits for the count.

The four constants live in `core/webhook_refusal.rs` with this reasoning at each
one:

| Constant | Value | Why |
|---|---|---|
| `REFUSALS_BEFORE_DEGRADED` | 3 | one workflow run's worth |
| `DISABLED_REFUSALS_BEFORE_DEGRADED` | 1 | nothing was read, so the loss is certain |
| `REFUSAL_RUN_BEFORE_DEGRADED_SECS` | 1800 | the patience the ingress check already spends |
| `REFUSAL_RUN_GOES_QUIET_SECS` | 1209600 | longer than any quiet stretch this hook has shown |

**One refusal is enough for a switched-off hook**, and that asymmetry is the
point rather than an exception. There is nothing to be uncertain about: the
delivery was thrown away before anything was read. The clock still applies, so a
straggler arriving right after the user's own click cannot page them.

### Recovery is positive evidence, and every way out has one

ADR 0143 requires positive evidence to retract, and a fault this narrow has four
ways to end. Leaving any of them out would strand a bar nothing could clear,
which is the failure both ADR 0143 and
[ADR 0158](0158-webhook-ingress-verdict-needs-a-measurement.md) guard against.

| Resolution | The evidence |
|---|---|
| `accepted` | `last_accepted_at` is newer than the declared run's start |
| `reconfigured` | the enabled flag moved, so the fault named is not the live one |
| `quiet` | nothing has arrived for a fortnight |
| `removed` | the webhook is gone |

`quiet` is the one that is easy to skip and expensive to omit. Without it a red
bar stands for good in one case: a user switches a hook off deliberately, the
sender goes away, and nothing arrives to retract it.

**`accepted` is read off the row, never inferred from an empty run.** The two
look equivalent and are not: a fresh refusal restarts the run, so "empty" goes
false again within minutes of the delivery that verified. A hook with two
senders hits that on every recovery, and a trigger routing on `accepted` would
never fire for it.

### Two bars, not one with two messages

The two faults are independent and can stand together: the public path can be
wedged while a hook is also switched off. They also want different actions.

A bar that had to say both would say neither clearly. The ingress bar's Discuss
button suits a fault diagnosed address by address, and this one already names
its own cause.

## Consequences

- **A disabled hook still taking deliveries is reported within about an hour**,
  against 18 days. The bound is the traffic: the engine can only count what
  arrives.
- **The evidence of an outage survives being investigated.** A hand-run probe
  adds one to its own reason instead of erasing the rest.
- **A reason outside `DeliveryRefusal` is now unrepresentable.** `record_refused`
  takes the enum, so no caller can invent one, and the tally keys on a closed
  set that no sender can influence.
- **`core/webhook_refusal.rs` holds no clock.** Postgres measures every age it
  reads, so a drifting database cannot silence the check (ADR 0053). That is
  what the two computed columns in `WEBHOOK_COLUMNS` are for.
- **A retraction reports the WHOLE run, and stops at the acceptance.** Two
  errors are available and it avoids both. The declaring event's own age
  understates by the 30-minute floor at least. On a hook quiet enough to take a
  week reaching the count, it understates by that week. Aging to the cycle that
  noticed overstates by up to one 15-minute interval. An acceptance is an
  instant the database stamped, so a recovery it caused is dated there.
- **Two new wire values a workspace trigger can code against**, on the model of
  ADR 0143's payloads: `WebhookDeliveriesRefused` and its retraction.
- **The ingress check is untouched.** Its stages, verdicts, debounce and events
  are exactly as they were. This is a second, independent reading.
- **`GET /api/v1/webhooks` grew a nullable `refusal_run`**, which is what
  `lucidos webhooks list` prints and where a diagnosis now starts.
- **A fourth app-shell banner**, with its own height property. The reservation
  rule in `components/layout/appBanner.ts` already required that.
- **A quiet hook is still slow to report.** Three refusals is three refusals,
  and a hook that takes one delivery a week takes three weeks to reach it. The
  disabled case dodges this by needing one, and the verification case does not.

## Alternatives considered

- **Widen the ingress probe to POST a correctly signed body.** Rejected, and not
  close. The engine would have to hold a sender's signing secret in a form it
  can sign with. It would then either emit a synthetic domain event or suppress
  one, and every cycle would fire the workspace's trigger. The probe's whole
  value is that it proves the path while changing nothing.
- **Read the outcome stamps directly, with no run.** Rejected: that is the state
  the bug was found in. One stray probe makes
  `last_refused_at > last_accepted_at` true. A rule over the two columns then
  either pages on one bad payload, or needs a window it cannot measure.
- **A bounded refusal history instead of a tally.** Rejected on the table above.
  It answers a question nothing asks, and ADR 0122 already refused its shape.
- **Two event pairs, one per cause.** Rejected: four variants, two declaration
  readers and two bars, to express what one `cause` field expresses. ADR 0172's
  `LocalEgressBlocked` is the precedent, and it is a variant inside an existing
  vocabulary rather than a parallel one.
- **A step inside the ingress cycle.** Rejected on the gates. It would inherit
  the blindness that hid the original incident, which is the one thing this must
  not do.
- **A debounce counter in memory, like the ingress check's two strikes.**
  Rejected as a worse version of what the row already holds. The run counts real
  deliveries rather than probe cycles, and it survives a restart.
- **Declare from the read route rather than gating on the declaration.**
  Rejected: the bar would rise before any event existed. A user would then see a
  fault with nothing on the timeline and no retraction to pair it with.
- **Retract a disabled declaration only on a verified delivery.** Rejected. Being
  switched on is the symmetric fact to being switched off, and it is observable
  with certainty. Holding the bar until traffic happens to arrive would keep
  telling the user to turn on what they just turned on.
- **Reset the run whenever the hook is reconfigured.** Rejected as a second
  mechanism for what the cause column already handles. The run restarts when a
  refusal of the other cause arrives, and a re-enabled hook's disabled run
  simply stops matching the live flag. Nothing is erased on a config change
  nobody has delivered against yet.
- **Derive the cause from the tally instead of storing it.** This shipped first
  and was wrong. A run spanning a flag toggle then reports the other cause's
  count, start and breakdown, which is the one thing the split exists to
  prevent. Storing it makes the run homogeneous by construction.
- **Add a confirmation dialog to the Disable button.** Out of scope here, and
  worth its own decision. The reachable half of this incident is a one-click
  toggle that silently ends every delivery. A guard on the click is still a
  different change from reporting the consequence.

Plan: `docs/plans/2026-09-21-a-refused-delivery-is-an-outage.md`.
