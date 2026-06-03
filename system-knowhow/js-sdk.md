---
name: Lucidos JavaScript SDK
description: Complete API reference for the lucidos JS SDK — functions, types, parameters, and return types for building Lucidos apps
---

# Lucidos JavaScript SDK

The SDK is available as `lucidos` in app UIs. Import from `@anthropic/lucidos-sdk` in external projects.

> From a Claude Code subprocess, prefer the `lucidos` CLI for `data.*` and `events.*` operations — see [`lucidos-cli.md`](./lucidos-cli.md).

## Setup

App HTML is served as static content — the engine doesn't inject anything (except `?commit=X` rewriting on historical-version requests). Apps opt into each piece they want.

The standard Lucidos app boilerplate:

```html
<!DOCTYPE html>
<html>
  <head>
    <meta charset="utf-8">
    <title>My App</title>
    <script src="/api/v1/sdk-prefs.js"></script>
    <link rel="stylesheet" href="/api/v1/sdk-iframe.css">
    <script src="/api/v1/sdk-iframe-audio.js"></script>
    <script src="/api/v1/sdk.js"></script>
    <script>
      lucidos.ui.applyPreferences();
      lucidos.ui.watchPreferences();
    </script>
  </head>
  <body>
    <!-- app content -->
  </body>
</html>
```

What each piece does — include only what you need:

| Tag | Provides | Skip if |
|---|---|---|
| `<title>` | Tab title | (always include — browsers require it) |
| `<script src="/api/v1/sdk-prefs.js"></script>` | Synchronous prefs script — reads the user's theme/font/scale from `localStorage` (shared with the parent shell via same-origin sandboxing) and sets `data-theme`, `--bg-primary`, and `--font-ui` on `<html>` (plus `--user-ui-scale` when the user has set one) *before* any subsequent stylesheet evaluates. Eliminates the flash-of-default-theme between iframe load and `applyPreferences()`. **Place as early in `<head>` as possible — before `sdk-iframe.css`, before any other `<link rel="stylesheet">`, and before any inline `<style>` that reads theme vars.** Inlining `--bg-primary` directly (not just `data-theme`) is what makes the body's `background: var(--bg-primary, …)` paint correctly even when stylesheets are loaded asynchronously (JS-injected, dynamic `import()`, dev-mode bundlers like Vite that ship CSS as JS modules). | App doesn't use `sdk-iframe.css` (no FOUC to fix) |
| `<link rel="stylesheet" href="/api/v1/sdk-iframe.css">` | Theme tokens (`--bg-primary`, `--accent`, etc.), dark/light variables, default body/input/scrollbar styling | App ships its own complete stylesheet and doesn't want Lucidos theming |
| `<script src="/api/v1/sdk-iframe-audio.js"></script>` | Monkey-patches `AudioContext` so app code reuses a gesture-unlocked instance, survives iOS PWA background cycles. **Must be in `<head>` before any code that creates an `AudioContext`.** | App doesn't play audio |
| `<script src="/api/v1/sdk.js"></script>` | The `lucidos.*` API. Also installs an iframe-friendly link interceptor: `target="_blank"` links resolve in-frame; external `http(s)://` links route through `lucidos.ui.navigate()` | App doesn't use `lucidos.*` |
| `lucidos.ui.applyPreferences()` | Reads the user's theme/font/scale and sets `data-theme` + CSS vars on `<html>`. Pairs with `sdk-iframe.css` to apply the right palette. | Skip (app keeps default dark palette) |
| `lucidos.ui.watchPreferences()` | Re-applies preferences whenever the user changes them (SSE-driven) | Static apps that don't need live preference updates |

**Theme integration is opt-in.** An app that omits both `<script src="/api/v1/sdk-prefs.js">` and `<link rel="stylesheet" href="/api/v1/sdk-iframe.css">` gets no `data-theme` attribute, no CSS variables, and no Lucidos default styling — the engine never auto-injects either tag. This is the right choice for apps that ship their own complete visual identity (charts, games, embedded third-party UIs).

Apps using `lucidos._capture()` don't need to include `html2canvas` — the SDK loads it on demand from `/api/v1/static/html2canvas.min.js`.

External-host apps point `baseUrl` at the Lucidos instance:

```js
lucidos.configure({ baseUrl: 'https://your-lucidos.example' });
```

## Error Handling

All async methods throw `SdkError` on failure:

```js
class SdkError extends Error {
  httpCode: number;
  reason: string;
}
```

## lucidos.data — File Operations

Read, write, and manage files in the workspace `data/` directory.

> **Paths are relative to `data/`, not `data/artifacts/`.** App code lives in `apps/{id}/`, but app *data* must be written under `artifacts/` explicitly — e.g. `artifacts/{app-id}/data.json`. Omitting the prefix gives a 404 `SdkError` from `read` and a silent failure from `write`.

```ts
lucidos.data.read(path: string): Promise<string>
lucidos.data.write(path: string, content: string): Promise<WriteResult>
lucidos.data.delete(path: string): Promise<void>
lucidos.data.list(pattern?: string): Promise<string[]>
lucidos.data.url(path: string): string   // synchronous, returns URL
lucidos.data.edit(path: string, operations: EditOperation[]): Promise<void>
lucidos.data.upload(file: File): Promise<UploadResult>  // 120s timeout
```

### Types

```ts
interface WriteResult { success: boolean; commit?: string }
interface UploadResult { success: boolean; filename?: string; error?: string }
interface EditOperation {
  json_path?: string;   // JSON path edit (see syntax below)
  json_value?: unknown;
  find?: string;        // Text find-replace edit
  replace?: string;
}
```

#### `json_path` syntax

Mix any of these forms in a single path:

| Form                              | Example                            | Resolves to                  |
|-----------------------------------|------------------------------------|------------------------------|
| Dot notation                      | `metadata.author.name`             | `/metadata/author/name`      |
| Array index                       | `sections[1]`                      | `/sections/1`                |
| Quoted key (double or single)     | `dailyLog["2026-05-04"]`           | `/dailyLog/2026-05-04`       |
| JSONPath root marker              | `$.streak`                         | `/streak`                    |
| Raw JSON Pointer (RFC 6901)       | `/sections/1/title`                | `/sections/1/title`          |
| Mixed                             | `habits[0].dailyLog["2026-05-04"]` | `/habits/0/dailyLog/2026-05-04` |

Use **quoted keys** whenever a key contains characters that aren't a bare identifier — dates (`"2026-05-04"`), slugs with dots (`"foo.bar"`), or anything with spaces. Inside a quoted key, `\` escapes the next character. RFC 6901 escaping (`~` → `~0`, `/` → `~1`) is applied automatically.

### Examples

```js
// Read a JSON file
const raw = await lucidos.data.read('artifacts/habits/data.json');
const data = JSON.parse(raw);

// Write content
await lucidos.data.write('artifacts/notes.md', '# My Notes\nContent here.');

// List files matching a pattern
const csvFiles = await lucidos.data.list('artifacts/imported/**/*.csv');

// Edit JSON in-place
await lucidos.data.edit('artifacts/habits/data.json', [
  { json_path: '$.streak', json_value: 5 }
]);

// Edit a key whose name isn't a bare identifier (here: an ISO date)
await lucidos.data.edit('artifacts/habits/data.json', [
  { json_path: 'habits[0].dailyLog["2026-05-04"]', json_value: 3 }
]);

// Get a URL for embedding in HTML
const src = lucidos.data.url('artifacts/screenshots/latest.png');
```

### `url` and app-bundled assets

`lucidos.data.url(path)` normally returns a `/data/...` URL, which always serves from the live workspace. When the SDK is loaded inside an app iframe (`/app/<id>/...`) and `path` points at the app's own bundled folder (`apps/<id>/<rest>`), it instead returns a `/app/<id>/<rest>` URL and carries over `?thread_id=` or `?commit=` from the iframe. This makes JS-set asset URLs (e.g. `img.src = lucidos.data.url('apps/my-app/icon.png')`) load correctly in WIP-preview and historical views — without it, the engine's HTML rewriter only covers markup `src` / `href` attributes and JS-set sources silently 404 against the live workspace. Cross-app references (`apps/<other>/...`) and non-app paths (`artifacts/...`, `knowhow/...`) keep the `/data/` route unchanged.

## lucidos.events — Event Store

Emit and query domain events.

```ts
lucidos.events.emit(type: string, payload: Record<string, unknown>, options?: EmitOptions): Promise<void>
lucidos.events.query(params?: EventQuery): Promise<LucidosEvent[]>
```

### Types

```ts
interface EventQuery {
  event_type?: string;
  since?: string;    // ISO 8601
  until?: string;    // ISO 8601
  limit?: number;    // default 100
}

interface LucidosEvent {
  id: string;
  event_type: string;
  payload: Record<string, unknown>;
  created: string;
  aggregate?: string;
  aggregate_id?: string;
}

interface EmitOptions {
  /** Skip persistence — broadcast on SSE only. */
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

// Emit a transient coordination signal — reaches SSE consumers but
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
```

## lucidos.proxy — Call External APIs

Call backends configured in `data/config/apis.json` through the engine. The engine injects the configured auth header from the credential store and strips `Cookie`/`Origin`/`Referer`/`Host` from the forwarded request — **the credential never enters the iframe**.

This is the preferred way for app UIs to talk to external HTTP APIs. Direct `fetch` from the iframe runs into two walls:

- **Mixed content** — apps load over HTTPS, so `fetch('http://localhost:5005/...')` is blocked by the browser.
- **CORS** — the upstream rarely whitelists the engine's origin, so cross-origin XHR fails.

`lucidos.proxy` sidesteps both: the request goes to the same-origin engine, which forwards server-side.

```ts
lucidos.proxy(name: string): ProxyClient

interface ProxyClient {
  fetch(path: string, init?: RequestInit): Promise<Response>;
}
```

`fetch` returns the raw `Response` so the caller picks how to read the body (`.json()`, `.text()`, `.blob()`, …). The auth header is added server-side; do not set `Authorization` from the iframe.

### Configure the backend (one-time)

`data/config/apis.json`:

```json
{
  "sonos":   { "base_url": "http://localhost:5005" },
  "comfort": {
    "base_url": "https://accsmart.panasonic.com",
    "auth": { "type": "bearer", "credential": "comfort-cloud" }
  }
}
```

Authentication is configured per-API and applied server-side — the iframe never sees credentials, and the URL pattern (`/api/v1/proxy/<name>/<path>`) is identical regardless of auth mode. See `system-knowhow/lucidos-cli.md` § `lucidos proxy` for the full `apis.json` schema (bearer / api_key / basic / query_param / hmac_signed / script_handshake). Omit `auth` for unauthenticated backends (e.g. local services).

### Examples

```js
// GET — unauthenticated local backend
const res = await lucidos.proxy('sonos').fetch('/Spisestua/play');
if (!res.ok) throw new Error(`Sonos: HTTP ${res.status}`);

// POST JSON — auth header injected by engine
const res = await lucidos.proxy('comfort').fetch('/api/v1/devices', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ deviceGuid: 'abc' }),
});
const data = await res.json();
```

### When to use which

| Want to … | Use |
|---|---|
| Read/write workspace files | `lucidos.data.*` |
| Emit/query domain events | `lucidos.events.*` |
| Call an external HTTP API | `lucidos.proxy(name).fetch(path, init)` |
| Hit the engine's own `/api/v1/*` | Plain `fetch` (same origin, no proxy needed) |

If the iframe needs an external API the workspace doesn't have a proxy entry for, add one to `data/config/apis.json` rather than embedding the credential in the app.

## lucidos.oauth — OAuth Token Access

Fetch a short-lived OAuth access token for a connected provider, for in-browser SDKs that need a bearer token in JavaScript (e.g. the Spotify Web Playback SDK). The engine looks up the connected account, refreshes the token if it's expired or expiring within 60s, and returns ONLY the access token — the refresh token never leaves the engine.

```ts
lucidos.oauth.getAccessToken(provider: string): Promise<AccessToken>

interface AccessToken {
  accessToken: string;
  expiresAt: Date | null;  // null when the upstream provider didn't include an expiry
}
```

### When to use

- **You need a bearer token in the iframe**: a third-party SDK like `Spotify.Player` calls a `getOAuthToken` callback expecting a raw token string. There is no other way to hand it the credential — `lucidos.proxy(...)` can't help because the SDK initiates the request itself, not through your code.
- **You are NOT making ordinary HTTP calls to the upstream API**: for those, use `lucidos.proxy(<provider>).fetch(...)` instead — the engine attaches the bearer header server-side and the iframe never sees the token. Only fall back to `getAccessToken` when something forces you to hand a raw token to in-browser code.

### Example — Spotify Web Playback SDK

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

The SDK calls `getOAuthToken` on first init and again when it detects the token has expired — each call hits the engine, which refreshes from the stored refresh token if needed.

### Errors

- `404` — the provider is not connected for this workspace. Ask the user to connect it via the OAuth account settings (or through the LLM `connect_oauth_account` tool).
- `502` — the engine could not refresh the token (missing client credentials, upstream rejected the refresh, network failure).

### Security note

The refresh token, client_id, client_secret, and PKCE state stay on the server. The iframe receives ONLY the short-lived access token, scoped to the connected account. Apps therefore must NOT cache the access token in `localStorage` / `sessionStorage` — re-call `getAccessToken` whenever you need a fresh one (the engine handles caching and refresh).

## lucidos.triggers — Scheduled Tasks

CRUD operations for cron-based and event-based triggers.

```ts
lucidos.triggers.list(): Promise<Trigger[]>
lucidos.triggers.create(trigger: CreateTrigger): Promise<ApiResult>
lucidos.triggers.update(id: string, trigger: UpdateTrigger): Promise<ApiResult>
lucidos.triggers.delete(id: string): Promise<ApiResult>
```

### Types

```ts
type TriggerRun =
  | { type: 'intent'; intent: string }
  | { type: 'script'; path: string };

// One event the trigger listens for, with an optional payload filter scoped
// to that event. A trigger may carry several entries — it fires when an
// incoming event matches *any* entry's event_type AND that entry's
// condition (if set) evaluates true against the payload. Conditions are
// per-entry so different events with different payload shapes never
// constrain each other.
interface EventSubscription {
  event_type: string;
  condition?: Record<string, unknown>;
}

interface Trigger {
  id: string;
  name: string;
  cron_expressions: string[];
  timezone: string;
  paused: boolean;
  last_run?: string;
  next_run?: string;
  run: TriggerRun;
  // Event subscriptions. Empty for schedule-only triggers; the engine omits
  // the field rather than emitting `[]`, so readers must tolerate absence.
  on?: EventSubscription[];
}

interface CreateTrigger {
  name: string;
  run: TriggerRun;
  cron_expressions: string[];
  on?: EventSubscription[];
  /** Optional *trigger group* id; omit for ungrouped. */
  group_id?: string;
}

interface UpdateTrigger {
  name?: string;
  run?: TriggerRun;
  cron_expressions?: string[];
  paused?: boolean;
  // Full replacement for the subscription list. Send the complete new set —
  // there is no partial edit. Pass `[]` to clear all subscriptions.
  on?: EventSubscription[];
  /** Move into a group (string id), clear membership (null), or leave it
   *  unchanged (absent). */
  group_id?: string | null;
}

interface ApiResult {
  success: boolean;
  error?: string;
}
```

### Subscribing to multiple events from one trigger

Pass several entries in `on` when one workflow should react to more than one event type:

```js
await lucidos.triggers.create({
  name: 'Important inbound nudge',
  run: { type: 'intent', intent: 'Summarize what just happened and ping me.' },
  cron_expressions: [],
  on: [
    { event_type: 'MessageReceived', condition: { from: 'partner' } },
    { event_type: 'EmailReceived',   condition: { from: 'boss@example.com' } },
  ],
});
```

Each entry's `condition` only applies to its own `event_type` — the `from: 'partner'` filter on `MessageReceived` does NOT block `EmailReceived` from firing on its own filter.

Trigger groups are user-visible folders shown in the triggers panel. Pure organizational labels — they have no schedule, run no code, and don't coordinate firing. Apps that organize the triggers they create can pass `group_id` to `create` / `update`; the engine validates the id against the workspace's group registry and rejects unknown values. The SDK does not expose group CRUD today — group management lives behind the engine's HTTP and LLM-tool surfaces.

## lucidos.apps — App Management

```ts
lucidos.apps.list(): Promise<App[]>
lucidos.apps.get(id: string): Promise<App>
```

### Types

```ts
interface App {
  id: string;
  name: string;
  description: string;
  instructions?: string;
  has_ui: boolean;
  created_at: string;
  updated_at?: string;
}
```

## lucidos.preferences — User Settings

```ts
lucidos.preferences.get(deviceId?: string | null): Promise<Preferences>
lucidos.preferences.set(key: string, value: string, deviceId?: string): Promise<void>
```

`get()` defaults to the parent device id (read from the shared `lucidos-device-id`
in localStorage), so iframes see the same merged view as the parent UI. Pass
`null` to fetch only globally-scoped preferences.

### Types

```ts
type Preferences = Record<string, string>;
```

### Common keys

| Key | Values | Description |
|-----|--------|-------------|
| `theme` | `dark`, `light`, `system` | UI theme |
| `font-family` | `monospace`, `system`, `inter`, `jetbrains-mono`, `ibm-plex-mono` | Font |
| `ui-scale` | Number in 12.5% steps from 75 to 200 (`75`, `87.5`, `100`, `112.5`, `125`, `137.5`, `150`, `162.5`, `175`, `187.5`, `200`); or the legacy strings `small` / `medium` / `large` (= `100` / `112.5` / `125`). Off-grid numbers snap to the nearest valid step. | Scale |

## lucidos.notifications — Notification Center

```ts
lucidos.notifications.list(params?: {
  limit?: number;
  before?: number;
  filter?: string;
}): Promise<NotificationListResult>

lucidos.notifications.markRead(id: string): Promise<void>
lucidos.notifications.markAllRead(): Promise<void>
```

### Types

```ts
type NavigateTarget =
  | 'files' | 'apps' | 'triggers' | 'changes' | 'notifications'
  | 'settings' | 'app' | 'file' | 'trigger' | 'thread'
  | 'new-app' | 'new-trigger' | 'new-chat' | 'url';

interface NavigateUi {
  target: NavigateTarget;
  settings_view?: 'devices' | 'accounts' | 'backup' | 'memory' | 'repositories';
  app_id?: string;
  file_path?: string;
  id?: string;
  url?: string;
  event_id?: string;
  prompt?: string;
}

/** What a notification tap does. `modal` opens the inbox modal showing the
 *  message body. `none` marks the row read with no navigation (passive
 *  pushes — "OAuth completed"). `navigate` delegates to the same router the
 *  `navigate_ui` LLM tool uses; `to` is its arg shape. Every kind marks the
 *  source notification read on tap. */
type Tap =
  | { kind: 'modal' }
  | { kind: 'none' }
  | { kind: 'navigate'; to: NavigateUi };

interface Notification {
  id: string;
  task_id?: string;
  app_id?: string;
  /** Originating thread, when the notification has one. Drives the inbox
   *  modal's "Open thread" button. */
  thread_id?: string;
  /** Specific event UUID inside `thread_id` that raised this notification —
   *  the §4 in-app matrix uses it to silently mark-read when the user is
   *  looking at the source event. Distinct from `tap.to.event_id` (which
   *  is the scroll-and-pulse target when the tap navigates to a thread). */
  event_id?: string;
  title: string;
  message: string;
  created_at: string;
  read: boolean;
  /** What happens on tap. See `Tap`. Default `{kind:'modal'}` when absent. */
  tap?: Tap;
}

interface NotificationListResult {
  notifications: Notification[];
  unread_count: number;
  has_more: boolean;
}
```

### Tap shapes — examples

The SDK only exposes `list` / `markRead` / `markAllRead` for reading the inbox. Creating a notification from app code goes through the engine HTTP API directly (`POST /api/v1/notifications` — same wire shape the `lucidos notify` CLI and the `send_notification` LLM tool produce):

```js
// Default: open the inbox modal showing the message body.
await fetch('/api/v1/notifications', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    title: 'Daily summary',
    message: 'Here is your summary…',
    tap: { kind: 'modal' },
  }),
});

// Passive: mark read on display, no navigation. Use for "OAuth completed",
// "Build succeeded" — the push IS the message.
await fetch('/api/v1/notifications', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    title: 'OAuth completed',
    message: 'Google account connected.',
    tap: { kind: 'none' },
  }),
});

// Navigate to a panel.
await fetch('/api/v1/notifications', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    title: '5 changes ready to apply',
    message: 'Review the Changes panel.',
    tap: { kind: 'navigate', to: { target: 'changes' } },
  }),
});

// Navigate to a thread, optionally scroll-and-pulse a specific event row.
await fetch('/api/v1/notifications', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    title: 'Claude is asking',
    message: 'Permission needed.',
    thread_id: 't-9',
    event_id: 'e-7',
    tap: { kind: 'navigate', to: { target: 'thread', id: 't-9', event_id: 'e-7' } },
  }),
});

// Navigate to an app's UI.
await fetch('/api/v1/notifications', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    title: 'Habit tracker reminder',
    message: 'Tap to log today.',
    app_id: 'habit-tracker',
    tap: { kind: 'navigate', to: { target: 'app', app_id: 'habit-tracker' } },
  }),
});
```

From scripts (Python/bash), use the `lucidos notify` CLI — it constructs the same body. From LLM threads, use the `send_notification` tool.

## lucidos.threads — Thread Management

```ts
lucidos.threads.list(opts?: ThreadsListOptions): Promise<ThreadSummary[]>
lucidos.threads.count(opts?: Omit<ThreadsListOptions, 'limit'>): Promise<number>
```

`list()` calls `GET /api/v1/threads/list` and returns a newest-first array of `ThreadSummary` rows from the projection. `count()` calls `GET /api/v1/threads/count` and resolves to the integer count under the same filter — cheaper on big workspaces than reading `(await list()).length`.

Same canonical surface as the `lucidos threads list` / `lucidos threads count` CLI and the `list_threads` / `count_threads` LLM tools. Use this when an app needs to render or react to thread state (counts, status indicators) without subscribing to the full SSE stream.

### Types

```ts
interface ThreadsListOptions {
  /** true → only threads where the agentic loop is mid-flow
   *  (status 'running' or 'waiting_for_user_answer'). false → invert.
   *  Omit → no filter. Note: 'waiting' is NOT active — it means CC has
   *  stopped and proposed changes the user must act on. */
  active?: boolean;
  /** Comma-separated source filter: 'chat', 'trigger', 'claude_code'. */
  source?: string;
  /** Server clamps to 1..=1000 (default 100). */
  limit?: number;
}

/** Projected snapshot of a thread's metadata, derived from the event
 *  stream by the `thread_summaries` projection. */
interface ThreadSummary {
  thread_id: string;
  title: string;
  channel: string;
  initiator: 'user' | 'system';
  created_at: string;
  last_activity: string;
  message_count: number;
  /** 'inbox' | 'archived' — stored in thread_summaries.archive_state. */
  section: string;
  active_children_count: number;
  total_children_count: number;
  /** 'idle' | 'running' | 'waiting' | 'failed' | 'waiting_for_user_answer'. */
  status: string;
  coding_agent_has_diff: boolean;
  coding_agent_proposed: boolean;
  coding_agent_requires_restart: boolean;
  coding_agent_is_external_repo: boolean;
  coding_agent_applying: boolean;
  last_revived_at: string | null;
  parent_thread_id?: string | null;
  parent_thread_title?: string | null;
  trigger_id?: string | null;
  trigger_name?: string | null;
  cc_repo_id?: string | null;
  cc_repo_name?: string | null;
  /** Compose state machine — `composing` | `active` | `discarded`. The
   *  archive flag is on the separate `section` field; an archived thread
   *  carries `state: 'active'` and `section: 'archived'`. */
  state: 'composing' | 'active' | 'discarded';
  compose_text: string;
  compose_images: string[];
  compose_mode?: 'lucidos' | 'claude_code' | null;
}
```

### When to use which

| Want to … | Use |
|---|---|
| Render a list of threads in an app UI | `lucidos.threads.list()` |
| Show "N active threads" badge | `lucidos.threads.count({ active: true })` |
| React to thread state changes in real time | Subscribe to `lucidos.sse` instead |
| Spawn a new thread from an app | `lucidos.ui.startThread({ prompt })` |

## lucidos.ui — UI Control

```ts
lucidos.ui.applyPreferences(): Promise<void>
lucidos.ui.navigate(target: string, params?: Record<string, string>): Promise<void>
lucidos.ui.startThread(opts?: { prompt?: string }): Promise<void>
lucidos.ui.confirm(options: ConfirmOptions): Promise<boolean>
lucidos.ui.Select.create(opts: SelectCreateOptions): SelectInstance
lucidos.ui.enhanceSelects(root?: ParentNode): SelectInstance[]
```

`applyPreferences()` fetches user preferences and applies theme, font, and scale as CSS variables. Call once on app load.

`navigate()` sends a navigation request to the Lucidos frontend via SSE.

### Navigation targets

| Target | Params | Description |
|--------|--------|-------------|
| `thread` | `id` | Focus a specific thread |
| `app` | `id` | Open an app UI |
| `new-chat` | `prompt` (optional) | Open a fresh chat thread, optionally prefilling the compose textarea. Prefer `lucidos.ui.startThread()` — it's the typed wrapper around this target. |

### Starting a fresh chat with a prefilled prompt

`lucidos.ui.startThread()` opens a new chat thread. If you pass a `prompt`, it lands in the compose textarea **prefilled** — the user reviews, edits, and clicks Send. It is never auto-submitted, so the user always stays in control of what gets sent on their behalf.

```js
// "Set this up for me" button — pops a fresh chat with a ready-to-send prompt.
document.querySelector('#setup-trigger').addEventListener('click', () => {
  lucidos.ui.startThread({
    prompt: 'Create a daily 9am trigger that summarizes my unread email.',
  });
});
```

Call with no arguments (`lucidos.ui.startThread()`) to just open a blank fresh chat — equivalent to the user pressing the "new thread" shortcut.

### Confirmation dialogs

`lucidos.ui.confirm` shows a modal rendered by the Lucidos shell (not inside your app iframe), so it inherits the user's theme and sits above all app content. Use it instead of `window.confirm()`.

```ts
interface ConfirmOptions {
  /** Optional heading. Renders above the message. */
  title?: string;
  /** Required. Plain text. Use \n for line breaks. */
  message: string;
  /** Default: "Confirm". */
  okLabel?: string;
  /** Default: "Cancel". */
  cancelLabel?: string;
  /** Style the OK button as destructive (red). Default: false. */
  danger?: boolean;
}
```

Resolves `true` on OK click or Enter; `false` on Cancel, Esc, or backdrop click.

If a second `confirm` is called while one is visible, the previous one resolves `false` and the new one replaces it.

**Example:**

```js
const ok = await lucidos.ui.confirm({
  title: 'Delete node?',
  message: 'Delete "Reduce CPAC by 50%" and its 3 descendants?',
  okLabel: 'Delete',
  danger: true,
});
if (!ok) return;
// proceed with deletion
```

### lucidos.ui.Select — themed dropdown

Replaces native `<select>` (whose popup the OS draws and CSS can't reach) with a fully themed dropdown that uses the same tokens as the rest of Lucidos. Supports keyboard nav, type-to-select, light + dark mode.

#### Types

```ts
interface SelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

interface SelectCreateOptions {
  options: SelectOption[];
  value?: string;
  placeholder?: string;
  disabled?: boolean;
  className?: string;
  onChange?: (value: string, option: SelectOption | undefined) => void;
}

interface SelectInstance {
  element: HTMLElement;          // insert into the DOM
  getValue(): string | undefined;
  setValue(value: string | undefined): void;
  setOptions(options: SelectOption[]): void;
  setDisabled(disabled: boolean): void;
  open(): void;
  close(): void;
  destroy(): void;               // removes listeners + detaches the element
}
```

#### Keyboard

| Key | Action |
|---|---|
| `ArrowDown` / `ArrowUp` | Open the menu, then move focus through options |
| `Home` / `End` | Jump to first / last option (when open) |
| `Enter` / `Space` | Open the menu, or commit the focused option |
| `Escape` | Close the menu without changing the value |
| Letter keys | Jump to the next option whose label starts with the typed prefix (multi-character buffer, resets after 500 ms) |
| `Tab` | Close the menu and move focus to the next focusable element |

#### Programmatic usage

```js
const sel = lucidos.ui.Select.create({
  options: [
    { value: 'apple', label: 'Apple' },
    { value: 'banana', label: 'Banana' },
    { value: 'cherry', label: 'Cherry' },
  ],
  value: 'apple',
  placeholder: 'Pick a fruit…',
  onChange: (v) => console.log('picked', v),
});
document.querySelector('#my-container').appendChild(sel.element);

// Later, mutate it from outside:
sel.setValue('banana');
sel.setOptions([{ value: 'd', label: 'Durian' }]);
sel.setDisabled(true);

// Tear down when the host UI unmounts:
sel.destroy();
```

#### Declarative usage — enhance existing `<select>` elements

`enhanceSelects()` walks `root` (default `document`) and replaces every
`<select class="lucidos-select">` it finds. The native element stays in the DOM
(hidden) — its `value` mirrors the user's selection and `change` events still
fire on it, so existing form code keeps working unchanged. Already-enhanced
selects are skipped, so it's safe to call again after adding new ones.

```html
<select class="lucidos-select" data-placeholder="Choose…">
  <option value="todo">To do</option>
  <option value="doing">In progress</option>
  <option value="done">Done</option>
</select>
<script>
  lucidos.ui.enhanceSelects();
</script>
```

## lucidos.sse — Real-time Events

Subscribe to server-sent events for live updates.

```ts
lucidos.sse.connect(): void
lucidos.sse.disconnect(): void
lucidos.sse.on(eventType: string, callback: (data: unknown, raw: SseEvent) => void): () => void
```

`on()` returns an unsubscribe function. Subscribe by inner event name — the SDK unwraps the wire format.

### Types

```ts
interface SseThreadEvent {
  type: 'ThreadEvent';
  data: {
    thread_id: string;
    event: { type: string; [key: string]: unknown };
    created: string;
    seq?: number;
    event_id: string;
  };
}

interface SseSystemEvent {
  type: string;
  data: Record<string, unknown>;
}

type SseEvent = SseThreadEvent | SseSystemEvent;
```

### Examples

```js
lucidos.sse.connect();

// Listen for navigation requests
const unsub = lucidos.sse.on('NavigationRequested', (data) => {
  console.log('Navigate to:', data);
});

// Listen for notifications
lucidos.sse.on('NotificationCreated', (data) => {
  showToast(data.title);
});

// Wildcard — all events
lucidos.sse.on('*', (raw) => {
  console.log('Event:', raw);
});

// Cleanup
unsub();
lucidos.sse.disconnect();
```

## lucidos.utils — Utilities

```ts
lucidos.utils.timeAgo(iso: string): string      // "5m ago", "2d ago", "just now"
lucidos.utils.escapeHtml(str: string): string    // HTML-escape
lucidos.utils.formatDate(iso: string): string    // Locale-formatted date string
```

## App UI Pattern

Standard app initialization:

```js
import { lucidos } from '@anthropic/lucidos-sdk';

// Apply user theme/font/scale
await lucidos.ui.applyPreferences();

// Connect SSE for live updates
lucidos.sse.connect();

// Load data
const raw = await lucidos.data.read('artifacts/my-app/data.json');
const state = JSON.parse(raw);

// Listen for relevant events
lucidos.sse.on('MyAppDataUpdated', (data) => {
  // Re-render with new data
});
```
