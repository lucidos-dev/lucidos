---
name: Lucidos JavaScript SDK, lucidos.ui: UI Control (continued)
description: Part of Lucidos JavaScript SDK. Its index lists the sections it covers.
---

One part of `system-knowhow/js-sdk`, which lists every part.

## lucidos.ui: UI Control (continued)

### Showing the app once its content is ready

While your app opens, the host covers it with the theme background. After a
short delay it also runs a thin progress bar along the top of the pane. By
default the cover lifts on your page's `load` event. An app that fetches its
data after `load` then shows an empty screen until the data arrives.

To keep the cover up until your content is drawn, opt in from `manifest.json`
and call `lucidos.ui.ready()` once the first data has rendered:

```json
{ "name": "Habit Tracker", "description": "Daily habits", "reveal": "on-ready" }
```

```js
const habits = await lucidos.data.read('artifacts/habit-tracker/habits.json');
renderHabits(JSON.parse(habits));
lucidos.ui.ready();
```

| `reveal` | The cover lifts | Fuse if the signal never comes |
|---|---|---|
| `on-load` (default) | on the page's `load` event | 3 s |
| `on-ready` | when the app calls `lucidos.ui.ready()` | 15 s |

Four rules:

- **Opt in only if you call it.** An `on-ready` app that never calls `ready()`
  sits behind the cover for the full 15 s on every open.
- **Call it on every path that finishes the first render**, including the
  empty state and the error state. A `catch` that shows an error must call it
  too, or the user waits 15 s to see the error.
- **The call is harmless anywhere else.** A repeated call does nothing more,
  and neither does a call from an `on-load` app. In its own browser tab the
  app has no host, so nothing is sent.
- **The manifest decides, not the call.** The host reads `reveal` before your
  page loads, so the cover can wait for a call that has not happened yet. A
  value other than `on-load` or `on-ready` opens the app as `on-load`.

The progress bar needs nothing from you. It also keeps running when the 3 s
fuse lifts the cover before `load`, so a slow page still says it is loading.

### Sizing a widget

A *widget* draws into a *widget box*: a fixed box of its *widget size*. Its
thread card and its dashboard tile draw the same box at the same size, so the
widget looks the same in both. The full contract is
`system-knowhow/building-an-app/when-an-app-is-the-right-answer.md` § Widgets, "The widget box".

- **Read the size, never measure the page.** Style under
  `html[data-widget-size="small"]` and its siblings, or read
  `lucidos.widget.size()`. The viewport is the box, so `100%` and `100vh`
  mean the box.
- **Fit by showing less, never by shrinking text.** Keep type on the theme
  tokens, so the UI scale reaches it.
- **Never scroll inside.** Content past the box's bottom or right edge is
  clipped under a fade, and ⋯ › Open in Canvas shows the widget full size.
- **An option has no size.** There the frame takes the height the SDK
  reports for `document.body`, so never size the body to the viewport in an
  option.
- **Load the SDK.** It stamps the size, and the host reads the content
  height and width it reports to decide each fade.
- **The SDK also measures for the *widget fit check*.** When the host's size
  push carries a `measure` token, the SDK waits for the page to settle. Then
  it reports each text, image, SVG, canvas or video the box cuts off. The
  engine turns that report into the `[WIDGET FIT]` note
  (`system-knowhow/building-an-app/when-an-app-is-the-right-answer.md` § The widget box, rule 8). A widget
  does nothing for it but load the SDK.
- A fit-only agent renderer uses this
  same measurement path. Its launcher holds the report credential; the SDK
  never receives it. See `docs/widget-fit-renderer.md` for the host contract.
- The same widget opened full size in the Canvas pane gets no size, so there
  it is an ordinary app.

### Reading the thread the user has open

`lucidos.ui.focusedThread()` gives the id of the *focused thread*: the thread
the user has open in the thread pane. Use it for a "This thread" view, such as
a cost or a file list for the thread the user is in.
`lucidos.ui.onFocusedThreadChange(callback)` calls `callback` with the new id
each time the user opens another thread. It returns a function that stops the
calls.

```js
const show = (threadId) => {
  document.querySelector('#scope').textContent = threadId ?? 'No thread open';
};
// Subscribe first, then read, so no change can land between the two.
lucidos.ui.onFocusedThreadChange(show);
show(await lucidos.ui.focusedThread());
```

- **The host decides, not your app.** A widget on the desktop dashboard sits beside
  whatever thread the user is in. So the value is that thread, not the thread
  the widget came from.
- **`null` means the empty compose view.** No thread is open there.
- **A draft and Home are threads.** A draft the user is typing gives its id.
- **The Canvas pane does not change it.** A panel or an app beside the thread
  leaves the value alone, as does a phone swipe to the Canvas pane. The thread
  is still open.
- **No host, no answer.** In its own tab your app has no thread pane, so
  `focusedThread()` rejects. The subscription is never called there.
- **Every frame hears every change.** The Canvas app, each inline widget and
  each dashboard tile get the same calls.

### Navigation targets

| Target | Params | Description |
|--------|--------|-------------|
| `thread` | `id` | Focus a specific thread |
| `app` | `id` (or `app_id`), `fragment` (optional) | Open an app UI, optionally at a place inside it. See the fragment param below. |
| `settings` | `settings_view` (optional), `fragment` (optional) | Open Settings, optionally a sub-section: `providers`, `models`, `intelligence`, `permissions`, `mcp`, `coding-agents`, `accounts`, `locale`, `marketplaces`, `access`, `devices`, `users`, `updates` (updates and installs), `appearance`, `keyboard-shortcuts`, or a System page (`system` is the list of them; `system-overview`, `release-notices`, `whats-new`, `backup`, `memory`, `disk-usage`, `environment-variables`, `thread-queue`, `debugging` are the pages themselves). Omit `settings_view` for the Settings home list. A `fragment` scrolls to a row on that page: `settings_view: 'providers'` with `fragment: 'xai'` lands on the xAI block. An unknown row still opens the page, then shows a toast. |
| `new-chat` | `prompt`, `coding_agent`, `folder` (all optional) | Open a fresh chat thread, optionally prefilling the compose textarea and preselecting the coding agent and folder. Prefer `lucidos.ui.startThread()`, the typed wrapper around this target. |
| `plugins` | `id` (optional) | Open the Plugins panel's Installed tab. With `id` (a plugin id), scroll to and pulse-highlight that plugin's row. The plugin-update notification uses this, so a tap on one update lands on its plugin. |
| `app-store` | (none) | Open the Plugins panel's Store (marketplace) tab. |
| `dashboard` | (none) | Show the *dashboard* of the device's layout: the rail on desktop, shown if it was hidden, and the dashboard panel over the open thread on a phone. |
| `file` | `file_path`, `line` (optional), `line_end` (optional) | Open a file in the preview pane, optionally at a line. See the two accepted path forms and the line params below. |
| _other panels_ | (none) | `files`, `apps`, `triggers`, `thread-queue`, `changes`, `notifications`; plus `trigger` (`id`), `url` (`url`), `new-app`, `new-trigger`. |

#### `fragment`: opening at a place inside the app

**Whenever you name a specific item, pass it.** The app opens with that string as
its `location.hash`, so an app that routes on the hash lands on the item. Without
it the app opens on whatever the reader last looked at, which on a sorted board
can be several cards away.

```js
// "the habit whose streak broke", not "the habit board".
await lucidos.ui.navigate('app', {
  app_id: 'habit-tracker',
  fragment: 'habit-hydration',
});
```

- Used with `target: 'app'` and `target: 'settings'`. On any other target it is ignored. On `settings` it names a row on the page, and Lucidos checks it.
- For an app, Lucidos never inspects it: only the app knows what its own targets are.
- An app that ignores the hash still opens, at whatever it shows by default.
- **Absent is not empty.** Omitting it leaves an already-open app where the
  reader put it; `''` would move them to the app's default view.
- An app already on screen is **moved, never reloaded**: the hash change is a
  same-document navigation, so the app sees a `hashchange` event.

An `app:<id>#<fragment>` link carries the same string. So a chat link, a
file-preview link and a notification tap all name a place the same way.

#### `file_path`: workspace data vs a registered repository

`file_path` takes one of two forms:

- **A workspace data path:** `artifacts/…`, `knowhow/…`, `apps/…`, `triggers/…`, or `system-knowhow/…`. A path with none of those prefixes is treated as an artifact, so `notes.md` opens `artifacts/notes.md`.
- **A repo-encoded path**: `repo:<repoId>:file:<repo-relative path>` opens a file from a **registered repository** (a local clone added under Settings → Coding Agents). `<repoId>` is that Repository's id, as returned by `GET /api/v1/repositories`. The file is read at the clone's current `HEAD`.
  - `lucidos.ui.navigate` also takes the repository's **name** there, and sends the id in its place. A name or id that no registered repository has rejects the call with an error naming it.

```js
// Open src/main/resources/transforms/order.jslt from a registered repo clone.
await lucidos.ui.navigate('file', {
  file_path: `repo:${repoId}:file:src/main/resources/transforms/order.jslt`,
});
```

The preview pane binds itself to that repository, so the Files panel behind it and the preview's changed-files sidebar stay on the same repo. A malformed `repo:…` string is not a repo path: it falls back to the artifact rule above.

##### Naming a revision: `repo:<repoId>:file#<ref>:<path>`

The bare form reads the clone's `HEAD`. A file a coding agent edited lives on that agent's worktree branch, and a citation into a release means a tag or a sha. Add `#<ref>` to the `file` segment to name the revision.

```js
// The file as it stands on a coding agent's branch, not as it stands on HEAD.
await lucidos.ui.previewFile({
  file_path: `repo:${repoId}:file#${branchName}:src/main.rs`,
  line: 510,
});
```

- `<ref>` is anything `git show` accepts as a revision: a branch, a tag, a full or short sha.
- It works on both calls and in the href form below, since it is part of the path string.
- Omit it and you get `HEAD`.
- A ref that does not exist (or a file that does not exist at it) shows the preview's normal "failed to load" state. It is not a thrown error.
- **Every segment must be non-empty.** `repo:<repoId>:file#:<path>` names no revision and is not a repo path at all. So it falls back to the artifact rule like any other malformed `repo:…` string. Leave the `#` off instead.

A ref cannot contain `:` (git forbids it), which is what keeps the `:`-separated form unambiguous; a `/` in a branch name is fine.

`diff` locators do not take a `#<ref>`: `repo:<repoId>:diff#<changeId>:<path>` already names its revisions through the change.

#### `line` / `line_end`: opening at a cited line

**Whenever you cite a specific line, pass it.** The preview then scrolls that line into view and highlights it, as if the reader had clicked its line number. Without it the file opens at the top, and the reader must find line 510 of a `file.rs:510` citation by hand.

- `line` is **1-based**. `line_end` is the last line of the range and is **inclusive**; omit it to highlight a single line.
- Both work for either `file_path` form, a workspace data path or a repo-encoded one.
- A file that renders (markdown, CSV, SVG) switches to its **source view**, since a rendered document has no lines to highlight.
- The highlight is the same one a manual line selection produces, so the reader can send it straight into a chat message as context.

```js
// "src/main.rs:510-520" in a report, made clickable.
await lucidos.ui.navigate('file', {
  file_path: `repo:${repoId}:file:src/main.rs`,
  line: 510,
  line_end: 520,
});
```

A line the file can't honour never costs the reader the file. The preview ignores `0`, a negative or fractional number, a line past the end, and a format with no source view (PDF, an image). The file then opens at the top. A citation's line number is the part that goes stale, so this is deliberate.

#### Linking to a repo file from an HTML artifact

An `<a href>` inside a **previewed HTML or markdown artifact** can use the repo-encoded path directly, with a GitHub-style line suffix:

```html
<a href="repo:REPO_ID:file:src/main.rs#L510-L520">src/main.rs:510-520</a>
```

The host routes that click through the same navigation, so a report full of citations works as a plain artifact, with no app needed. The artifact runs sandboxed and loads no SDK, so a link is its only way to reach the host. See `system-knowhow/best-practices/workspace-layout.md` § What a standalone HTML document can do.

`#L510` is a single line, and `#L510-L520` (or `#L510-520`) is a range. The suffix exists only for hrefs, since an anchor has no other way to carry a param. From JavaScript, use `line` / `line_end` above.

The revision form composes with it. The two `#` never compete: the line suffix is the trailing one, and the ref is the one inside the `file` segment.

```html
<a href="repo:REPO_ID:file#release/2.4:src/main.rs#L510-L520">src/main.rs:510-520 on release/2.4</a>
```

### Showing a cited file without leaving your app

`navigate('file', …)` takes the whole shell into the Files panel, so a reader of a report loses their place. `lucidos.ui.previewFile(params)` shows the file in a **file preview modal** over your app instead, so they glance at the code and carry on.

```js
// "src/main.rs:510-520" in a report, glanceable.
await lucidos.ui.previewFile({
  file_path: `repo:${repoId}:file:src/main.rs`,
  line: 510,
  line_end: 520,
});
```

```ts
interface FilePreviewParams {
  /** The same forms `navigate('file', …)` accepts: a workspace data path, or
   *  `repo:<repoId>:file:<repo-relative path>` for a registered repository
   *  clone (its current HEAD), or `repo:<repoId>:file#<ref>:<path>` for a
   *  named branch, tag or sha. */
  file_path: string;
  /** 1-based first line to highlight and scroll to. */
  line?: number;
  /** Inclusive last line of the range; omit for a single line. */
  line_end?: number;
}
```

`params` is the `file` target's own params, with the same field names, so one object drives either call:

```js
const at = { file_path: 'artifacts/report.md', line: 42 };
await lucidos.ui.previewFile(at);        // glance, your app stays put
await lucidos.ui.navigate('file', at);   // leave for the Files panel
```

Everything the two sections above specify applies unchanged: every `file_path` form (named revision included), 1-based inclusive `line` / `line_end`, and the same degradation for a bad line. The modal renders as the Files panel does, with the same highlight and line numbers. Its **Open in Files** link escalates to the same `navigate('file', …)`, at the same lines.

Name the revision here above all. The modal may show a repository the Files panel is not bound to, so it cannot fall back to that panel's branch. Without a `#<ref>` it reads `HEAD`.

| Want to … | Use |
|---|---|
| Let the reader check a citation and keep reading | `lucidos.ui.previewFile({ file_path, line })` |
| Send the reader to the file to work with it (edit, pick a range for chat, browse the tree) | `lucidos.ui.navigate('file', { file_path, line })` |

Three things to know:

- **It resolves when the preview is on screen, not when the reader dismisses it.** Your app is not blocked while a glance stays open. It rejects when the host cannot put it on screen, so the escalation makes a natural fallback:

  ```js
  try { await lucidos.ui.previewFile(at); }
  catch { await lucidos.ui.navigate('file', at); }
  ```

  Two causes make it reject, and both mean nothing would have appeared. Your app runs with **no host shell around it** (its own tab, or the SDK in a plain page). Or **something is fullscreen that the host cannot render over**: in practice, your app called `requestFullscreen` on its own content. Fullscreen from the Lucidos content header is fine.

- **Read-only.** There is no editing in the modal: `navigate('file', …)` leads to the editable preview. A second `previewFile` replaces a showing one.
- **A `repo:…:diff#…:…` locator previews the file, not the diff.** The diff view belongs to the Files panel, so use `navigate` for it. The modal shows the file at that change's end state, so a citation into a coding agent's work shows the work. As a file view it honours lines, unlike the same locator through `navigate`.

`previewFile`, `confirm`, `prompt` and `toast` are all rendered by the host. All of them appear over your app when the reader put it in fullscreen from the content header. Escape closes what is in front. With the app pseudo-fullscreen (iOS, and anywhere the Fullscreen API is unavailable), one Escape closes the modal and the app stays fullscreen. With real fullscreen the browser takes the first Escape to leave fullscreen, so the modal stays up and the next Escape closes it.

### Opening a link outside Lucidos

`lucidos.ui.openExternal(url)` sends a URL out of the app. **Use it instead of
`window.open` for any link that leaves Lucidos.**

The SDK's link interceptor already routes plain `<a href="https://…">` clicks
here. Call `openExternal` when you open a URL from JavaScript (a button handler,
a row action, a redirect after a fetch).

```js
document.querySelector('#docs-btn').addEventListener('click', () => {
  lucidos.ui.openExternal('https://example.com/docs');
});
```

Two rules:

- **Call it synchronously from the click handler.** WebKit gives a tab, or the
  hand-off to the Safari app, only to an open made inside the tap. A plain
  browser tab opens the page itself, framed or not, or the share sheet in "Ask"
  mode. In an installed iOS PWA set to `safari`, a framed app opens
  `x-safari-<url>` itself, the same way. The desktop app, and an installed iOS
  PWA in `ask` mode, open through the host instead. So do async work first,
  then open from a later interaction.
- **Don't fall back to `window.open`.** Inside an installed iOS PWA `window.open`
  cannot leave the app: WebKit renders an in-app web view with no address bar,
  no tabs and no shared Safari session. The user's `external_link_target`
  preference (`safari` / `ask` / `in-app`, default `safari`) controls that
  overlay, so a fallback overrides their choice.

Non-http(s) URLs (`mailto:`, `tel:`) go to the platform unchanged. If the
browser blocks the frame's own open, the host opens it or shows a toast whose
Open button retries. Every link tap in an app frame also writes
`[Client/app-link]` lines to the engine log: the touch, the route and the
outcome. Look there when a link does nothing. The promise resolves once the
open is dispatched. A user dismissing the share sheet resolves normally rather
than rejecting.

