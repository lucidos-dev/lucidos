---
name: Lucidos JavaScript SDK, lucidos.media
description: Part of Lucidos JavaScript SDK. Its index lists the sections it covers.
---

<!-- domain-event: HabitCompleted: Example domain event emitted by a habit-tracker app. -->

One part of `system-knowhow/js-sdk`, which lists every part.

## lucidos.media

`lucidos.media.poster(clip, { maxSize? })` returns a promise of
`{ poster, width, height, duration }`.
`poster` is a JPEG data URL; dimensions and duration describe the original clip.
`clip` is a workspace path or HTTPS URL.
`maxSize` is the box's long side in CSS pixels, default 640.
The host scales by device pixel ratio, capped at 1280 pixels.

```js
const still = await lucidos.media.poster('artifacts/demo.mp4', { maxSize: 400 });
image.src = still.poster;
```

Use a poster for an idle video widget instead of loading its video source.
Attach the video source only on play, a scrub or full screen.
The host captures one poster at a time and releases its decoder afterwards.
It checks credential-less access before capture or cache lookup.
Posters are cached by URL and ETag or Last-Modified, in memory and Cache Storage.
Without a validator, a later call captures again.

An HTTPS server must allow CORS for capture.
On rejection, show the stage and play button, then load the video on intent.
Workspace apps with their own videos must adopt this method themselves.

## lucidos.events: Event Store

Emit domain events, and query the workspace's event store.

**`query` reads the whole store, not just what your app emitted.** Workspace
domain events (`HabitCompleted`) and the engine's own thread / system events
(`ChildThreadCompleted`, `ResponseGenerated`, `ChangeApplied`, `TriggerCompleted`)
are rows in one `events` table. One call returns them, filtered by `event_type`,
time, `thread_id`, or a paging cursor. There is no second stream. See
`system-knowhow/thread-events/one-table-two-enums.md` § "One table, two enums" for the
`ThreadEvent` / `engine::event_bus::SystemEvent` distinction.

```ts
lucidos.events.emit(type: string, payload: Record<string, unknown>, options?: EmitOptions): Promise<void>
lucidos.events.query(params?: EventQuery): Promise<LucidosEvent[]>
```

`query` masks every payload against the credentials stored now, so a value a stored credential holds comes back as `[REDACTED]`. When the mask cannot load them, the call fails with a 500 rather than returning raw rows.

### Types

```ts
interface EventQuery {
  event_type?: string;
  since?: string;             // ISO 8601
  until?: string;             // ISO 8601
  limit?: number;             // default 100, clamped to 1..1000
  before_event_id?: string;   // walk backward from this event, exclusive
  after_event_id?: string;    // tail-follow forward from this event, exclusive
  thread_id?: string;         // restrict to one thread
  event_id?: string;          // resolve ONE event by id; uuid or 'evt-<32 hex>'
}

interface LucidosEvent {
  id: string;
  event_type: string;
  payload: Record<string, unknown>;
  created: string;
  /** Engine thread events only (absent, not null, on domain events). */
  thread_id?: string;
  /** Monotonic insertion order across the workspace. Always present. */
  sequence: number;
}

interface EmitOptions {
  /** Skip persistence: broadcast on SSE only. */
  transient?: boolean;
}
```

### Examples

```js
// Emit an event
await lucidos.events.emit('HabitCompleted', {
  summary: 'Completed meditation',
  habit: 'meditation',
  streak: 5
});

// Emit a transient coordination signal: it reaches SSE consumers but
// is not written to the event store. Use for heartbeats and ephemeral
// state broadcasts (e.g. presenter↔remote view sync).
await lucidos.events.emit('SlidePresenterState', {
  slide_index: 3,
  is_paused: false,
}, { transient: true });

// Query recent events
const events = await lucidos.events.query({
  event_type: 'HabitCompleted',
  since: '2026-04-01T00:00:00Z',
  limit: 50
});

// Read the outcome of child threads the workspace has spawned. This is an
// ENGINE event, not one your app emitted, and it comes back from the same
// call: `thread_id` is the PARENT thread, and the payload carries the child.
// A clean report its child's wake mode files lands as `ChildReportFiled`
// instead, with the same payload, so query both.
const completions = await lucidos.events.query({
  event_type: 'ChildThreadCompleted',
  limit: 20
});
for (const e of completions) {
  console.log(
    e.thread_id,                      // parent thread
    e.payload.child_thread_id,
    e.payload.child_thread_title,
    e.payload.status,                 // success | failure | no_changes | canceled | interrupted
    e.payload.summary
  );
}
```

### Paging with `before_event_id` / `after_event_id`

Rows come back **newest first** (`created DESC, id DESC`) and `limit` is clamped to 1000, so a longer read needs a cursor. Pass the oldest id you received as `before_event_id` to page backwards. Pass the newest id you stored as `after_event_id` to tail-follow what arrived since. Both cursors are exclusive, and both take `LucidosEvent.id` (not `sequence`).

The two are mutually exclusive: set both and the engine answers 400. A cursor id matching no event is a 404, never a silently unfiltered page. `after_event_id` still returns newest-first, so a tail longer than `limit` gives the most recent slice, not the rows right after the cursor.

```js
// Walk backwards from newest until we reach an event we already have.
async function eventsNewerThan(knownId) {
  const collected = [];
  let before;
  for (;;) {
    const page = await lucidos.events.query({
      event_type: 'HabitCompleted',
      limit: 1000,
      before_event_id: before,   // omitted on the first call: start at newest
    });
    if (page.length === 0) return collected;
    for (const e of page) {
      if (e.id === knownId) return collected;   // caught up
      collected.push(e);
    }
    before = page[page.length - 1].id;          // oldest row of this page
  }
}
```

## lucidos.proxy: Call External APIs

Call backends configured in `data/config/apis.json` through the engine. The engine injects the configured auth header from the credential store. It strips `Cookie`/`Origin`/`Referer`/`Host` from the forwarded request, plus every `x-lucidos-*` header and the two `x-forwarded-*` ones the gateway owns. So **the credential never enters the iframe**, and no Lucidos credential reaches the upstream.

This is the preferred way for app UIs to call external HTTP APIs. Direct `fetch` from the iframe hits two walls:

- **Mixed content:** apps load over HTTPS, so the browser blocks `fetch('http://localhost:5005/...')`.
- **CORS:** the upstream rarely allows the engine's origin, so cross-origin XHR fails.

`lucidos.proxy` sidesteps both: the engine makes the upstream call server-side. From an app frame the request travels over the host bridge, and from a standalone app tab it goes direct.

```ts
lucidos.proxy(name: string): ProxyClient

interface ProxyClient {
  fetch(path: string, init?: RequestInit): Promise<Response>;
}
```

`fetch` returns the raw `Response`, so you pick how to read the body (`.json()`, `.text()`, `.blob()`, …). The engine adds the auth header, so do not set `Authorization` from the iframe.

**A model call through it records its own cost.** When the upstream is a model provider, the engine reads the usage block from the reply and writes a `ContextCaptured` with `purpose: "proxy"`. An app never reports its own model spend, and the Token Cost app counts it. To read that block, the engine asks a model provider for an uncompressed reply, so `Content-Encoding` is absent there.

OpenAI speech (`/audio/speech`) reports usage only on an SSE stream. So for `mp3`, `aac`, `pcm` and `wav` the engine asks for one, records it, and hands the app plain audio. `tts-1` records its input's character count, which is what it is billed by. **`flac` and `opus` are refused with a 400**, because neither can report its cost: ask for another format (ADR 0423).

**The upstream cannot act on the Lucidos origin.** The engine serves the response from its own origin, so it passes only the headers a caller reads:

- content, caching and range headers, `Location`, `Link` and `Retry-After`
- `Accept-Patch`, `Accept-Post`, `Allow`, `Date`, `Deprecation`, `Digest`,
  `Repr-Digest`, `Server`, `Sunset`, `Traceparent`, `Tracestate`, `Vary` and `Warning`
- `RateLimit`, `RateLimit-Limit`, `RateLimit-Policy`, `RateLimit-Remaining`,
  `RateLimit-Reset` and `Request-Id`
- `x-` extensions except browser-control headers, `x-lucidos-*` and
  `x-forwarded-*`
- the `anthropic-` and `openai-` header families, for every proxy entry

The engine drops everything else. That covers `Set-Cookie`, `Clear-Site-Data`, `Strict-Transport-Security`, a CSP, CORS headers, `Alt-Svc` and `WWW-Authenticate`, and any other vendor header. Every response carries `X-Content-Type-Options: nosniff`. An HTML, XML or untyped one also carries `Content-Security-Policy: sandbox`, so a proxy URL opened in a tab never runs upstream script.

**The injected credential never comes back.** An upstream can repeat what it received: a pagination `Link`, a redirect `Location`, an error body quoting the request. The engine replaces every credential it injected with `[REDACTED]` in the response headers, the body and its own error text. That covers the raw value, a bearer or basic token and a basic password, each also URL-encoded or with `/` written as `\/`. Other escapes, such as `\uXXXX`, are not matched. A reply that names none arrives byte for byte.

A credentialed call asks the upstream for an uncompressed reply, so `Content-Encoding` is absent. An upstream that compresses anyway gets a 502, because the engine cannot check that body. A `Link` you follow back through the proxy carries `[REDACTED]` where the key was, and the engine injects the real one again.

**A response is buffered, so it does not stream.** The engine reads the whole upstream body before it answers, from a frame or a standalone tab. A token stream arrives complete, so render the finished answer.

**The engine waits 30 seconds on the upstream by default, then answers 504.** A streamed reply counts in full. Raise the wait for every route with the `proxy_timeout_secs` preference, or for one entry with `timeout_secs` in `apis.json`. Both accept 1 to 600 (`system-knowhow/lucidos-cli/subcommands-13.md` § Timeouts). One proxied call never runs past 600 seconds in total, and the bridge waits a little longer, so it never gives up first.

### Configure the backend (one-time)

`data/config/apis.json`:

```json
{
  "sonos":   { "base_url": "http://localhost:5005" },
  "comfort": {
    "base_url": "https://accsmart.panasonic.com",
    "auth": { "pipeline": [
      { "type": "static_credential", "kind": "bearer", "credential": "comfort-cloud" }
    ] }
  }
}
```

Auth runs server-side through an ordered `auth.pipeline`. The proxy URL stays
`/api/v1/proxy/<name>/<path>`. Omit `auth` for an unauthenticated backend.
See [the CLI proxy reference](../lucidos-cli/subcommands-13.md#configure-the-backend-one-time)
and [auth pipeline examples](../building-an-auth-handshake.md) for layer schemas.

### Examples

```js
// GET: unauthenticated local backend
const res = await lucidos.proxy('sonos').fetch('/living-room/play');
if (!res.ok) throw new Error(`Sonos: HTTP ${res.status}`);

// POST JSON: the engine injects the auth header
const res = await lucidos.proxy('comfort').fetch('/api/v1/devices', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ deviceGuid: 'abc' }),
});
const data = await res.json();
```

### Built-in model-provider proxies (no `apis.json` entry needed)

The engine already holds credentials and routing for every model provider in the model registry (set up in Settings → Providers), plus TypeSafe. It exposes them as **built-in provider proxies** under the SAME route. So an app can call an LLM / image / judgment provider with no `apis.json` entry. When `<name>` matches one and has no `apis.json` entry, the engine forwards to that provider's API root and injects its credential server-side:

| `proxy(name)` | Base URL | Injected server-side | You send |
|---|---|---|---|
| `openai` | `https://api.openai.com/v1` | `Authorization: Bearer <key>` | path as-is, e.g. `/chat/completions`, `/images/generations` |
| `openrouter` | `https://openrouter.ai/api/v1` | `Authorization: Bearer <key>` | path as-is, e.g. `/chat/completions` |
| `xai` | `https://api.x.ai/v1` | `Authorization: Bearer <key>` | path as-is, e.g. `/chat/completions` |
| `anthropic` | `https://api.anthropic.com/v1` | `x-api-key: <key>` (or `Authorization: Bearer` for an OAuth credential) | path as-is, e.g. `/messages`; set your own `anthropic-version` header |
| `claude-gateway` | configured gateway origin plus `/v1` | issuer-bound `Authorization: Bearer`, refreshed inside the engine | only `POST /messages`; set `anthropic-version: 2023-06-01` |
| `local` | your configured local base (Ollama default `http://localhost:11434/v1`) | `Authorization: Bearer <key>` (omitted if keyless) | path as-is, e.g. `/chat/completions` |
| `vertex` | `https://<region>-aiplatform.googleapis.com/v1/projects/<project>/locations/<region>` (engine-owned prefix) | `Authorization: Bearer <access-token>` (minted + refreshed server-side) | ONLY the suffix, e.g. `/publishers/anthropic/models/claude-opus-4-8@default:rawPredict` |
| `typesafe` | `https://api.typesafe.ai/v1` | `Authorization: Bearer <key>` | path as-is, e.g. `/systemone` |

- **The engine injects auth.** Set `Content-Type` and `anthropic-version` in `init`. Gateway calls also identify Lucidos and resolve models against the account's managed policy.
- **`apis.json` overrides the builtin.** A same-name entry in `data/config/apis.json` wins, so you can point `openai` at a mock/gateway or add auth layers.
- **Vertex is addressed by suffix.** The engine owns the `…/projects/<project>/locations/<region>` prefix (from its own Vertex config, region default `europe-west1`) and mints the OAuth token. Send only `/publishers/<publisher>/models/<model>:<method>`: any other path answers 400, because the token could manage every resource in the project. The region is the engine's configured one. A model that must run elsewhere (e.g. a `global`-only Gemini variant) needs an `apis.json` override.
- **Not configured → 404.** With no credential/config and no `apis.json` entry, the call returns 404 naming what to set.
- **Every default base already includes `/v1`.** Send `/models`, never `/v1/models`, which doubles the segment and answers 404. A `local` base you configure yourself may lack it.
- **`opencode-free` has no proxy.** The keyless free tier serves chat only, and an app must not build on an anonymous endpoint that can vanish without notice (ADR 0104).
- **`chatgpt-plan` has no proxy.** Apps and scripts never spend the user's ChatGPT plan (ADR 0636). The `openai` proxy always uses the API key.
- **`claude-gateway` shares the engine's sign-in.** Configure and sign in under Settings → Providers.
  Send `/messages` with the normal Anthropic body: `model`, `max_tokens`, and `messages`; `stream: true` is supported.
  Choose a model served to your account. The engine resolves listed snapshots and enforces managed model policy.
  A model id ending in `[1m]` goes upstream without the suffix, plus the 1M-context `anthropic-beta`.

  It refreshes once after a rejected token. Rate/spend refusals retain their body and `Retry-After`; they never trigger provider fallback.
  Successful usage-bearing replies are recorded as proxy spend with gateway provider attribution.
- **The same proxies serve scripts and the agent.** `lucidos proxy <name>` and the `proxy_request` tool resolve names exactly as this route does.

```js
// Chat via the built-in OpenAI proxy: no apis.json, no key in the app
const res = await lucidos.proxy('openai').fetch('/chat/completions', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ model: 'gpt-5.6-sol', messages: [{ role: 'user', content: 'hi' }] }),
});

// Claude on Vertex: the app sends only the publisher/model suffix
const res = await lucidos.proxy('vertex').fetch(
  '/publishers/anthropic/models/claude-opus-4-8@default:rawPredict',
  { method: 'POST', headers: { 'Content-Type': 'application/json' }, body },
);
```

### When to use which

| Want to … | Use |
|---|---|
| Read/write workspace files | `lucidos.data.*` |
| Emit a domain event, or query the event store (domain AND engine events) | `lucidos.events.*` |
| Call a model provider the engine already has (LLM / image / judgment) | `lucidos.proxy('openai' \| 'vertex' \| 'openrouter' \| 'xai' \| 'anthropic' \| 'local' \| 'typesafe').fetch(...)`, no `apis.json` needed |
| Call any other external HTTP API | `lucidos.proxy(name).fetch(path, init)` + an `apis.json` entry |
| Hit an engine endpoint no SDK method covers | `lucidos.request('/<suffix>', init)`. It travels the same bridge, and the engine decides which routes an app may reach. See § `lucidos.request`. |

For any other external API, add an entry to `data/config/apis.json` rather than embedding the credential in the app.

