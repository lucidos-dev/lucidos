---
name: Lucidos JavaScript SDK, lucidos.triggers: Scheduled Tasks
description: Part of Lucidos JavaScript SDK. Its index lists the sections it covers.
---

<!-- domain-event: EmailReceived: Example domain event emitted by an email-ingestion integration. -->

One part of `system-knowhow/js-sdk`, which lists every part.

## lucidos.triggers: Scheduled Tasks

CRUD operations for cron-based and event-based triggers.

```ts
lucidos.triggers.list(): Promise<Trigger[]>
lucidos.triggers.create(trigger: CreateTrigger): Promise<ApiResult>
lucidos.triggers.update(id: string, trigger: UpdateTrigger): Promise<ApiResult>
lucidos.triggers.delete(id: string): Promise<ApiResult>
lucidos.triggers.run(id: string): Promise<TriggerRunResult>
```

`run` fires an existing trigger **once, right now**, outside its schedule (an
*off-schedule run*). It is a real fire: it records `TriggerExecuted` /
`last_run` and runs under the trigger's own identity, program grants and
`go_to_review` routing, indistinguishable downstream from a scheduled fire. Use it for a "Sync now" button
rather than re-implementing the trigger's work in the app.

It resolves when the run is **admitted**, not when it finishes, so a truthy
`success` does not mean the work is done. Branch on `status`:

| `status` | Meaning |
|---|---|
| `started` | Running now. |
| `queued` | Over capacity; runs when capacity frees. |
| `already-running` | A fire was already active or queued, so **nothing new started**. Never render this as a started run. |

`success: false` means refused, with the reason in `message`: the trigger is
paused, or it has no cron schedule (it is event-only, so emit its subscribed
event with `lucidos.events.emit` instead).

### Types

```ts
type TriggerRun =
  | { type: 'intent'; intent: string }
  | { type: 'script'; path: string };

// One event the trigger listens for, with an optional payload filter scoped
// to that event. A trigger may carry several entries. It fires when an
// incoming event matches *any* entry's event_type AND that entry's
// condition (if set) evaluates true against the payload. Conditions are
// per-entry, so events with different payload shapes never constrain
// each other.
interface EventSubscription {
  event_type: string;
  condition?: Record<string, unknown>;
}

// A historical trigger category. Category grants ended on upgrade.
type SideEffectCategory =
  | 'email'
  | 'external_api'
  | 'cloud_cli'
  | 'out_of_workspace_destruction'
  | 'other';

interface Trigger {
  id: string;
  name: string;
  // Stable kebab-case id, the directory of its knowhow at
  // data/triggers/<slug>/knowhow/.
  slug: string;
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
  // Historical categories. They confer no authority.
  side_effect_grant?: SideEffectCategory[];
  // Effective program scopes the owner approved for this trigger.
  program_grants?: { pattern: string; migrated_invocation: boolean }[];
  /** Folders outside the workspace its subprocesses may write. */
  folder_grants?: { folder: string }[];
  // Whether its runs may start coding-agent threads (triggers.md
  // § "Coding-agent grant"). Always present.
  coding_agent_grant: boolean;
  // MCP server ids its fires use. Only these run unattended in its threads.
  mcp_servers?: string[];
  // Chat model this trigger's intent fires on, and its thinking budget. Both
  // omitted when the trigger follows the account default (Settings → Models →
  // Chat & triggers). Intent triggers only: a script trigger runs no LLM.
  model?: string;
  reasoning_effort?: string;
  // The backend the pinned model runs on. Omitted when the model decides.
  provider?: string;
  // Payload paths naming one thread (system-knowhow/triggers/writing-cron-expressions.md § "Continue key"). Omitted
  // when every fire starts a new thread.
  continue_key?: (string | string[])[];
  // The payload path of the email of the person behind a fire (triggers.md
  // § "Actor"). Omitted when fires credit nobody.
  actor?: string;
}

interface CreateTrigger {
  name: string;
  /** Stable kebab-case id. Omit to derive it from `name`. */
  slug?: string;
  run: TriggerRun;
  cron_expressions: string[];
  on?: EventSubscription[];
  /** Optional *trigger group* id; omit for ungrouped. */
  group_id?: string;
  /** Side-effect grant: irreversible categories this trigger may perform
   *  unattended. Omit / `[]` = none granted (the safe default). Only the
   *  owner grants it: an app frame sending a category is refused. */
  side_effect_grant?: SideEffectCategory[];
  /** Let its runs start coding-agent threads. Only the owner grants it: an
   *  app frame setting `true` is refused. Omit = false. */
  coding_agent_grant?: boolean;
  /** MCP server ids its fires use. A fire refuses a call to any other server. */
  mcp_servers?: string[];
  /** Pin the intent to a chat model and a thinking budget
   *  (`none|low|medium|high|xhigh|max`). Omit either for the account default. */
  model?: string | null;
  reasoning_effort?: string | null;
  /** The backend for the pinned model. Needs `model`, and must name one of its
   *  providers. A pin to an unconfigured backend refuses the fire. */
  provider?: string | null;
  /** A fire whose key matches an open thread continues it. Intent triggers
   *  only (system-knowhow/triggers/writing-cron-expressions.md § "Continue key"). Omit / null / `[]` = none. */
  continue_key?: (string | string[])[] | null;
  /** The payload path of the email of the person behind a fire. Intent
   *  triggers only (system-knowhow/triggers/actor.md § "Actor"). Omit / null = none. */
  actor?: string | null;
}

interface UpdateTrigger {
  name?: string;
  run?: TriggerRun;
  cron_expressions?: string[];
  paused?: boolean;
  // Full replacement for the subscription list. Send the complete new set:
  // there is no partial edit. Pass `[]` to clear all subscriptions.
  on?: EventSubscription[];
  /** Move into a group (string id), clear membership (null), or leave it
   *  unchanged (absent). */
  group_id?: string | null;
  /** Send `[]` to clear historical categories. */
  side_effect_grant?: [];
  /** Revoke program scopes listed on the trigger. */
  revoke_program_grants?: string[];
  /** Folder grants to revoke, as listed on the trigger. Only the owner may:
   *  an app frame is refused. */
  revoke_folder_grants?: string[];
  /** Revoke (false) coding agents. An app frame may never set true. */
  coding_agent_grant?: boolean;
  /** Full replacement for the MCP server ids; pass `[]` to clear them. */
  mcp_servers?: string[];
  /** Pin the intent's model / thinking budget / backend (string), clear it
   *  back to the default (null), or leave it unchanged (absent). Changing the
   *  model drops a backend pin it cannot use. */
  model?: string | null;
  reasoning_effort?: string | null;
  provider?: string | null;
  /** Full replacement for the continue key spec; null or `[]` clears it. */
  continue_key?: (string | string[])[] | null;
  /** Set the actor field path (string), clear it (null), or leave it
   *  unchanged (absent). */
  actor?: string | null;
}

interface ApiResult {
  success: boolean;
  error?: string;
  /** Trigger create / update only: the engine's read-back on the cron it just
   *  stored. Absent on an update that did not touch the schedule. */
  cron_preview?: CronPreview;
}

interface CronPreview {
  /** The next few fire times (RFC3339), merged across the whole expression
   *  array. Empty when the trigger has no cron at all. */
  next_runs: string[];
  /** Non-fatal advice. A cron that can NEVER fire is a hard error instead, so
   *  it arrives as `error` with `success: false`. */
  warnings: string[];
}

// Result of an off-schedule run. `success: true` with
// status: 'already-running' means the request was valid and NOTHING new
// started, because scheduled fires coalesce to at most one pending run per
// trigger. `success: false` means refused (paused, or event-only), and
// `message` says which.
interface TriggerRunResult {
  success: boolean;
  status?: 'started' | 'queued' | 'already-running';
  message: string;
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

Each entry's `condition` applies only to its own `event_type`. The `from: 'partner'` filter on `MessageReceived` does NOT block `EmailReceived` from firing on its own filter.

### Cron validation on create and update

Within one cron expression the fields are **ANDed**. Across the array they are **ORed**. So `0 0 9 1 * Mon` fires only when the 1st IS a Monday (roughly 1.7 times a year). The 1st plus every Monday takes two expressions. See `system-knowhow/triggers/writing-cron-expressions.md` § "Writing cron expressions" for the nth-weekday and last-weekday recipes.

An expression that can **never** fire (`0 0 9 31 2 *`, Feb 31, and its relatives) is rejected: `success: false` with an `error` naming the offending fields. Do not retry it or present it as a transient failure: the expression itself is wrong.

Every accepted create / update returns `cron_preview`. Show `next_runs` in your confirmation so the user sees what they scheduled. Surface each entry of `warnings` (currently the day-of-month/day-of-week AND footgun) rather than dropping it.

Trigger groups are user-visible folders in the triggers panel: pure labels that have no schedule, run no code and don't coordinate firing. An app can pass `group_id` to `create` / `update`. The engine checks the id against the workspace's group registry and rejects unknown values. The SDK has no group CRUD: group management lives behind the engine's HTTP and LLM-tool surfaces.

## lucidos.apps: App Management

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
  /** The app icon: the manifest's `icon` as a path inside the app folder
   *  (`assets/icon.svg`). Omitted unless it meets building-an-app.md
   *  § App icon. */
  icon?: string;
  /** When the host lifts its loading cover, from the manifest's `reveal`.
   *  Always present: `on-load` unless the manifest says `on-ready`. */
  reveal: 'on-load' | 'on-ready';
  /** `widget` for a small answer shown inline in a thread. */
  kind: 'app' | 'widget';
  /** The thread a widget was made in. Omitted for an app. */
  origin_thread_id?: string;
  /** The plugin that ships a widget, instead of a thread. */
  origin_plugin_id?: string;
  /** A reusable widget may be shown in any thread. False for an app. */
  reusable: boolean;
  /** The names a widget takes in its params. Omitted when it declares none. */
  params?: Record<string, { description: string; required?: boolean }>;
  /** A built-in widget: shipped with Lucidos and read-only. Omitted otherwise. */
  built_in?: boolean;
}
```

The shape mirrors the app's `manifest.json`, plus the `id` and, for a built-in widget, `built_in` derived from its folder. The manifest fields are `name` / `description` / `icon` /
`reveal` / `kind` / `origin_thread_id` or `origin_plugin_id` / `reusable` / `params`. `list()` hits `GET /api/v1/apps`, which lists apps only, never a *widget*.
`get(id)` hits `GET /api/v1/app?id=<id>` and answers for a widget too. It throws a `404` `SdkError` for an unknown id.

### Example

```js
const apps = await lucidos.apps.list();
const me = await lucidos.apps.get('habit-tracker');
console.log(me.name, me.icon ?? '(no icon)');
```

## lucidos.params: Widget Params

```ts
lucidos.params(): Record<string, unknown>
```

A *widget*'s params for the place it shows (ADR 0415): the JSON object given to a *widget embed* or a pin. It returns `{}` when the widget was given none, or when the value is not a JSON object.

The host writes the object into the frame URL under the one query key `params`. So a param name never collides with a host name. The widget's `manifest.json` declares the names it takes (`system-knowhow/building-an-app/when-an-app-is-the-right-answer.md` § Widgets). Params are visible in the URL, so never pass a credential. Params over 2 KB are refused.

### Example

```js
const { clip } = lucidos.params();
if (!clip) document.body.textContent = 'No clip given';
```

## lucidos.widget: Where a Widget Shows

```ts
lucidos.widget.size(): 'small' | 'medium' | 'large' | 'xl' | null
lucidos.widget.onSize(callback: (size) => void): () => void
lucidos.widget.place(): 'dashboard' | 'inline' | 'option' | null
lucidos.widget.visible(): boolean
lucidos.widget.onVisibility(callback: (visible: boolean) => void): () => void
```

What the host told a *widget* about the place it shows in (ADR 0432). The SDK also stamps two attributes on `<html>`, so most widgets adapt in CSS alone:

- **`data-widget-place`**: `dashboard` on a *dashboard*, phone or desktop, `inline` in a thread (a reply's embed included), or `option`. Absent in the Canvas, where a widget opens as an app. Before ADR 0496 a dashboard tile read `home`, so a widget that still compares the place to `home` no longer matches.
- **`data-widget-size`**: the *widget size* of its *widget box*, `small`, `medium`, `large` or `xl`, in a thread card and on a dashboard alike. An `xl` box keeps its true width where the host is narrower, and the host scrolls it sideways. Absent in an option and in the Canvas, where `size()` is `null`.

In a thread card and on a dashboard the host sets the box and the frame fills it. Draw for the size you are told, with no scroll inside (§ Sizing a widget). A widget whose manifest declares no sizes takes small, medium and large, and only a declared `xl` adds the fourth (`system-knowhow/building-an-app/when-an-app-is-the-right-answer.md` § Widgets).

`visible()` turns false while a frame stays loaded out of sight, such as a card scrolled off or a hidden dashboard, just before it unloads. Stop timers then, and read again when it turns true.

A right-click anywhere in a widget opens the widget's own menu at the pointer (ADR 0528). A finger held on a dashboard tile opens it too, with a haptic tick, and a dashboard tile draws no ⋯. A finger on a tile turns off text selection and iOS's own long-press menu, links included, until the next press. On a right-click, content keeps the native menu, by the host's own rule:

- a link, an input, a text area, editable content, an image, a video or audio;
- a selection the press landed in;
- the text itself of `pre`, `code` or an element marked `data-native-context-menu`;
- anything, with Alt held.

To keep a right-click for your own menu, call `preventDefault()` in your own `contextmenu` handler. The SDK listens on `window` in the bubble phase, so your handler runs first. A widget showing as a question option forwards nothing, since an option has no widget menu.

The SDK writes an attribute only when the host's value changes. A widget may set `data-widget-size` itself, for a preview, and the SDK leaves it alone until the host names a new size. A `MutationObserver` over `<html>` must write only when a value differs. Even `classList.add` of a class already present queues a fresh record, so an observer that writes on every record loops forever and freezes the page.

### Example

```css
html[data-widget-size="small"] .detail { display: none; }
html[data-widget-size="large"] .chart { height: 60%; }
```

```js
lucidos.widget.onVisibility((visible) => {
  if (visible) render(); else clearInterval(tick);
});
```

