# 0602: Serve verification proves gateway identity and never presents a credential

- **Status**: Accepted
- **Date**: 2026-10-30

## Context

The engine publishes `workspace_serve_url` only after it proves that a
`tailscale serve` route reaches this workspace. The Access page and the pairing
QR use it as the HTTPS origin behind serve.

The old proof fetched `<candidate>/<slug>/api/v1/health` and compared
`workspace_path`. Then the gateway began to authenticate every network caller
(`auth_api::enforce`, first in v0.29.0). The probe carries no credential, and
`tailscale serve` proxies from `127.0.0.1`, so the gateway answers 401. The URL
never verified again, and the QR fell back to plain HTTP on every install.

## Decision

A serve candidate is proven by gateway identity. The engine reads its own
gateway's `instance_id` from the public `/~/api/v1/health` over loopback. It
then asks the candidate origin for the same path over the tailnet, with TLS
validated. It publishes the candidate only when the two ids match.

The tailnet probe never presents the local token or any device credential.

## Rationale

- **The proof needs no secret.** The gateway spawned this engine under its slug,
  so reaching that gateway reaches this workspace. `instance_id` is random per
  gateway boot, so another gateway cannot match it by accident.
- **A credential would leak.** A serve route can proxy to any host. A bearer
  secret sent before the proof goes to whoever answers.
- **It is the proof the tree already trusts.** `lucidos pair --qr`
  (`reaches_this_gateway`) and the gateway's external-address probe prove an
  origin the same way.

## Consequences

- The serve URL verifies again under device authentication, on every gateway.
- The public health path is now load-bearing for the engine too. It must stay
  public (`auth_api::is_public_path`), and it carries `instance_id`.
- A source-scan test forbids certificate bypass, the local token header and
  `gateway_auth` inside the tailnet probe.

## Alternatives considered

- **Send the local token with the probe.** The gateway would admit it, but the
  token would go to whatever the serve route proxies to, before anything is
  proven. Rejected.
- **Make `/<slug>/api/v1/health` public.** It widens the unauthenticated
  surface to every engine, for a check the gateway's own health already
  answers. Rejected.
- **Trust a TCP or TLS listener on the port.** It says nothing about which
  gateway answers. On a two-gateway machine it publishes the other gateway's
  route. Rejected.
