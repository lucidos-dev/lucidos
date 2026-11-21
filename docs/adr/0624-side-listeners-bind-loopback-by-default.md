# 0624: The hook socket and public listener bind loopback by default, widened by env for containers; Slack hooks echo url_verification

- **Status**: Accepted
- **Date**: 2026-11-07

## Context

Lucidos also runs in containers, for example a team deployment in a Kubernetes
pod. There a Service or Ingress reaches the pod IP, never the pod's loopback.

Two gateway listeners were loopback, always: the *hook socket* (ADR 0097) and
the *public listener* (ADR 0615). No container could expose either. The main
port was already configurable through `LUCIDOS_GATEWAY_BIND_ALL` and
`LUCIDOS_GATEWAY_BIND_ADDR`.

Slack's Events API also refused every Lucidos Request URL. Slack posts a signed
`url_verification` body and wants its `challenge` echoed back with a 200. The
engine answered 202 and stored the challenge as an ordinary domain event.

## Decision

- `LUCIDOS_HOOK_BIND_ADDR` and `LUCIDOS_PUBLIC_BIND_ADDR` take an IP literal
  for those two listeners. Unset, they bind `127.0.0.1` as before. An
  unparseable value logs and stays on loopback.
- A verified delivery to a hook whose `signature_header` is `X-Slack-Signature`
  that carries a `url_verification` body gets `200 {"challenge": ...}`. It
  emits no event and writes no dedupe row.

## Rationale

- **Loopback stays the default.** ADR 0097's reasoning holds for every desktop
  install: funnel and `tailscale serve` proxy from the machine, so loopback is
  all they need. A container operator opts in, and an invalid value never
  widens the bind.
- **One IP literal is enough.** `0.0.0.0`, `::` or a pod IP covers every
  container layout. The keywords of `[gateway] bind` add nothing here.
- **The Slack echo runs after verification.** Only the holder of the signing
  secret can make a hook echo, so the gate needs no extra configuration. Slack's
  v0 scheme requires that header, so the header identifies a Slack hook.

## Consequences

- An operator who widens a bind owns what fronts it. The hook socket still
  authenticates every delivery. The public listener still requires pairing, and
  docs require an authenticating proxy in front of it.
- A non-Slack hook never echoes, even for a body shaped like a challenge.

## Alternatives considered

- **An opt-in `slack_url_verification` field on the webhook.** It needs a
  column migration plus create, update and row wiring, and adds no safety: the
  echo already requires a valid signature. Rejected.
- **Reusing `LUCIDOS_GATEWAY_BIND_*` for the side listeners.** That would widen
  the hook socket whenever the main port widens. A user exposing the UI on a LAN
  did not ask to expose webhooks there. Rejected.
