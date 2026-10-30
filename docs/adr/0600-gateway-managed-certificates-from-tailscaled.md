# 0600: The gateway takes its HTTPS certificate from tailscaled; no ACME client, issuer or hosted service

- **Status**: Accepted
- **Date**: 2026-10-29

## Context

A phone reaching a gateway over a tailnet got plain `http://` unless the user
hand-set `LUCIDOS_TLS_CERT`/`KEY` or fronted the gateway with `tailscale serve`.
Plain HTTP means no service worker, no web push and no home-screen app. The
packaged `Lucidos.app` launches its gateway with a fixed environment, so it
could not take a certificate at all.

The ask was HTTPS that is automatic for every user, "like Tailscale". Tailscale
feels automatic because its control server answers the Let's Encrypt DNS-01
challenge for each node's name. Headscale has node certificate support on
`main`, but no Headscale release includes it as of 2026-10-30.
[Upstream PR #3554](https://github.com/juanfont/headscale/pull/3554) adds DNS-01
answers for each node. A Headscale tailnet gets automatic HTTPS once its admin
deploys a release with this support and enables `dns.certificates` with a
challenge provider. The control server then reports certifiable names as
`CertDomains`, and tailscaled fetches and renews the certificate.

## Decision

The gateway picks its certificate from a fixed ladder, first that works wins:

1. **Operator**: `LUCIDOS_TLS_CERT`/`KEY`, on every listener, as before.
2. **Tailnet**: when tailscaled reports `CertDomains` for this node, the
   gateway fetches the pair for the first name from tailscaled's LocalAPI. It
   serves that *managed certificate* only on the tailnet addresses tailscaled
   reports for this node, and refreshes it daily without a restart. It stops
   serving it at expiry, or once tailscaled no longer reports its name.
3. **None**: the listener stays plain HTTP, and every surface says why.

Plain HTTP to a tailnet address gets a `308` to the certified name, so an old
`http://` address lands on the secure origin. Any other address, such as the
LAN, stays plain HTTP. Lucidos runs no ACME
client, no certificate issuer and no hosted service.

## Rationale

- **The control server already proves the name.** It owns the DNS zone, so it
  can answer DNS-01. tailscaled only gets a certificate for its own node.
  Lucidos would need DNS credentials to do the same, which no user has handy.
- **One source covers both tailnet flavours.** Stock Tailscale and Headscale
  with node certificates enabled use the same `CertDomains` and LocalAPI route.
  Headscale needs a release that includes this support.
- **The authority is tailscaled, not a guess.** The name comes from
  `CertDomains`. Nothing matches a domain suffix, address range or interface.
- **Loopback stays plain.** The packaged window, its Tauri capability, the
  raw-socket health checks and the `tailscale serve` target all assume
  `http://127.0.0.1`. Loopback TLS is a separate plan.
- **No hosted service.** ADR 0550 stands: a Lucidos-run issuer or relay would
  make every install depend on us.

## Consequences

- A gateway with a non-loopback bind on a tailnet that issues certificates
  serves HTTPS with no setup, packaged and source installs alike.
- The pairing QR, `lucidos pair --qr` and Connect URLs prefer the gateway's own
  HTTPS origin, right after the external address.
- A tailnet that issues no certificates, LAN-only reach and other VPNs stay on
  plain HTTP. The Access page and `lucidos tls status` say why.
- Until a Headscale release includes node certificates and its admin deploys
  it and enables them, a Headscale tailnet stays on plain HTTP.
  Source and headless operators can set `LUCIDOS_TLS_CERT` and `LUCIDOS_TLS_KEY` instead.
  The packaged app cannot take an operator certificate.
- A tailnet admin must turn HTTPS certificates on. The gateway cannot.
- The private key copy lives owner-only in `<gateway data>/tls/`.

## Alternatives considered

- **An operator-run tailnet issuer beside Headscale, plus an ACME client in the
  gateway.** The first plan draft. Headscale's native support on `main` provides
  the future path once a release includes it. Lucidos needs no separate issuer.
- **An ACME client in the gateway with the user's DNS credentials.** Needs a
  DNS provider token per user, which is not automatic.
- **A local CA (mkcert style).** Every device must install and trust the root,
  which is a per-device step. It stays a manual route.
- **A Lucidos-hosted certificate or relay service.** Rejected by ADR 0550.
- **Make `tailscale serve` the only answer.** It still works, but it needs a
  command per gateway, so a non-loopback bind stayed plain until someone ran it.
