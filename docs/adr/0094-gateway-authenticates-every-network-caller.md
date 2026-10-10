# 0094: The gateway authenticates every network caller, and loopback proves nothing

- **Status**: Accepted
- **Date**: 2026-08-19

## Context

Lucidos had no inbound API authentication. `system-knowhow/remote-access.md`
said so outright: anything that could reach the port acted as the user, with
full access to every workspace, every credential and every coding-agent
capability. The tailnet was the entire boundary.

That is thin for a system holding every credential a person owns, and it fails
completely the moment anything is exposed more widely. The user asked for real
auth "in addition to being on tailscale", which rules out deriving identity from
the tailnet.

Four questions had to be settled first: what a credential identifies, where the
check runs, whether a workspace is a boundary, and how a local process proves it
is local.

## Decision

**The device is the principal.** A device is paired once and remembered. This
promotes the existing `devices` concept from a display hint to something real,
without inventing accounts. `x-lucidos-device-id` is untouched and stays
attribution-only (ADR 0050). **Amended: the gateway now writes that header.**
See "Amendment: one device id, minted here" below.

**The gateway is the boundary.** It authenticates every caller reaching it over
the network. Engines keep their loopback posture. The loopback population (the
`lucidos` CLI, the Python shim, coding-agent sessions, cross-workspace calls) is
governed by the thread-bound origin token it already had.

**A loopback peer address authorizes nothing.** This is the load-bearing part.
The shipped remote-access route is
`tailscale serve --bg --https=443 http://127.0.0.1:5252`, and Serve proxies from
this machine, so a phone's request arrives with a loopback peer address.
"Trust `127.0.0.1`" would therefore have trusted the whole tailnet, and the
public internet the moment Funnel fronted anything.

A local process instead proves it is local by reading a mode 0600 file,
`~/.lucidos/local-token`. `gateway::auth::authorize` takes **no peer address
parameter at all**, so the invariant is held by the signature rather than by
everyone remembering it.

**That file is a pairing authority, not a bypass.** Anything that can read it
may mint a pairing code. This states trust that already exists, since a local
shell can read every credential and drive every workspace anyway.

**No Tailscale anywhere in the auth path.** No identity headers, no tailnet
assumptions. Auth behaves identically over Serve, mkcert and a plain LAN
address. Tailscale is transport.

**Pairing is entry: a paired device reaches every workspace.** Per-workspace
device grants were rejected as the cheap half of a property whose expensive half
is confining the agent.

**Scopes exist only on principals with no shell behind them.** A paired phone is
the user, so restricting it is theater. A webhook token is genuinely confined.

## Rationale

**Per-workspace grants would have been a boundary in the UI and not in
reality.** An agent inside one workspace already reaches every other one through
three shipped paths. Out-of-workspace reads are a deliberate feature, which
`command_guard.rs` says in as many words. Every workspace database is created
`OWNER {PG_USER}` on one shared cluster (`postgres.rs`). Cross-workspace
launching is a product feature. Gating one door of a house with three open side
doors is worse than not gating it: it invites reliance the property cannot
support.

**`HttpOnly` is load-bearing rather than hygiene.** App iframes are served
same-origin with `allow-same-origin` (`AppUiInline.tsx`), so any credential
JavaScript can read is one an app can ship off-machine. Apps keeping the user's
authority is the status quo and is deliberately unchanged. A *stealable*
credential would have been a regression, and `HttpOnly` is what prevents it.

**Credentials are stored as digests.** The local token stays plaintext because
callers send it back for comparison. A device credential is different: a bearer
cookie held by a remote device. Storing it in the clear would turn one leaked
file into durable remote access.

**A shared crate, not four hand-copies.** `lucidos-local-token` has no
dependencies, so the gateway, engine, CLI and app can all take it, exactly as
`lucidos-tailscale` is shared under ADR 0014 §1. Tailscale exists because two
hand-copied halves drifted and a fix reached only some of them. A stale copy
here is worse than a missing feature: it is a caller that silently cannot
authenticate.

## Consequences

- **A browser pairs even on the host machine.** Proving locality means reading a
  file, and a browser cannot. This is the main ergonomic cost. It shipped with
  `lucidos pair` as the only answer, which stranded a DMG user with no terminal.
  The desktop window now pairs itself through its own Rust side, which can read
  that file: `docs/plans/2026-08-19-nobody-is-stranded-by-the-pairing-update.md`.
- **The picker shell is served without a credential**, by necessity: an unpaired
  device needs somewhere to land. It is static assets only, and every API behind
  it is gated. `auth_api::is_public_path` is the entire exemption list, and it is
  exact-matched rather than prefix-matched so an exemption is never inherited.
- **An unauthenticated navigation is answered with the pairing screen, not
  refused**, so a phone reaches it instead of a bare 401. Anything else still
  gets 401. It shipped as a 307 to `/~/` and became an in-place render. An
  unpaired PWA cannot update the service worker that would follow one, so the
  redirect never reached the installs that needed it most.
- **A pairing ends on revocation and on nothing else.** No idle timeout and no
  absolute one, so age is never an input to the auth decision. Two alternatives
  were weighed and rejected. A server-enforced idle window reaps only forgotten
  devices, since a stolen credential in use stays fresh; Tailscale's 180-day
  node-key expiry is the field's most-complained-about behaviour. Token rotation
  with reuse detection buys a whole failure class, which Hermes Agent currently
  carries as an open replay bug. What the store does record is a daily
  last-seen, so the devices list can say which rows are live:
  `docs/plans/2026-08-19-a-paired-device-says-when-it-was-last-seen.md`.
- **Workspaces are explicitly not a boundary against each other.** This is now a
  stated property rather than an accident, and `remote-access.md` says so.
- **The ADR 0014 app-iframe residual stays open.** Apps still act with the
  user's authority. Closing it needs a distinct app origin, which is its own
  change.

## Alternatives considered

- **Trust loopback.** Rejected: `tailscale serve` proxies from loopback, so this
  fails open on the exact route the product recommends.
- **Tailscale identity headers** (`Tailscale-User-Login`). Genuinely attractive,
  since Serve injects them, strips spoofed copies, and Funnel never carries
  them, so it fails closed. Rejected on the user's instruction not to couple
  auth to Tailscale. It would also bind a security property to one transport,
  and it is sound only while the gateway is loopback-only.
- **A shared secret instead of per-device credentials.** Nothing to revoke short
  of rotating for everybody, and no forensic trail.
- **User accounts.** Generalizes past what a single-user product needs, and
  brings password or WebAuthn handling, session lifecycle and recovery.
- **A separate loopback-only port for local callers.** Unforgeable and needs no
  secret, but it stays correct only while nobody points a proxy at it. A
  topological invariant is easy to state and easy to violate later.

The full decision trail, including the webhook half of the same design, is
`docs/plans/2026-08-17-clients-pair-to-the-gateway-and-webhooks-get-their-own-socket.md`.

## Amendment: one device id, minted here

The Decision above says `x-lucidos-device-id` "is untouched". That left two
device identities for one browser, and the user found it: "it's confusing that
we have device in access and another device in devices, and the same device is
not the same device."

The wording predicted the collision. Pairing "promotes the existing `devices`
concept", but it minted a second id beside it and joined nothing. Settings then
had a device in two places. They carried different labels, different lifetimes
and different scope, and Revoke and Remove did not touch each other.

The granularity was identical all along. Both are per browser storage
container, so they were one thing minted twice.

**The gateway mints the one id.** `auth_api::enforce` stamps the authenticated
device on the request, and the proxy strips every inbound `x-lucidos-device-id`
and re-injects that one. It is the treatment `x-forwarded-prefix` already had,
whose comment calls the stripping the trust boundary.

**The engine's read path is unchanged**, which is what makes this small.
`HEADER_DEVICE_ID` was already the contract, so push, presence, preferences and
actor chips follow the new id without moving. `DeviceStore::hand_over` migrates
an existing row, in the one page load where a client knows both ids.

**Two consequences.** The attribution gate gets a real signal on the network
path, where it previously conceded that anyone could register a device: a
network caller can no longer name itself. The loopback path keeps the ADR 0050
posture, because a browser reaching an engine port directly has no pairing list
to disagree with.

Plan: `docs/plans/2026-08-22-one-device-identity-minted-at-the-gateway.md`.
