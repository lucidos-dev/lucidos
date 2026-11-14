# 0601: A loopback engine refuses a non-loopback Host with 421, shutting out DNS rebinding

- **Status**: Accepted
- **Date**: 2026-10-30

## Context

ADR 0096 binds every gateway-spawned engine to loopback. On that bind
`api::local_auth` checks no credential, because reaching the socket was taken to
prove a local process. The only browser gate is `api::browser_origin`, which
refuses a cross-origin request.

DNS rebinding breaks that premise. A page on `attacker.example` whose DNS answer
flips to `127.0.0.1` fetches the engine port as its own origin. The browser
sends `Sec-Fetch-Site: same-origin` and `Host: attacker.example:<port>`. Both
gates passed it. Every GET answered, and `POST /api/v1/devices/register` let the
page mint a device id and drive `/api/v1/chat/stream`.

## Decision

On a loopback bind, `local_auth::enforce` checks every `Host` header and the URI
authority. Each must name a loopback IP literal or `localhost`, on any port.
Anything else gets `421 Misdirected Request` and a log line naming
the Host. An absent Host is admitted. A wide bind is unchanged: the credential
decides.

## Rationale

- **The Host is the one thing a rebound page cannot forge.** The browser fills it
  from the URL the page loaded, so it always names the attacker's domain.
- **Every legitimate caller already dials a loopback name.** The gateway hop
  stamps `Host: 127.0.0.1:<port>`. The CLI, the Python runtime and both e2e
  suites use `127.0.0.1` or `localhost`. So nothing that works today breaks.
- **It belongs in `local_auth`.** That module answers "who may call", on the same
  bind predicate. The loopback premise becomes "reaching the socket by a
  loopback name". No new layer and no new state.
- **421 is the honest code.** It says this server does not answer for that name.
- **An absent Host marks a non-browser client.** A browser always sends one, and
  the loopback premise already covers a local process.
- **`LUCIDOS_PERMISSIVE_CORS` does not skip it.** That variable grants
  cross-origin access. A rebound page is same-origin, and the variable never
  meant "answer for any name".

## Consequences

- A reverse proxy pointed straight at an engine's loopback port, forwarding an
  outside Host, now gets a 421. No supported setup does this: remote access goes
  through the gateway (ADR 0096), and engine ports are ephemeral.
- The gateway needs no change. Its auth plane trusts no loopback peer, and its
  host-only device cookie never rides a rebound name.
- `POST /api/v1/devices/register` keeps its exemption from the mutating gate. A
  rebound page no longer reaches it.

## Alternatives considered

- **Require the local token on loopback too.** It closes the hole, but it breaks
  the CLI, the Python runtime and e2e callers that rely on the loopback premise.
  Rejected as a far wider change for the same protection.
- **Check only `Host`.** HTTP/2 and an absolute-form target carry the URI
  authority instead. Checking one would leave the other as a bypass.
- **Strip a trailing root dot (`localhost.`).** No loopback caller sends one.
  Refusing keeps the allowlist exact.
- **Admit `*.localhost` too.** RFC 6761 reserves it for loopback, and Chrome,
  Firefox and macOS answer it locally. But a Linux resolver without
  systemd-resolved forwards it to DNS. A hostile resolver can then rebind
  `evil.localhost` the same way. No caller dials such a name, so refusing it
  costs nothing.
