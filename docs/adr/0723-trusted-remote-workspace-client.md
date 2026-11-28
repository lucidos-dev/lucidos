# 0723: Remote workspaces use the connecting gateway frontend and an explicit client contract

- **Status**: Accepted
- **Date**: 2026-11-16
- **Amends**: [0591](0591-the-relay-is-the-one-proxied-hop.md), [0725](0725-gateway-pairing-return-access.md)
- **Amended by**: [0807](0807-trusted-pairing-serves-the-peer-client.md), which adds an opt-in, per-pairing *trusted pairing*. With it on, A serves B's own client. With it off, this ADR applies unchanged.

## Context

Pairing A to B lets A's devices use B's workspaces. It must not let B act as
those devices on A or on another gateway C paired to A. ADR 0591's foreign
frontend at A's origin violates this boundary: B's scripts inherit A's storage,
authenticated fetches and service-worker authority. Removing credentials from
the network hop does not remove that browser authority.

## Decision

1. A serves its own installed frontend and executable assets for remote
   workspace navigation. B supplies workspace data, never privileged shell,
   scripts, styles, workers or missing-asset fallbacks. Local engine/frontend
   snapshot pinning is unchanged. A stamps the selected remote data prefix.
2. A negotiates an explicit **workspace-client contract** before serving remote
   workspace data to that client. The contract is independent of gateway
   transport protocol and build identity. Both ends advertise supported contract
   versions/capabilities; A selects a supported intersection. Missing support or
   an empty intersection returns a clear upgrade requirement. Refresh stale
   discovery through authoritative health; never fall back to B's frontend.

   Gateway support never certifies the running engine. A checks both through
   fresh health probes; B returns only its selected engine's contract over the
   signed pairing channel. The probe exposes no private health fields.

   A's installed frontend also carries its generated client version and an
   installed client inventory of final file hashes. Only listed, matching bytes become trusted assets.
   Missing or mismatched inventories fail visibly rather than serving stale
   assets or falling through to B. Source, DMG and headless installs use the
   same contract, without requiring identical build ids.
3. Within a contract version, backend changes preserve existing field meanings,
   request semantics, SSE events and capability behavior. Additive optional
   features negotiate capabilities. Removing/changing a required field, route,
   event or security assumption requires a new contract version and an explicit
   supported range. Compatible different builds remain usable. Implementations
   must test both differing-build compatibility and old/missing-contract denial.
4. B's content is untrusted even when returned by an API route. Foreign documents,
   including direct-open HTML/SVG, receive enforcing CSP `sandbox` without
   `allow-same-origin` and `nosniff`. Existing opaque app frames remain opaque;
   their bridges bind to the selected workspace. Markdown, styles, navigation
   and event data cannot become privileged code or unscoped deputy requests.
5. Credential-bearing peer transport never follows redirects. Browser-facing
   redirects are resolved with a URL parser and admitted only within the exact
   selected remote workspace. Cross-workspace/gateway redirects, encoded path
   escapes and origin-wide response headers are refused. Private gateway routes
   enforce browser-source checks before side effects, independently of CORS.
6. From A's trusted entry, retire legacy remote-path service-worker registrations
   and executable caches before remote navigation. Block foreign worker serving
   and registration. Preserve unrelated workspace data. This upgrade procedure
   cannot reverse an installation that was already compromised.
7. Pairing remains directional. Browser isolation does not imply a return access
   grant; ADR 0725's optional, device-controlled agent grant is separate.

## Rationale

A is the authority for A's browser origin. A therefore owns every trusted byte
executed there. CSP delivered only by an iframe attribute would leave direct
document opens exposed; CORS would hide responses while leaving simple-request
side effects possible. The boundary must cover code, content and request sources.

The client contract permits independent upgrades on source, DMG and headless
macOS/Linux installs. Requiring identical builds would make paired workspaces
unusable during normal rolling upgrades. No device needs direct connectivity to
B, a new port/hostname or another device pairing.

## Consequences

Remote workspace URLs, familiar interface, SSE, apps/files, scoped storage and
explicit gateway manage grants remain supported. Older peers without the client
contract require an upgrade to use remote UI. Paired gateways relay voice and
terminal WebSockets at the same auth boundary as HTTP (ADR 0591). There is no
foreign-frontend compatibility escape hatch.

`relay_isolation_tests.rs` replaces the byte-for-byte foreign-page expectation
with malicious shell/document/worker/redirect regressions. Its explicit browser
probe runs Chromium and mobile WebKit against disposable gateway routers, using
real browser storage, authenticated fetch and service-worker registration.
These begin as red tests for the implementation phases, not proof of a fix.

## Alternatives considered

- Trust B like A: rejected as a default; violates the directional pairing
  requirement. ADR 0807 later allows it as an owner's opt-in per pairing.
- Only constrain agent routing: rejected; B's browser code still has A authority.
- Opaque-frame B's entire SPA: rejected; native storage, push/workers, navigation
  and gateway Settings would require a broad new mediation layer.
- Navigate directly to B: rejected; requires browser connectivity and another
  device pairing, losing the existing relay workflow.
- Separate port: rejected; cookies are not isolated by port and extra listeners
  would not by themselves define the required authority boundary.
