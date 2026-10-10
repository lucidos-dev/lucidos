# 0158: A webhook ingress verdict requires a measurement, and one resolver's refusal is not the answer

- **Status**: Accepted
- **Date**: 2026-08-29

## Context

ADR 0143 built the ingress check: resolve the funnel hostname against a public
resolver, probe every resolved address, and judge each address family on its
own. It shipped, and then it cried wolf.

The engine emitted `WebhookIngressDegraded` for a funnel that was answering. The
event claimed both IPv4 and IPv6 were unreachable, with `addresses: []`,
`healthy: 0` and `total: 0` on both families. The host was verified by hand the
same morning, through both public addresses, and real deliveries had landed
overnight.

Two defects stacked to produce it.

**The DoH walk stopped at the first parsed answer.** `resolve_one_type` advanced
to the next endpoint only on a transport error or an unreadable body. A settled
NXDOMAIN parses, so it ended the walk.

**An empty address list became a verdict.** `judge_no_public_record` mapped "the
resolvers named nothing" to `degraded` on both families, over zero requests.

A measurement settled the first one. Ten paired queries for the live funnel
hostname, A and AAAA:

| Endpoint | A answered | AAAA answered |
|---|---|---|
| Cloudflare | 3 of 5, two were NXDOMAIN | 4 of 5, one was NXDOMAIN |
| Google | 5 of 5 | 5 of 5 |

Cloudflare refuses `ts.net` funnel names intermittently, not permanently. One
round refused A and AAAA together, which is the exact shape that reached
`NoRecord`. A quieter case sat beside it: when A is refused and AAAA answered,
the engine probes IPv6 alone and reports IPv4 as `not-probed`. Nothing is
emitted and nothing looks wrong, yet the IPv4 relays go unwatched, which is the
failure ADR 0143 exists to catch.

## Decision

Two rules, both narrower than they sound.

**The endpoint walk ends on a non-empty answer, not on any answer.** An endpoint
that settles the question with no usable record is recorded as having answered,
and the walk continues. The endpoint list is unchanged and unreordered.

**A family verdict requires at least one attempted request.** `judge` is the
only producer of a family verdict, so no path can pronounce over zero
measurements. A hostname the resolvers named no record for leaves the cycle
undetermined: it emits nothing, and a standing outage keeps standing.

This narrows ADR 0143 in one place: what "falls back to `8.8.8.8`" means. That
ADR never said what a hostname with no public record should mean. The
implementation filled the gap by calling it a total outage. So the reversal is
of the code, and of the reading of 0143 it stood for. That ADR's Status line
and its resolver section both point here now.

## Rationale

### A refusal one resolver gives and its neighbour does not is not a fact

The whole reason ADR 0143 asks a public resolver is that the answer must come
from outside this machine. Two endpoints exist so one can be wrong. Taking the
first parsed answer threw that away: the fallback was reachable only through a
transport error, which is the one failure mode the public resolvers almost never
have.

The measurement is what makes this a bug rather than a preference. One resolver
answered NXDOMAIN for a name it resolved on the next query, minutes apart, from
the same client. That is not a property of the hostname.

### Three outcomes, still three

`PublicAddresses` tells "there is no such record" from "nobody told us", and
that is its whole value. The fix must not blur the two while widening the walk.
So the walk tracks whether ANY endpoint answered:

| Outcome | Meaning |
|---|---|
| a non-empty list | at least one endpoint named an address |
| an empty list | every endpoint answered, and none named a record |
| nothing known | anything less than that |

Collapsing the middle into the last would be the mirror-image bug: a host that
genuinely has no AAAA record would read as a resolver failure.

**The middle row needs unanimity, which is the same lesson once more.** A lone
"no such record" beside a silent neighbour settles nothing, because that answer
is what the measurement showed to be unreliable. Both readings hold a standing
outage today, so the difference is in the log line. It keeps the type honest for
whatever reads it next.

### Never call something unreachable without reaching for it

A health check earns its warnings by measuring. `judge_no_public_record`
returned `degraded` with `total: 0`, which said, in one payload, both "this
family failed" and "nothing was sent". The second half refutes the first.

Deleting it rather than repairing it is the load-bearing part. It was the only
thing in the tree that could build a degraded family with no probes, so its
removal makes the property structural: every verdict now comes out of `judge`,
over real per-address results.

### An unmeasured cycle holds, it does not retract

ADR 0143 already drew this line for a wedged daemon: a gate that closes retracts
a standing outage, and a question we could not ask holds it. A hostname with no
record joins the second group. Nothing was probed, so the cycle emits neither
event and the debounce chain breaks.

That is deliberately not the same as calling the path healthy. A resolver
answering "no record" while an outage stands is not evidence of recovery, and
recovery in this design requires positive evidence.

### The v4-only host survives

The repair ADR 0143 made for a machine with no IPv6 egress still holds, through
a different route. A family with no probed address is `not-probed`, which
degrades nothing. `judge` answered that way already, so routing `NoRecord` away
from a verdict cost nothing there.

## Consequences

- **A phantom total outage is unreachable.** Reporting a family down now costs at
  least one request that left the machine.
- **An IPv4 blind spot closes.** A refused A record no longer means the IPv4
  relays go unprobed while IPv6 carries the verdict.
- **A resolver wobble is silent rather than loud.** It leaves the previous state
  alone, which is right in both directions: no false alarm, and no false all
  clear during a real outage.
- **One cycle of latency in the worst case.** A cycle that measured nothing
  clears the two-strike memory, so a real outage starting during a resolver
  wobble is declared one cycle later.
- **The event payloads are unchanged.** `verdict` is still `healthy`,
  `degraded` or `not-probed`, and a trigger written against ADR 0143 keeps
  working. There is deliberately no fourth verdict: an unmeasured cycle emits no
  event at all, so nothing needs a word for it.
- **The read route carries more.** `GET /api/v1/webhooks/ingress` gained
  `webhook_name` and the probed `addresses`, so the notice can hand a reader,
  or an agent, the diagnosis rather than the headline.

## Alternatives considered

- **Add a third DoH endpoint, or drop the one that refuses.** Rejected: it
  treats a symptom. The exit condition was the bug, and three endpoints sharing
  a wrong exit condition fail the same way whenever the first one flaps.
- **Retry the same endpoint before moving on.** Rejected: the second endpoint IS
  the retry, and it is a better one, being a different operator. A per-endpoint
  retry also spends the cycle's time budget on the resolver least likely to
  answer.
- **Keep `NoRecord` as a degraded verdict, but only after N cycles.** Rejected:
  debouncing a claim does not make it true. The engine would still be reporting
  a family unreachable having sent it nothing, just less often.
- **Add a fourth `verdict` value, `unknown`.** Rejected: it widens a payload a
  workspace trigger already codes against, to describe a cycle that emits no
  event. Nothing would ever read the value.
- **Treat a hostname with no record as a closed gate, and retract the outage.**
  Rejected: it is the wrong half of ADR 0143's own distinction. A funnel torn
  down is a gate closing, and a resolver naming nothing is a question that went
  unanswered. Retracting on it would clear a live warning during a DNS outage,
  which is exactly when deliveries are most likely to be failing.
- **Judge each record type as its own family.** Rejected as out of scope, and
  probably wrong. An unanswered AAAA question would then drag the whole cycle
  down, discarding a good IPv4 measurement over a resolver hiccup. Today an
  unanswered family reports `not-probed`, which degrades nothing.

Plan: `docs/plans/2026-08-29-webhook-ingress-verdict-needs-a-measurement.md`.
