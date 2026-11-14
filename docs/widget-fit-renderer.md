# Fit renderer launcher contract

This opt-in path is for a renderer that cannot answer as an attested Device.
It works with source and packaged engines on macOS and Linux; Lucidos does not
install or launch a browser for it. Ordinary paired windows use the existing
Device path without a session. Registration does not pair a browser.

1. Open the real Lucidos shell in the launcher's browser. Keep its ordinary
   gateway/local door authentication. Read that shell's logical device id from
   `localStorage['lucidos-device-id']`. Never copy paired credentials.
2. In the launcher, send `POST /api/v1/widgets/fit-session` with the inherited
   verified `x-lucidos-agent-origin-token` and JSON `{thread_id, app_id,
   device_id}`. The thread must be the token's source. The response contains
   `renderer_session_id`, `credential` and `expires_in_seconds` (120).
3. Put only the public `renderer_session_id` in the shell tab's
   `sessionStorage['lucidos-fit-renderer-session']`. Keep `credential` and the
   origin token outside the shell. Register a fresh session for each check.
4. Install a browser-network interceptor that adds
   `x-lucidos-fit-renderer: <credential>` only for the selected shell's main frame.
   Require POST, the exact engine origin, and the `/api/v1/widgets/fit-report`
   path, including any workspace URL prefix. Require no query and the selected
   logical `device_id` in the body. All other requests continue without that header.

   Block service workers in this disposable context. Disable tracing, HAR,
   network-header logging and persistent profiles. Never inject the origin
   token into browser requests or put either secret in a URL, event, saved
   report or frame message.
5. From the launcher, send `POST /api/v1/widgets/fit` under the same verified
   origin with JSON `{thread_id, app_id, renderer_session_id, device_id}`.
   The shell receives a transient request attributed to the agent's source
   thread. It selects the matching public handle and logical device, then mounts
   the real WidgetBox and SDK on both surfaces at every declared size.
6. Await both the real network POST and the fit operation's `{note}`. Save
   only sanitized measurements, status and note. HTTP 200 alone is not a
   passing layout: missing/failed boxes, required params, overflow and timeout
   keep their server-written caveats. Clear the public handle and interceptor
   when done. A refused report never consumes the genuine pending request.

The credential is scoped to one workspace engine, source thread, widget,
logical device and selected request. It answers once. Expiry, cancellation and
restart invalidate it. Each engine permits at most 128 live sessions, cleans
expired/canceled entries on access, and bounds scope text. It is never a
general identity or an owner credential; the report still needs its ordinary
door/attribution credential. A dishonest renderer can invent geometry, so
acceptance must include genuine SDK measurements from an unmodified shell.

The disposable example is
`crates/lucidos-app/e2e/fake-claude-code/fit-renderer-probe.mjs`. It tests wrong
credentials before the genuine POST and replay afterward without saving
secrets. The recorder app's own adapter belongs with that app.
