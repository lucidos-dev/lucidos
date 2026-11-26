---
name: Lucidos JavaScript SDK, lucidos.threads: Thread Management
description: Part of Lucidos JavaScript SDK. Its index lists the sections it covers.
---

One part of `system-knowhow/js-sdk`, which lists every part.

## lucidos.threads: Thread Management

```ts
lucidos.threads.list(opts?: ThreadsListOptions): Promise<ThreadSummary[]>
lucidos.threads.count(opts?: Omit<ThreadsListOptions, 'limit'>): Promise<number>
lucidos.threads.liveness(threadId: string): Promise<ThreadLiveness>
lucidos.threads.users  // driver and watchers, see § lucidos.threads.users
```

`list()` calls `GET /api/v1/threads/list` and returns a newest-first array of `ThreadSummary` rows from the projection. `count()` calls `GET /api/v1/threads/count` and resolves to the count under the same filter, cheaper on big workspaces than `(await list()).length`.

`liveness()` calls `GET /api/v1/threads/:thread_id/liveness` and resolves to the thread's *thread liveness*. `list()` rows carry the same object as `liveness`. Read what the thread is doing from `state`. The `status` field is the turn status. `idle` there means no turn is open, and `waiting` is a legacy turn-status value.

```ts
interface ThreadLiveness {
  state: 'working' | 'quiet' | 'waiting_for_user_answer' | 'paused'
       | 'failed' | 'queued' | 'held' | 'stalled' | 'waiting' | 'idle';
  status: string;                  // the turn status
  last_activity_age_secs?: number; // since its newest event
  session?: { ordinal: number; continuity: 'first' | 'fresh' | 'handoff'; age_secs: number };
  handoff?: 'pending';             // only while a successor session is owed
  live_event_waits?: Array<{
    reason: string;
    on: EventSubscription[];
    expires_at: string;
    expires_in_secs: number;       // time left at as_of
    background_timing?: { state: 'queued' } | { state: 'running'; deadline: string };
  }>;
  awaited_children?: Array<{ thread_id: string; title: string }>;
  expected_wake?: { at: string; source: 'agent' | 'decision'; reason: string };
  hold?: { by: 'stop' } | { by: 'open_card'; card_id: string } | { by: 'hold'; reason: string; root_thread_id: string };
  as_of: string;                   // the engine's clock when it looked
}
```

The CLI equivalents are `lucidos threads list` and `lucidos threads count`.
The LLM uses the grouped `threads` tool with `list` or `count`.
Use these for thread metadata without subscribing to the full SSE stream.

**`active` is a union, `status` is precise.** `active: true` selects `running` OR `waiting_for_user_answer`, which are opposites: the workspace working, or waiting on a person. "Is anything busy?" wants `status: 'running'`. "N threads I have something invested in" wants `active: true`. Passing both is a 400.

A *held* thread parked on a question matches neither `status: 'waiting_for_user_answer'` nor `active: true`, and its `liveness.state` reads `held`. Its row's `status` still says `waiting_for_user_answer`, so count what needs the owner with the filter, never from `status`.

### Types

```ts
interface ThreadsListOptions {
  /** The UNION of 'running' and 'waiting_for_user_answer'. true selects it,
   *  false inverts it, omitting it filters nothing. For "is the workspace
   *  busy?" pass status: 'running': a thread awaiting an answer is blocked
   *  on the human. 'waiting' is in neither, and appears only on older rows.
   *  A thread carrying changes to review is 'idle', or the verdict its turn
   *  ended on. Mutually exclusive with status. */
  active?: boolean;
  /** Comma-separated status filter naming exactly the statuses to keep, in the
   *  same spelling each row's `status` field carries: 'idle', 'running',
   *  'waiting', 'waiting_for_user_answer', 'paused', 'failed'. The precise form
   *  of `active`, and mutually exclusive with it. An unrecognized or empty
   *  value is a 400, never a silently empty or unfiltered result. */
  status?: string;
  /** Comma-separated source filter: 'chat', 'trigger', 'coding-agent'.
   *  Legacy 'claude_code' is also accepted. */
  source?: string;
  /** Server clamps to 1..=1000 (default 100). */
  limit?: number;
  /** Thread id. Restrict to that thread's DIRECT children, never its
   *  grandchildren. Same filter as the `--parent` CLI flag and the
   *  `threads` tool's `my_children` on its `list` action, except an app names the thread.
   *  A malformed uuid is a 400, never a silently unfiltered list. */
  parent?: string;
}

/** Why a turn end left a coding agent's work unproposed. */
type UnproposedReason =
  | 'plan_missing'
  | 'plan_changed' // The committed plan changed after approval.
  | 'plan_unreadable' // Lucidos could not verify the plan. Retry the check.
  | 'plan_awaiting_approval'
  | 'plan_only_code'
  | 'outside_bound'
  | 'hardening_missing'
  | 'turn_incomplete'
  | 'awaiting_landing'
  | 'integrated_into_parent'
  | 'carried_by_successor' // Legacy successor threads only.
  | 'hook_refused'; // An event hook refused the proposal.

type CodingAgentChangeState =
  | { kind: 'none' }
  | { kind: 'unproposed'; reason: UnproposedReason | null }
  | { kind: 'proposed'; requires_restart: boolean };

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
   *  change apply-or-discard). The thread drawer sorts by this; agent churn does
   *  not bump it. */
  last_user_action: string;
  /** When the agent (or trigger) last did something: streaming, a terminal
   *  response, an idle, a trigger fire/complete, or asking the user. */
  last_agent_action: string;
  message_count: number;
  /** Whether the user parked this thread in the Saved section (stored in
   *  thread_summaries.is_saved). */
  saved: boolean;
  /** 'inbox' | 'archived', stored in thread_summaries.archive_state. */
  section: string;
  active_children_count: number;
  total_children_count: number;
  /** Transitive descendants currently in a state that blocks this thread from
   *  being archived (Running / WaitingForUserAnswer / pending in-workspace
   *  coding-agent changes). `> 0` ⇒ "N sub-threads still busy". */
  blocking_descendant_count: number;
  /** Strict subset of `blocking_descendant_count` that drops the Running case:
   *  descendants needing *user attention* (WaitingForUserAnswer, or pending
   *  changes). Drives REVIEW bubbling up the ancestor chain. */
  attention_descendant_count: number;
  /** Sub-threads, at any depth, that "Archive completed sub-threads" would
   *  archive now: finished, unblocked, with every descendant clear. */
  archivable_sub_thread_count: number;
  /** Pending changes held by this thread's sub-threads, at any depth, not
   *  counting its own. Present on `lucidos.threads.list` rows only; absent on
   *  every other read, so an absent field never means zero. */
  pending_sub_thread_change_count?: number;
  /** 'idle' | 'running' | 'waiting' | 'paused' | 'failed' | 'waiting_for_user_answer'.
   *  The same values the `status` filter accepts. `running` is the workspace
   *  working; `waiting_for_user_answer` is it waiting on a person (the `active`
   *  union covers both). `paused` = the user's own version switch interrupted
   *  the turn and the engine is resuming it, so nothing is asked of anyone.
   *  Any OTHER interruption (a crash, or a switch whose resume the boot could
   *  not deliver) is `failed` and offers a Continue button. */
  status: string;
  /** What the coding agent's branch holds: its *change state*. `none` means
   *  no work. `unproposed` means work that no pending change carries; a
   *  `reason` says a turn end withheld it, and `null` means none did (still
   *  running, an event wait, an external repo, or set aside). `proposed`
   *  means a pending change the user can apply. */
  coding_agent_change_state: CodingAgentChangeState;
  coding_agent_is_external_repo: boolean;
  last_revived_at: string | null;
  parent_thread_id?: string | null;
  parent_thread_title?: string | null;
  successor?: { thread_id: string; title: string | null };    // legacy successor thread
  successor_of?: { thread_id: string; title: string | null }; // predecessor of a legacy successor thread
  trigger_id?: string | null;
  trigger_name?: string | null;
  cc_repo_id?: string | null;
  cc_repo_name?: string | null;
  /** Coding-agent thread flavor: `'lucidos' | 'app' | 'external'`. Omitted for
   *  non-coding-agent threads (and legacy rows, which consumers default to
   *  `'lucidos'`). */
  coding_agent_kind?: string;
  /** Canonical folder the coding agent operates on: `<ws>/data/apps/<id>/` for
   *  an app thread, the repo root otherwise. Omitted for non-coding-agent threads. */
  coding_agent_folder?: string;
  /** Which backend drives the thread: `'claude-code' | 'codex'`. Omitted for
   *  non-coding-agent threads (legacy rows default to `'claude-code'`). */
  coding_agent?: string;
  /** Compose state machine: `composing` | `active` | `discarded`. The
   *  archive flag is on the separate `section` field; an archived thread
   *  carries `state: 'active'` and `section: 'archived'`. */
  state: 'composing' | 'active' | 'discarded';
  compose_text: string;
  compose_images: string[];
  compose_mode?: 'lucidos' | 'claude_code' | null;
  /** The reader fields. Present on `lucidos.threads.list` rows only, like
   *  `pending_sub_thread_change_count`. `has_draft` says whether the thread
   *  holds an unsent *draft*; `draft_preview` (its first 200 characters) and
   *  `draft_length` (in characters) appear only when it does. `link` is the
   *  *thread link*, `thread:<workspace>/<thread_id>`. */
  has_draft?: boolean;
  draft_preview?: string;
  draft_length?: number;
  link?: string;
  /** The thread's liveness, on `list` rows only. See `liveness()`. */
  liveness?: ThreadLiveness;
}
```

### When to use which

| Want to … | Use |
|---|---|
| Render a list of threads in an app UI | `lucidos.threads.list()` |
| Ask "is the workspace busy?" | `lucidos.threads.count({ status: 'running' })` |
| Show whether one thread is working, quiet or stalled | `lucidos.threads.liveness(id)` |
| Show "N threads need me" | `lucidos.threads.count({ status: 'waiting_for_user_answer' })` |
| Show "N active threads" badge (working AND asking) | `lucidos.threads.count({ active: true })` |
| Render one thread's children (a fan-out board) | `lucidos.threads.list({ parent: id })` |
| React to thread state changes in real time | Subscribe to `lucidos.sse` instead |
| Spawn a new thread from an app | `lucidos.ui.startThread({ prompt })`, plus `coding_agent` and `folder` to preselect a coding target |
| Open a link outside Lucidos from JS | `lucidos.ui.openExternal(url)` (never `window.open`) |

## lucidos.threads.users: Driver and Watchers

In a *team workspace*, a root thread and its sub-threads have at most one *driver* and any number of *watchers* (ADR 0623). Each is a *user*. Roles route pushes and feed the factory app's views. They are never authority: anyone can still answer any gate.

```javascript
const { items, user } = await lucidos.threads.users.list();
// items: open root threads, newest activity first, at most 200:
// [{ root_thread_id, title, last_activity, driver, watchers,
//    gate_open, escalated, needs_team, yours }]
// user: the calling device's user, or null

const users = await lucidos.threads.users.get(threadId);
// { root_thread_id, driver: { id, name } | null, watchers: [{ id, name }] }

await lucidos.threads.users.take(threadId);    // Take, or Take over from the driver
await lucidos.threads.users.release(threadId); // stop driving; a no-op if you do not drive
await lucidos.threads.users.watch(threadId);
await lucidos.threads.users.unwatch(threadId); // from any of your devices
```

- Routes: `GET /api/v1/threads/users`, `GET /api/v1/threads/:thread_id/users`, and `POST` on `.../users/take`, `release`, `watch` and `unwatch`.
- `list` returns root threads that are not archived, the home thread aside. Archiving the root completes its work.
- `gate_open`: a thread in the family waits on a question. `needs_team` is the Team view: an open gate with no driver, or one that escalated. `yours`: your user drives or watches it, so Mine is `items.filter(i => i.yours)`.
- Any thread id names its root thread, through its family root.
- Each change acts as the user holding the calling device and returns the users after it. A repeat that changes nothing records nothing.
- A device that belongs to no user gets a 409 with `reason: "no_user"`. Add it to a user in Settings, Users.
- Answering a gate also makes you the driver. So does a keyed trigger fire whose *actor* email you hold.
- A push about a root thread with a driver reaches the driver's and watchers' devices. With no driver, or once a gate escalated past `driver_timeout_hours`, it reaches the whole team. A gateway with one user always pushes everyone.
- Watch the role events (`RootThreadDriverTaken` and the rest, `thread-events.md`) through `lucidos.sse` to refresh a view. Existing subscriptions using their previous names still receive those events.
- `lucidos.workItems` was the retired name of this namespace. An app copied before the rename must be copied again.
- `lucidos.threads.people` is a deprecated alias of `lucidos.threads.users`, kept for apps written before the user rename (ADR 0655). Its `list()` also names the caller `person`. Write `users` in new code.

## lucidos.modelRequest

Builds a typed model-provider request from a `ModelPicker` value without sending
it. It reads the current registry, resolves the selected route and wire ID, and
uses engine-owned effort settings. Unknown routes and missing protocol metadata
reject; credentials remain inside the proxy.

```js
const req = await lucidos.modelRequest(picker.getValue(), {
  messages: [{ role: 'user', content: 'Explain this result.' }],
  system: 'Be concise',
  stream: false,
});
const response = await lucidos.proxy(req.proxy).fetch(req.path, {
  method: 'POST', headers: req.headers, body: JSON.stringify(req.body),
});
if (!response.ok) throw new Error(await response.text());
const result = await response.json();
```

Options: `messages` (text-only user/assistant messages), optional `system`,
`stream` (default false), and `maxTokens`. The returned object has `proxy`,
`protocol`, `path`, `headers` and a JSON `body`. Null effort preserves the model's
provider default, including models that think by default. Non-null efforts clamp
to the route's offered tiers. `maxTokens` overrides the engine's Anthropic reply
allowance; keep it above any enabled thinking budget. For tools, images or other
provider-specific payloads, extend the returned body or use `lucidos.proxy`
directly with the provider's wire contract.

Responses remain provider-native: `anthropic-messages` (including gateway) and
`vertex-anthropic` return Anthropic messages; `openai-responses` returns Responses;
`openai-chat-completions` returns chat completions (OpenAI, OpenRouter, xAI and
local); `vertex-google` returns Google candidates. Streaming returns each
protocol's event stream; this helper does not parse or normalize replies.
Vertex requests use publisher-model suffixes; never include a project or region.
Builtin support is discovered from metadata, including gateway when installed.

## lucidos.ui: UI Control

```ts
lucidos.ui.applyPreferences(): Promise<void>
lucidos.ui.watchPreferences(): void
lucidos.ui.navigate(target: NavigateTarget, params?: NavigateParams): Promise<void>
lucidos.ui.openExternal(url: string): Promise<void>
lucidos.ui.startThread(opts?: { prompt?: string; coding_agent?: 'claude-code' | 'codex'; folder?: string }): Promise<void>
lucidos.ui.previewFile(params: FilePreviewParams): Promise<void>
lucidos.ui.confirm(options: ConfirmOptions): Promise<boolean>
lucidos.ui.toast(message: string, type?: ToastType, opts?: ToastOptions): void
lucidos.ui.dismissToast(key: string): void
lucidos.ui.ready(): void
lucidos.ui.prompt(options: PromptOptions): Promise<string | null>
lucidos.ui.focusedThread(): Promise<string | null>
lucidos.ui.onFocusedThreadChange(callback: (threadId: string | null) => void): () => void
lucidos.ui.Select.create(opts: SelectCreateOptions): SelectInstance
lucidos.ui.enhanceSelects(root?: ParentNode): SelectInstance[]
lucidos.ui.disableTooltips(): void
```

`applyPreferences()` fetches user preferences and applies the theme mode, theme, font and scale as CSS variables. It resolves a `system` theme mode to the live OS light or dark. Call it once on load, and style with the theme variables (§ Theme variables, under Setup) rather than hardcoded colors. For each setting it prefers the server value, then what `sdk-prefs.js` already put on `<html>`, then a default. So a device with no server-scoped `theme-mode` keeps the user's appearance instead of resetting to dark.

`applyPreferences()` also applies the user's **style overrides**. The
`style_overrides` preference maps CSS custom properties to values. It writes them
onto `<html>` after the theme mode, theme, font and scale, so an override of one
of those wins. Clearing an override uncovers the theme's value for that token.
This keeps an app's chrome matching a host the user has retuned. You need
nothing beyond calling `applyPreferences()`.

Any app can write the map, so it must not inject a declaration or fetch from
another origin. Only custom properties are honoured. A value containing `;`,
`{`, `}`, `<`, `>`, `@`, a backslash, `url(`, `image-set(`, `expression(` or a
comment opener is dropped. An override never sets five kinds of name:

- a `--protected-*` token, which protected surfaces read;
- a `--z-*` stacking token;
- the UI font tokens;
- `--user-ui-scale`, which the UI scale preference sets;
- `--part-screen-background-image` (the screen's scanlines), which the engine
  clamps the protected palette against.

A shadow token such as `--shadow-md` is dropped if it reaches more than 2rem
past its box.

`watchPreferences()` subscribes to SSE `PreferencesChanged` and re-applies changes. It also re-fetches the active theme when its file is written or deleted (`DataFileWritten`, `DataFileEdited`, `DataFileDeleted`) and when a plugin is installed or uninstalled. Call it once beside `applyPreferences()`, and the app reacts with no reload. That covers a light/dark toggle, an OS appearance change under `system`, and a value retuned from the Style Remote. It also covers the Autocorrect switch (§ Text fields and autocorrect, under Setup).

**Inside the host shell, the app repaints with it.** The shell pushes what it painted to every watching app frame, in the same task. So a scale drag, a zoom gesture or a theme switch moves the app with the shell rather than after the save. The push covers theme mode, theme, font, UI scale, style overrides, motion and theme effects. From the first push on it owns the app's appearance, and `PreferencesChanged` re-reads only the external-link target and Autocorrect. A popped-out app tab has no shell, so it follows `PreferencesChanged` as above.

Under `system`, `watchPreferences()` watches the OS appearance two ways. The `prefers-color-scheme` media query covers a flip while the app is on screen. The frame's resume (`visibilitychange`, `focus`, `pageshow`) covers one announced while it was not, the normal case in an installed iOS PWA.

Both sample a moment after the event. A moved theme repaints only if a second read, about a second later, still sees the move. So a brief iOS wake that still reports the app-switcher snapshot's appearance repaints nothing. Only that read, or a shell push carrying the shell's own mode, moves an app already on `system`. A fetch answer keeps the mode on screen. Your app does none of this itself.

`navigate()` sends a navigation request to the Lucidos frontend via SSE. `target`
and `params` (`NavigateParams` = `NavigateUi` minus `target`) are typed against
the generated navigation contract. So valid `target`s and `settings_view`s are
type-checked (§ Types, under lucidos.notifications).

