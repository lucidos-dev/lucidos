# 0819: Probe a stated public webhook URL before Funnel discovery

- **Status**: Accepted
- **Date**: 2026-11-26

## Context

[ADR 0143](0143-webhook-ingress-probed-per-address-family.md) discovers the
public webhook route through Tailscale Funnel. Cloudflare Tunnel and public
reverse proxies can carry the same hook socket. Without Funnel, the monitor
could silently stand down while the workspace still expected deliveries.

## Decision

Use the workspace's public webhook URL when set. Otherwise discover Funnel.
With a webhook enabled, declare `WebhookIngressUnverified` once if neither
route exists or the stated URL has no public address. A completed probe round
or a closed hook gate emits `WebhookIngressVerified` to clear that declaration.

## Rationale

The workspace owner knows which route carries the hook socket. A stated
HTTPS URL makes that route explicit and keeps discovery read-only. The probe
still uses public DNS, checks each address separately, and rejects tailnet
addresses. A public IP literal needs no DNS lookup.

## Consequences

This amends ADR 0143's Funnel discovery and gate-closing rules. A missing
route retracts its standing outage and leaves an Unverified notice. An
unreadable preference, failed Funnel status read or inconclusive DNS
lookup preserves both states.
Missing Tailscale software means there is no Funnel to discover.

The probe appends the workspace slug and webhook ID to the optional URL
prefix. It keeps TLS verification and the sleep rules in
[ADR 0367](0367-a-sleeping-host-is-not-a-dead-ingress.md). The existing outage
and recovery event payloads stay byte-compatible.

## Alternatives considered

Discovering routes from local proxy configuration would miss remotely
managed proxies. The stated URL covers both local and remote routes.
