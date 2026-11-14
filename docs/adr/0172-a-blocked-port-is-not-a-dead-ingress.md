# 0172: A blocked local port is not a dead webhook ingress

- **Status**: Accepted; extends
  [ADR 0143](0143-webhook-ingress-probed-per-address-family.md) with a seventh
  stage.
- **Date**: 2026-09-01

## Context

The ingress probe dials the public funnel relays from the host it monitors, and
reads any transport error as `ingress-unreachable`. A probe running on the
monitored host cannot tell "I cannot reach it" from "it cannot be reached".

That gap declared two false outages inside four days. On the second one the
engine reported `a130610.tailcb34bd.ts.net:8443` degraded over IPv4, with all
three relays timed out. GitHub's own ping to the same hook returned 202 at the
time. The network the Mac sat on filters outbound traffic on the funnel port.

ADR 0143 already handles the family-level version of this question. A host with
no IPv6 route reads `local-stack-unavailable` and declines to blame the ingress.
The missing question is one level down: the route exists, and the port still
does not get through.

## Decision

A seventh stage, `local-egress-blocked`. It joins `local-stack-unavailable` as a
fact about this host, so a family reading it comes out `not-probed` and never
`degraded`.

The check runs only for a family whose every address failed at transport. It
compares two raw TCP dials: the funnel port, and a **reference port** of 443, on
the addresses this cycle already resolved. Silence on the funnel port plus an
answer on 443 is a blocked local egress. Anything else leaves the reading as it
was.

## Rationale

### A dial can only ever prove that a port works

A completed handshake proves it. So does a refusal: the packet left, and an
answer came back. Silence proves nothing on its own, because a target that
serves nothing on that port is silent too.

Measured from the affected host, both effects appear at once:

| Dial | Result |
|---|---|
| `1.1.1.1:443` | connects, 15 ms |
| `1.1.1.1:8080` | connects, 20 ms, then the TLS handshake fails |
| `1.1.1.1:8443` | silent |
| `8.8.8.8:8080` | silent |
| `185.40.234.37:443` | connects, 42 ms |
| `185.40.234.37:8443` | silent |

`8.8.8.8:8080` is silent on the same network, in the same minute, where
`1.1.1.1:8080` completes a handshake. Silence on a port is therefore a property
of the target as much as of the network.

### So the reference port supplies the missing half

An address that answers on 443 and stays silent on the funnel port is reachable,
and the port is not. That is the measurement the manual diagnosis used, and it
is the only cheap one that discriminates.

The addresses come from the DNS-over-HTTPS answer this cycle already has, so the
check adds no lookup and asks the monitored host nothing. They are Tailscale's
shared ingress relays rather than the workspace's own machine.

### Blocked needs positive evidence on both legs

A relay that is merely down is silent on 443 as well, and that reads `Unknown`.
Unknown keeps today's `ingress-unreachable`, per the rule in
`.claude/rules/rust.md` that a probe which could not run is never a "no".

### The one fault this reading cannot tell apart

A relay that answers on 443 and *silently drops* the funnel port looks exactly
like a filtered local egress. That residual risk is accepted, on three grounds.

The concrete ways a funnel port fails do not have that shape. A listener that
went away answers with a reset, which this check counts as an answer and leaves
as `ingress-unreachable`. The relay wedge ADR 0143 was built for completes the
handshake and dies in TLS, which is also an answer. A relay that is gone
entirely is silent on 443 as well, which reads `Unknown`.

The hostname has three A records in different networks, so a relay-side drop
would have to hit all three the same way. A local filter hits all three by
construction.

The reading is never silent. The cycle logs the host, the port and the blocked
family, so a suppressed reading leaves a trail even though it emits no event.

### A funnel on 443 is not asked

There is no reference port to compare against. It costs nothing, because 443 is
the port a network almost never filters, so the false reading does not arise on
it.

### The check is family-agnostic

IPv4 is where the bug appeared, but a host with an IPv6 route and a filtered
IPv6 port has the identical fault. One path for both families is smaller than an
IPv4 special case.

## Consequences

- **A family the engine could not measure is silent, not degraded.** The bar is
  not drawn, no `WebhookIngressDegraded` is emitted, and a standing declaration
  is neither repeated nor retracted. ADR 0143's rule that only positive evidence
  recovers an outage already covers the retraction half.
- **`Stage::measured_the_ingress` is now the single filter `judge` reads.** A
  third local stage cannot be added and forgotten on the way into a verdict.
- **The wire vocabulary grew by one value and changed none.** A trigger coding
  against the six existing strings is unaffected. A payload carrying a stage a
  reader does not know is still dropped whole rather than half.
- **The distinction reaches a human only beside a family that did fail.** A
  degraded IPv6 event can carry an IPv4 address reading `local-egress-blocked`,
  with the detail "this host cannot reach port N on any address".
- **The failure path costs up to six extra dials per cycle**, each capped at two
  seconds against the request's fifteen. The healthy path costs none.
- **A genuine relay wedge is untouched.** It completes the TCP connect and dies
  in the TLS handshake, so the funnel-port leg answers and the reading stays
  `ingress-unreachable`.

## Alternatives considered

- **Dialling an unrelated public host on the funnel port.** This is the obvious
  design and the measurements above rule it out. `1.1.1.1:8443` is silent on the
  blocked network, and there is no reason to think it answers on a healthy one.
  Counting that silence as evidence would silence a real outage on every network
  where the control happens to drop the port.
- **Requiring an off-path control as a second condition.** Rejected as cost
  without discrimination. A control that drops the port is silent in both the
  blocked and the healthy case, so adding it changes no verdict.
- **Reading the failure shape out of the `reqwest` error.** A connect timeout
  and a slow response both surface as `is_timeout`, so a wedged gateway would
  read as a blocked port. The raw dial answers the question directly.
- **Comparing against a different reference port, 80 say.** Rejected: 443 is the
  port the relays certainly serve, and a plaintext port proves less about a TLS
  ingress path.
- **Skipping the probe entirely on a network known to filter.** Rejected: the
  engine cannot know that in advance. A probe that declines to run also reports
  nothing, rather than reporting what it could not measure.
- **Treating a blocked family as `degraded` with a softer message.** Rejected:
  the whole point is that the engine measured nothing. ADR 0143 pins
  `not-probed` as the reading for exactly that, and two spellings of "we do not
  know" would split the trigger's logic.

Plan: `docs/plans/2026-09-01-a-blocked-port-is-not-a-dead-ingress.md`.
