---
name: Lucidos JavaScript SDK, lucidos.apiUrl
description: Part of Lucidos JavaScript SDK. Its index lists the sections it covers.
---

<!-- domain-event: ReportOpened: Example domain event emitted when the app opens a report. -->

One part of `system-knowhow/js-sdk`, which lists every part.

## lucidos.apiUrl

```ts
lucidos.apiUrl(suffix: string): string   // synchronous, returns URL
```

Builds an absolute URL onto the engine's `/api/v1` surface, carrying the
**workspace address** (the `/<slug>` path prefix) this app is served under.
Pass the path *after* `/api/v1`.

```js
// A stylesheet loads from an app frame, and this is the URL it needs.
link.href = lucidos.apiUrl('/fonts/fira-code.css');
```

**A `fetch` of what this returns does NOT work inside the host shell.** The
frame's origin is opaque, so CORS refuses the engine's answer. WebKit reports
`Load failed` and Chromium a `TypeError`, and neither names the cause. A URL
the browser fetches for you still works: a `src`, an `href`, a stylesheet.

**So this builds URLs, and never makes calls.** For an endpoint with no SDK
method, use `lucidos.request` below, which travels the bridge. For everything
else use the SDK method: it resolves the prefix, travels the bridge, and
carries the timeout, error shape and response parsing a raw `fetch` lacks.

### Why a hand-written `/api/v1/…` does not work

An app iframe is served at `/<workspace>/app/<app-id>/`, and the engine's HTTP
surface lives at `/<workspace>/api/v1/…`. The two URLs an author reaches for
first both resolve somewhere else:

| Written in JS | Resolves to | Answer |
|---|---|---|
| `new URL('api/v1/events/query', document.baseURI)` | your app's own directory, plus `api/v1/events/query` | `404` |
| `fetch('/api/v1/events/query')` | `/api/v1/events/query` | `404 unknown workspace 'api'` |

**Inside the host shell the call fails before it gets that far.** Every request
from the opaque-origin frame to the engine is cross-origin, and the engine
grants no CORS. The `fetch` rejects with a `TypeError` and your code never sees
a status. The table shows what a standalone app tab, on the engine's own origin,
resolves.

The relative form fails because **`document.baseURI` is your app's own
directory**. Behind a gateway that directory also carries your frame's pass,
which reaches no engine route, so the URL is wrong twice over. The root-absolute
form fails because the gateway reads the **first path segment as a workspace
name**, and there is no workspace called `api`.

**Markup is rewritten on the way out, runtime JS is not.** The engine rewrites
root-absolute `src` / `href` **attributes** in the HTML it serves. So the
`<script src="/api/v1/sdk.js">` in `index.html` reaches the browser as
`<script src="/<workspace>/api/v1/sdk.js">`, which is why the § Setup
boilerplate works. Nothing rewrites a string your JavaScript builds at runtime.
**The same `/api/v1/…` string is correct in markup and broken in JS.**

`apiUrl` derives the prefix as the SDK does internally: the `<base href>` when
the document has one, minus your frame's pass, otherwise everything before
`/app/` in the path. Don't re-derive it, and never hardcode a slug: the
workspace name is not the app's to know. `lucidos.configure({ baseUrl })` is the
one override, for an app hosted outside the engine.

**The failure mode is silence.** A wrong URL is a plain 404, and a refused one is
`Load failed` or a `TypeError`. An app that catches it, warns to the console and
falls back to a second data source looks healthy while it renders stale numbers.
If a fetch of yours has a fallback path, surface the failure in the UI too.

## lucidos.request: an endpoint no namespace covers

```ts
lucidos.request<T>(suffix: string, init?: RequestInit): Promise<T>
```

Call the engine's `/api/v1` surface directly. Pass the path *after* `/api/v1`,
and an optional `init` of the shape `fetch` takes. The answer is parsed as JSON,
a non-2xx raises `SdkError`, and an empty body resolves to `null`. Same 10s
deadline and `TimeoutError` as every other method.

From an app frame it travels the host bridge, so it works where a raw
`fetch(lucidos.apiUrl(...))` is refused. From its own browser tab it goes
direct, with the same contract.

```js
// Read the workspace's environment variables. They are NOT secret: every app
// can read them, and a value appears in logs and events. Use a credential for
// an API key, a token or a password.
const { env_vars } = await lucidos.request('/env-vars');

// The engine refuses a write. An env var reaches every command the agent runs,
// so a name the interpreter loads from would be host code execution. Store an
// app's own setting with lucidos.preferences or lucidos.data instead.

// Emit a workspace event.
await lucidos.request('/events/emit', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ event_type: 'ReportOpened', payload: { summary: 'x' } }),
});

// Turn a model id into the label the user sees.
const { models } = await lucidos.request('/models');
```

### Not every endpoint is reachable

Workspace routing is unavailable to apps. `/workspace-operations`,
`/workspace-operations/receive` and `/workspace-operations/receive-start` all
refuse app bridge requests.

The live diagnostic log query is agent-only for every source.
An app cannot read engine, gateway or desktop client logs.

Host-log transfer routes are agent-only. An app cannot prepare or execute
a transfer through `lucidos.request`; see `system-knowhow/lucidos-cli/subcommands-2.md` § `lucidos diagnostic-log prepare-host-transfer`.

Gateway control requests at `/api/v1/engine/gateway-control` are agent-only too.
Apps cannot read gateway status or change update channels through this route.

The engine classifies every route, and an app frame reaches only the ones marked
app-reachable. Anything else answers `SdkError` with a 403, whichever realm the
app runs in. The whole reachable set, with the methods each opens:

| Route | Methods |
|---|---|
| `/data`, `/data/*path` | GET, and PUT plus DELETE on a path |
| `/data/edit`, `/data/upload` | POST |
| `/events/query`, `/events/count`, `/events/types` | GET |
| `/events/emit` | POST |
| `/triggers` | GET, POST, PUT, DELETE |
| `/triggers/run` | POST |
| `/triggers/historical`, `/trigger-groups` | GET |
| `/preferences` | GET, PUT |
| `/notifications` | GET, POST |
| `/notification`, `/notifications/before` | GET |
| `/notification/read`, `/notifications/read-all` | POST |
| `/apps`, `/app` | GET |
| `/threads/list`, `/threads/count`, `/threads/:thread_id/liveness` | GET |
| `/oauth/:provider/access-token` | GET |
| `/proxy/:name/*path` | any method |
| `/ui/navigate` | POST |
| `/env-vars` | GET |
| `/models`, `/model-prices`, `/knowhow`, `/knowhow/read`, `/health` | GET |
| `/fonts`, `/fonts/:file` | GET |
| `/themes`, `/theme`, `/themes/tokens`, `/themes/parts` | GET |
| `/themes/resolve` | POST |

Most of those have a namespace of their own, which is the better way to call
them. The SDK's copy of the list is generated from `ROUTE_REACH` in
`crates/lucidos-agents-api/src/api/app_reach.rs`, so it cannot drift.

Denied, and why:

| Denied | Why |
|---|---|
| credentials, backup keys, OAuth accounts, the email account | the secret never enters the iframe, which is what `lucidos.proxy` exists for |
| `/chat/stream`, thread creation, follow-ups, compose | `lucidos.ui.startThread()` prefills and never submits, so the user always sends their own prompt |
| answering a question, every consent route, the `/internal/` tree | an app never answers as the user |
| `/permissions`, permission batches, a thread's `/hold` | only the owner grants, revokes, holds or resumes, and an app is never the owner |
| applying a change, restarting, rebuilding, installing a plugin | an app does not change the platform under the user |
| writing an env var (the read is open) | an env var reaches every command the agent runs, so a loader-hook name would be host code execution |
| writing a preference the Lucidos Agent may not write | a security setting such as the command guard or the local model host (`local_base_url`) stays the user's, changed in Settings |
| writing the coding-agent paths or their permission mode | a path could run the app's own script as the user |
| reading or writing engine bookkeeping such as `backup_last_run` | it is engine state, not a setting: a read leaves it out |
| message bodies, history, search, memory | an app sees that a thread exists, never what is in it |
| repositories, `/browse-directories`, `/workspaces` | outside the workspace |

Provider hold status and manual clear are host-only: apps cannot call `/models/spend-cap-holds` or `/models/spend-cap-holds/:provider/clear`.
Use Settings → Providers for **Try now**. Provider hold notifications use the existing navigation tap to that page.

If your app needs a denied route, say so rather than working around it. The
list lives in `crates/lucidos-agents-api/src/api/app_reach.rs`, and opening a route
is a deliberate decision (ADR 0231).

**Prefer a namespace where one exists.** `lucidos.data.read` gives you text
rather than JSON, `lucidos.triggers.create` validates the cron before it sends,
and `lucidos.proxy` handles a non-JSON body. This is the hatch, not the front
door.

## lucidos.oauth: OAuth Token Access

Fetch a short-lived OAuth access token for a connected provider, for in-browser SDKs that need a bearer token in JavaScript (e.g. the Spotify Web Playback SDK). The engine looks up the connected account and refreshes the token if it is expired or expires within 60s. It returns ONLY the access token: the refresh token never leaves the engine.

```ts
lucidos.oauth.getAccessToken(provider: string): Promise<AccessToken>

interface AccessToken {
  accessToken: string;
  expiresAt: Date | null;  // null when the upstream provider didn't include an expiry
}
```

### When to use

- **You need a bearer token in the iframe**: a third-party SDK like `Spotify.Player` calls a `getOAuthToken` callback expecting a raw token string. `lucidos.proxy(...)` can't help, because the SDK makes the request itself.
- **Not for ordinary HTTP calls to the upstream API**: use `lucidos.proxy(<provider>).fetch(...)`, so the engine attaches the bearer header and the iframe never sees the token.

### Example: Spotify Web Playback SDK

```js
const player = new Spotify.Player({
  name: 'My Sonos App',
  getOAuthToken: async (cb) => {
    const tok = await lucidos.oauth.getAccessToken('spotify');
    cb(tok.accessToken);
  },
  volume: 0.5,
});
await player.connect();
```

The SDK calls `getOAuthToken` on first init and again when the token expires. Each call hits the engine, which refreshes from the stored refresh token if needed.

### Errors

- `404`: the provider is not connected for this workspace. Ask the user to connect it in the OAuth account settings (or through the LLM `connect_oauth_account` tool).
- `502`: the engine could not refresh the token (missing client credentials, upstream rejected the refresh, network failure).

### Security note

The refresh token, client_id, client_secret and PKCE state stay on the server. The iframe receives ONLY the short-lived access token, scoped to the connected account. Do NOT cache it in `localStorage` / `sessionStorage`: re-call `getAccessToken` when you need one, and the engine handles caching and refresh.

