# 0828: Frontend previews use the gateway path and trusted relay

- **Status**: Accepted
- **Date**: 2026-11-27
- **Amends**: [0055](0055-frontend-preview-is-a-separate-origin.md), [0267](0267-frontend-preview-authenticates-through-the-gateway.md), [0745](0745-loopback-serves-tls-from-a-per-user-ca.md)

## Context

A frontend preview on a remote gateway could not be opened through the gateway
relay. Its separate port named the opening gateway's host, while its cookie
check and API proxy expected credentials from the source gateway. The same
second-port dependency broke desktop windows and gateway-only SSH forwards.

## Decision

Serve previews at `/~preview/<key>/<slug>/` on the gateway's existing origin.
The key is the first 16 hex digits of SHA-256 of
`lucidos-frontend-preview:` followed by the source gateway id. It is a routing
key, not a credential. The path stays identical through every relay hop.

The engine supervises Vite on a private Unix socket, mode 0600 inside a 0700
directory. Vite has no TCP listener, TLS leaf, credential check or API proxy.
The gateway authenticates HTML, modules and HMR as it does workspace requests.
It strips credentials before proxying to Vite and supplies the validated workspace
prefix for the HTML base. Data uses ordinary workspace routes on that origin.

A relay forwards previews only for a trusted pairing, after device authorization.
It checks the peer's public preview key before requesting executable content.
Unknown, untrusted, old or stopped peers yield an inert explanatory page.
Relayed HMR uses the signed device claim and closes when pairing is revoked.

Dev bundles namespace localStorage and IndexedDB with `preview:` using the
shared peer storage helpers. The API returns a path; clients resolve it on
their own origin, and the CLI joins it to the gateway origin it reached.

## Rationale

The existing gateway route already reaches browsers, phones and desktop clients
through direct access, a reverse proxy, SSH forwarding or a trusted relay. A
stable path lets Vite's absolute module URLs work on both gateways. Keeping the
gateway as the sole trust boundary removes duplicate credential handling.
Trusted pairing is required because the peer's WIP JavaScript runs at the
opening gateway's origin. Storage namespacing protects normal preview use; it
is not a security boundary against deliberately hostile same-origin code.

## Consequences

The workspace URL keeps serving built `dist/`. Previews remain source-only,
one per workspace, with no service worker, push subscription or voice sockets.
A packaged opening gateway can relay a source gateway's preview. A packaged
source gateway cannot start one. This covers macOS and Linux without another
client-facing port or certificate. A proxy must forward websocket upgrades
for HMR; otherwise users reload manually.

This replaces ADR 0055's separate-origin decision, ADR 0267's Vite cookie gate,
and only ADR 0745's preview certificate handoff. Gateway TLS remains unchanged.

## Alternatives considered

- A second listener on the opening gateway still needs another exposed port
  and an origin desktop windows cannot open.
- A different preview path per hop breaks Vite's absolute module URLs.
- A preview hostname needs DNS and certificate provisioning and reliable
  desktop webview resolution.
- Teaching Vite the relay's signed claims adds a second credential owner.
