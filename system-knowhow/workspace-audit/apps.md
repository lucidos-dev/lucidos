---
name: Workspace audit: apps
description: The apps audit section: SDK boilerplate, what an isolated app frame can no longer do, escaping, theme names, hand-drawn controls, the ready signal, widget manifests.
---

# Workspace audit section: apps

One *audit section* of `system-knowhow/workspace-audit`. The root says how to
run it alone or in a full pass, how to merge its receipts, and how to write
the report. This file carries its scan, its checks and its fixes.

## Scan

Append these lines to the root's scan preamble, before its receipts tail:

```bash
scan storage "localStorage|sessionStorage|document\.cookie|indexedDB" $inc apps
scan host-realm "window\.parent|parent\.document|window\.top|top\.location" $inc apps
scan engine-fetch "fetch\(['\"\`]/api/v1|new URL\(['\"\`]api/v1|new EventSource\(|apiUrl\(" $inc apps
scan relative-fetch "fetch\([[:space:]]*['\"\`][A-Za-z0-9_.-]+[/.]" $inc apps
scan download-link "<a [^>]*download" --include=*.html apps
scan media-capture "getUserMedia" $inc apps
scan web-share "navigator\.share" $inc apps
scan url-mutation "history\.(replaceState|pushState)|location\.(href|assign|replace)[[:space:]]*=|window\.location[[:space:]]*=" $inc apps
scan theme-rename "data-theme([^-]|\$)|data-look-|look-effects|lucidos-look|/looks?[/?'\"\`]|preferences\.(set|get)\(['\"](look|theme)['\"]" $inc --include=*.css apps
scan hand-rolled-ui "(^|[^.[:alnum:]_])(alert|confirm|prompt)\(|window\.(alert|confirm|prompt)\(|function[[:space:]]+(toast|showToast|snackbar)[[:space:]]*\(|class=[\"'][^\"']*(toast|snackbar)|^[[:space:]]*\.(toast|snackbar)[[:space:]{.,:]|<select[[:space:]>]|[[:space:]]title=[\"']|role=[\"']switch|(toggle|switch)[[:alnum:]_-]*(thumb|knob)|^[[:space:]]*\.[[:alnum:]_-]*(spinner|loader|badge|chip|pill|tabs?)[[:space:]{.,:]|role=[\"']tab[\"']|^[[:space:]]*(input|textarea)[[:space:]{.,:[]" $inc --include=*.css --exclude-dir=tests apps
scan attr-escape "=[[:space:]]*[\"'](\\\$\{|[\"'][[:space:]]*\+)[^}]*escapeHtml\(" $inc apps
scan ready-signal "\"reveal\"[[:space:]]*:|lucidos\.ui\.ready\(" --include=manifest.json $inc apps
scan app-icon "\"icon\"[[:space:]]*:" --include=manifest.json apps
```

## Checks

For each finding, capture: **location**, **what's wrong**, **which reference owns the rule** (link, don't quote), **suggested fix**.

Per `system-knowhow/js-sdk.md`:

- `index.html` matches the current boilerplate (script order, which pieces are required vs optional).
- Every `lucidos.*` call used in app code appears in the SDK reference. Calls not listed are either deprecated or invented.
- **External-API calls from the iframe: USE `lucidos.proxy(name).fetch(path, init)`.** The engine forwards the request server-side, strips this side's headers, and injects the configured auth header from the credential store. The credential never reaches the iframe. Configure the backend once in `data/config/apis.json`. Reference: `system-knowhow/js-sdk.md` § `lucidos.proxy`, which owns the strip list.

  **DO NOT USE** either of the following. Flag each occurrence and recommend the SDK helper:

  - `fetch('http://...')` or `fetch('https://<external-host>/...')` from inside an iframe. Mixed-content / CORS blocks it; if it works, the credential is sitting in the iframe. Suggest a `data/config/apis.json` entry and `lucidos.proxy(name).fetch(...)`.
  - `fetch('/api/v1/proxy/<name>/...')`: the SDK helper's wire format, bypassing the helper. The proxy name becomes a typo-prone magic string, and future SDK-side handling (timeouts, retries, response parsing, error shape) won't apply. Suggest `lucidos.proxy('<name>').fetch(path, init)`.

  A credential header written into app code belongs to the `cross-cutting` section.

- **The app calling the engine with its own `fetch`.** Walk `data/apps/**/*.{js,ts,html}` for any engine call JavaScript makes itself: `fetch('/api/v1/events/query')`, `new URL('api/v1/events/query', document.baseURI)`, `new EventSource('/api/v1/events')`, `fetch(lucidos.apiUrl('/<suffix>'))`, and any `location.pathname`-splicing that rebuilds the workspace address. In the host shell none reaches the engine: the frame's origin is opaque and CORS refuses it. Severity **broken**. WebKit reports `Load failed` and Chromium a `TypeError`. The failure usually lands in a falling-back `catch`, so the symptom is stale data, not an error.

  **`lucidos.apiUrl` is in that list deliberately.** Where an SDK method covers the endpoint, name that method. Otherwise the remedy is `lucidos.request('/<suffix>', init)`, which travels the bridge (ADR 0231). Never recommend `apiUrl` for a call: that moves a working app onto a pattern that cannot run. Reference: `system-knowhow/js-sdk.md` § `lucidos.request`.

  **A route an app may not reach has no remedy, and saying so is the finding.** The engine classifies every route, and `lucidos.request` refuses the rest with a 403 in both realms. Credentials, thread contents, consent routes and platform control are denied on purpose. Report the call and what it reaches for, and invent no way around. Reference: `system-knowhow/js-sdk.md` § "Not every endpoint is reachable".

  An `/api/v1/` path in a markup `src` / `href` attribute is correct and must NOT be flagged, nor is `apiUrl` building one. Five are exempt from the gateway's device gate: `sdk.js`, `sdk-prefs.js`, `sdk-iframe.css`, `sdk-iframe-audio.js`, and anything under `fonts/`. Behind a gateway, any OTHER `/api/v1/` path in a tag is refused: the frame's pass to its own files reaches no engine route. Severity: **broken**, and the remedy is the `lucidos.*` method that covers it.

- **What an isolated app frame can no longer do.** In the host shell an app runs at an opaque origin, in its own renderer process, so it cannot freeze or read the shell. The price: the frame's own `fetch`, `EventSource` and browser storage all fail, and the SDK carries those three over a bridge. Reference: `system-knowhow/js-sdk.md` § Setup, and *app frame* / *app bridge* in `docs/glossary.md`. Walk `data/apps/**/*.{js,ts,html}` for code that goes around it, skipping any vendored `node_modules/` tree:

  - **Browser storage touched directly**: `localStorage`, `sessionStorage`, `document.cookie`, `indexedDB`. Each throws a `SecurityError` in the frame. The remedy is in § Rewriting an app for an isolated frame. Report the two shapes apart, because they fail differently:
    - **broken** where the call sits outside a `try`. The throw stops the script, so the app renders nothing.
    - **stale** where a `try` / `catch` wraps it, the common shape written for Safari private mode. The app runs and silently stops remembering anything.
  - **A read of the host realm**: `window.parent`, `parent.document`, `window.top`, or the device id lifted out of the shell's storage. All blocked. Severity: **broken**.
  - **`<a href="<the app's own file>" download>`**, in an app whose `index.html` loads no `/api/v1/sdk.js`. A browser ignores `download` on a cross-origin link, so the click navigates the frame to the file. Severity: **broken**.
  - **The camera or the microphone.** `navigator.mediaDevices.getUserMedia`. Both browsers refuse media capture to an opaque origin, whatever the frame is granted, so no in-frame remedy exists. Severity: **broken**. Say the app has to run in its own tab. Reference: `system-knowhow/js-sdk.md` § Setup, which lists what the frame is granted.
  - **The OS share sheet.** `navigator.share` called directly. The frame is not granted `web-share`, and iOS refuses the delegation anyway. Severity: **broken**. The remedy is `lucidos.ui.openExternal(url)`, which opens the link through the host. A hit inside a vendored `sdk.js` is the SDK's own fallback, so read the call site first.
  - **Session-history URL writes.** `history.replaceState`, `history.pushState`, or assignment to `location.href` / `window.location` / `location.assign()` / `location.replace()`. The frame is sandboxed without `allow-same-origin`. So the browser refuses any session-history URL write whose path or fragment differs from the frame's real URL. The error reads "Paths and fragments must match for a sandboxed document". Split the severity the way the failure does:
    - **broken** where the call sits outside a `try`. It throws, and anywhere in the render or boot path the app renders nothing.
    - **stale** where a `try` / `catch` wraps it. The URL stops reflecting state, so deep links out of the app break while nothing looks wrong.

    The remedy: an app must not write its own URL, and has no way to. Reading a fragment still works: apply `location.hash` at boot and subscribe to `hashchange`, which is how inbound deep links arrive. To share its state, the app builds the link string and copies or shows it. Reference: `system-knowhow/js-sdk.md` § Setup (what the frame is granted) and § "fragment: opening at a place inside the app".
  - **A write to `/env-vars`**, through `lucidos.request` or the app's own `fetch`. The route opens `GET` only. A user env var reaches every command the agent runs, so a name the interpreter loads from would be host code execution. Severity: **broken**, since the call answers 403 and the app's settings never persist. § Remediation below carries the replacement.
  - **A write of a human-only preference**, through `lucidos.preferences.set` or a `PUT /preferences`. The route stays open, but two key classes are refused to an app. One the Lucidos Agent may not write either: `command_guard`, `max_tool_calls`, `network_bind`, `local_base_url`, the judge settings and the `provider_enabled_*` switches. One the agent may write but an app may not: the coding-agent paths and permission mode. These would let an app run its own script as the user, or read local-model chat. Severity: **broken** (403); § Remediation below carries the answer.
  - **A read or write of engine bookkeeping**, such as the Web Push keypair in `vapid_keys`. A preference read leaves it out, and an app's `PUT /preferences` naming it is refused. Severity: **broken** (403 on write, a missing key on read); § Remediation below carries the answer.
  - **The app's own bundled file, fetched by a relative path.** `fetch('data/song.json')`, ``fetch(`audio/clips/${name}.json`)``, any `fetch` whose first argument is a relative path. The frame's origin is opaque, so the browser refuses it like an engine call: `Load failed` in WebKit, a `TypeError` in Chromium. It reads as safe, and the engine-fetch pattern misses it, since nothing in the string says `/api/v1`. Split the severity the way the failure does:
    - **broken** where the throw escapes setup, or the fetch is the app's only data path. The app renders an error banner, or nothing.
    - **stale** where a `catch` falls back to `lucidos.data` and the app keeps running, minus whatever the bundled file carried.

    The remedy is `lucidos.data.read('apps/<app-id>/<path>')`, which travels the bridge. **`lucidos.data.url()` is not a remedy.** It builds a URL for a `src` or an `href`, and fetching one is refused the same way.
  - **A `<base href>` the app declares itself.** The first base in a document wins, so it replaces the pass the engine stamps for the app's own files. Behind a gateway every relative `src` / `href` then answers **401**. Severity: **broken**. Delete it: relative refs already resolve against the app's own directory. Reference: `system-knowhow/js-sdk.md` § Setup, and [ADR 0238](https://github.com/lucidos-dev/lucidos/blob/main/docs/adr/0238-app-frame-carries-a-capability-to-its-own-files.md).

  **A separate `app.js`, `style.css` or image is NOT a finding.** The engine gives each framed document a short-lived pass to its own files. Do not flag one, and do not recommend inlining. The same holds for the app's own `@font-face` and `<script type="module">`. The engine grants both across the opaque origin, the module behind the gateway every install runs ([ADR 0289](https://github.com/lucidos-dev/lucidos/blob/main/docs/adr/0289-app-frames-load-fonts-and-modules-across-origins.md)).

  An app opened in its own tab keeps all of this, as a top-level document. Never report it unaffected on that basis: the same app is reachable both ways, and the frame is the usual one.

- **A text-only escaper writing into an attribute value.** The `attr-escape`
  scan finds `escapeHtml(...)` spliced into a quoted attribute, as in
  `` `<a title="${lucidos.utils.escapeHtml(x)}">` ``. That helper leaves `"`
  and `'` raw, so a quote in the value breaks out and runs script with the
  app's full authority. Severity: **broken**. The remedy is
  `lucidos.utils.escapeHtmlAttr`. Reference: `system-knowhow/js-sdk.md`
  § "Escaping: which helper goes where".

  Read the call site first. An app may define its own `escapeHtml` that also
  escapes both quotes, and that one is correct.

- **Names from before the theme rename.** *Look* became *theme*, and the
  light/dark `theme` preference became `theme-mode` (ADR 0316). The `theme-rename`
  scan finds the old names. Judge each hit:
  - **broken**: `data-theme` in a selector or an attribute read. An app frame
    no longer carries it, so those styles never match. Recommend
    `data-theme-mode`.
  - **broken**: `data-look-effects`, `data-look-parts`, the `look` or
    `look-effects` preference, or a `/looks` or `/look?id=` route. None of
    them exists any more. Recommend `data-theme-effects`, `data-theme-parts`,
    the `theme` and `theme-effects` preferences, and `/themes` or `/theme?id=`.
  - **broken**: `theme` set to `light`, `dark` or `system`. The engine refuses
    it. Recommend `theme-mode`. A `theme` set to a theme id is correct.

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
  - **drift**: a bare `input`/`textarea` CSS rule giving it its own border or
    background. The `.text-input` class already is correct and is not a hit.

  Read the call site before you report a hit. An app can define its own
  `confirm()` method, and a `title` on an `<iframe>` or `<abbr>` is an
  accessible name, not a tooltip. Leave those.

  **A pattern with no SDK counterpart is not a finding.** Cards and custom
  modals have none yet, so an app that draws its own is correct. Owns the rule:
  `system-knowhow/js-sdk.md` § Toasts, § Confirmation dialogs, § Prompts,
  § Tooltips, § lucidos.ui.Select and § Component classes. § Remediation below
  carries the replacements.

- **The ready signal.** An app can hold the host's loading cover until its
  content is drawn: `"reveal": "on-ready"` in `manifest.json`, then a
  `lucidos.ui.ready()` call. The `ready-signal` scan lists both halves per
  app. Read them together, plus the app's startup code. Owns the rule:
  `system-knowhow/js-sdk.md` § Showing the app once its content is ready.
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

Per `system-knowhow/building-an-app.md` § App icon:

- **The app icon.** The `app-icon` scan lists each manifest's `icon`. Check
  each value against the rule that section states.
  - **stale**: `icon` breaks that rule. The app shows its monogram tile, so
    nothing breaks, but the author meant an icon.
  - **nit**: an app or widget with no `icon`. Recommend drawing one.

Per `system-knowhow/building-an-app.md` § Widgets:

- **A widget's origin.** A manifest with `"kind": "widget"` must carry exactly one owner. That is an `origin_thread_id` naming a thread in this workspace (`lucidos threads list`, or `thread_summaries`), or an `origin_plugin_id` naming an installed plugin (`plugins.md` § Shipping widgets). A `reusable` thread widget is exempt from the thread check, since no thread owns it, but still needs the field.
  - **broken**: no owner, or an `origin_thread_id` that is not a uuid. The widget shows on no shelf it can find its way back to, and no Delete (a thread) will ever remove it.
  - **broken**: both `origin_thread_id` and `origin_plugin_id`. Lucidos refuses to read the folder, so the widget is gone from every shelf. Keep the one that owns it.
  - **broken**: an `origin_plugin_id` widget that is not `reusable`, or names a plugin that is not installed. No uninstall will remove it. Suggest reinstalling the plugin or removing the folder.
  - **stale**: it names a thread that no longer exists, and the widget is not `reusable`. Nothing shows it or removes it. Suggest making it reusable or removing the folder.
  - **broken**: `kind` holds a value other than `app` or `widget`. The folder lists as an app, which is not what the author meant.

Per `system-knowhow/best-practices.md`:

- `manifest.json` carries only user-facing metadata; no operational knowledge has leaked in.
- Single-app docs live under `apps/<id>/knowhow/`; multi-consumer docs live under shared `data/knowhow/`.
- App data persists under `data/artifacts/<app-id>/`.

## Remediation

Fixes run only on request, as the root's § Remediation says. A fix thread gets
the table for its finding.

### Rewriting an app for an isolated frame

Hand a fix thread this table and the rules under it:

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
| `lucidos.preferences` reading or writing engine bookkeeping such as `vapid_keys` | nothing. It is engine state, not a setting: delete the read or the write |
| a call to a route the engine keeps from apps | nothing. Report it and name what it reaches for |
| `<a href="report.pdf" download>` on the app's own file | load `/api/v1/sdk.js`, which rewrites the click, or build a `blob:` URL |
| a `<base href>` the app declares | delete it. It replaces the pass the engine stamps for the app's own files |
| `history.replaceState(...)` / `history.pushState(...)` writing the app's own state into the URL | delete the write. Keep the read: apply `location.hash` at boot and on `hashchange` |
| `location.href = ...` / `location.assign(...)` navigating the frame itself | `lucidos.ui.navigate(...)` for a Lucidos destination, `lucidos.ui.openExternal(url)` for anything outside |

- **Await `lucidos.storage.ready` before the first read.** Reads are
  synchronous after it. A read before it sees nothing stored, which is the very
  bug being fixed. Move the startup read after the `await`, and drop the
  `try` / `catch` that hid the old `SecurityError`.
- **Pick the store by who should see the value.** `lucidos.storage` is per
  device, like `localStorage`. `lucidos.data` is one store for every device.
  A sound toggle or a display currency is per device. A list the user curates
  is usually shared. Say which you chose in the report.
- **No `window.localStorage` shim.** Rename the calls. Reference:
  `system-knowhow/js-sdk.md` § `lucidos.storage`.

### Opting an app into the ready signal

Hand a fix thread this table and the rule under it:

| Old | New |
|---|---|
| an app that renders empty until its startup data loads | add `"reveal": "on-ready"` to `manifest.json`, and call `lucidos.ui.ready()` after the first render on every path, error and empty included |
| `"reveal": "on-ready"` with no `lucidos.ui.ready()` call | add the call after the first render, or drop the opt-in |
| `reveal` set to anything else, such as `"onready"` | `"on-ready"` if the app calls `lucidos.ui.ready()`, else delete the field |

- **Find every path that ends the first render.** A fix tends to miss the
  empty state and the `catch` that shows an error, which then wait 15 s for
  the fuse.

### Giving an app its icon

| Old | New |
|---|---|
| `icon` that breaks `building-an-app.md` § App icon | fix the file, or draw a new icon as that section says |
| no `icon` | draw one as that section says |

### Replacing a control the app draws itself

Hand a fix thread this table and the rules under it:

| Old | New |
|---|---|
| `alert(message)` | `lucidos.ui.toast(message, 'error')` for a failure, else `'info'` |
| the app's own `toast(message)` and its element | `lucidos.ui.toast(message, type)`, then delete the element and its CSS |
| `if (confirm(message))` | `if (await lucidos.ui.confirm({ message }))`, with `danger: true` for a destructive action |
| `const value = prompt(message)` | `const value = await lucidos.ui.prompt({ message })`. It also returns `null` on cancel |
| a plain `<select>` | `lucidos.ui.enhanceSelects()` once the markup is on the page |
| `title="…"` as a tooltip | `data-tooltip="…"` |
| a switch the app draws itself | `<label class="toggle-switch"><input type="checkbox" role="switch"><span class="toggle-slider"></span></label>`, then delete the app's own switch CSS |
| a spinner the app draws itself | `<span class="mini-spinner" aria-hidden="true"></span>` beside its text, then delete the app's own spinner CSS and keyframes |
| a status or category chip the app draws itself | `<span class="label">`, plus `label-success`, `label-warning`, `label-error` or `label-neutral` for a status. Map the app's own colours onto those four and the bare accent, then delete its chip CSS |
| tabs the app draws itself | `<div class="pill-bar" role="tablist">` with a `<button class="pill-bar-btn" role="tab" aria-selected>` per view, then delete the app's own tab CSS |
| filter pills the app draws itself | the same `.pill-bar`, with `role="group"` and `aria-pressed` on each button |
| a text input or textarea the app draws itself | `<input class="text-input">` / `<textarea class="text-input">`, then delete the app's own border/background/focus CSS for it |

- **Confirm and prompt become asynchronous.** The SDK's return a promise, so
  the calling function becomes `async`, and every caller has to await it.
- **Keep the message and its language.** Pick the toast type from what the
  message says: success, a failure, or neutral information.
- **Every helper needs `sdk.js`.** An app whose `index.html` does not load
  `/api/v1/sdk.js` gets it first, per the boilerplate in `system-knowhow/js-sdk.md`
  § Setup.
