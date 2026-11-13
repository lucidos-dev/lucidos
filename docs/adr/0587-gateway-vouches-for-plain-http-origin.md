# 0587: The gateway answers the same-origin question for an ordinary request with no fetch metadata, extending 0163

- **Status**: Accepted
- **Date**: 2026-10-26

## Context

A phone paired with the packaged app's gateway over a tailnet reaches it at a
plain-HTTP IP, `http://100.70.0.1:5252`. Every mutating request from it failed
with `403 cross-origin browser requests are not allowed`: add a widget to the
phone dashboard, mark a notification read, compose, device presence. The engine
log shows `sec-fetch-site: None` and `origin: Some("http://100.70.0.1:5252")` on
each refusal.

Browsers send fetch metadata only to a potentially trustworthy URL, which is
https or a loopback host. A plain-HTTP tailnet or LAN address is neither. So no
browser sends `Sec-Fetch-Site` there, and the engine gate falls back to
`Origin == Host`. Behind the gateway, `Host` is the internal hop, so our own page
fails the compare. A same-origin GET carries no `Origin`, which is why reads
worked and only writes failed.

The gate's doc called this "an accepted and shrinking-population limitation".
That premise was false. The population is every browser on a non-loopback
plain-HTTP address, a route ADR 0550 ships and ADR 0014 serves.

## Decision

**The gateway answers the same-origin question for an ordinary request that
carries an `Origin` and no `Sec-Fetch-Site`, and consumes an `Origin` it has
vouched for.**

- `Origin` matches the request `Host`: the gateway does not forward `Origin`. The
  engine sees a hop with no browser signal and passes it.
- `Origin` does not match, or there is no `Host`: forwarded unchanged. The
  engine refuses it as before.
- A request with `Sec-Fetch-Site` is untouched. Both headers are forwarded and the
  engine decides.

## Rationale

This is ADR 0163's argument applied to a second input. The gateway is the only
hop that still holds the client's own `Host`. The engine cannot answer the
question behind it.

The gateway vouches but never refuses on this path. A refusal there would
duplicate the engine's decision and break its app-asset exemption: a font fetch
from an opaque-origin app frame carries `Origin: null`. Forwarding a foreign
`Origin` keeps the one decision, its log line and its exemption in the engine.

`Origin` on a request that has no fetch metadata cannot be forged by page
script, on the same terms as in 0163: it is a forbidden header.

## Consequences

- A phone on a plain-HTTP address can use the app. Pairing by name instead of IP
  would not have fixed it, since the trigger is plain HTTP.
- Page script on another port of the same machine still fails. Its `Origin`
  differs from the gateway `Host`, so it reaches the engine and is refused.
- A future route behind the gateway cannot read `Origin` from a same-origin
  browser request that has no fetch metadata. `api::browser_origin` says so.
- Engine code that reads `Origin` goes blind on a vouched request, and none of it
  worked on this path before, since the gate refused first:
  - `secret_reveal::reveal_request_allowed` takes the absence of both `Origin`
    and `Sec-Fetch-Site` for a CLI. A vouched request with no `Referer` skips
    `RefererRule::Required`. ADR 0117 and 0144 already call that check defense
    in depth, since app script can pick a same-origin referrer. The reveal
    routes stay behind pairing.
  - `chat.rs` caches the first `Origin` as the user-facing frontend origin, so a
    deployment reached only over plain HTTP never learns it.
  - `notifications::sanitize_push_scope_url` skips its scope-against-`Origin`
    check.
- ADR 0163's "the HTTP path is untouched" no longer holds for that one case. A
  note there points here.

## Alternatives considered

- **Inject `x-forwarded-host` for the engine's compare.** Rejected in 0163, and
  the reasons hold: a forged value reaches an upstream that may trust it.
- **Inject a synthesized `Sec-Fetch-Site`.** Rejected in 0163. It forges the one
  header the engine treats as unforgeable.
- **Refuse a foreign `Origin` at the gateway.** Breaks the app-asset exemption
  and splits one decision across two crates.
- **Serve https from the packaged gateway.** Gives fetch metadata, but is a TLS
  project of its own. `tailscale serve` is the workaround until then.
