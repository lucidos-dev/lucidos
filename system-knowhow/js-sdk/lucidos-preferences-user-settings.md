---
name: Lucidos JavaScript SDK, lucidos.preferences: User Settings
description: Part of Lucidos JavaScript SDK. Its index lists the sections it covers.
---

One part of `system-knowhow/js-sdk`, which lists every part.

## lucidos.preferences: User Settings

Only a person can set `landing_setting`, in Settings → Coding Agents.

```ts
lucidos.preferences.get(deviceId?: string | null): Promise<Preferences>
lucidos.preferences.set(key: string, value: string, deviceId?: string): Promise<void>
```

`get()` defaults to the device the app runs on, so it sees the same merged view
as the shell. Theme, font, scale and `agent-icon-colors` are device-scoped.
`agent-icon-colors` colors the agent icons in the thread drawer, the thread pane
and navigation history: `brand` (the default), `theme` or `muted`.
Status indicators stay unchanged.

A read naming no
device gets only the global rows. The device id is per-workspace, and an app
never handles it. An app frame names its device, and the host substitutes the
id. A standalone app tab reads the id itself.

A popped-out tab follows the shell it left, named by the `?device=` in its URL,
so it keeps that shell's theme. Otherwise the tab reads the workspace-scoped
`ws:<slug>:lucidos-device-id`. Pass `null` to fetch only global preferences.

A remote workspace's popout is a shell tab with no `?device=` (ADR 0726). It
uses the browser's own device for that workspace. From the desktop client, that
can differ from the shell it left.

`set()` refuses any key the Lucidos Agent may not write, such as
`command_guard`, `max_tool_calls`, `network_bind`, `local_base_url` or
`model_permission_answer`. Those are security settings the user changes in
Settings. It also refuses the keys
that choose what a coding-agent session spawns (`coding_agent_claude_path`,
`coding_agent_codex_path`, `coding_agent_claude_permission_mode`), and
`opencode_free_enabled`, which sends chat to an anonymous third-party relay.

Engine bookkeeping, such as the last backup's outcome in `backup_last_run`, is
not a setting: `get()` leaves it out, and `set()` refuses it.

Every refusal rejects the promise with the engine's reason, including a value
the engine will not store, such as a malformed timezone.

### Types

```ts
type Preferences = Record<string, string>;
```

### Common keys

The generated preference catalog takes its fallbacks from the engine's preference catalog.
Its `model_codex` fallback is `gpt-6.2-sol`; a stored model choice still wins.
See `preferences.md` for coding-agent model and effort defaults.

| Key | Values | Description |
|-----|--------|-------------|
| `theme-mode` | `dark`, `light`, `system` | Light or dark. `system` (the default) follows the OS. Read the resolved value as `data-theme-mode` on `<html>` |
| `font-family` | `theme`, `system`, `geist`, `atkinson-hyperlegible-next`, `inter`, `roboto`, `open-sans`, `manrope`, `source-serif-4`, `lora`, `literata`, `fira-code`, `monospace`, `geist-mono`, `atkinson-hyperlegible-mono`, `jetbrains-mono`, `ibm-plex-mono`, `source-code-pro`, `commit-mono`, `cascadia-code`, `vt323` | Font. `theme` (the default) follows the active theme's suggested font, else `fira-code`; `GET /api/v1/fonts` lists the fonts, each with its `group` (`sans`, `serif`, `mono`). `applyPreferences()` resolves it for you into `--font-ui` and loads the font from the local engine. `fira-code`, `jetbrains-mono` and `cascadia-code` also enable programming ligatures, on code and `pre` blocks only, via `--font-features-text` / `--font-features-code` |
| `ui-scale` | Number in 12.5% steps from 75 to 200 (`75`, `87.5`, `100`, `112.5`, `125`, `137.5`, `150`, `162.5`, `175`, `187.5`, `200`); or the legacy strings `small` / `medium` / `large` (= `100` / `112.5` / `125`). Off-grid numbers snap to the nearest valid step. | Scale |
| `autocorrect` | `true`, `false` | Whether text fields autocorrect on this device. Unset, on everywhere. `sdk.js` applies it to your fields (§ Text fields and autocorrect, under Setup) |
| `motion` | `system`, `reduce`, `full` | Whether this device reduces motion. `system` (the default) follows the OS. Read it as `data-motion` on `<html>` (§ Reduced motion, under Setup) |
| `theme-effects` | `system`, `reduce`, `full` | Whether this device shows a theme's part shadows, filters and scanlines. `system` (the default) drops them when the OS asks for more contrast or less transparency. Read it as `data-theme-effects` on `<html>` (§ Theme parts, under Setup) |

## lucidos.priceTable: Model Prices

```ts
lucidos.priceTable.get(): Promise<PriceTable>
```

Reads the *price table* (`GET /api/v1/model-prices`): the model prices Lucidos
ships, merged card by card with the workspace override in
`data/config/model-prices.json`. Each model holds a list of dated cards, oldest
first. A card is in force from its `from` day until the next card's. To price
a past day, take the newest card whose `from` is on or before it. Each card
says where it came from in `origin`.

An invalid override is ignored whole, and `override.error` says why. To change
a price, write the override with `lucidos.data`. The engine re-reads it on
change.

### Types

```ts
interface PriceRates {
  uncached_in: number;       // per million tokens, in `currency`
  cache_write: number;       // the 5-minute cache write
  cache_write_1h?: number;   // omitted means cache_write
  cache_read: number;
  out: number;
}

interface PriceCard extends PriceRates {
  from?: string;             // YYYY-MM-DD, local time; omitted = since the beginning
  long?: PriceRates & { threshold_tokens: number; inclusive?: boolean };
  flex?: PriceRates;
  per_minute?: number;       // billed by the minute; token rates unused
  source: string;
  origin: 'shipped' | 'override';
}

interface PriceTable {
  currency: string;
  checked: string;           // YYYY-MM-DD the prices in force were last checked
  unpriced_cache_shares: { read: number; write_5m: number; write_1h: number };
  models: Record<string, PriceCard[]>;
  override: { path: string; present: boolean; checked: string | null; error: string | null };
}
```

The override file holds `currency` (optional, must match), `checked`
(optional, YYYY-MM-DD) and `models`, in the same card format without
`origin`. A card replaces the shipped card from the same day and joins the
list otherwise.

### Example

```js
const table = await lucidos.priceTable.get();
const cards = table.models['claude-sonnet-5-5'] ?? [];
const today = new Date().toLocaleDateString('sv'); // YYYY-MM-DD
const card = cards.filter(c => !c.from || c.from <= today).at(-1);
```

## lucidos.storage: Per-Device App State

```ts
lucidos.storage.local: AppStore    // kept across reloads, on this device
lucidos.storage.session: AppStore  // kept until the Lucidos tab closes
lucidos.storage.ready: Promise<void>
lucidos.storage.onError(handler: (failure: StorageFailure) => void): () => void
lucidos.storage.quota: number      // per app, per area: characters of key plus value
lucidos.storage.valueMax: number   // the largest single value, in characters

interface AppStore {               // the `Storage` interface
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
  clear(): void;
  key(index: number): string | null;
  readonly length: number;
}

interface StorageFailure {
  op: 'set' | 'remove' | 'clear';
  area: 'local' | 'session';
  key: string | null;              // null for clear()
  message: string;
}
```

An app frame has an opaque origin, so its own `localStorage`, `sessionStorage`,
IndexedDB and cookies throw. These two stores take their place, with the same
calls. The values live in the Lucidos shell's own browser storage, so they stay
on this device, like `localStorage`. Each app sees only its own keys: the host
decides which app a frame is, and nothing the app sends can name another.

**Await `ready` before the first read.** The frame cannot read the shell's
storage directly, so the SDK fetches this app's values once at load. Reads are
synchronous after that. A read before `ready` resolves sees nothing stored, and
the console says so once. `ready` resolves at once in a standalone app tab.

```js
await lucidos.storage.ready;
const saved = JSON.parse(lucidos.storage.local.getItem('state') || '{}');

function save(state) {
  lucidos.storage.local.setItem('state', JSON.stringify(state));
}
```

Moving off `localStorage` is a rename plus that one `await`. There is no
`window.localStorage` shim: an unchanged app reads before `ready`, so a shim
would still forget its state.

**Limits.** Each app may keep `quota` characters per area, and one value may be
`valueMax` characters long. A write past either throws a `QuotaExceededError`
`DOMException` at once, as `localStorage` does, and stores nothing.

**A write the host refuses later** rolls back in the frame and reaches every
`onError` handler. With no handler, the SDK logs it to the console.
The host toast names the app and gives recovery steps for full or blocked
browser storage. Both storage areas share browser capacity with other apps
and shell state in the same area.

**When a store fills**, remove only known keys the app can rebuild. Keep
preferences and drafts. A full store can reject a small write from any app,
so the app named in the toast did not necessarily fill it.

**What it is not:**

- Not shared across devices or browsers. Use `lucidos.data` for that.
- Not synced between two open frames of one app, and no `storage` event fires.
- Not for secrets such as tokens. See § `lucidos.oauth`.
- Cleared when the app is deleted, but only on a device with Lucidos open at
  that moment. Another device keeps its copy, and an app later installed there
  under the same id sees it.

## lucidos.notifications: Notification Center

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
`navigate_ui` tool**. The source is the `NAVIGATE_TARGETS` / `NAVIGABLE_SETTINGS_VIEWS` consts in
`crates/lucidos-core/src/llm/tools/misc.rs`, so the SDK and the LLM tool
schema cannot drift. To change the set, edit those Rust consts.

```ts
type NavigateTarget =
  | 'dashboard' | 'files' | 'apps' | 'app-store' | 'plugins' | 'triggers' | 'thread-queue' | 'changes' | 'notifications'
  | 'settings' | 'app' | 'file' | 'trigger' | 'thread'
  | 'new-app' | 'new-trigger' | 'new-chat' | 'url';

// Settings sub-section for `target: 'settings'`. No category is platform-gated,
// so none is withheld from a caller with no platform signal. `system` opens the
// list of System pages; `system-overview` is the one holding versions. Three pages
// are absent because nothing has asked to link them: `webhooks`, `event-hooks`
// and `communication-surfaces`.
type SettingsViewTarget =
  | 'providers' | 'models' | 'intelligence' | 'permissions' | 'mcp' | 'coding-agents' | 'accounts' | 'locale' | 'marketplaces'
  | 'access' | 'devices' | 'users' | 'system' | 'appearance' | 'keyboard-shortcuts'
  | 'system-overview' | 'release-notices' | 'whats-new'
  | 'thread-queue' | 'backup' | 'memory' | 'disk-usage' | 'environment-variables' | 'debugging';

interface NavigateUi {
  target: NavigateTarget;
  settings_view?: SettingsViewTarget;
  app_id?: string;
  /** The place INSIDE the target. With `target: 'app'` it is the app iframe's
   *  `location.hash`. With `target: 'settings'` it names a row on the page,
   *  such as `'xai'` on `'providers'`. See § Navigation targets. */
  fragment?: string;
  file_path?: string;
  /** 1-based line to open `file_path` at, and the inclusive last line of the
   *  range. See § Navigation targets for the degradation rules. */
  line?: number;
  line_end?: number;
  id?: string;
  url?: string;
  event_id?: string;
  /** Only with `target: 'new-chat'`: the compose text to prefill. */
  prompt?: string;
  /** Only with `target: 'new-chat'`: the coding agent backend to preselect. */
  coding_agent?: 'claude-code' | 'codex';
  /** Only with `target: 'new-chat'`: the coding target to preselect. A registered
   *  repository (name or id) or `data/apps/<id>`. */
  folder?: string;
}

/** What a notification tap does. `modal` (default) opens the inbox detail
 *  showing the message body. `navigate` delegates to the same router the
 *  `navigate_ui` LLM tool uses; `to` is its arg shape. Both mark the source
 *  notification read on tap. Every notification is openable: the old passive
 *  `none` kind is retired, and a historical `{kind:'none'}` is coerced to
 *  `modal`. The field was once a bare string (`'modal'`, `'open_app'`,
 *  `'open_thread'`, `'none'`), and the engine now answers those with 400. To
 *  sweep a workspace for leftovers, run `system-knowhow/workspace-audit.md`. */
type Tap =
  | { kind: 'modal' }
  // Engine restore notifications: requests admission and confirmation for
  // the recovery snapshot. Receiving or opening it never executes a restore.
  // Only the engine writes it: a sent notification with it is refused.
  | { kind: 'restore-snapshot'; workspace_id: string; handle: string }
  // Engine config change notifications on a team workspace: asks to confirm,
  // then undoes that change. Only the engine writes it, like restore-snapshot.
  | { kind: 'undo-config-change'; event_id: string }
  | { kind: 'navigate'; to: NavigateUi };

interface Notification {
  id: string;
  task_id?: string;
  app_id?: string;
  /** Originating thread, when the notification has one. Drives the inbox
   *  modal's "Open thread" button. */
  thread_id?: string;
  /** Specific event UUID inside `thread_id` that raised this notification.
   *  The §4 in-app matrix uses it to silently mark-read when the user is
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

### Tap shapes: examples

The SDK exposes `list` / `markRead` / `markAllRead` for reading the inbox. Create a notification with `lucidos.request('/notifications', …)`. Each `body` below is the wire shape the `lucidos notify` CLI and the `send_notification` LLM tool also take. The examples run from an app frame or a standalone app tab.

```js
// Default: open the inbox detail showing the message body. Use this for any
// info-only notification too ("OAuth completed", "Build succeeded"): every
// notification is openable, and there is no passive kind. For ephemeral
// status that should NOT land in the inbox at all, use a plain `showToast`.
await lucidos.request('/notifications', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    title: 'Daily summary',
    message: 'Here is your summary…',
    tap: { kind: 'modal' },
  }),
});

// Navigate to a panel.
await lucidos.request('/notifications', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    title: '5 changes ready to apply',
    message: 'Review the Changes panel.',
    tap: { kind: 'navigate', to: { target: 'changes' } },
  }),
});

// Navigate to a thread, optionally scroll-and-pulse a specific event row.
// Both ids must be uuids, or the engine answers 400. An unresolvable tap is a
// dead deep link the reader meets as `Thread "<id>" no longer exists`. The
// agent-only `current` alias is refused too, since an app has no current
// thread. Take the ids from whatever you are notifying about. The event must
// live in that thread: a domain event, or one from another thread, is a 400.
const threadId = '4f1c2e8a-9d3b-4c17-8a55-0b6e2f7d1c93';
const eventId = 'b7e04a12-5f6c-4d29-9e31-8c2a6d4b70f5';
await lucidos.request('/notifications', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    title: 'Coding agent is asking',
    message: 'Permission needed.',
    thread_id: threadId,
    event_id: eventId,
    tap: { kind: 'navigate', to: { target: 'thread', id: threadId, event_id: eventId } },
  }),
});

// Navigate to an app's UI, at the one place the notification is about.
// `fragment` arrives as the app's location.hash (§ Navigation targets).
await lucidos.request('/notifications', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    title: 'Habit tracker reminder',
    message: 'Tap to log today.',
    app_id: 'habit-tracker',
    tap: {
      kind: 'navigate',
      to: { target: 'app', app_id: 'habit-tracker', fragment: 'today' },
    },
  }),
});
```

From scripts (Python/bash), use the `lucidos notify` CLI, which builds the same body. From LLM threads, use the `send_notification` tool.

