# 0622: The gateway owns the OAuth loopback callback port and routes callbacks by state; a browser on another machine gets a web redirect URI, superseding 0068's per-engine bind

- **Status**: Accepted
- **Date**: 2026-11-05

Supersedes the per-engine bind in [ADR 0068](0068-oauth-callback-port-has-one-owner.md).
Its supersede slot stays in force for `LUCIDOS_NO_GATEWAY` engines.
Plan: `docs/plans/2026-11-05-gateway-owned-oauth-callback.md`.

## Context

A user connected a Google account from a *remote workspace* (ADRs 0590 and
0591). The engine bound `127.0.0.1:14981` on its own host. The provider sent the
browser to `http://127.0.0.1:14981/oauth/callback`, and the browser resolved
loopback to its own machine. Nothing answered, and the flow timed out silently.
A phone, an iPad, or a laptop that opens a remote gateway directly fails the
same way.

A second bug shared the cause. Every engine bound the fixed port itself, so two
workspaces on one machine could not connect accounts at the same time.

## Decision

1. **The gateway binds `127.0.0.1:14981` and `[::1]:14981` for its whole life.**
   Under a gateway no engine binds the port. A remote engine can start a flow at
   any time, so the port cannot be bound per flow.
2. **`state` carries the route.** The engine writes `state = <prefix>~<nonce>`.
   The prefix is the outside prefix the request arrived under (`/dev/` or
   `/dev@cloud/`). The catching gateway routes by the prefix, and the engine
   checks the nonce exactly as before.
3. **The gateway forwards server-side.** It answers `/oauth/callback` only on its
   loopback listener, and forwards only `code`, `state`, `error` and
   `error_description`. ADR 0636 adds `client_id`, which Sign in with ChatGPT
   returns beside the code.
   - An own workspace gets them on `GET /api/v1/internal/oauth/callback` over
     the engine socket (ADR 0445). The engine accepts that route only from its
     gateway, and renders the browser page.
   - A remote workspace gets them over the relay (ADR 0591), as a *device-less
     claim*. Gateway B admits that claim on exactly one engine route: the
     callback delivery.
4. **Same-machine installs hand off.** The gateway that wins the bind is the
   owner, and its own workspaces come first. With no match, it hands the
   callback to each same-machine peer gateway at `/~/oauth/handoff`. A peer
   delivers only to its own workspaces and never hands on. A gateway that lost
   the bind retries, so it takes over when the owner quits.
5. **A browser on another machine gets a web redirect URI.** The gateway facing
   the browser stamps a *local browser*: the connection started on its own
   machine, from loopback or from the local address it reached. The engine
   then picks the redirect before it builds the authorize
   URL:
   - local browser: the loopback URI, as before;
   - otherwise: `https://<browser origin>/~/oauth/callback`, if that URI is in
     the credential's `web_redirect_uris` list and the credential has a secret;
   - otherwise: refuse upfront, with one sentence naming the fix.
6. **The relay carries the stamp as its own HMAC header.** Gateway A sends
   `x-lucidos-relay-local-browser`, keyed by the pairing credential over the
   claim plus a fixed suffix. B honours it only when the HMAC verifies, and
   strips any copy a client sent.

## Rationale

**The port belongs to the process that lives longest on the machine.** The
gateway is one per install and runs for the install's whole life. An engine
cannot hold the port for a flow a remote engine starts on another machine.

**Routing in `state` needs no new provider registration.** Providers echo
`state` verbatim. The prefix tells the gateway where to go, and the nonce still
proves the flow. A tampered prefix sends the code to an engine that knows no
such nonce, which refuses it with no token request.

**A server-side forward keeps the code off every other origin.** The browser
only ever talks to a gateway on its own machine, or to the address it already
uses. Nothing bounces the code through a page whose origin `state` names.

**Locality is a fact the gateway reads, not a guess.** The TCP peer address of
the accepted socket is authoritative. A forwarding header or a `Host` value is
not.

**The origin comes from the browser's `Origin` header.** On https the browser
sends it on the Connect POST, and the gateway passes it through. On plain http
to a non-loopback host the gateway consumes it, so the engine sees no origin and
refuses. That is correct: providers accept a non-loopback redirect only over
https.

**A separate stamp header, not a version 2 claim.** The old claim decoder
rejects unknown versions and extra fields. So a new claim version would fail
every relayed request from an upgraded A to an older B. An older B ignores an
unknown header instead.

**A web redirect URI needs a secret.** Providers accept a non-loopback redirect
only on a Web (confidential) client type. Desktop and native clients accept
loopback only.

## Consequences

- Two workspaces on one machine, or two installs, connect accounts at once.
- A remote workspace opened through a local gateway connects accounts with the
  loopback URI, with no setup.
- A phone or remote browser connects only with https on the gateway address and
  a registered web redirect URI. Otherwise Connect refuses at once, instead of a
  silent timeout.
- The web callback route serves only this gateway's own workspaces. Its URI
  names this gateway, so a peer install never receives it.
- The web callback route is public. Every unclaimed callback gets the same
  failure page, so a caller cannot probe which workspaces exist. It caps the
  callbacks in flight, as the callback port does.
- **Transition cost.** An older install's engine on the same machine still binds
  14981 per flow. It gets `AddrInUse` while a new gateway holds the port, until
  that install upgrades.
- **Transition cost.** An older remote gateway B refuses the device-less claim
  (401) and ignores the stamp. A remote workspace on that B keeps failing until
  B upgrades.
- **Known limit.** An SSH port-forward makes a remote browser look local: its
  TCP peer is loopback and it carries no forwarding header. It is unsupported.
- Claude Code's MCP login is the `claude` CLI's own random-port loopback flow.
  This change does not reach it. `system-knowhow/remote-access.md` names the
  workaround.

## Alternatives considered

**Keep the per-engine bind and forward the port.** The port must be bound before
a remote engine knows a flow starts, and two engines still collide.

**A paste-the-redirect-URL fallback.** The user copies a dead loopback URL back
into Lucidos. It works everywhere and reads as broken every time.

**The device code flow (RFC 8628).** Google limits it to a few scopes, so the
common providers cannot use it.

**A hosted bounce page on a lucidos.dev origin.** An open bounce to an origin
named in `state` hands a victim's code to an attacker's origin.

**Put the local-browser flag in a version 2 relay claim.** Breaks every relayed
request to an older B, as Rationale explains.
