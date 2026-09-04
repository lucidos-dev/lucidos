# 0143: Webhook ingress is checked from outside the machine, per resolved address and per address family

- **Status**: Accepted; the resolver fallback below is **revised by
  [ADR 0158](0158-webhook-ingress-verdict-needs-a-measurement.md)**. The walk
  now falls through an answer naming no record, and only a probe that was
  actually sent can degrade a family. The stage table is **extended by
  [ADR 0172](0172-a-blocked-port-is-not-a-dead-ingress.md)** with a seventh
  value. Everything else here still holds.
- **Date**: 2026-08-27

## Context

An inbound webhook delivery reaches a workspace over a public path. It arrives
at `tailscale funnel`, which forwards to the gateway's hook socket on loopback,
which forwards to the engine. The engine verifies the delivery and emits the
event the hook is configured for.

On 2026-08-26 that funnel began refusing IPv4 connections on its own. Thirty
consecutive GitHub deliveries returned 500 with an EOF in the TLS handshake.
Nothing was misconfigured. The daemon was 1.102.3, so this was not the known
1.102.1 regression. The serve config was intact, the node was online, the cert
was valid, and `tailscale funnel status` reported everything on.

The discriminator was address family. The funnel hostname has three public A
records and one AAAA. Over IPv6 the gateway answered 401, which is the correct
reply to an unsigned probe. That answer proved the whole chain behind the
ingress was healthy. All three IPv4 relays died in the TLS handshake, and
GitHub delivers over IPv4.

Nothing reported any of it. The Settings Webhooks page showed the hook as
active. The trigger row stayed green. The only symptom was events that never
arrived, and a nightly CI orchestrator was blind for eight hours.

Two facts from that diagnosis constrain any check worth having. A loopback
self-probe would have passed all night, so the check has to leave the machine
and come back over the public path. A single request to the hostname would have
passed all night too, because a dual-stack client prefers IPv6.

## Decision

Three layers. None of them repairs anything.

**Layer 0 records what a delivery leaves behind.** `webhooks` gains
`last_accepted_at`, `last_refused_at` and `last_refusal_reason`. They are
stamped on every delivery and shown on the page row.

**Layer 1 probes the public path.** Every 15 minutes the engine resolves the
funnel hostname against a public resolver. It then POSTs to a real hook once
per resolved address, with the hostname pinned for TLS and SNI. It expects 401.
The outcome is judged per address and per address family, never as one boolean.

**Layer 2 reports.** The engine emits `WebhookIngressDegraded` and
`WebhookIngressRecovered`, edge-triggered. It never notifies, never repairs and
never runs a mutating `tailscale` command. The workspace reacts to the events in
a trigger. The app shows the state on the Webhooks row and in a third app bar.

## Rationale

### A refusal is a signal; the absence of one is not

"Arrived and was refused" and "never arrived" have identical symptoms today and
completely different causes. A rotated secret is indistinguishable from a dead
funnel when the only evidence is silence. Layer 0 costs three columns and tells
them apart, which is most of the diagnostic value of this whole change.

### An unsigned POST proves the entire chain

The probe presents no valid credential and expects a 401. That one answer
proves TLS terminated at the funnel and a relay forwarded the request. It also
proves the gateway routed the workspace slug, the engine found the hook, and
the verifier ran and refused. No secret is needed, no event is emitted, and no
route is added. On the wire it is an ordinary wrong-token delivery.

The failure stage is the diagnosis, so the classification is the product:

| Result | Stage | Meaning |
|---|---|---|
| 401 | `healthy` | the full chain answered |
| handshake failure, refused, timeout | `ingress-unreachable` | the outage |
| 502, 503, 504 | `backend-unreachable` | ingress up, gateway or engine down |
| 404 | `route-missing` | wrong slug or hook id |
| anything else, 200 included | `unexpected-responder` | not Lucidos answering |
| no local route for the family | `local-stack-unavailable` | not judged |
| a route, and the port filtered outbound | `local-egress-blocked` | not judged |

The three backend codes are one stage because the gateway produces all three.
`hook_socket.rs` answers 502 when the engine hop fails, 503 when its
concurrency limit is full, and 504 when a delivery passes its 30-second
timeout. Each says the ingress carried the request and the far side could not
serve it.

### The resolver must be public, or the feature is a silent no-op

MagicDNS resolves the node's own name to its tailnet 100.x address on this
machine. A probe using the system resolver would take the internal path, never
touch the funnel, and pass forever. During the incident the truth only came out
of asking a public resolver directly.

So the lookup speaks DNS over HTTPS to `1.1.1.1`, and falls back to `8.8.8.8`.
Both are IP literals, so there is no bootstrap lookup and no system resolver
anywhere in the path. Any answer inside `100.64.0.0/10` is dropped before a
request is built, and a test pins that property.

**What "falls back" means is widened by
[ADR 0158](0158-webhook-ingress-verdict-needs-a-measurement.md).** As first
built, only a transport error reached the second endpoint, so a settled "no such
record" from the first one ended the lookup. One resolver answers that way
intermittently for a live funnel name, which reported a working ingress as
dead.

### A family that fails is degraded, even when the other one is perfect

This is the lesson of the incident, stated as a rule. Reporting one boolean for
the hostname reproduces the outage: the healthy family carries the verdict and
the dead one disappears. So every family gets its own verdict, and the event
carries both the per-address results and the per-family summary.

### Funnel discovery reads one command, and gives up rather than guessing

Reconnaissance ran `tailscale serve status --json` on a real machine before
anything depended on its shape. Four findings, all load-bearing:

```json
{
  "TCP": { "443": {"HTTPS": true}, "8443": {"HTTPS": true}, "9443": {"HTTPS": true} },
  "Web": {
    "<node>.<tailnet>.ts.net:8443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:5261" } } },
    "<node>.<tailnet>.ts.net:9443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:5261" } } }
  },
  "AllowFunnel": { "<node>.<tailnet>.ts.net:8443": true }
}
```

- `AllowFunnel` is the only thing that says funnel is on, and for which public
  port. Its keys are `host:port` and its values are booleans.
- `Web[key].Handlers[path].Proxy` is a URL string naming the loopback target.
  Its port is what identifies the hook socket.
- The hook port was reachable on 8443 (public) and 9443 (tailnet only). So the
  two maps must be intersected. Trusting either alone probes the wrong port.
- A client/server version-skew warning goes to stderr while stdout stays clean
  JSON. The parser reads stdout only.

Anything the parser cannot establish yields "do not probe". A guessed port
reports a phantom outage, which is worse than no check at all.

### Report, never repair

Reaching into the user's tailnet on a heuristic is not the engine's business.
The recovery step is sender-specific anyway. Asking GitHub to redeliver the
lost window is thirty lines against their deliveries API, and could never live
in Rust. So the engine stops at the events, and the workspace reacts in a
trigger, where the behaviour is visible and editable.

## The event payloads

A trigger is written against these, so they are a contract. Both events carry
aggregate `webhook` and the webhook's uuid as the aggregate id.

```json
{
  "type": "WebhookIngressDegraded",
  "data": {
    "webhook_id": "6f1c…",
    "webhook_name": "github-ci",
    "host": "<node>.<tailnet>.ts.net",
    "port": 8443,
    "degraded_families": ["ipv4"],
    "families": [
      { "family": "ipv4", "verdict": "degraded", "healthy": 0, "total": 3 },
      { "family": "ipv6", "verdict": "healthy", "healthy": 1, "total": 1 }
    ],
    "addresses": [
      { "address": "203.0.113.7", "family": "ipv4", "stage": "ingress-unreachable",
        "status": null, "detail": "tls handshake eof" },
      { "address": "2001:db8::1", "family": "ipv6", "stage": "healthy",
        "status": 401, "detail": null }
    ]
  }
}
```

`WebhookIngressRecovered` carries the same fields, with `recovered_families` in
place of `degraded_families`. It adds `down_since` (RFC 3339) and `down_secs`,
both measured from the degraded event it closes.

Field rules, so nothing has to be guessed:

- `verdict` is `healthy`, `degraded` or `not-probed`.
- `stage` is one of the values in the table above, kebab-case.
- `status` is the HTTP status when one arrived, and `null` when none did.
- `detail` is a short human string, present only on a failure.
- `families` always lists both families, including a `not-probed` one.

## Consequences

- **The page can no longer read "active" through an outage.** The row states
  the per-family verdict, and a third app bar shows while any family is down.
  The bar removes itself on recovery.
- **The bar reads its own state, never `connectionStatus`.** That signal is
  this client's own health poll. Reusing it would claim the app is offline
  while it is online.
- **Two consecutive failed cycles declare degraded, and one success recovers.**
  A single lost packet is not an outage. Recovery is deliberately faster than
  failure, because a stale warning is its own kind of lie.
- **Edge state is read from the events table each cycle**, so an engine restart
  during an outage cannot emit a second `WebhookIngressDegraded`. The same read
  supplies `down_since` for the recovery payload.
- **That read ignores which webhook the declaration named.** The ingress is one
  funnel, and the hook is only the probe target. Scoping the read per hook would
  strand a live warning the moment the probed hook was deleted or disabled.
- **No probe runs without something to protect.** No enabled webhook means no
  probe. A funnel serving something other than the hook port means no probe.
- **A gate that closes retracts a standing outage; a question we could not ask
  holds it.** No delivery can arrive once the funnel is gone or the last hook is
  disabled, so the warning would name a dead path. The retraction carries both
  families as `not-probed` and no addresses, so a trigger can tell it from a
  real recovery. An unreadable answer is different, because a resolver that
  never replied, or a `tailscale` CLI that hung, holds the outage instead. The
  read route drops a declaration separately, once no enabled webhook is left.
- **The warning is drawn on every enabled hook, not on the probed one.** The
  outage is a property of the shared path. Scoping it to the named hook would
  hide a live outage as soon as that hook was deleted. The next cycle would see
  no change and emit nothing, so the page would stay blind.
- **A staleness alarm is out of scope.** Webhook cadence is bursty CI, and a
  false "your webhook is broken" at 03:00 teaches the user to ignore the real
  one.
- **One new read-only route**, `GET /api/v1/webhooks/ingress`, so a cold page
  load can draw the bar. It sits with the CRUD and is not reachable from the
  hook socket.

## Alternatives considered

- **A loopback self-probe.** Rejected: it would have passed for all eight
  hours. The failure was in the public path, which loopback never touches.
- **One request to the hostname.** Rejected for the same reason. A dual-stack
  client prefers IPv6, and IPv6 was the healthy family.
- **One boolean for the hostname.** Rejected: it reproduces the outage in the
  report. Any-address-healthy calls the incident healthy.
- **The system resolver.** Rejected: MagicDNS answers with the node's own
  tailnet address, so the probe would take the internal path and pass forever.
  This is the trap most likely to defeat the feature silently, which is why it
  is a pinned test rather than a comment.
- **A DNS crate.** Rejected: `reqwest` is already a dependency, and DNS over
  HTTPS on an IP literal needs no bootstrap lookup. A resolver crate would add
  a dependency to do less.
- **Putting the probe in `lucidos-tailscale`.** Rejected: that crate has `libc`
  as its only dependency and must keep it (ADR 0014 §1). The probe needs an
  HTTP client, so it lives in the engine.
- **Re-arming the funnel when the probe fails.** Rejected: see "Report, never
  repair" above.
- **One "cannot check" outcome for every early return.** Rejected: the cycle
  gives up at nine gates, and only two of them mean the ingress is out of
  service. A funnel torn down, or a last hook disabled, retracts a standing
  outage: no delivery can arrive, so the warning would name a dead path. A
  resolver that never answered, or a `tailscale` CLI that hung, holds the
  outage instead. `PublicAddresses` therefore tells "DNS answered with nothing"
  from "no resolver answered", and `FunnelState` tells a torn-down funnel from
  an unreadable one. `decide` reads the per-family verdicts, so it recovers on
  positive evidence rather than on the absence of a complaint.
- **Rendering the outage age from `down_since` in the browser.** Rejected: it
  subtracts a server instant from a browser one, which is the skew ADR 0053
  banned. The engine measures `down_secs` in Postgres and sends it beside the
  instant. Nothing re-reads it while the outage stands, though, because the
  next frame arrives only on recovery. So the client stamps the browser clock
  when the answer lands, then ages the span by the gap to a later reading of
  it. An eight-hour silence reading "for 2 minutes" is the exact failure this
  feature exists to report.
- **Letting a failed refresh clear the bar.** Rejected: an engine that cannot
  answer is itself one way this path breaks, so a retraction right then hides
  the fault when it matters most. A refresh that fails keeps the value it
  already had (`failedIfFresh`), and only a first load records the failure.

### Two repairs to the handed-down design

Reconnaissance found both. This ADR records each one rather than resolving it
silently.

**The probe would have poisoned Layer 0.** A credential-less POST to a real
hook is a refusal, so `deliver` would stamp `last_refused_at` every 15 minutes.
The page would then read "last refused 2 minutes ago" on a healthy workspace,
burying the one signal Layer 0 exists to expose.

The probe therefore mints a fresh random token for each cycle and sends it as
the bearer. `deliver` refuses it exactly as it refuses anything else, with the
same 401 and the same body, and skips only the timestamp. Nothing changes on
the wire, so the probe stays indistinguishable to an outside observer.

Two sub-alternatives lost here. A **boot-lifetime token** was rejected because
an engine can run for months, and nothing needs the token to outlive its cycle.
Per-cycle minting also removes the need to pick a rotation period. A
**forgeable marker**, a distinctive `User-Agent` say, was rejected because it
would let an attacker hide a signature-guessing run from the page. A cleaner
marker is impossible anyway: the gateway drops every `x-lucidos-*` header from
a public caller, by design.

**A host with no IPv6 egress would have reported degraded forever.** The rule
"a whole family failing is degraded" is right about the ingress and wrong about
the local stack. A v4-only laptop cannot reach an AAAA record for a reason that
has nothing to do with the funnel.

So each family's local route is tested first, with a UDP `connect` that does a
route lookup and sends nothing. A family with no route reports `not-probed` and
never `degraded`. The lesson survives intact: a family that can be reached and
fails is still degraded, even when the other family is perfect.

Plan: `docs/plans/2026-08-27-webhook-ingress-health-checking.md`.
