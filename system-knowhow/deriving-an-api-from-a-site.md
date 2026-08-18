---
name: Deriving an API from a Site
description: Use when a logged-in site has no usable public API and the user wants one. Triggers: "turn this website into an API", "the site has no API", "capture the calls the page makes", "derive a proxy entry". Covers interception, endpoint grouping, auth classification, and the two required artifacts.
---

# Deriving an API from a site

A modern web app talks to its own backend in JSON. Drive the site once, watch
what it calls, and turn that into a proxy entry the whole workspace can reuse.

The payoff is that one derivation lights up three surfaces at once, with no
further work:

- **Chat**: `proxy_request` reaches it immediately.
- **A custom app**: `lucidos.proxy(<name>).fetch(path)`, with the credential
  injected server-side so it never enters the iframe.
- **A trigger**: the same entry, on a schedule.

## What you must produce

Two artifacts. **Both, or the derivation failed.**

| Artifact | Holds | Read by |
|---|---|---|
| A `data/config/apis.json` entry | base URL, auth pipeline, credential *reference* | the engine, per request |
| `data/knowhow/<name>-api.md` | the endpoint catalog | the LLM, when reasoning |

The entry alone is useless. It is pure transport, so chat would know a proxy
exists and nothing about what to call. The catalog alone is equally useless,
because nothing can authenticate. Ship both.

The secret is a third thing and it lives in neither file. Take it with
`request_credential` and reference it by name.

## Before you start

**This is replay of the user's own session, never a bypass.** If the site
answers with a CAPTCHA or a bot wall, stop and tell the user. Do not try to
defeat it. `browser_open` already reports that case.

**Capture only on the site the user named.** Interception is explicit and
scoped to one operation. Never leave it installed while the user browses
elsewhere, and never run it as a background recorder. That boundary is ADR
0067: data the user did not choose to share is not ours to collect.

## Step 1: open the site and let the user log in

Call `browser_open` with `visible=true` so the user can authenticate. Their
session then persists in the browser profile.

## Step 2: install the interceptor

`browser_eval` this. It wraps `fetch` and `XMLHttpRequest`, and buffers JSON
calls on `window.__lucidosCapture`.

```js
(function () {
  if (window.__lucidosCapture) return 'already installed';
  const calls = [];
  window.__lucidosCapture = calls;
  const keep = (h) => /json/i.test(h || '');
  const origFetch = window.fetch;
  window.fetch = async function (input, init) {
    const res = await origFetch.apply(this, arguments);
    try {
      const clone = res.clone();
      if (keep(clone.headers.get('content-type'))) {
        calls.push({
          method: (init && init.method)
            || (typeof input === 'object' && input.method) || 'GET',
          url: typeof input === 'string' ? input : input.url,
          status: res.status,
          reqHeaders: init && init.headers
            ? Object.fromEntries(new Headers(init.headers)) : {},
          reqBody: init && init.body ? String(init.body).slice(0, 2000) : null,
          resBody: (await clone.text()).slice(0, 4000),
        });
      }
    } catch (e) { /* never break the page */ }
    return res;
  };
  const open = XMLHttpRequest.prototype.open;
  const send = XMLHttpRequest.prototype.send;
  const setHeader = XMLHttpRequest.prototype.setRequestHeader;
  XMLHttpRequest.prototype.open = function (m, u) {
    this.__m = m; this.__u = u; this.__h = {};
    return open.apply(this, arguments);
  };
  XMLHttpRequest.prototype.setRequestHeader = function (k, v) {
    (this.__h = this.__h || {})[k] = v;
    return setHeader.apply(this, arguments);
  };
  XMLHttpRequest.prototype.send = function (body) {
    this.addEventListener('load', function () {
      try {
        if (!keep(this.getResponseHeader('content-type'))) return;
        // responseText throws unless responseType is '' or 'text'.
        const text = this.responseType === '' || this.responseType === 'text'
          ? this.responseText : JSON.stringify(this.response);
        calls.push({
          method: this.__m, url: this.__u, status: this.status,
          reqHeaders: this.__h || {},
          reqBody: body ? String(body).slice(0, 2000) : null,
          resBody: String(text).slice(0, 4000),
        });
      } catch (e) { /* never break the page */ }
    });
    return send.apply(this, arguments);
  };
  return 'installed';
})()
```

Four limits, and you must plan around them:

- **A navigation wipes it.** A full page load builds a fresh JS context, so
  re-install after every one.
- **The calls made during page load are unreachable.** Do not try to catch them
  by reloading: that wipes the interceptor, so you capture nothing. Drive the
  app by clicking within it instead, which keeps the context alive.
- **`HttpOnly` cookies are invisible to it.** That is the browser's design, and
  it is how most session-authed sites hold a login. See "When the auth is a
  cookie" below.
- **It sees only what the page itself calls.** A service worker or a
  same-origin iframe may bypass the patched functions.

## Step 3: drive the operation once

Perform the single operation the user wants: run the search, load the feed,
open the report. Keep it to one operation, because a focused capture derives a
clean catalog.

## Step 4: read the capture back, redacted

**Never serialize the raw buffer into a tool result.** A tool result is written
to the transcript, and the buffer holds live bearer tokens. Redact in the page
first, so only the shape crosses the boundary:

```js
(function () {
  const str = (s) => /^\d{4}-\d{2}-\d{2}T/.test(s) ? 'string(iso8601)'
    : /^[0-9a-f]{8}-[0-9a-f]{4}-/i.test(s) ? 'string(uuid)'
    : `string(len=${s.length})`;
  const walk = (v, d) => {
    if (v === null) return 'null';
    if (Array.isArray(v)) {
      if (d > 2) return 'array';
      return v.length ? [walk(v[0], d + 1)] : [];
    }
    if (typeof v === 'object') {
      if (d > 2) return 'object';
      return Object.fromEntries(
        Object.entries(v).slice(0, 40).map(([k, x]) => [k, walk(x, d + 1)]));
    }
    return typeof v === 'string' ? str(v) : typeof v;
  };
  const shapeOf = (raw) => {
    try { return walk(JSON.parse(raw), 0); } catch (e) { return 'not json'; }
  };
  // A query_param credential lives in the URL, so redact that too.
  const safeUrl = (u) => {
    try {
      const p = new URL(u, location.origin);
      [...p.searchParams].forEach(([k, v]) => {
        if (/key|token|auth|sig|secret|session|pass/i.test(k)) {
          p.searchParams.set(k, `redacted-len-${v.length}`);
        }
      });
      return p.toString();
    } catch (e) { return u; }
  };
  return JSON.stringify(window.__lucidosCapture.map(c => ({
    method: c.method,
    url: safeUrl(c.url),
    status: c.status,
    authShape: Object.entries(c.reqHeaders)
      .filter(([k]) => /^(authorization|x-api-key|x-auth|cookie)/i.test(k))
      .map(([k, v]) => {
        const s = String(v);
        const scheme = s.includes(' ') ? s.split(' ')[0] : '(raw)';
        return `${k}: ${scheme} len=${s.length}${/eyJ/.test(s) ? ' jwt' : ''}`;
      }),
    reqShape: c.reqBody ? shapeOf(c.reqBody) : null,
    resShape: shapeOf(c.resBody),
  })), null, 1);
})()
```

Three things are redacted, and each is a real leak otherwise:

- **The auth header** becomes a scheme, a length and a JWT flag. That is all you
  need to classify it. The value reaches the credential store through
  `request_credential`, where the user pastes it themselves.
- **The URL** gets its secret-looking query params masked. A `query_param`
  credential is a supported auth shape, so the key can sit in the URL. Ordinary
  params like `page` survive, because the catalog needs them.
- **Both bodies** become field names and types, never values. A login POST body
  holds a password, and a response body holds the user's own rows.

A type tree is what the catalog needs anyway. Strings report a length rather
than content, and call out an ISO date or a UUID, since a caller needs the
format.

## Step 5: group the calls into endpoints

1. Drop anything that is not the app's own API: analytics, telemetry, fonts,
   error reporting.
2. Group by origin. The origin carrying the most JSON calls is your `base_url`.
3. Collapse identifier-shaped path segments into a named parameter. A numeric
   id, a UUID, or a long hex string becomes `{id}`.
4. Keep exactly one example per method and collapsed path.

So four calls to `/api/users/8813/posts`, `/api/users/9021/posts` and two
siblings collapse to one entry: `GET /api/users/{id}/posts`.

## Step 6: classify the auth

Match what you observed against the four layer types. The full schema for each
one is in `system-knowhow/building-an-auth-handshake.md`.

| What you saw | Layer |
|---|---|
| The same `Authorization: Bearer <token>` on every call | `static_credential`, kind `bearer` |
| A constant custom header, e.g. `X-Api-Key` | `static_credential`, kind `api_key` |
| A key in the query string | `static_credential`, kind `query_param` |
| A login POST returning a short-lived token | `script_handshake` |
| A per-call signature over a timestamp | `hmac_signed`, or `wasm_signer` if the shape differs |
| No auth header at all, yet it needs the login | a cookie session, see below |

**When the auth is a cookie.** This is the common case, and `browser_eval`
cannot finish it: an `HttpOnly` cookie is unreadable from JS by design. Say so
plainly rather than deriving an entry that will 401. Two honest ways forward:
ask the user for a token the site exposes elsewhere, or wait for the engine's
own capture, which reads cookies over CDP.

## Step 7: write the two artifacts

The proxy entry, merged into `data/config/apis.json`:

```jsonc
{
  "<name>": {
    "base_url": "https://example.com/api",
    "auth": {
      "pipeline": [
        {"type": "static_credential", "kind": "bearer", "credential": "<name>-token"}
      ]
    }
  }
}
```

The catalog, at `data/knowhow/<name>-api.md`:

```md
---
name: <Service> API
description: <one routing sentence naming the service and what it answers>
---

# <Service> API

Proxied as `<name>`. Call it with
`proxy_request(name="<name>", path="/...")`.

## GET /api/users/{id}/posts

The signed-in user's posts. `{id}` is the account id, from `/api/me`.

Query params: `page` (int, from 1), `limit` (int, max 100).

Returns `{items: [{id, title, created_at, ...}], total, page}`.
Dates are ISO 8601 in UTC.

## Quirks

Anything that would waste a later caller's turn: pagination that starts at 0,
a 200 that carries an error body, a required header the docs would not guess.
```

Write the field names and types. A caller that has to fetch an endpoint just to
learn its shape has gained nothing from the catalog.

## Step 8: verify browserless, then claim success

Call each derived endpoint with `proxy_request` and compare against the
captured example. Do this with the browser closed, since a passing call then
proves the entry stands on its own.

Report per endpoint. Partial success is the normal outcome, and an endpoint
that 401s is a finding rather than a failure of the whole derivation. Never
tell the user it works before a browserless call has returned.

## Step 9: clean up

Run `browser_eval` with `delete window.__lucidosCapture` and close the browser.
The capture never gets written under `data/`, and it is never committed.

## Sharing it

The catalog and the entry travel as a plugin. Ship the catalog in the plugin's
`knowhow/`, and put the `apis.json` snippet in the manifest `setup` field.

Never ship the credential. The installing user supplies their own, and
`system-knowhow/plugin-setup.md` already tells the setup agent to take it with
`request_credential`. See `system-knowhow/plugins.md` for packaging.
