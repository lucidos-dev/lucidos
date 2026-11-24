---
name: Building an App, Updating an app
description: Part of Building an App. Its index lists the sections it covers.
---

One part of `system-knowhow/building-an-app`, which lists every part.

## Updating an app

There is no `update_app` tool. After `create_app`, all changes go through `write_file` / `edit_file` on `apps/{id}/index.html` (and `manifest.json` when the name or description changes). Don't recreate the app to change one button. A second `create_app` for an existing id is refused: it would rewrite `index.html` and orphan every other file in the app.

### Editing an app with a coding agent

Two paths, picked per request:

- **Chat path (quick edits)**: file tools (`write_file`, `edit_file`) on the live `data/apps/<id>/` files, for one-line tweaks, copy fixes, and small CSS adjustments. The change lands immediately on workspace `main`. **You don't need to refresh manually.** At the end of your turn the engine reloads the open app UI (`AppUiRefreshRequested`) and refreshes the apps list (`AppUpdated`), once per edited app. So don't spam `refresh_app` mid-turn. A brand-new app from `create_app` already appears via `AppCreated`.
- **App coding-agent thread (heavier work)**: `run_coding_agent(folder='data/apps/<id>')` spawns a *coding-agent thread* in an isolated sparse-checkout *worktree* narrowed to that one app folder. Branch name shape: `lucidos-<coding-agent>-app-<id>-<slug>-<thread id>` (for example `lucidos-claude-code-app-habit-tracker-add-streaks-401a2d19`). Best for multi-file refactors, new features, and work that needs review before landing. It produces a *change* the user reviews and clicks *Apply* on. Apply ff-merges to workspace `main`, emits `AppUiRefreshRequested` so open iframes reload, and does **not** restart the engine. `/harden` does not run for app changes: apps own their hardening (ship a `.claude/commands/harden.md` if you want one).

**When you name an app the user can open, link it** with the `app:<id>` scheme, on either path: `[Habit Tracker](app:habit-tracker)`. A bare prose mention of the app name is NOT a link.

While the app coding-agent thread is open, the user can preview the in-flight app via `?thread_id=<id>` on the app UI URL. The panel-overlay slot then swaps from the live workspace copy to the WIP worktree copy. SDK calls (`lucidos.data.*`, `lucidos.events.*`) still hit live workspace data, so data-coupled UI edits show their full effect only after Apply.

### Checking your work with `capture_app` / `refresh_app`

`capture_app` / `refresh_app` are **agent self-check tools**: they reload the open app UI and snapshot it back to you. Use them *only while iterating on an app with the user*, when that app is the subject of the current turn. Never use them in threads that aren't about an app (research, data tasks, general chat).

- **You usually don't need them at all.** The engine auto-refreshes edited apps at the end of your turn (see the chat path above). Reserve a manual `capture_app` for when you must *look* at the result mid-turn before the next edit.
- **They reach one screen: the turn's *last used device*,** the same one `navigate_ui` sends to. To capture another device, pass `capture_app` a `device` from Known devices, as `navigate_ui` takes. Only the target device reloads and answers. A failure names it: `Error from "<device>", …` when it has no app open, or a timeout naming the device that did not answer. A timeout adds a hint when Lucidos is not visible there. Tell the user which device you looked at.
- **On "No app UI is currently open", open the app yourself.** Call `navigate_ui(target="app", app_id=...)` with the same `device`, then capture again. Never ask the user to open it. Do this only while the app is the subject of the turn, because it takes over the user's screen.
- **To keep a screenshot, pass `save_as_artifact`** (`artifacts/screenshots/board.png`). The picture is then sharp, at the device's pixel ratio, in the format the extension names: `.png`, `.jpg` or `.webp`. You see the same picture. A device that cannot encode the format saves nothing and says so; retry with `.png`.
- **Never retry the same `capture_app` / `refresh_app` call after it fails.** The identical call trips the circuit breaker. On "No app UI is currently open", **switch strategy**: `navigate_ui` once, then capture.
- **In background / trigger / cron threads, don't attempt `capture_app` / `refresh_app` (or navigate-to-app) at all.** Such a turn has no *last used device*, so no single screen takes the navigate. The capture goes to every connected page, and the first answer wins, usually "No app UI is currently open".

## Common mistakes to avoid

- **Storing data in `apps/{id}/`.** App data goes in `artifacts/{app-id}/`. The app code is git-tracked source; the data is user state. (See `best-practices.md`.)
- **Forgetting the `artifacts/` prefix in `lucidos.data.*` paths.** Paths are relative to `data/`, not `data/artifacts/`. App data lives at `artifacts/{app-id}/data.json`, *not* `{app-id}/data.json`. Without the prefix, `read` returns a 404 `SdkError` and `write` fails silently. It usually surfaces as "the checkbox toggles back" or "state doesn't persist".
- **Hardcoding colors, so the app is light-only or dark-only.** This is the most common theming regression: a light-mode workspace gets a jarring dark app. See *Scaffolding defaults* items 1 to 3.
- **Hardcoding a `font-family` stack on `body`.** It looks like ordinary boilerplate, but it replaces the user's chosen font with the system one, with no error. **Delete the declaration; don't fix the stack.** See *Scaffolding defaults* item 3.
- **Sizing in `px`, or picking magic `font-size` numbers instead of the type scale.** `px` ignores the user's UI-scale preference. A hand-dialed `0.95rem` drifts off the shared scale, so a title ends up subtly off from the same element elsewhere. See *Scaffolding defaults* item 5.
- **Building for the desktop pane only, and never checking phone width.** A fixed multi-column layout looks fine in a wide preview pane and overflows on a phone, where the user meets it just as often. See *Responsive by default*.
- **A `flex: none` nowrap pill holding free text.** The page cannot scroll sideways, so the pill is cut off at the right edge instead. Let it shrink and wrap. See *Responsive by default*.
- **Putting a control in the top-right corner, or pinning one to a side edge.** The exit-fullscreen button covers the corner, and it is the user's only way out on iOS. On mobile, the back-gesture guards take edge taps first. See *The host paints over a fullscreen app*.
- **Hand-rolling a secondary button, *or* writing `.action-btn-secondary` without the base class.** Don't invent an outlined or ghost button with `var(--accent)`. Write `class="action-btn action-btn-secondary"`: a lone variant falls back to a plain grey browser button. See *Scaffolding defaults* item 4.
- **Showing the current sort, filter or view as a blue `.action-btn`.** Users read it as a button to press. Use `.segmented-control` or `.pill-bar`. See *Scaffolding defaults* item 4.
- **Writing a `:hover` outside `@media (hover: hover)`.** On a phone it stays on after a tap, so a control looks disabled. See *Scaffolding defaults* item 4.
- **Using the browser's dialogs or a native `<select>` instead of `lucidos.ui.*`.** That includes a hand-rolled toast banner. Use `lucidos.ui.toast` / `lucidos.ui.confirm` / `lucidos.ui.prompt` / `lucidos.ui.Select` (or `enhanceSelects()`). See *Interaction primitives* above and `system-knowhow/js-sdk/lucidos-threads-thread-management.md` § lucidos.ui.
- **Creating an app for a one-shot.** If the user only wants the answer once, just give it: in text, or as a widget when it is easier to scan than to read.
- **Inventing SDK calls, or guessing the surface and checking later.** Load `system-knowhow/js-sdk.md` **before** you write the first SDK call or theme-CSS `var()`. A scaffold written from memory tends to ship several guesses at once. A later check catches the JS but misses CSS class and token errors. Two common guesses:
  - There is no `lucidos.chat` namespace, so `lucidos.chat.send` doesn't exist. Open a chat with the typed `lucidos.ui.startThread({ prompt })`, preferred over the low-level `lucidos.ui.navigate('new-chat', …)`.
  - `lucidos.data.read()` returns a **string**: `JSON.parse` it on read and `JSON.stringify` on write.
- **Hand-rolling the proxy URL with raw `fetch`.** Both shapes in *External APIs* above are wrong. Use `lucidos.proxy(name).fetch(path, init)` and configure the backend in `data/config/apis.json`.
- **Calling the engine with your own `fetch`.** Inside the host shell CORS refuses it, as *Use the SDK for everything stateful* explains. Use an SDK method. A standalone app tab does reach the engine, and there the two obvious URLs both 404:
  - `new URL('api/v1/events/query', document.baseURI)` resolves under the app's own directory, since an app page has no `<base href>`.
  - `fetch('/api/v1/events/query')` hands the gateway `api` as a workspace name.

  Markup is the exception. The engine rewrites `src` / `href` attributes it serves, so `<script src="/api/v1/sdk.js">` works in `index.html`. Nothing rewrites a string JS builds at runtime: **the same path is correct in markup and broken in JS.** Either way it fails *silently*, landing in a `catch` that goes on rendering stale data. See `system-knowhow/js-sdk/lucidos-apiurl.md` § `lucidos.apiUrl`.
- **Hardcoding credentials in iframe code.** The credential belongs in the engine's credential store, referenced by name from `apis.json`. The SDK never sees the secret.
- **Batching large rewrites.** After scaffold, prefer small visible changes the user can react to per round.
