# 0151: The gateway proxies a WebSocket upgrade at the same auth boundary as an HTTP request

- **Status**: Accepted
- **Date**: 2026-08-28

## Context

Voice needs a duplex channel. Audio flows in while audio flows out, and both are
binary. Nothing in Lucidos speaks WebSocket today: `axum` is declared without
the `ws` feature, and there is no tungstenite.

Engines bind loopback, in dev as well as packaged (ADR 0096). The gateway is the
only network path into a workspace, so voice on a phone depends on it.

The gateway cannot carry an upgrade as it stands. `proxy.rs` forwards through
`reqwest`, which has no upgrade support, and it strips `upgrade` and
`connection` as hop-by-hop headers on the way past.

## Decision

The gateway proxies a WebSocket upgrade **transparently**, by splicing bytes. It
does not terminate the WebSocket and it does not re-frame messages.

An upgrade request takes a separate path inside `proxy.rs`, and that path keeps
every rule the HTTP path already applies:

- `enforce` has already run, because it is a router layer covering the fallback.
  An unpaired caller is refused before the upgrade is ever considered.
- Client-supplied `x-forwarded-prefix`, `x-forwarded-host` and
  `x-lucidos-device-id` are stripped. The gateway's own prefix and the
  authenticated device id are injected in their place.
- The `/<slug>` prefix is stripped, exactly as for an HTTP request.

The handshake headers themselves survive, because on this path they are the
payload rather than framing to discard.

The upgrade path dials the engine over plain TCP. It refuses, with a logged 502,
when the engine serves TLS.

## Rationale

Splicing bytes is what the rest of this file already does, one layer down. ADR
0014 §3 made the proxy a pure strip-and-forward streamer for a reason. Reading
and rewriting what it forwards is what produced the gzip-502 and
lost-compression failures of 0013. A relay that re-frames every message is the
same mistake in a new protocol.

Transparency is also the cheaper contract. Subprotocols, extensions, ping and
pong, and close frames all pass through with no gateway code to get them wrong.
The gateway needs to know that a socket exists, not what is on it.

Keeping the auth boundary identical is not a nicety. The gateway is the trust
boundary, and a second path through it is a second place to forget that. A
forged `x-lucidos-device-id` would let a paired caller act as any device. The
header is stamped from the authenticated device precisely so a client cannot
choose it.

Plain TCP is enough because `engine_tls` is false in both shipped topologies.
`engine_loopback` defaults to true, and `engine_tls` requires it to be off. Only
`LUCIDOS_GATEWAY_ENGINE_LOOPBACK=0` turns it on, and that reopens the pre-0096
topology nothing sets. A TLS leg for a path nothing reaches would be speculative
code, and the refusal is loud rather than silent.

## Consequences

- The engine gains the axum `ws` feature and a WebSocket endpoint. The gateway
  gains no WebSocket library at all, because it never parses a frame.
- The upgrade path is the one place in `proxy.rs` that must NOT strip
  `connection` and `upgrade`. That is a real asymmetry, and the code says so.
- Setting `LUCIDOS_GATEWAY_ENGINE_LOOPBACK=0` leaves HTTP working and voice
  refused. Adding TLS then is a connector, not a redesign.
- The gateway's auth tests grow an upgrade mirror: refused unpaired, device id
  injected, spoofed device id stripped.
- A dropped socket needs no gateway cleanup beyond the splice ending, because
  the gateway holds no session state.
- ~~The engine's cross-origin guard covers the handshake, and every current
  browser passes it.~~ **Wrong, and corrected by ADR 0163.** WebKit does send
  `Sec-Fetch-Site` on a handshake. Chromium and Gecko send no fetch metadata
  there at all, so the guard's authoritative arm never runs for a socket. Behind
  the gateway its fallback then compares against the internal upstream `Host`
  and refuses our own page. Every call from Chrome was refused until the gateway
  took the question over.

## Alternatives considered

**Terminate the WebSocket and relay messages.** The gateway accepts the socket,
opens its own to the engine, and copies `Message` values across. Rejected: it
puts a frame parser in the one network-facing process, and it silently drops
whatever the relay does not model. It is also the shape ADR 0014 §3 already
rejected at the HTTP layer.

**SSE downstream plus POST upstream.** Lucidos already streams SSE, so no new
transport would be needed. Rejected: SSE is one-way and text, so audio in would
be a new HTTP request per chunk. That is a duplex channel rebuilt badly, and the
latency is the whole product.

**A direct browser-to-provider WebRTC connection.** The lowest latency, and what
several voice products do. Rejected: it puts a provider credential in a browser
page, which page script can read. It also puts session state in the client,
where the engine cannot event-source it.

**Let the client reach the engine port directly.** No gateway work at all.
Rejected by ADR 0096: engines bind loopback, so no such path exists from a
phone, and re-opening one would hand out a whole workspace with no credential.
