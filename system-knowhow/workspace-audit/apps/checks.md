---
name: Workspace audit: apps, Remediation
description: Part of Workspace audit: apps. Its index lists the sections it covers.
---

One part of the `system-knowhow/workspace-audit/apps` audit section. Load every part of the section to run it; the section file lists them.

## Checks (continued)

- **A control the app draws itself that the SDK provides.** The host draws the
  SDK's helpers, so they pick up every later fix and theme change. An app's own
  copy stays as it was the day someone wrote it. The `hand-rolled-ui` scan finds
  the copies. Judge each hit:
  - **drift**: the browser's own `alert()`, `confirm()` or `prompt()`. They
    still work in the frame, unthemed.
  - **drift**: the app's own toast or snackbar, meaning a `toast()` function or
    a `.toast` element with its own CSS.
  - **drift**: a `<select>` in an app that never calls `lucidos.ui.Select` or
    `lucidos.ui.enhanceSelects`.
  - **drift**: a `title="…"` attribute used as a tooltip. It never shows on a
    touch device.
  - **drift**: an on/off switch the app draws itself, such as a `role="switch"`
    button with its own track and thumb. A `role="switch"` checkbox inside a
    `.toggle-switch` label is the shared one and is correct.
  - **drift**: a spinner the app draws itself, meaning a `.spinner` or
    `.loader` rule of its own. Where it stands in for loaded data, say so: data
    loading draws a skeleton, and only a working state takes the ring.
  - **drift**: a badge, chip or pill the app draws itself to show a status or
    a category. A pill the user taps to filter or switch views is the next case.
  - **drift**: tabs or filter pills the app draws itself. A `role="tab"` button
    with the `pill-bar-btn` class is the shared one and is correct.
  - **drift**: a widget's own choice buttons for a pick among data items, or
    its own ring gauge or bar series. The `.choice`, `.ring` and `.bar-run` classes are
    the shared ones and are correct.
  - **drift**: a bare `input`/`textarea` CSS rule giving it its own border or
    background. The `.text-input` class already is correct and is not a hit.

  Read the call site before you report a hit. An app can define its own
  `confirm()` method, and a `title` on an `<iframe>` or `<abbr>` is an
  accessible name, not a tooltip. Leave those.

  **A pattern with no SDK counterpart is not a finding.** Cards and custom
  modals have none yet, so an app that draws its own is correct. Owns the rule:
  `system-knowhow/js-sdk/lucidos-ui-ui-control-2.md` § Toasts, § Confirmation dialogs, § Prompts,
  § Tooltips, § lucidos.ui.Select and `system-knowhow/js-sdk/setup-2.md` § Component classes. § Remediation below
  carries the replacements.

- **A choice drawn as a button, a hover that sticks on touch.** The
  `touch-hover` and `state-as-action` scans find the candidates. Read the
  context of each hit. Owns the rule: `system-knowhow/building-an-app.md`
  § Scaffolding defaults, item 4.
  - **drift**: a `:hover` rule with no enclosing `@media (hover: hover)`. On
    a phone it stays on after a tap, so a control looks disabled.
  - **drift**: code that swaps `.action-btn` variants to mark the current view,
    sort or filter. A variant added once when a button is built is correct.
  - **broken**: `.action-btn-secondary`, `-confirm` or `-danger` without the
    base `action-btn` class. It draws a plain browser button.

- **A left-edge stripe.** The `left-stripe` scan finds left borders and inset
  shadows. A pseudo-element bar spans several lines, so read each `::before`
  and `::after` rule in the app's CSS by hand. Owns
  the rule: `system-knowhow/building-an-app/visual-quality-bar-make-it-look-sleek-an.md`
  § Smell test before finishing.
  - **drift**: a coloured left border, a `::before` bar or an inset shadow on a
    card, callout, banner or notice.

- **The ready signal.** An app can hold the host's loading cover until its
  content is drawn: `"reveal": "on-ready"` in `manifest.json`, then a
  `lucidos.ui.ready()` call. The `ready-signal` scan lists both halves per
  app. Read them together, plus the app's startup code. Owns the rule:
  `system-knowhow/js-sdk/lucidos-ui-ui-control.md` § Showing the app once its content is ready.
  - **broken**: the manifest says `on-ready` and no app code calls
    `lucidos.ui.ready()`. Every open sits behind the cover for the full
    15 s fuse.
  - **broken**: `reveal` holds any value other than `on-load` or `on-ready`.
    The app opens as `on-load`, so the opt-in the author meant is ignored.
  - **stale**: the app calls `lucidos.ui.ready()` but its manifest does not
    say `on-ready`. The call does nothing.
  - **stale**: the call exists, but a startup path that finishes the first
    render skips it, typically the `catch` that shows an error. That path
    waits for the fuse.
  - **nit**: no opt-in, and the startup code awaits `lucidos.data`,
    `lucidos.events`, `lucidos.proxy` or `lucidos.request` before its first
    render. The user sees an empty app until the data lands. Recommend the
    opt-in. Skip an app that draws a skeleton or its own placeholder first.

- **The dashboard tile.** The `widget-tile` scan lists each manifest that
  sets `sizes` or `transparent`. Read the hit with its `kind`. Owns the rule:
  `system-knowhow/building-an-app/when-an-app-is-the-right-answer.md` § Widgets, the Sizes and Transparent
  paragraphs.
  - **broken**: `sizes` is empty, names a size other than `small`, `medium`,
    `large` or `xl`, or names one twice. The widget loads with small, medium
    and large, so the sizes the author meant are ignored.
  - **broken**: `sizes` or `transparent` on an app that is not a widget. The
    loader ignores both.

Per `system-knowhow/building-an-app/scaffolding-defaults.md` § App icon:

- **The app icon.** The `app-icon` scan lists each manifest's `icon`. Check
  each value against the rule that section states.
  - **stale**: `icon` breaks that rule. The app shows its monogram tile, so
    nothing breaks, but the author meant an icon.
  - **nit**: an app or widget with no `icon`. Recommend drawing one.

Per `system-knowhow/building-an-app/when-an-app-is-the-right-answer.md` § Widgets:

- **A widget's origin.** A manifest with `"kind": "widget"` must carry exactly one owner. That is an `origin_thread_id` naming a thread in this workspace (`lucidos threads list`, or `thread_summaries`), or an `origin_plugin_id` naming an installed plugin (`system-knowhow/plugins/shipping-widgets.md` § Shipping widgets). A `reusable` thread widget is exempt from the thread check, since no thread owns it, but still needs the field.
  - **broken**: no owner, or an `origin_thread_id` that is not a uuid. No thread owns the widget, so no Delete (a thread) will ever remove it.
  - **broken**: both `origin_thread_id` and `origin_plugin_id`. Lucidos refuses to read the folder, so the widget is gone from every thread and dashboard. Keep the one that owns it.
  - **broken**: an `origin_plugin_id` widget that is not `reusable`, or names a plugin that is not installed. No uninstall will remove it. Suggest reinstalling the plugin or removing the folder.
  - **stale**: it names a thread that no longer exists, and the widget is not `reusable`. Nothing shows it or removes it. Suggest making it reusable or removing the folder.
  - **broken**: `kind` holds a value other than `app` or `widget`. The folder lists as an app, which is not what the author meant.
- **A widget drawn for its content height.** A widget draws into a *widget box* in its thread card and on a dashboard ("The widget box"). Read its HTML, CSS and JS.
  - **drift**: no `data-widget-size` rule and no `lucidos.widget.size()` read, so it draws one layout for every size and clips in its card. Suggest a layout per declared size.
  - **drift**: text sized in `px`, or shrunk to fit a size. The UI scale cannot reach px type. Suggest the `--font-size-*` tokens, and showing less at a smaller size.
  - **drift**: data text cut with `text-overflow: ellipsis` (rule 8). Suggest showing less at that size.
  - **drift**: content a box cuts off. Run `lucidos widgets fit --app-id <id>` with a Lucidos window open, and report each line of its `[WIDGET FIT]` note. A `not checked` note is no finding and no pass: say so.
  - **broken**: an automation recipe treats a logical device id as an attested Device for fit reports.
    A verified source-thread launcher can explicitly select a fit-only renderer instead (`docs/widget-fit-renderer.md`).
    Require genuine SDK measurements and the server's caveats; HTTP 200 alone proves no fit.

Per `system-knowhow/best-practices.md`:

- `manifest.json` carries only user-facing metadata; no operational knowledge has leaked in.
- Single-app docs live under `apps/<id>/knowhow/`; multi-consumer docs live under shared `data/knowhow/`.
- App data persists under `data/artifacts/<app-id>/`.

SDK GET transport recovery is automatic; see `js-sdk.md` for its retry limit.
Do not add an immediate transport retry around SDK reads.

## Remediation

Fixes run only on request, as the root's § Remediation says. A fix gets the
table for its finding. The root's § Inline or a coding agent defines the two
routes, and each table below names its own.

### Rewriting an app for an isolated frame

**Route: a coding agent, one per app.** The storage rows add a startup `await`
and drop a `catch`, which changes control flow. Two rows go inline: deleting a
`<base href>`, and the `location.hash = …` swap for the app's own state.

Hand a coding agent this table and the rules under it:

| Old | New |
|---|---|
| `localStorage.getItem('k')` / `setItem` / `removeItem` holding per-device state | the same call on `lucidos.storage.local`, after one `await lucidos.storage.ready` at startup |
| `sessionStorage.*` | the same call on `lucidos.storage.session`, after the same `await` |
| `localStorage` holding state every device should share | `await lucidos.data.read('artifacts/<app-id>/state.json')` and `lucidos.data.write(…)` |
| `indexedDB` or `caches` holding a large cache | `lucidos.data` under `artifacts/<app-id>/`. There is no in-frame replacement |
| `localStorage.getItem('lucidos-device-id')` | delete it. The host stamps the device on every bridged call, and the frame is not meant to know which one |
| an engine call the app's own `fetch` makes | the `lucidos.*` method that covers it, else `lucidos.request('/<suffix>', init)` |
| `fetch('<the app's own bundled file>')` | `await lucidos.data.read('apps/<app-id>/<file>')`, then parse. Not `lucidos.data.url()`, which builds a `src` and is refused when fetched |
| `lucidos.request('/env-vars', { method: 'POST' })` storing the app's own setting | `lucidos.preferences` for a user-facing one, else `lucidos.data.write('artifacts/<app-id>/settings.json', …)`. The read stays, so a genuine read of the workspace's variables is left alone |
| `lucidos.preferences.set` on a human-only key such as `command_guard` | nothing. It is a security setting, and the user changes it in Settings |
| `lucidos.preferences` reading or writing engine bookkeeping such as `backup_last_run` | nothing. It is engine state, not a setting: delete the read or the write |
| a call to a route the engine keeps from apps | nothing. Report it and name what it reaches for |
| `<a href="report.pdf" download>` on the app's own file | load `/api/v1/sdk.js`, which rewrites the click, or build a `blob:` URL |
| a `<base href>` the app declares | delete it. It replaces the pass the engine stamps for the app's own files |
| `history.replaceState(...)` / `history.pushState(...)` naming another path | write only a relative `'#…'` or `'?…'`, or set `location.hash`. Keep the read: apply `location.hash` at boot and on `hashchange` |
| `location.assign('#…')` / `location.href = '#…'` for the app's own state | `location.hash = …`. A relative URL there resolves against the stamped base and reloads the app |
| `location.href = ...` / `location.assign(...)` navigating the frame itself | `lucidos.ui.navigate(...)` for a Lucidos destination, `lucidos.ui.openExternal(url)` for anything outside |

- **Await `lucidos.storage.ready` before the first read.** Reads are
  synchronous after it. A read before it sees nothing stored, which is the very
  bug being fixed. Move the startup read after the `await`, and drop the
  `try` / `catch` that hid the old exception with `name="SecurityError"`.
- **Pick the store by who should see the value.** `lucidos.storage` is per
  device, like `localStorage`. `lucidos.data` is one store for every device.
  A sound toggle or a display currency is per device. A list the user curates
  is usually shared. Say which you chose in the report.
- **No `window.localStorage` shim.** Rename the calls. Reference:
  `system-knowhow/js-sdk/lucidos-preferences-user-settings.md` § `lucidos.storage`.
- **A storage toast is evidence of a failed app operation.** Trace the write
  and check the limits and cleanup guidance in the same SDK section before
  attributing the failure to that app's stored data.

### Opting an app into the ready signal

**Route: a coding agent.** The call has to land on every path that ends the
first render, which takes a read of the startup code. Hand it this table and
the rule under it:

| Old | New |
|---|---|
| an app that renders empty until its startup data loads | add `"reveal": "on-ready"` to `manifest.json`, and call `lucidos.ui.ready()` after the first render on every path, error and empty included |
| `"reveal": "on-ready"` with no `lucidos.ui.ready()` call | add the call after the first render, or drop the opt-in |
| `reveal` set to anything else, such as `"onready"` | `"on-ready"` if the app calls `lucidos.ui.ready()`, else delete the field |

- **Find every path that ends the first render.** A fix tends to miss the
  empty state and the `catch` that shows an error, which then wait 15 s for
  the fuse.

### Giving an app its icon

**Route: inline.** An icon is one file and a manifest field.

| Old | New |
|---|---|
| `icon` that breaks `system-knowhow/building-an-app/scaffolding-defaults.md` § App icon | fix the file, or draw a new icon as that section says |
| no `icon` | draw one as that section says |

### Replacing a control the app draws itself

Each row names its route. The inline rows swap one pattern for another. The
`:hover` and lone-variant rows are also what `[APP LINT]` reports, so each
write's result confirms its fix. The coding-agent rows change control flow, or
delete CSS the app may still use elsewhere. Hand a fix this table and the rules
under it:

| Old | New | Route |
|---|---|---|
| `alert(message)` | `lucidos.ui.toast(message, 'error')` for a failure, else `'info'` | inline |
| the app's own `toast(message)` and its element | `lucidos.ui.toast(message, type)`, then delete the element and its CSS | coding agent |
| `if (confirm(message))` | `if (await lucidos.ui.confirm({ message }))`, with `danger: true` for a destructive action | coding agent |
| `const value = prompt(message)` | `const value = await lucidos.ui.prompt({ message })`. It also returns `null` on cancel | coding agent |
| a plain `<select>` | `lucidos.ui.enhanceSelects()` once the markup is on the page | coding agent |
| `title="…"` as a tooltip | `data-tooltip="…"` | inline |
| a switch the app draws itself | `<label class="toggle-switch"><input type="checkbox" role="switch"><span class="toggle-slider"></span></label>`, then delete the app's own switch CSS | coding agent |
| a spinner the app draws itself | `<span class="mini-spinner" aria-hidden="true"></span>` beside its text, then delete the app's own spinner CSS and keyframes | coding agent |
| a status or category chip the app draws itself | `<span class="label">`, plus `label-success`, `label-warning`, `label-error` or `label-neutral` for a status. Map the app's own colours onto those four and the bare accent, then delete its chip CSS | coding agent |
| tabs the app draws itself | `<div class="pill-bar" role="tablist">` with a `<button class="pill-bar-btn" role="tab" aria-selected>` per view, then delete the app's own tab CSS | coding agent |
| filter pills the app draws itself | the same `.pill-bar`, with `role="group"` and `aria-pressed` on each button | coding agent |
| a widget's own choice buttons, ring gauge or bar series | `.choice-row` with a `<button class="choice">` per item, `.ring`, or `.bar-run` (`system-knowhow/js-sdk/setup-2.md` § Widget shapes), then delete the widget's own CSS for them | coding agent |
| `.action-btn` variants swapped to mark the current choice | `.segmented-control` with `.active` on the current `.segmented-btn` for two or three options, else the `.pill-bar` rows above | coding agent |
| a `:hover` rule outside any gate | the same rule inside `@media (hover: hover) { … }` | inline |
| a left-edge stripe on a card, callout, banner or notice | bold text, a raised `--bg-tertiary` surface, spacing or an inline icon. Delete the stripe | inline |
| a lone `.action-btn-X` variant | `class="action-btn action-btn-X"` | inline |
| a text input or textarea the app draws itself | `<input class="text-input">` / `<textarea class="text-input">`, then delete the app's own border/background/focus CSS for it | coding agent |

- **Confirm and prompt become asynchronous.** The SDK's return a promise, so
  the calling function becomes `async`, and every caller has to await it.
- **Keep the message and its language.** Pick the toast type from what the
  message says: success, a failure, or neutral information.
- **Every helper needs `sdk.js`.** An app whose `index.html` does not load
  `/api/v1/sdk.js` gets it first, per the boilerplate in `system-knowhow/js-sdk.md`
  § Setup.

### Renaming the board's old names

**Route: inline.** Each row is a one-word swap at the hit:

| Old | New |
|---|---|
| `data-widget-place="home"` in a selector | `data-widget-place="dashboard"` |
| `lucidos.widget.place() === 'home'` | `lucidos.widget.place() === 'dashboard'` |
| `lucidos.ui.navigate('home-board')` | `lucidos.ui.navigate('dashboard')` |

## Diagnostic evidence

When this audit needs runtime evidence, follow [Agent Sandbox](../../agent-sandbox.md) for bounded diagnostic access.
A maintainer bundle omits legacy log text; its absence does not establish an audit finding.
