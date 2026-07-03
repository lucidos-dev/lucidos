---
name: Lucidos JavaScript SDK
description: Complete API reference for the lucidos JS SDK — functions, types, parameters, and return types for building Lucidos apps
---

# Lucidos JavaScript SDK

The SDK is available as the `lucidos` global in app UIs (loaded via `<script src="/api/v1/sdk.js">`). The host frontend imports it from the `@lucidos/sdk` package directly.

> From a coding-agent subprocess, prefer the `lucidos` CLI for `data.*` and `events.*` operations — see [`lucidos-cli.md`](./lucidos-cli.md).

## Setup

App HTML is served as static content — the engine doesn't inject anything (except `?thread_id=` rewriting on WIP-preview requests). Apps opt into each piece they want.

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
| `<link rel="stylesheet" href="/api/v1/sdk-iframe.css">` | Theme tokens (`--bg-primary`, `--accent`, etc.), dark/light variables, default body/input/scrollbar styling, **and Lucidos's shared component classes** (`.action-btn` + `.action-btn-confirm`/`.action-btn-danger`, `.icon-btn`, `.label`, `.title`, `.segmented-control`/`.segmented-btn`, `.list-row*`, `.markdown-content`, `.progress-bar`, `.empty-state`, `.accent-link`) — use these class names and the app's buttons/lists/etc. render identically to the host shell. The body inherits the root font-size (the user's UI scale), matching Lucidos. | App ships its own complete stylesheet and doesn't want Lucidos theming |
| `<script src="/api/v1/sdk-iframe-audio.js"></script>` | Monkey-patches `AudioContext` so app code reuses a gesture-unlocked instance, survives iOS PWA background cycles. **Must be in `<head>` before any code that creates an `AudioContext`.** | App doesn't play audio |
| `<script src="/api/v1/sdk.js"></script>` | The `lucidos.*` API. Also installs two iframe-only side effects: a link interceptor (`target="_blank"` links resolve in-frame; external `http(s)://` links route through `lucidos.ui.navigate()`) and a keyboard-shortcut forwarder (host shortcuts like focus/hide a pane, narrow/widen, new thread, search, and Escape keep working while the app has focus — iframe keydowns otherwise never reach the host). Only modifier-bearing chords and Escape are forwarded; plain typing stays in the app. | App doesn't use `lucidos.*` |
| `lucidos.ui.applyPreferences()` | Reads the user's theme/font/scale (resolving a `system` preference to the live OS light/dark) and sets `data-theme` + CSS vars on `<html>`. Pairs with `sdk-iframe.css` to apply the right palette. | **Don't skip if you include `sdk-iframe.css`** — without it the app ignores the user's light/system setting and stays on the default dark palette. Skip only when opting out of Lucidos theming entirely. |
| `lucidos.ui.watchPreferences()` | Re-applies preferences live: when the user changes one (SSE `PreferencesChanged`), and — under a `system` preference — when the OS light/dark appearance flips (a `prefers-color-scheme` listener, off iOS, matching the host shell) | Static apps that have opted out of Lucidos theming |

**Inherit the theme by default.** A normal app includes the theme assets, calls `applyPreferences()` + `watchPreferences()`, and styles with the theme variables (below) — so it follows the user's light/dark (OS) appearance just like the rest of Lucidos. Theme integration is *technically* opt-in: the engine never auto-injects these tags, so an app that omits both `<script src="/api/v1/sdk-prefs.js">` and `<link rel="stylesheet" href="/api/v1/sdk-iframe.css">` gets no `data-theme` attribute, no CSS variables, and no Lucidos default styling. Opt out only for an app that ships its own complete visual identity (charts, games, embedded third-party UIs) — otherwise inheriting is the default, and **hardcoding colors is a bug** (a light-mode workspace gets a dark-only app, or vice versa).

### Theme variables

`sdk-iframe.css` defines these CSS custom properties on `<html>` and flips their values between light and dark automatically — driven by the `data-theme` attribute, which `applyPreferences()` sets (resolving `system` to the OS setting) and `watchPreferences()` keeps in sync. Style your app with `var(--name)` and it tracks the user's appearance for free. The canonical values live in the engine's `sdk-iframe.css`; **the names are the contract**:

| Group | Variables |
|---|---|
| Backgrounds | `--bg-primary`, `--bg-secondary`, `--bg-tertiary`, `--bg-quaternary`, `--bg-hover`, `--bg-selected` |
| Text | `--text-primary`, `--text-secondary`, `--text-muted`, `--text-on-accent` |
| Border | `--border-color` |
| Accents | `--accent`, `--accent-light`, `--accent-green`, `--accent-yellow`, `--accent-red` |
| Focus | `--focus-ring` — a ready-made `box-shadow` value (a soft accent band) for focus indicators; the `.action-btn`/`.icon-btn` classes use it, and your own controls match the host with `:focus-visible { box-shadow: var(--focus-ring); }` |
| Shadows | `--shadow-sm`, `--shadow-md`, `--shadow-lg` |
| Layout (theme-independent) | `--font-ui`, `--font-mono`, `--transition`, `--user-ui-scale` — plus the spacing / radius / motion scales below |

The user's UI font is **`--font-ui`** — that's the canonical token, set live to the
user's font choice. You rarely need to apply it yourself: `sdk-iframe.css` already
sets `body { font-family: var(--font-ui) }` (and on inputs/buttons), so any element
that inherits gets the right font for free. Only re-declare `font-family` when you
deliberately override it, and then use `var(--font-ui)`. As a safety net,
`--font-family` and `--font` are tolerated **aliases** of `--font-ui` (so the
intuitive guess still resolves to the user's font instead of silently dropping to a
hardcoded fallback) — but `--font-ui` is the name to write.

The spacing, radius, motion, icon, and type scales are theme-independent and have
fixed values — **use the token, not a magic number, and never a `px` fallback
that disagrees with the real value** (`var(--space-xl, 28px)` is a latent bug —
`--space-xl` is `1.5rem` = 24px). When you include `sdk-iframe.css` these are
always defined, so a fallback is dead noise at best:

| Token | Value | | Token | Value |
|---|---|---|---|---|
| `--space-xs` | `0.25rem` (4px) | | `--radius-sm` | `0.25rem` (4px) |
| `--space-sm` | `0.5rem` (8px) | | `--radius-md` | `0.375rem` (6px) |
| `--space-md` | `0.75rem` (12px) | | `--radius-lg` | `0.5rem` (8px) |
| `--space-lg` | `1rem` (16px) | | `--icon-size-sm` | `0.875rem` (14px) |
| `--space-xl` | `1.5rem` (24px) | | `--icon-size-md` | `1rem` (16px) |
| `--duration-fast` | `0.15s` | | `--icon-size-lg` | `1.25rem` (20px) |
| `--duration-normal` | `0.2s` | | `--duration-slow` | `0.3s` |

**Type scale — `--font-size-*`.** The sanctioned font sizes; the host shell and
this SDK stylesheet both size text from these, so use the token instead of a raw
`rem`. All `rem`, so every step scales with the user's UI-scale preference.

| Token | Value | Role | | Token | Value | Role |
|---|---|---|---|---|---|---|
| `--font-size-3xs` | `0.5625rem` (9px) | micro-label / tiny badge | | `--font-size-lg` | `0.875rem` (14px) | emphasis |
| `--font-size-2xs` | `0.625rem` (10px) | dots, micro-meta | | `--font-size-xl` | `1rem` (16px) | section heading |
| `--font-size-xs` | `0.6875rem` (11px) | dense metadata | | `--font-size-2xl` | `1.125rem` (18px) | larger heading |
| `--font-size-sm` | `0.75rem` (12px) | labels, secondary | | `--font-size-3xl` | `1.25rem` (20px) | large heading |
| `--font-size-md` | `0.8125rem` (13px) | body default | | `--font-size-display` | `2.25rem` (36px) | hero |

```css
.card {
  background: var(--bg-secondary);
  color: var(--text-primary);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  padding: var(--space-lg);
}
.card a { color: var(--accent); }
```

#### Respect the user's font size — size in `rem`, never `px`

The user's UI-scale preference is applied as the **root font-size**
(`html { font-size: var(--user-ui-scale, 100%) }`), so **only `rem`/`em` units
scale with it.** An app that sizes text, padding, gaps, and radii in `px`
renders at a fixed size and silently ignores the user's font-size setting — the
single most common "the app doesn't respect my font size" bug. Size everything
in `rem` (divide px by 16: 14px → `0.875rem`, 24px → `1.5rem`), and prefer the
`--space-*` / `--radius-*` tokens above for spacing and corners. For text, prefer
the `--font-size-*` type-scale tokens over a raw `rem` — body text is
`--font-size-md` (13px), small/meta `--font-size-xs` (11px), emphasis
`--font-size-lg` (14px), headings `--font-size-xl`+ (or the `h1`–`h6` defaults
`sdk-iframe.css` already ships). (`1px` borders are the one acceptable `px`
exception, same as the host shell.)

### Component classes

`sdk-iframe.css` also ships Lucidos's shared component layer — literally the
**same CSS the host shell uses, from one source**: the engine appends
`crates/lucidos-app/src/styles/global/shared-components.css` (which the host
itself imports via `global.css`) to the served stylesheet. There is no copy and
nothing to keep in sync — apply these class names and your app's controls render
exactly like the rest of Lucidos (and track the theme + UI scale for free). The
one exception is the app-facing `.action-btn-secondary` below, which lives in the
engine's `sdk-iframe.css` (the host has no equivalent, so it isn't in the shared
file). The class names are the contract:

| Class | Use for |
|---|---|
| `.action-btn` (+ `.action-btn-confirm` green, `.action-btn-danger` red) | The filled primary CTA button — blue, with the confirm/danger variants additive (`class="action-btn action-btn-danger"`) |
| `.action-btn-secondary` | A neutral, outlined secondary button for a lower-emphasis action beside a primary CTA — additive: `class="action-btn action-btn-secondary"`. **Use this instead of hand-rolling an off-palette outlined button.** |
| `.icon-btn` | A small borderless icon button (wrap an SVG sized via `--icon-size-sm`) |
| `.accent-link` | An inline text link/button in the accent color |
| `.label` | A small uppercase badge |
| `.title` | A list/panel/modal title |
| `.segmented-control` + `.segmented-btn` (`.active`) | A toggle button group |
| `.list-rows`, `.list-row`, `.list-row-info`, `.list-row-name`, `.list-row-actions`, `.list-section-title`, … | List/row layouts |
| `.markdown-content` | A container for rendered markdown (headings, tables, code, blockquotes) |
| `.progress-bar` + `.progress-bar-fill`, `.progress-label` | A progress indicator |
| `.empty-state`, `.error-text` | Empty/error placeholders |

Prefer these over hand-rolling buttons and rows — a plain unclassed `<button>`
gets a neutral default that does **not** match Lucidos's primary blue button.

Apps using `lucidos._capture()` don't need to include `html2canvas` — the SDK loads it on demand from `/api/v1/static/html2canvas.min.js`. `html2canvas` can't rasterize CSS Color 4 functions (`color()`, `oklab()`, `oklch()`, `color-mix()`); when the screenshot fails for any reason the capture degrades to **DOM-only** — it returns an empty `screenshot` plus a `dom` layout snapshot (element positions + classes) prefixed with the failure reason, rather than throwing. The agent still sees the rendered layout instead of going blind.

External-host apps point `baseUrl` at the Lucidos instance with `lucidos.configure`:

```ts
lucidos.configure(opts: { baseUrl?: string; token?: string }): void
```

```js
lucidos.configure({ baseUrl: 'https://your-lucidos.example' });
```

`baseUrl` overrides the auto-derived workspace base path (in-app iframes don't
need it — the SDK reads the gateway prefix from `<base href>` / the `/app/` URL).
`token`, when set, is sent as an `Authorization: Bearer <token>` header on every
SDK request — for embedders calling a remote engine that requires auth. Both are
optional and each call merges into the existing config.

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

`lucidos.data.url(path)` normally returns a `/data/...` URL, which always serves from the live workspace. When the SDK is loaded inside an app iframe (`/app/<id>/...`) and `path` points at the app's own bundled folder (`apps/<id>/<rest>`), it instead returns a `/app/<id>/<rest>` URL and carries over `?thread_id=` from the iframe. This makes JS-set asset URLs (e.g. `img.src = lucidos.data.url('apps/my-app/icon.png')`) load correctly in WIP-preview — without it, the engine's HTML rewriter only covers markup `src` / `href` attributes and JS-set sources silently 404 against the live workspace. Cross-app references (`apps/<other>/...`) and non-app paths (`artifacts/...`, `knowhow/...`) keep the `/data/` route unchanged.

One other special case: a `system-knowhow/...` path is routed through the engine's `/api/v1/data/...` endpoint (these files live in the engine repo, not the workspace, so the static `/data` mount can't serve them).

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

// Irreversible-side-effect category a trigger can be granted. Only enforced
// when the workspace's command guard is on (Settings → Permissions → Command
// Safety). A trigger that hits an irreversible command whose category isn't in
// its grant is failed (it can't be asked to approve — it runs unattended).
type SideEffectCategory =
  | 'email'
  | 'external_api'
  | 'cloud_cli'
  | 'out_of_workspace_destruction'
  | 'other';

interface Trigger {
  id: string;
  name: string;
  cron_expressions: string[];
  timezone: string;
  paused: boolean;
  last_run?: string;
  // Outcome of the most recent completed firing. Absent until the trigger has
  // run once under an engine that records status (legacy runs → timestamp only).
  last_run_status?: 'ok' | 'failed';
  next_run?: string;
  run: TriggerRun;
  // Event subscriptions. Empty for schedule-only triggers; the engine omits
  // the field rather than emitting `[]`, so readers must tolerate absence.
  on?: EventSubscription[];
  // Side-effect grant — irreversible categories this trigger may perform
  // unattended. Omitted when empty (= no grant).
  side_effect_grant?: SideEffectCategory[];
}

interface CreateTrigger {
  name: string;
  run: TriggerRun;
  cron_expressions: string[];
  on?: EventSubscription[];
  /** Optional *trigger group* id; omit for ungrouped. */
  group_id?: string;
  /** Side-effect grant — irreversible categories this trigger may perform
   *  unattended. Omit / `[]` = none granted (the safe default). */
  side_effect_grant?: SideEffectCategory[];
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
  /** Full replacement for the side-effect grant; pass `[]` to clear all. */
  side_effect_grant?: SideEffectCategory[];
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
  id: string;             // folder name under data/apps/
  name: string;
  description: string;
  /** Optional icon from the app's manifest.json (emoji or asset path).
   *  Omitted when the manifest has none. */
  icon?: string;
}
```

The shape mirrors the app's `manifest.json` (`name` / `description` / `icon`) plus
the `id` derived from its folder. `list()` hits `GET /api/v1/apps`; `get(id)`
hits `GET /api/v1/app?id=<id>` and throws a `404` `SdkError` for an unknown id.

### Example

```js
const apps = await lucidos.apps.list();
const me = await lucidos.apps.get('habit-tracker');
console.log(me.name, me.icon ?? '(no icon)');
```

## lucidos.preferences — User Settings

```ts
lucidos.preferences.get(deviceId?: string | null): Promise<Preferences>
lucidos.preferences.set(key: string, value: string, deviceId?: string): Promise<void>
```

`get()` defaults to the parent device id, so iframes see the same merged view as
the parent UI. The device id is per-workspace (each workspace has its own device
identity); the SDK reads it from the workspace-scoped `lucidos-device-id`
(`ws:<slug>:lucidos-device-id`) so the iframe and the parent agree. Pass `null`
to fetch only globally-scoped preferences.

### Types

```ts
type Preferences = Record<string, string>;
```

### Common keys

| Key | Values | Description |
|-----|--------|-------------|
| `theme` | `dark`, `light`, `system` | UI theme |
| `font-family` | `monospace`, `system`, `inter`, `jetbrains-mono`, `ibm-plex-mono`, `fira-code` | Font (`fira-code` also enables programming ligatures) |
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

`NavigateTarget` and `SettingsViewTarget` are **generated from the engine's
`navigate_ui` tool** (the `NAVIGATE_TARGETS` / `NAVIGABLE_SETTINGS_VIEWS` consts in
`crates/lucidos-engine/src/llm/tools/misc.rs`) into
`packages/lucidos-sdk/src/generated/navigate-targets.ts`, so the SDK and the LLM
tool schema cannot drift. To change the set, edit those Rust consts and run
`cargo test -p lucidos-engine --lib generate_navigate_targets_file -- --ignored`.

```ts
type NavigateTarget =
  | 'files' | 'apps' | 'app-store' | 'plugins' | 'triggers' | 'thread-queue' | 'changes' | 'notifications'
  | 'settings' | 'app' | 'file' | 'trigger' | 'thread'
  | 'new-app' | 'new-trigger' | 'new-chat' | 'url';

// Settings sub-section for `target: 'settings'` (platform-gated 'mobile-access' /
// 'experimental' are intentionally excluded).
type SettingsViewTarget =
  | 'models' | 'appearance' | 'devices' | 'accounts' | 'repositories' | 'marketplaces'
  | 'permissions' | 'keyboard-shortcuts' | 'system'
  | 'thread-queue' | 'backup' | 'memory' | 'disk-usage' | 'environment-variables';

interface NavigateUi {
  target: NavigateTarget;
  settings_view?: SettingsViewTarget;
  app_id?: string;
  file_path?: string;
  id?: string;
  url?: string;
  event_id?: string;
  prompt?: string;
}

/** What a notification tap does. `modal` (default) opens the inbox detail
 *  showing the message body. `navigate` delegates to the same router the
 *  `navigate_ui` LLM tool uses; `to` is its arg shape. Both mark the source
 *  notification read on tap. Every notification is openable — the old passive
 *  `none` kind is retired; a historical `{kind:'none'}` is coerced to `modal`. */
type Tap =
  | { kind: 'modal' }
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
// Default: open the inbox detail showing the message body. Use this for any
// info-only notification too ("OAuth completed", "Build succeeded") — every
// notification is openable; there is no separate passive kind. For ephemeral
// status that should NOT land in the inbox at all, use a plain `showToast`.
await fetch('/api/v1/notifications', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    title: 'Daily summary',
    message: 'Here is your summary…',
    tap: { kind: 'modal' },
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
    title: 'Coding agent is asking',
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
   *  Omit → no filter. Note: 'waiting' is NOT active — it means the coding agent has
   *  stopped and proposed changes the user must act on. */
  active?: boolean;
  /** Comma-separated source filter: 'chat', 'trigger', 'coding-agent'.
   *  Legacy 'claude_code' is also accepted. */
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
  /** When the user last drove the thread forward (message/answer/permission/
   *  change apply-or-discard). The UI drawer sorts by this; agent churn does
   *  not bump it. */
  last_user_action: string;
  /** When the agent (or trigger) last did something — streaming, a terminal
   *  response, an idle, a trigger fire/complete, or asking the user. */
  last_agent_action: string;
  message_count: number;
  /** 'inbox' | 'archived' — stored in thread_summaries.archive_state. */
  section: string;
  active_children_count: number;
  total_children_count: number;
  /** Transitive descendants currently in a state that blocks this thread from
   *  being archived (Running / WaitingForUserAnswer / pending in-workspace
   *  coding-agent changes). `> 0` ⇒ "N sub-threads still busy". */
  blocking_descendant_count: number;
  /** Strict subset of `blocking_descendant_count` that drops the Running case —
   *  descendants needing *user attention* (WaitingForUserAnswer, or pending
   *  changes). Drives REVIEW bubbling up the ancestor chain. */
  attention_descendant_count: number;
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
  /** Coding-agent thread flavor — `'lucidos' | 'app' | 'external'`. Omitted for
   *  non-coding-agent threads (and legacy rows, which consumers default to
   *  `'lucidos'`). */
  coding_agent_kind?: string;
  /** Canonical folder the coding agent operates on — `<ws>/data/apps/<id>/` for
   *  an app thread, the repo root otherwise. Omitted for non-coding-agent threads. */
  coding_agent_folder?: string;
  /** Which backend drives the thread — `'claude-code' | 'codex'`. Omitted for
   *  non-coding-agent threads (legacy rows default to `'claude-code'`). */
  coding_agent?: string;
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
lucidos.ui.watchPreferences(): void
lucidos.ui.navigate(target: NavigateTarget, params?: NavigateParams): Promise<void>
lucidos.ui.startThread(opts?: { prompt?: string }): Promise<void>
lucidos.ui.confirm(options: ConfirmOptions): Promise<boolean>
lucidos.ui.toast(message: string, type?: ToastType, opts?: ToastOptions): void
lucidos.ui.prompt(options: PromptOptions): Promise<string | null>
lucidos.ui.Select.create(opts: SelectCreateOptions): SelectInstance
lucidos.ui.enhanceSelects(root?: ParentNode): SelectInstance[]
```

`applyPreferences()` fetches user preferences and applies theme, font, and scale as CSS variables (resolving a `system` theme to the live OS light/dark). Call once on app load, and style your app with the theme variables (§ Theme variables, under Setup) so it follows the user's appearance — don't hardcode colors. For each setting it prefers the server value, then the value the synchronous `sdk-prefs.js` script already applied from the parent shell's `localStorage`, and only then a default — so a device with no server-scoped value (e.g. only `ui-scale` stored, no `theme`) keeps the user's appearance instead of resetting to dark.

`watchPreferences()` subscribes to live preference changes (SSE `PreferencesChanged`) and re-applies them automatically. Call it once alongside `applyPreferences()` so the app reacts when the user toggles light/dark — or when the OS appearance changes under a `system` preference — without a reload.

`navigate()` sends a navigation request to the Lucidos frontend via SSE. `target`
and `params` (`NavigateParams` = `NavigateUi` minus `target`) are typed against the
generated navigation contract, so valid `target`s and `settings_view`s are
discoverable and type-checked (§ Types, under lucidos.notifications).

### Navigation targets

| Target | Params | Description |
|--------|--------|-------------|
| `thread` | `id` | Focus a specific thread |
| `app` | `id` (or `app_id`) | Open an app UI |
| `settings` | `settings_view` (optional) | Open Settings, optionally a sub-section — `models`, `appearance`, `permissions`, `keyboard-shortcuts`, `devices`, `accounts`, `repositories`, `marketplaces`, or a System subpanel (`system`, `backup`, `memory`, `disk-usage`, `environment-variables`, `thread-queue`). Omit `settings_view` for the Settings home list. |
| `new-chat` | `prompt` (optional) | Open a fresh chat thread, optionally prefilling the compose textarea. Prefer `lucidos.ui.startThread()` — it's the typed wrapper around this target. |
| `plugins` | `id` (optional) | Open the Plugins panel's Installed tab. With `id` (a plugin id), scroll to and pulse-highlight that plugin's row — used by the plugin-update notification so a tap lands on the plugin that has the pending update. |
| `app-store` | — | Open the Plugins panel's Store (marketplace) tab. |
| _other panels_ | — | `files`, `apps`, `triggers`, `thread-queue`, `changes`, `notifications`; plus `file` (`file_path`), `trigger` (`id`), `url` (`url`), `new-app`, `new-trigger`. |

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

### Toasts

`lucidos.ui.toast` shows a transient status banner rendered by the Lucidos shell
(above all app content, themed by the user's preferences). It's **fire-and-forget**
— no return value, no result to await. Use it for success/error feedback instead
of hand-rolling your own banner.

```ts
type ToastType = 'success' | 'info' | 'warning' | 'error';

interface ToastOptions {
  /** Auto-dismiss after this many ms. Omit for the host default: errors and
   *  warnings stay until dismissed; success/info auto-close. */
  durationMs?: number;
  /** false = hide the close (X) button. Default true. */
  dismissable?: boolean;
  /** Stable key for in-place replacement. A later toast with the same key
   *  updates the existing toast (message/type/etc.) instead of stacking a new
   *  one — e.g. an 'Opening…' toast becoming 'Opened'. */
  key?: string;
}
```

`type` defaults to `'info'`; an unknown value degrades to `'info'`. Only this
serializable subset is exposed — the host's toast action buttons take `onClick`
callbacks, which can't cross the app-iframe boundary, so they aren't available
from an app.

**Example:**

```js
lucidos.ui.toast('Saved', 'success');
lucidos.ui.toast('Could not reach the server', 'error');
lucidos.ui.toast('Working on it…', 'info', { durationMs: 2000 });

// Collapse a two-step status into one toast that updates in place:
lucidos.ui.toast('Opening from Drive…', 'info', { key: 'drive-open' });
lucidos.ui.toast('Opened "Q3 deck"', 'success', { key: 'drive-open' });
```

### Prompts

`lucidos.ui.prompt` shows a single-field text-input modal rendered by the Lucidos
shell (themed, above all app content) — the text-input sibling of `confirm`. Use
it instead of `window.prompt()`.

```ts
interface PromptOptions {
  /** Required. The question/instruction shown above the input. Plain text. */
  message: string;
  /** Optional heading rendered above the message. */
  title?: string;
  /** Prefilled input value. */
  defaultValue?: string;
  /** Placeholder shown when the input is empty. */
  placeholder?: string;
  /** OK button label. Default "OK". */
  okLabel?: string;
  /** Cancel button label. Default "Cancel". */
  cancelLabel?: string;
  /** Render a multi-line textarea instead of a single-line input. Default false. */
  multiline?: boolean;
}
```

Resolves the entered string on OK click or Enter; `null` on Cancel, Esc, or
backdrop click. (A `multiline` prompt uses Enter for newlines — submit with the
OK button.) If a second `prompt` is called while one is visible, the previous one
resolves `null` and the new one replaces it.

**Example:**

```js
const name = await lucidos.ui.prompt({
  title: 'Rename board',
  message: 'New name for this board:',
  defaultValue: 'Untitled',
});
if (name === null) return; // user cancelled
// proceed with `name`
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
// In an app iframe the `lucidos` global is already present (sdk.js).
// The host frontend / external embedders import it from the package:
import { lucidos } from '@lucidos/sdk';

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
