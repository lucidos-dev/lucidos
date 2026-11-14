# 0685: Owner-granted device sessions attest direct-port pages

- **Status**: Accepted
- **Date**: 2026-11-14
- **Amends**: [0647](0647-only-a-signed-claim-proves-the-owner.md), [0671](0671-workspace-broker.md)

## Context

A rig opens a machine-made workspace on its engine's loopback port.
Its registered device id is a claim, without evidence of the owner.
Sends therefore carry an unattested origin, and targeted widget fit answers fail.

## Decision

The owner may grant one device session through the workspace broker.
Each mint spends an Allow once permission for the workspace, device and lifetime (ADR 0688).
The shared permission menu offers no lasting scope for minting.
The gateway signs it under that engine's stamp key, for machine-made workspaces only.
The page presents `x-lucidos-device-session` alongside its device id.

The engine accepts a valid session as evidence of that exact device.
Sends retain a `session-device` origin and render with the standard device bubble.
Targeted fit answers resolve.

Owner-device gates, including voice, refuse delegated sessions.
Session input cannot grant permissions by taps, typed words or standing instructions.
Local agent runs remain delegated; their cross-workspace spawn verdict requires the target owner's approval.
Requests carrying a session header cannot reach stored secret plaintext.
The gateway strips that header from HTTP requests and upgrades.

## Rationale

A registered row alone proves nothing: registration is open and device ids are readable.
A device origin carries owner authority, including question answers and words that grant.
Minting therefore needs the owner's decision on every call, even for a machine-made workspace.

The permission breadth encodes `[workspace, device, ttl_secs]` as canonical JSON.
This uses the shared permission-request and spend path without changing permission storage.
Changing any mint parameter requires another Allow once.

## Consequences

Sessions default to two hours, with a four-hour maximum.
The verifier checks mint time, expiry, lifetime, device, workspace and signature.
Session claims have their own kind, separate from gateway device and workspace-call stamps.
A new engine key invalidates every session; rigs must mint again after re-keying.
No revocation list is introduced.

The CLI writes sessions atomically to a mode-0600 output file.
It prints the expiry, never the credential.
Rigs pass the credential through browser-context extra headers; page scripts never read it.

This covers gateway-keyed machine-made workspaces on macOS and Linux, packaged or source.
User and unknown workspaces are refused, as are keyless engines.

## Alternatives considered

- Accept registered ids on loopback: gives any local process the owner's voice.
- Mint during registration: registration needs no owner identity, so it has the same hole.
- Skip the card for machine-made workspaces: rejected by the owner.
- Change frontend bubbles: changes presentation without proving the caller's identity.
