# 0745: Loopback serves HTTPS from a per-user, name-constrained CA

- **Status**: Accepted
- **Date**: 2026-11-19
- **Amends**: [0028](0028-the-packaged-window-is-a-remote-origin.md),
  [0196](0196-cross-gateway-link-navigates-to-the-peer-origin.md),
  [0267](0267-frontend-preview-authenticates-through-the-gateway.md),
  [0600](0600-gateway-managed-certificates-from-tailscaled.md)

## Context

The packaged desktop window loaded `http://localhost:<port>`. WebKit speaks
HTTP/1.1 there, with six connections per origin. Each relayed workspace window
pins one for its event stream, so a few windows starve the rest. The relay
plan measured this (`docs/plans/2026-11-19-gateway-relay-request-queue.md`).

Plain loopback also left a squatter gap. Chrome and Firefox send `Secure`
cookies over plain `http://localhost` on any port, `__Host-` ones included.
Another local OS user who binds our port first, while the gateway is down,
receives every device cookie a restored tab sends. It can also serve pages into
the origin that holds the unsent-work outbox in IndexedDB.

No port-holding scheme closes that gap. The listener has to prove who it is
before the browser sends the credential. Plan:
`docs/plans/2026-11-19-desktop-window-https.md`.

## Decision

**Every loopback listener serves HTTPS with a leaf from a per-user loopback
CA.** The CA lives in the private subtree (ADR 0645), at
`~/.lucidos/private/loopback-ca/`, key 0600 in a 0700 directory. Its name
constraints permit only `localhost`, 127.0.0.0/8 and ::1/128. Every gateway the
user runs shares it, and each mints its leaf in memory.

**Trust goes only into the user's own store, behind a blocking Trust step.**
That is the login keychain's user domain on macOS. On Linux it is the NSS user
db and each Firefox profile, through `certutil`. The desktop shows a bundled
Trust screen before the window opens, and nothing falls back to plaintext.
On macOS that one store is enough: a manual check trusted a CA there, then
loaded a `https://localhost` page it signed in Safari, Chrome and Firefox, each
without a warning.

**On an https origin the device cookie is `__Host-` and `Partitioned`.** A
partitioned cookie is keyed by the schemeful top-level site, so no
`http://localhost` page ever receives it. A plain listener beyond loopback (the
LAN, a tailnet without certificates) keeps today's non-`Secure` cookie. Its
exposure is stated on the Access page and in knowhow.

**Plaintext on loopback splits on `X-Forwarded-For`.** A request carrying it
comes from a reverse proxy such as `tailscale serve`, and is served as today. A
direct client gets only a static hand-off page, its clear endpoint, or a `301`
to `https://localhost:<port>`. That path reads no cookie and calls no auth.

**Unsent work crosses the origin change, and lands as drafts.** The desktop
window passes its outbox to the native side over one narrow command, then
imports it on https. A browser opens the https origin from the hand-off page and
passes the records by `postMessage`. Imported work never sends by itself. The
restore journal travels too, since it holds recovered drafts not yet reviewed.

**The upgrade drops every device pairing once.** Browsers and phones pair again
with a code. The desktop window re-pairs silently. Gateway-to-gateway relay
pairings (ADR 0591) are a separate store and stay.

**Every in-tree TCP client of a loopback gateway verifies the CA.** That covers
the CLI, the core auth client, relay peers and eval. A squatter therefore
cannot harvest a credential over TCP either.

## Rationale

- **HTTP/2 needs TLS.** No browser speaks h2c, so h2 on the window means HTTPS.
- **Partitioning closes what `Secure` alone cannot.** Only WebKit refuses to
  send `Secure` cookies over plain `localhost`. CHIPS keys by scheme in every
  engine that supports it.
- **WKWebView trusts only the keychain.** wry handles no TLS auth challenge, so
  a pinned certificate in the shell is not possible. The Trust step is the only
  way in.
- **Name constraints bound the CA's blast radius.** Chrome 112+ and macOS
  enforce them on locally trusted roots. A stolen key cannot vouch for a real
  site.
- **`X-Forwarded-For` is the standard proxy marker.** `tailscale serve` and
  common reverse proxies set it, so no user's serve config needs a rewrite.
- **A hard cut beats a migration.** A thief can swap a stolen legacy cookie for
  a new one as easily as its owner can.

## Consequences

- The desktop window and local browsers speak h2 to the gateway.
- A fresh install asks for one macOS password, behind the Trust screen.
  `install.sh` trusts in a GUI session, else prints `lucidos tls trust`. Linux
  without `certutil` gets a message naming the package.
- Every paired browser and phone pairs again once.
- An SSH tunnel to a server's `localhost` stops working. A second laptop uses
  the gateway relay or the server's tailnet HTTPS address. No CA export exists.
- An external reverse proxy that strips `X-Forwarded-For` gets the redirect.
  Its operator targets `https+insecure://127.0.0.1:<port>` instead.
- Two installs on one host relay to each other over https, verified against
  the shared CA. A peer address stored as `http://` on loopback reads as https.
- Plain listeners beyond loopback keep their exposure, stated rather than
  fixed. Browsers without CHIPS (Chrome before 115) keep today's exposure too.
- The frontend preview serves a preview leaf from the same CA on loopback, and
  a user certificate by SNI beyond it.

## Alternatives considered

- **A window-only dual-scheme port.** Serve https to the window and plain http
  to browsers on the same port. Rejected: it fixes HTTP/2 for one client and
  leaves the squatter gap open for every browser.
- **Refuse a wide bind without a user certificate.** The superseded plan's
  choice. Rejected: LAN and Headscale users without node certificates would
  lose access entirely.
- **Rewrite the `tailscale serve` target to `https+insecure://`.** Rejected:
  every existing serve config would break on upgrade, for no gain over a
  loopback hop the gateway already holds.
- **HSTS on `localhost`.** It would remove the click-through button. Rejected:
  HSTS is host-wide, so it forces https onto every local dev server the user
  runs.
- **Keep credentials out of cookies** (an origin-storage header token).
  Rejected: it redesigns gateway, engine, SDK and frontend auth.
- **Swap legacy cookies on the first TLS visit.** Rejected: a thief swaps a
  stolen credential as easily as its owner.
- **A machine-wide trust store.** Rejected: it needs admin rights and trusts the
  CA for every user of the machine.
- **Export the CA for SSH-tunnel laptops.** Rejected: it spreads trust of one
  user's key to other machines. The relay already covers the case.
- **Depend on the local-token and held-listener plans first.** Rejected: they
  are not landed. Verifying the CA in every in-tree client closes the TCP
  harvest on its own, and those plans add defence in depth later.
- **Clear the http-era storage with a bare redirect.** Rejected: it deletes
  unsent work. The hand-off page carries it over first.
