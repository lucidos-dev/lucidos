# 0685: Owner-granted device sessions attest direct-port pages

- **Status**: Accepted
- **Date**: 2026-11-14
- **Amends**: [0647](0647-only-a-signed-claim-proves-the-owner.md), [0671](0671-workspace-broker.md)

- **Amended by**: [0699: Permission boundaries follow execution](0699-permission-boundaries-follow-execution.md) (executing policy, stamped scopes, prepared batches, definite-refusal restoration and paired-device refusal).

## Context

A rig opens a machine-made workspace on its engine's loopback port.
Its registered device id is a claim, without evidence of the owner.
Sends therefore carry an unattested origin, and targeted widget fit answers fail.

## Decision

The owner may grant one device session through the workspace broker.
Allow once spends for one workspace, device and lifetime. ADR 0699 also permits
creator-bound lasting consent for the same device and exact requested lifetime.
The gateway signs it under that engine's stamp key, for machine-made workspaces only.
The page presents `x-lucidos-device-session` alongside its device id.

The engine accepts a valid session as evidence of that exact device.
Sends have a standard device origin, and targeted fit answers resolve.
Requests carrying a session header cannot reach stored secret plaintext or owner-only actions.
The gateway strips that header from HTTP requests and upgrades.

## Rationale

A registered row alone proves nothing: registration is open and device ids are readable.
A device origin carries owner authority, including question answers and words that grant.
Minting therefore needs the owner's consent, either once or within an explicitly
engine-stamped creator-bound scope (ADR 0699).

The approval target encodes `[workspace, device, ttl_secs]` as canonical JSON.
This reuses the existing opaque target slot without changing permission storage.
A once grant binds every mint parameter. A lasting grant may cross targets
only within its gateway-verified creator workspace; device and lifetime stay exact.

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
