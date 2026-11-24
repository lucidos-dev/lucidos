---
name: Lucidos JavaScript SDK, lucidos.sse: Real-time Events
description: Part of Lucidos JavaScript SDK. Its index lists the sections it covers.
---

One part of `system-knowhow/js-sdk`, which lists every part.

## lucidos.sse: Real-time Events

Subscribe to server-sent events for live updates.

Direct SDK clients use the stream's workspace restore generation for HTTP
provenance. Start standalone data loading inside `lucidos.sse.onOpen(load)`;
this connects the stream and waits for its generation hello before calling
`load`, on first load and reconnect. `connect()` alone does not wait for hello.

A request begun
before that generation was observed keeps its original provenance; it is never
upgraded or retried automatically. If a connected page observes a different
generation, reload it and review intent before submitting again. Responses
whose headers or body arrive across that change are rejected. Isolated app
frames use the host's restore admission instead.

Starting a direct HTTP call before the first stream hello binds the page to
the original generation. A later restored hello closes that page's HTTP
transport too; it cannot authorize old page state for a subsequent call.

```ts
lucidos.sse.connect(): void
lucidos.sse.disconnect(): void
lucidos.sse.on(eventType: string, callback: (data: unknown, raw: SseEvent) => void): () => void
lucidos.sse.onOpen(callback: () => void): () => void
```

`on()` returns an unsubscribe function. Subscribe by inner event name: the SDK unwraps the wire format.

### Hearing the stream open: `onOpen`

**The stream does not replay what landed while it was down.** A network drop, an engine restart or a phone that slept all leave a gap, and nothing tells an `on()` listener what it missed. `onOpen(callback)` is where an app catches up: read the data again there.

- **It runs on the first open and on every reopen.** One `render` passed to it covers first load and every reconnect.
- **If the stream is already open, it runs once at once** (on the next microtask). The SDK may have opened the stream before your code ran, and you still get your first call.
- **So do not also call `render()` on load.** That reads everything twice at startup. Reading on the open also means the stream is live before the first read, so no change slips between the two.
- **It connects the stream.** No separate `connect()` call is needed.
- **It is a method of its own, not an `on()` type,** so no event can fire it, whatever its name.
- **It returns an unsubscribe function.** A callback unsubscribed before its first call never runs.

It works the same in an app frame, a widget, and an app in its own tab. The host relays each open of its own stream to the frame.

```js
lucidos.sse.onOpen(render);
lucidos.sse.on('ArtifactUpdated', (data) => {
  if (data.artifact_path === 'habits/data.json') render();
});
```

The full pattern, with the file events, the resume listeners and your own writes, is `system-knowhow/building-an-app/visual-quality-bar-make-it-look-sleek-an.md` § Live data.

### One stream per workspace

`connect()` is idempotent, and one connection fans out to every `on()` listener in your app.

The connection is also shared **across documents**. An app frame has an opaque origin and can open neither an `EventSource` nor a `SharedWorker` port. So the host relays every frame off the connection it already holds. A document that is not a frame (the Lucidos shell, or an app in its own tab) attaches to the `SharedWorker` holder directly. Opening more apps opens no more connections.

There is nothing to configure. For an app author:

- **A frame is identical either way.** A relayed frame carries the same payload a private connection would, so your handler does not change.
- **`disconnect()` detaches this document only.** It never takes the stream from another app or from the shell.
- **A frame hears only the types it listens to.** The SDK tells the host which types have an `on()` listener, and the host relays only those. A streaming reply is thousands of frames, so this keeps a page of live widgets cheap. An `on('*')` listener takes every frame, and `on('ThreadEvent')` every thread event.

Where `SharedWorker` is missing (Chromium on Android, and Android WebView), a document that is not an app frame opens a private `EventSource`. Same events, same order, one connection per document.

### A quiet stream is reopened

The engine sends a `keepalive` event every 30 seconds, and the SDK reopens a stream that hears nothing for much longer. A half-open connection, such as one a phone left behind when it slept, never errors, so silence is the only sign. The reopen is an ordinary drop and open, so `onOpen` runs and your app catches up as above.

The keepalive is a named SSE event. It reaches no `on()` listener and no `onmessage` handler, so an app never sees it.

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

// Wildcard: all events
lucidos.sse.on('*', (raw) => {
  console.log('Event:', raw);
});

// Cleanup
unsub();
lucidos.sse.disconnect();
```

## lucidos.utils: Utilities

```ts
lucidos.utils.timeAgo(iso: string): string      // "5m ago", "2d ago", "just now"
lucidos.utils.escapeHtml(str: string): string        // Escape for text position only
lucidos.utils.escapeHtmlAttr(str: string): string    // Escape for an attribute value
lucidos.utils.formatDate(iso: string): string        // Locale-formatted date string
```

### Escaping: which helper goes where

The two escapers are separate on purpose. Pick by where the value lands in the markup:

| The value lands… | Use | Example |
|---|---|---|
| Between tags, as text | `escapeHtml` | `` `<td>${lucidos.utils.escapeHtml(name)}</td>` `` |
| Inside a quoted attribute value | `escapeHtmlAttr` | `` `<a title="${lucidos.utils.escapeHtmlAttr(name)}">` `` |
| In an unquoted attribute, a `<script>`, a `style`, or an `on*` handler | Neither | Set it from code instead |

**Never use `escapeHtml` inside an attribute.** It escapes `&`, `<` and `>` but leaves `"` and `'` raw. A value carrying a quote closes the attribute and adds its own, such as `onmouseover`. That script runs with your app's full authority: data writes, proxy calls and OAuth tokens. An app rendering third-party content, such as a feed or a mailbox, is exposed.

`escapeHtmlAttr` escapes all five characters, so it is also safe in text position. Quote the attribute either way: no escaper makes an unquoted value safe.

**A URL attribute needs more than escaping.** `escapeHtmlAttr` keeps a value inside `href` or `src`, but a `javascript:` URL still runs there. Check the scheme first, or set the link from code.

**Building from code needs neither.** `el.textContent = value` and `el.setAttribute('title', value)` never parse the value as markup.

## App UI Pattern

Standard app initialization:

```js
// In an app iframe the `lucidos` global is already present (sdk.js).
// The host frontend / external embedders import it from the package:
import { lucidos } from '@lucidos/sdk';

// Apply user theme/font/scale
await lucidos.ui.applyPreferences();

const PATH = 'artifacts/my-app/data.json';
const ARTIFACT = 'my-app/data.json';   // artifact_path is relative to artifacts/
let shown = null;

// One render for every place the data can change. It updates in place.
async function render() {
  let text;
  try {
    text = await lucidos.data.read(PATH);
  } catch (err) {
    if (err.httpCode !== 404) throw err;
    text = '';   // deleted, or not written yet: draw the empty state
  }
  if (text === shown) return;   // the echo of your own write, or nothing new
  shown = text;
  const state = text ? JSON.parse(text) : null;
  // Update the existing elements, keeping scroll position and selection.
}

// The first load, and every reconnect. Do not also call render() on load.
lucidos.sse.onOpen(render);

// Whoever writes the file: the agent, a trigger, a script, another device.
for (const type of ['ArtifactCreated', 'ArtifactUpdated', 'ArtifactDeleted']) {
  lucidos.sse.on(type, (data) => {
    if (data.artifact_path === ARTIFACT) render();
  });
}

// A resumed phone PWA whose stream stalled while it slept.
document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'visible') render();
});
addEventListener('pageshow', (event) => {
  if (event.persisted) render();
});
```

The full pattern is `system-knowhow/building-an-app/visual-quality-bar-make-it-look-sleek-an.md` § Live data. It covers the events per path, the writes that announce nothing, a deleted file, and your own writes.

## Diagnostic evidence

Host theme breadcrumbs classify known category and code values separately from private message and data fields.
App frames cannot read diagnostic inbox contents; that query requires the receiving workspace's own agent token.

Follow [Agent Sandbox](../agent-sandbox.md) for bounded diagnostic access and the raw-host-log restriction.
