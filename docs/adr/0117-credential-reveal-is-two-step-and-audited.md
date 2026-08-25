# 0117: Revealing a credential is a two-step, audited act, and apps stay same-origin

- **Status**: Accepted
- **Date**: 2026-08-24

## Context

`GET /api/v1/credential-value?id=<uuid>` returned a credential's stored
plaintext to any caller, with no check of who was asking. The Settings page's
Copy buttons and the credential edit form's prefill are its only intended
callers.

App UIs load at `/app/<id>/` on the engine's own origin, in an iframe carrying
`allow-same-origin`. So an installed app's JS could `GET /api/v1/credentials`
for the ids and then read every secret the workspace holds. That undercuts the
premise of `api/proxy.rs` and `proxy_builtin.rs`, which exist precisely so an
iframe never sees a credential.

The route sat outside `browser_proxy_request_allowed`, which covers only
`/proxy/*`. That gate would have passed the app anyway: a same-origin iframe
presents `Sec-Fetch-Site: same-origin`, exactly like the Settings page.

## Decision

Keep the Settings buttons working. Make a reveal two steps, short-lived,
one-shot, refused from an app document, and recorded.

- `POST /api/v1/credential-reveal-token?id=<uuid>` mints a token bound to that
  one row. It lives 30 seconds and dies on first use, in engine memory.
- `GET /api/v1/credential-value?id=<uuid>&token=<token>` spends it.
- Both routes refuse a browser-shaped request whose `Referer` names an app
  document. The mint refuses one that presents no `Referer` at all; the redeem
  lets that through, because the token is what gates it. See the `RefererRule`
  note below.
- A successful reveal emits `CredentialRevealed`, carrying the service name, the
  auth type and the resolved device. Never the value.

## Rationale

**No header can authenticate the Settings page against a same-origin app, and
this ADR does not pretend otherwise.** The iframe shares the origin and carries
`allow-same-origin`, so app JS reaches `window.top` and can run
`window.top.fetch(...)` in the top document's realm. Every browser-set signal
then reads as the Settings page's own. Nothing stored client side helps either,
since the app shares the origin's `localStorage`, its cookies and its shell
HTML.

The gateway's control plane reached the same conclusion and wrote it into
`control.rs`. Its `Referer` block is "strong defense-in-depth, not an absolute
boundary", and its stated complete fix is serving app iframes from a distinct
origin. That is ADR 0014's open residual.

So this change buys three things a bare route did not have, and claims no
fourth. An app that fetches reachable GET endpoints finds no plaintext, because
the read is now two steps. A token that does leak is worth thirty seconds
against one row. And a reveal that should not have happened leaves an attributed
row somebody can find.

**One way this is stricter than the gateway, and one place it is not.** The
gateway lets a browser request with no `Referer` through. The mint refuses it,
because a page which suppressed its `Referer` has removed the only thing telling
it apart from an app.

The redeem keeps the gateway's looser rule, and that asymmetry is deliberate.
The mint is a `POST`, which the service worker hands straight to the browser.
The redeem is a `GET`, which the service worker re-issues on iOS. A re-issue is
meant to carry the original referrer. A browser that dropped it would take the
Copy button down in the installed PWA. The strict rule there would buy an
availability risk for nothing: a token exists only where a mint already passed
it, and it spends once, for one row.

A request with no browser metadata at all passes either rule. That is a
non-browser client on a loopback bind, which is the CLI and the API e2e suite.

## Consequences

- The Copy buttons and the edit-form prefill are unchanged for the user. The
  frontend client mints then spends inside `getCredentialValue`.
- That client retries the whole pair once on a 403. The service worker re-issues
  a `GET` whose response was lost, and the server already redeemed the token on
  the attempt that vanished. Without the retry, the mechanism that exists to
  rescue a flaky connection would turn one into a failed Copy. Re-minting is
  what a second click would do anyway, and one retry means a real refusal still
  surfaces rather than looping.
- `GET /api/v1/credential-value` is an API-contract change. A caller outside
  this repository that had the old one-step form gets a 403 naming the mint
  route.
- Reveal tokens are lost on restart, which costs the user one extra click.
- A workspace with many reveals accumulates `CredentialRevealed` rows. That is
  the point: the timeline is where a leak becomes visible.
- The residual stands. A deliberately hostile app can still reach the plaintext
  through `window.top.fetch`, and closing that needs the distinct origin.

## Alternatives considered

- **A `Sec-Fetch-*` check alone.** Rejected: that is a CSRF signal, not
  authentication, and an app iframe is same-origin. It refuses nothing the app
  would have sent.
- **The reveal token alone, with no origin check.** Rejected: the mint route is
  as reachable from the app as the read was, so the token would authenticate
  nobody. It is worth having for its window and its audit point, not as a gate.
- **Removing the plaintext from the API entirely.** The only change that truly
  closes it within one origin. Rejected because the Copy buttons are the
  feature: a "copy client secret" button cannot copy a secret the browser never
  receives.
- **Splitting the edit form's prefill off, so only the explicit Copy click
  reveals.** A genuine narrowing, and worth revisiting. Not done here: the OAuth
  Client form prefills a client secret, so the split would need the form to
  distinguish secret from non-secret fields per auth type. That is its own
  change.
- **Serving app UIs from a distinct origin.** The root cause, and the complete
  fix. Out of scope: it changes app URLs, the SDK, the gateway routing and every
  app's same-origin assumption. It stays ADR 0014's open item, named here so the
  residual is not quietly absorbed.
- **A confirmation the engine brokers out of band (a native prompt, a
  notification tap).** Would genuinely close it, since an app cannot answer for
  the user. Rejected on cost and reach: the headless install has no such surface,
  and a per-copy prompt is a heavy toll on an ordinary action.
