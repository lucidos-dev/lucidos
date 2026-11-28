# 0598: A voice socket redeems a call ticket minted by a plain fetch, not a device id in the query

- **Status**: Accepted
- **Date**: 2026-10-29

## Context

`GET /api/v1/voice` upgrades to a socket whose call records the caller's words
as the owner's, starts turns and answers cards. ADR 0169's amendment gated it
as mutating. A browser socket cannot set a header, so the page named its device
in a `device_id` query parameter.

That left a gap. The owner-device gate refuses an app document by its
`Referer`, and a browser sends no `Referer` on a socket handshake. An app page
on the engine's origin can read the shell's device id from `localStorage`. One
`new WebSocket(...)` call then placed a call as the owner.

## Decision

The upgrade carries a *call ticket*, not a device id. `POST
/api/v1/voice/ticket` with `{ "thread_id" }` mints it: single-use, thirty
seconds, bound to one device and one thread, held in memory. The query
`device_id` claim is gone.

## Rationale

- **A `fetch` carries what a handshake does not.** The mint is a same-origin
  `POST`, so the browser sends the page's `Referer`. The mint runs the strict
  origin rule of the secret reveals (`RefererRule::Required`, ADR 0117): an
  app `Referer` is refused, and so is a browser-shaped request naming no page.
  A `POST` passes the service worker untouched, so the shell's `Referer`
  always arrives.
- **The mint is where the person reads the refusal.** A browser hides a
  handshake's status and body, and it does not hide a `fetch`'s. So the mint
  runs every check the upgrade would: the owner's device, the voice switch,
  the thread's doer, home only, the thread busy. Their messages reach the user
  as written, rather than a generic "call refused".
- **The upgrade redeems, then re-checks.** Redeem spends the ticket whatever
  follows, so a refused upgrade cannot retry it. A ticket presented on another
  thread is spent too. Thirty seconds is long enough for state to move, so
  every mint check runs again with the ticket's device.
- **The gateway's word still wins.** Through the gateway, `x-lucidos-device-id`
  is the device it authenticated. A header naming another device than the
  ticket refuses the upgrade.
- **The mutating gate peeks.** For a socket route with no device header,
  `api::mutating_gate` peeks the ticket's device without spending it. No header
  and no live ticket is a 401, as before.
- **The client opens the microphone first.** A first-use permission prompt can
  outlive thirty seconds, so the order is microphone, mint, dial. A refused
  mint releases the microphone.

## Consequences

- **The residual of ADR 0144 stays, on purpose.** A standalone app tab can pass
  a same-origin `referrer` init to `fetch` and mint a ticket. It can mint a
  credential reveal token the same way. Over a plain-HTTP gateway address no
  `Sec-Fetch-Site` arrives, so the strict rule cannot refuse a missing
  `Referer` there either. The ticket raises the bar from one `new WebSocket`
  call to a deliberate referrer forgery: the posture every secret route has.
- A restart drops live tickets. The cost is one retry, inside a call setup the
  person is watching.
- Agents and scripts are refused at the mint, as they were at the upgrade.
- A workspace opened through another gateway still cannot call (ADR 0591).

## Alternatives considered

- **Keep the query `device_id` and add a `Referer` check on the upgrade.**
  Rejected: a browser sends no `Referer` on a handshake, so the check could
  never run.
- **Mint before opening the microphone.** Rejected: a first-use permission
  prompt can take longer than the ticket lives, and the call would fail on a
  dead ticket the person never saw.
- **Persist tickets.** Rejected: a ticket outlives no call setup, and a
  database row for thirty seconds of life buys nothing over one retry.
- **A longer-lived, reusable ticket.** Rejected: a leaked one would place calls
  until it expired. Single use and a short life make a leak worthless.
