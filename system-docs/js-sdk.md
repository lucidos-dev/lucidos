---
name: CognOS JavaScript SDK
description: Complete API reference for the cognos JS SDK — functions, types, parameters, and return types for building CognOS apps
---

# CognOS JavaScript SDK

The SDK is available as `cognos` in app UIs. Import from `@anthropic/cognos-sdk` in external projects.

> From a Claude Code subprocess, prefer the `cognos` CLI for `data.*` and `events.*` operations — see [`cognos-cli.md`](./cognos-cli.md).

## Setup

App HTML is served as static content — the engine doesn't inject anything (except `?commit=X` rewriting on historical-version requests). Apps opt into each piece they want.

The standard CognOS app boilerplate:

```html
<!DOCTYPE html>
<html>
  <head>
    <meta charset="utf-8">
    <title>My App</title>
    <link rel="stylesheet" href="/api/v1/sdk-iframe.css">
    <script src="/api/v1/sdk-iframe-audio.js"></script>
    <script src="/api/v1/sdk.js"></script>
    <script>
      cognos.ui.applyPreferences();
      cognos.ui.watchPreferences();
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
| `<link rel="stylesheet" href="/api/v1/sdk-iframe.css">` | Theme tokens (`--bg-primary`, `--accent`, etc.), dark/light variables, default body/input/scrollbar styling | App ships its own complete stylesheet and doesn't want CognOS theming |
| `<script src="/api/v1/sdk-iframe-audio.js"></script>` | Monkey-patches `AudioContext` so app code reuses a gesture-unlocked instance, survives iOS PWA background cycles. **Must be in `<head>` before any code that creates an `AudioContext`.** | App doesn't play audio |
| `<script src="/api/v1/sdk.js"></script>` | The `cognos.*` API. Also installs an iframe-friendly link interceptor: `target="_blank"` links resolve in-frame; external `http(s)://` links route through `cognos.ui.navigate()` | App doesn't use `cognos.*` |
| `cognos.ui.applyPreferences()` | Reads the user's theme/font/scale and sets `data-theme` + CSS vars on `<html>`. Pairs with `sdk-iframe.css` to apply the right palette. | Skip (app keeps default dark palette) |
| `cognos.ui.watchPreferences()` | Re-applies preferences whenever the user changes them (SSE-driven) | Static apps that don't need live preference updates |

Apps using `cognos._capture()` don't need to include `html2canvas` — the SDK loads it on demand from `/api/static/html2canvas.min.js`.

External-host apps point `baseUrl` at the CognOS instance:

```js
cognos.configure({ baseUrl: 'https://your-cognos.example' });
```

## Error Handling

All async methods throw `SdkError` on failure:

```js
class SdkError extends Error {
  httpCode: number;
  reason: string;
}
```

## cognos.data — File Operations

Read, write, and manage files in the workspace `data/` directory.

```ts
cognos.data.read(path: string): Promise<string>
cognos.data.write(path: string, content: string): Promise<WriteResult>
cognos.data.delete(path: string): Promise<void>
cognos.data.list(pattern?: string): Promise<string[]>
cognos.data.url(path: string): string   // synchronous, returns URL
cognos.data.edit(path: string, operations: EditOperation[]): Promise<void>
cognos.data.upload(file: File): Promise<UploadResult>  // 120s timeout
```

### Types

```ts
interface WriteResult { success: boolean; commit?: string }
interface UploadResult { success: boolean; filename?: string; error?: string }
interface EditOperation {
  json_path?: string;   // JSON path edit
  json_value?: unknown;
  find?: string;        // Text find-replace edit
  replace?: string;
}
```

### Examples

```js
// Read a JSON file
const raw = await cognos.data.read('artifacts/habits/data.json');
const data = JSON.parse(raw);

// Write content
await cognos.data.write('artifacts/notes.md', '# My Notes\nContent here.');

// List files matching a pattern
const csvFiles = await cognos.data.list('artifacts/imported/**/*.csv');

// Edit JSON in-place
await cognos.data.edit('artifacts/habits/data.json', [
  { json_path: '$.streak', json_value: 5 }
]);

// Get a URL for embedding in HTML
const src = cognos.data.url('artifacts/screenshots/latest.png');
```

## cognos.events — Event Store

Emit and query domain events.

```ts
cognos.events.emit(type: string, payload: Record<string, unknown>, options?: EmitOptions): Promise<void>
cognos.events.query(params?: EventQuery): Promise<CognosEvent[]>
```

### Types

```ts
interface EventQuery {
  event_type?: string;
  since?: string;    // ISO 8601
  until?: string;    // ISO 8601
  limit?: number;    // default 100
}

interface CognosEvent {
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
await cognos.events.emit('HabitCompleted', {
  summary: 'Completed meditation',
  habit: 'meditation',
  streak: 5
});

// Emit a transient coordination signal — reaches SSE consumers but
// is not written to the event store. Use for heartbeats and ephemeral
// state broadcasts (e.g. presenter↔remote view sync).
await cognos.events.emit('SlidePresenterState', {
  slide_index: 3,
  is_paused: false,
}, { transient: true });

// Query recent events
const events = await cognos.events.query({
  event_type: 'HabitCompleted',
  since: '2026-04-01T00:00:00Z',
  limit: 50
});
```

## cognos.triggers — Scheduled Tasks

CRUD operations for cron-based and event-based triggers.

```ts
cognos.triggers.list(): Promise<Trigger[]>
cognos.triggers.create(trigger: CreateTrigger): Promise<Trigger>
cognos.triggers.update(id: string, trigger: UpdateTrigger): Promise<Trigger>
cognos.triggers.delete(id: string): Promise<void>
```

### Types

```ts
interface TriggerRun {
  type: 'prompt' | 'script';
  value: string;
  model?: string;
}

interface Trigger {
  id: string;
  name: string;
  run: TriggerRun;
  cron_expressions: string[];
  enabled: boolean;
  on_event?: string;
  condition?: Record<string, unknown>;
  created_at: string;
  last_run_at?: string;
  next_run_at?: string;
}

interface CreateTrigger {
  name: string;
  run: TriggerRun;
  cron_expressions: string[];
  on_event?: string;
  condition?: Record<string, unknown>;
}

interface UpdateTrigger {
  name?: string;
  run?: TriggerRun;
  cron_expressions?: string[];
  enabled?: boolean;
  on_event?: string | null;
  condition?: Record<string, unknown> | null;
}
```

## cognos.apps — App Management

```ts
cognos.apps.list(): Promise<App[]>
cognos.apps.get(id: string): Promise<App>
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

## cognos.preferences — User Settings

```ts
cognos.preferences.get(deviceId?: string | null): Promise<Preferences>
cognos.preferences.set(key: string, value: string, deviceId?: string): Promise<void>
```

`get()` defaults to the parent device id (read from the shared `cognos-device-id`
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
| `ui-scale` | `100`, `113`, `125` (or `small`, `medium`, `large`) | Scale |

## cognos.notifications — Notification Center

```ts
cognos.notifications.list(params?: {
  limit?: number;
  before?: number;
  filter?: string;
}): Promise<NotificationListResult>

cognos.notifications.markRead(id: string): Promise<void>
cognos.notifications.markAllRead(): Promise<void>
```

### Types

```ts
interface Notification {
  id: string;
  task_id?: string;
  app_id?: string;
  title: string;
  message: string;
  created_at: string;
  read: boolean;
}

interface NotificationListResult {
  notifications: Notification[];
  unread_count: number;
  has_more: boolean;
}
```

## cognos.threads — Thread Management

```ts
cognos.threads.list(): Promise<Thread[]>
cognos.threads.search(query: string): Promise<Thread[]>
```

### Types

```ts
interface Thread {
  id: string;
  title: string;
  source: string;
  last_activity: string;
  message_count: number;
  is_pinned: boolean;
  has_response: boolean;
}
```

## cognos.ui — UI Control

```ts
cognos.ui.applyPreferences(): Promise<void>
cognos.ui.navigate(target: string, params?: Record<string, string>): Promise<void>
```

`applyPreferences()` fetches user preferences and applies theme, font, and scale as CSS variables. Call once on app load.

`navigate()` sends a navigation request to the CognOS frontend via SSE.

### Navigation targets

| Target | Params | Description |
|--------|--------|-------------|
| `thread` | `id` | Focus a specific thread |
| `app` | `id` | Open an app UI |

## cognos.sse — Real-time Events

Subscribe to server-sent events for live updates.

```ts
cognos.sse.connect(): void
cognos.sse.disconnect(): void
cognos.sse.on(eventType: string, callback: (data: unknown, raw: SseEvent) => void): () => void
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
cognos.sse.connect();

// Listen for navigation requests
const unsub = cognos.sse.on('NavigationRequested', (data) => {
  console.log('Navigate to:', data);
});

// Listen for notifications
cognos.sse.on('NotificationCreated', (data) => {
  showToast(data.title);
});

// Wildcard — all events
cognos.sse.on('*', (raw) => {
  console.log('Event:', raw);
});

// Cleanup
unsub();
cognos.sse.disconnect();
```

## cognos.utils — Utilities

```ts
cognos.utils.timeAgo(iso: string): string      // "5m ago", "2d ago", "just now"
cognos.utils.escapeHtml(str: string): string    // HTML-escape
cognos.utils.formatDate(iso: string): string    // Locale-formatted date string
```

## App UI Pattern

Standard app initialization:

```js
import { cognos } from '@anthropic/cognos-sdk';

// Apply user theme/font/scale
await cognos.ui.applyPreferences();

// Connect SSE for live updates
cognos.sse.connect();

// Load data
const raw = await cognos.data.read('artifacts/my-app/data.json');
const state = JSON.parse(raw);

// Listen for relevant events
cognos.sse.on('MyAppDataUpdated', (data) => {
  // Re-render with new data
});
```
