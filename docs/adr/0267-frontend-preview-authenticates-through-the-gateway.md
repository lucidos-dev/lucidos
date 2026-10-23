# 0267: The frontend preview reaches the engine through the gateway, and every byte it serves needs a paired device

- **Status**: Accepted
- **Date**: 2026-09-24
- **Amends**: [ADR 0055](0055-frontend-preview-is-a-separate-origin.md)

## Context

ADR 0055 had the preview's Vite proxy `/api`, `/app` and `/data` straight back
to the engine. That was safe while a dev engine was itself the front on its
port. ADR 0096 then put dev engines behind the gateway on loopback. ADR 0155
lets a loopback engine ask for no credential, because the gateway authenticates
everything in front of it.

The preview's Vite still listens on every interface. So it became a second,
unauthenticated front for the engine. Measured on a dev machine: a request from
the Wi-Fi address to `:6173/api/v1/threads/list` answered 200 with no
credential, on an interface the gateway itself does not even bind. Vite's own
`/@fs/` route also served any file in the worktree, including `docs/plans/**`
and `WORKSPACES.md`.

The same move broke the preview outright. The gateway strips `LUCIDOS_TLS_*`
from a loopback engine, so Vite inherited no cert and served plain HTTP, while
the Open link copied the page's `https:`. Chrome reported
`ERR_SSL_PROTOCOL_ERROR`.

## Decision

**The preview's Vite forwards to the workspace's gateway, never to the engine.**
`/api`, `/app` and `/data` go to `/<slug>/…` on the gateway, and `/<slug>/…`
passes through unchanged for app frames. The gateway authorizes each call by
the browser's device cookie, exactly as for the main app.

**Vite gates its own files on the same cookie.** A middleware asks the gateway's
public `/~/api/v1/auth/session` about the request's `Cookie` and admits only a
paired device. It forwards nothing else.

**The preview serves with the gateway's cert.** The gateway hands its pair to
engines as `LUCIDOS_GATEWAY_TLS_CERT` / `_KEY`, which the engine never serves
with, and the engine hands them to Vite. So the preview's scheme is always the
gateway's.

**No gateway, no preview.** A directly-launched engine refuses to start one.

## Rationale

The gateway is the one trust boundary (ADR 0094). The preview is a client of
the workspace like any other, so it goes through that boundary rather than
around it. The gateway never trusts a loopback peer (`auth.rs`), so a request
Vite forwards from 127.0.0.1 is authorized purely by the cookie it carries.

The device cookie follows the preview for free. It is host-only with `Path=/`,
and a browser does not scope cookies by port. So the cookie set on `localhost`
or on the tailnet name reaches `:6173` on the same host. It is `Secure` on a TLS
gateway, which is why the preview must serve TLS too: off `localhost`, a plain
HTTP page never receives it.

Asking the gateway, rather than reading the cookie in Vite, keeps one owner for
the credential. The gateway already answers that question for an unpaired
client, and revocation takes effect within the gate's ten-second verdict cache.

## Consequences

- **The tailnet phone preview works.** Vite's DNS-rebinding host check is
  switched off in preview mode, because the cookie gate covers the same threat:
  a rebound hostname never carries the host-only cookie.
- **The preview acts as the real device.** The gateway re-injects the
  authenticated device id on every forwarded call.
- **A new env pair crosses the gateway-to-engine boundary.** It carries paths,
  never key material, and the engine reads it only for the preview.
- **`LUCIDOS_NO_GATEWAY=1` and e2e engines have no preview.** Every dev harness
  runs a gateway, so no supported path loses it.
- **Still out of reach from the preview:** voice WebSockets (never proxied) and
  push subscribe (the engine refuses a scope that is not `/<slug>/`).

## Alternatives considered

**Bind the preview to loopback only.** Smallest change, and it closes the hole,
but a phone could no longer open the preview. The user chose to keep that.

**Keep proxying to the engine and add a credential there.** The engine would
need its own check of a cookie only the gateway can verify, which is a second
owner for one credential.

**Have Vite inject the machine-local token.** Rejected outright. It would make
every LAN caller a local process, the exact bypass this closes.

**Bind Vite to the gateway's interfaces.** One Vite listener takes one address,
and the gateway binds two (loopback and the tailnet), so this degenerates to the
loopback option.
