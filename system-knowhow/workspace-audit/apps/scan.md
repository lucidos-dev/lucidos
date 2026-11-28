---
name: Workspace audit: apps, Scan
description: Part of Workspace audit: apps. Its index lists the sections it covers.
---

One part of the `system-knowhow/workspace-audit/apps` audit section. Load every part of the section to run it; the section file lists them.

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
scan url-mutation "history\.(replaceState|pushState)|location\.(href|assign|replace)[[:space:]]*=|location\.(assign|replace)\(|window\.location[[:space:]]*=" $inc apps
scan theme-rename "data-theme([^-]|\$)|data-look-|look-effects|lucidos-look|/looks?[/?'\"\`]|preferences\.(set|get)\(['\"](look|theme)['\"]" $inc --include=*.css apps
scan dashboard-rename "data-widget-place[^=]*=[[:space:]]*['\"]?home[]'\"]|(place\(\)|widgetPlace)[[:space:]]*[!=]==?[[:space:]]*['\"\`]home['\"\`]|['\"\`]home-board['\"\`]" $inc --include=*.css apps
scan hand-rolled-ui "(^|[^.[:alnum:]_])(alert|confirm|prompt)\(|window\.(alert|confirm|prompt)\(|function[[:space:]]+(toast|showToast|snackbar)[[:space:]]*\(|class=[\"'][^\"']*(toast|snackbar)|^[[:space:]]*\.(toast|snackbar)[[:space:]{.,:]|<select[[:space:]>]|[[:space:]]title=[\"']|role=[\"']switch|(toggle|switch)[[:alnum:]_-]*(thumb|knob)|^[[:space:]]*\.[[:alnum:]_-]*(spinner|loader|badge|chip|pill|tabs?)[[:space:]{.,:]|role=[\"']tab[\"']|^[[:space:]]*(input|textarea)[[:space:]{.,:[]" $inc --include=*.css --exclude-dir=tests apps
scan attr-escape "=[[:space:]]*[\"'](\\\$\{|[\"'][[:space:]]*\+)[^}]*escapeHtml\(" $inc apps
scan ready-signal "\"reveal\"[[:space:]]*:|lucidos\.ui\.ready\(" --include=manifest.json $inc apps
scan app-icon "\"icon\"[[:space:]]*:" --include=manifest.json apps
scan widget-tile "\"(sizes|transparent)\"[[:space:]]*:" --include=manifest.json apps
scan touch-hover ":hover" $inc --include=*.css apps
scan left-stripe "border-(left|inline-start)(-color|-width)?[[:space:]]*:|box-shadow[[:space:]]*:[^;]*inset" $inc --include=*.css apps
scan state-as-action "classList[[:space:]]*\.[[:space:]]*(toggle|remove|replace)\([^)]*action-btn|action-btn-(secondary|confirm|danger)" $inc apps
```

## Checks


- **Apps writing the landing setting.** Only a person can designate the lander in Settings → Coding Agents.
- **Device-scoped public webhook URL writes.** Flag a write to `webhook_public_url` that supplies a device ID. Use the workspace preference with no device ID.
- **Remote app documents.** Standalone remote tabs are opaque too; their scoped file passes retain the remote URL alias.
  Check private operations against `system-knowhow/js-sdk/remote-workspace-client-contract.md` § Remote workspace client contract.
  The generated client version does not add an SDK method or broaden app authority.

- **Apps invoking host-log transfers.** Flag calls to `/engine/host-log-transfer` as **broken**; the routes require agent provenance.
  See `system-knowhow/js-sdk/lucidos-apiurl.md` § Not every endpoint is reachable.

- **Video previews retaining idle decoders.** Check custom players against `system-knowhow/js-sdk/lucidos-media.md` § `lucidos.media`.
  Flag a loaded video used only for an idle preview as **smell**; recommend a poster and loading on intent.
  Flag canvas capture of workspace video inside an opaque frame as **broken**; use the host poster helper.
  Poster failure must preserve a usable play control, including external clips whose servers refuse CORS.

- **Plan stages derived from code landings.** Apps consuming planning facts must distinguish plan-only changes from code.
  Match plan paths and approval scope; use `system-knowhow/thread-events.md` for stage evidence.
  A fix marker starts implementation without recording an owner decision.
  When a thread reports `plan_changed`, show that restoring the approved text reuses the approval.
  When a thread reports `plan_unreadable`, show that verification failed and can be retried.

- For app-owned scripts that write the workspace database, apply `system-knowhow/workspace-audit/scripts`' origin-aware table-policy check.

- **Client-derived thread liveness.** Use `lucidos.threads.liveness()` or the `liveness` on list rows, as documented in `system-knowhow/js-sdk.md`.
  Flag apps deriving working, quiet or stalled from timestamps as **stale**.

- **An app trying to clear a provider hold.** Flag calls to `/models/spend-cap-holds` and its clear route as **broken**.
  These routes are host-only; see `system-knowhow/js-sdk/lucidos-apiurl.md` § Not every endpoint is reachable.
  Route the user to Settings → Providers instead.


- **Capture fallback.** Screenshot failure still returns a DOM snapshot. Do not require a nonempty screenshot to accept capture.

- **Calls retained across a workspace restore.** Check custom request wrappers and SSE handlers against `system-knowhow/js-sdk.md` restore-generation rules.
  Flag replay of stale mutations as **broken**. Check that refused calls reach the app's error UI.

For each finding, capture: **location**, **what's wrong**, **which reference owns the rule** (link, don't quote), **suggested fix**.

Per `system-knowhow/js-sdk.md`:

- Model, effort and provider controls use `lucidos.ui.ModelPicker`, rather than
  copying the host's lists. For model calls, use `callableOnly` and
  `lucidos.modelRequest` before sending through `lucidos.proxy`.
  Check that apps await readiness, surface load failures and destroy removed fields.
- `index.html` matches the current boilerplate (script order, which pieces are required vs optional).
- Every `lucidos.*` call used in app code appears in the SDK reference. Calls not listed are either deprecated or invented.
- For thread roles, recommend `lucidos.threads.users` and its `user` result.
  The older alias still works; mark it **stale**, per `system-knowhow/js-sdk.md`
  § "lucidos.threads.users: Driver and Watchers".
- **External-API calls from the iframe: USE `lucidos.proxy(name).fetch(path, init)`.** The engine forwards the request server-side, strips this side's headers, and injects the configured auth header from the credential store. The credential never reaches the iframe. Use a builtin provider when available; otherwise configure the backend in `data/config/apis.json`. Reference: `system-knowhow/js-sdk/lucidos-media.md` § `lucidos.proxy`, which owns the strip list.

  **DO NOT USE** either of the following. Flag each occurrence and recommend the SDK helper:

  - `fetch('http://...')` or `fetch('https://<external-host>/...')` from inside an iframe. Mixed-content / CORS blocks it; if it works, the credential is sitting in the iframe. Suggest a `data/config/apis.json` entry and `lucidos.proxy(name).fetch(...)`.
  - `fetch('/api/v1/proxy/<name>/...')`: the SDK helper's wire format, bypassing the helper. The proxy name becomes a typo-prone magic string, and future SDK-side handling (timeouts, retries, response parsing, error shape) won't apply. Suggest `lucidos.proxy('<name>').fetch(path, init)`.

  A credential header written into app code belongs to the `cross-cutting` section.

- **The app calling the engine with its own `fetch`.** Walk `data/apps/**/*.{js,ts,html}` for any engine call JavaScript makes itself: `fetch('/api/v1/events/query')`, `new URL('api/v1/events/query', document.baseURI)`, `new EventSource('/api/v1/events')`, `fetch(lucidos.apiUrl('/<suffix>'))`, and any `location.pathname`-splicing that rebuilds the workspace address. In the host shell none reaches the engine: the frame's origin is opaque and CORS refuses it. Severity **broken**. WebKit reports `Load failed` and Chromium a `window.TypeError`. The failure usually lands in a falling-back `catch`, so the symptom is stale data, not an error.

  **`lucidos.apiUrl` is in that list deliberately.** Where an SDK method covers the endpoint, name that method. Otherwise the remedy is `lucidos.request('/<suffix>', init)`, which travels the bridge (ADR 0231). Never recommend `apiUrl` for a call: that moves a working app onto a pattern that cannot run. Reference: `system-knowhow/js-sdk/lucidos-apiurl.md` § `lucidos.request`.

  **A route an app may not reach has no remedy, and saying so is the finding.** The engine classifies every route, and `lucidos.request` refuses the rest with a 403 in both realms. Credentials, thread contents, consent routes and platform control are denied on purpose. Report the call and what it reaches for, and invent no way around. Reference: `system-knowhow/js-sdk/lucidos-apiurl.md` § "Not every endpoint is reachable".

  Workspace routing endpoints (`/workspace-operations`, `/workspace-operations/receive` and `/workspace-operations/receive-start`) also refuse app bridge calls. Report an app attempting any of them as **broken**, with no app-side replacement.
  The live diagnostic log query also refuses app bridge calls for every source.

  An `/api/v1/` path in a markup `src` / `href` attribute is correct and must NOT be flagged, nor is `apiUrl` building one. Five are exempt from the gateway's device gate: `sdk.js`, `sdk-prefs.js`, `sdk-iframe.css`, `sdk-iframe-audio.js`, and anything under `fonts/`. Behind a gateway, any OTHER `/api/v1/` path in a tag is refused: the frame's pass to its own files reaches no engine route. Severity: **broken**, and the remedy is the `lucidos.*` method that covers it.

- **What an isolated app frame can no longer do.** In the host shell an app runs at an opaque origin, in its own renderer process, so it cannot freeze or read the shell. The price: the frame's own `fetch`, `window.EventSource` and browser storage all fail, and the SDK carries those three over a bridge. The bridge runs three calls at a time across all apps, so an app that fires one call per row only slows itself. Reference: `system-knowhow/js-sdk/remote-workspace-client-contract.md` § Setup, and *app frame* / *app bridge* in `docs/glossary.md`. Walk `data/apps/**/*.{js,ts,html}` for code that goes around it, skipping any vendored `node_modules/` tree:

  - **Browser storage touched directly**: `localStorage`, `sessionStorage`, `document.cookie`, `indexedDB`. Each throws a browser exception with `name="SecurityError"` in the frame. The remedy is in § Rewriting an app for an isolated frame. Report the two shapes apart, because they fail differently:
    - **broken** where the call sits outside a `try`. The throw stops the script, so the app renders nothing.
    - **stale** where a `try` / `catch` wraps it, the common shape written for Safari private mode. The app runs and silently stops remembering anything.
  - **A read of the host realm**: `window.parent`, `parent.document`, `window.top`, or the device id lifted out of the shell's storage. All blocked. Severity: **broken**.
  - **`<a href="<the app's own file>" download>`**, in an app whose `index.html` loads no `/api/v1/sdk.js`. A browser ignores `download` on a cross-origin link, so the click navigates the frame to the file. Severity: **broken**.
  - **The camera or the microphone.** `navigator.mediaDevices.getUserMedia`. Both browsers refuse media capture to an opaque origin, whatever the frame is granted, so no in-frame remedy exists. Severity: **broken**. Say the app has to run in its own tab. Reference: `system-knowhow/js-sdk/remote-workspace-client-contract.md` § Setup, which lists what the frame is granted.
  - **The OS share sheet.** `navigator.share` called directly. The frame is not granted `web-share`, and iOS refuses the delegation anyway. Severity: **broken**. The remedy is `lucidos.ui.openExternal(url)`, which opens the link from the frame where the host allows it, and through the host otherwise. A hit inside a vendored `sdk.js` is the SDK's own fallback, so read the call site first.
  - **Session-history URL writes that leave the document.** `history.replaceState`, `history.pushState`, or assignment to `location.href` / `window.location` / `location.assign()` / `location.replace()`. The frame is sandboxed without `allow-same-origin`, so WebKit refuses a history write that names another path. That covers the iOS app and the desktop client. The error reads "Paths and fragments must match for a sandboxed document". Read the URL argument at each hit before you report it:
    - **Fine**: `history.replaceState` / `pushState` with a relative `'#…'` or `'?…'`, or with the frame's own path. The SDK resolves a relative one against the frame's URL. `location.hash = …` is fine too.
    - **A finding**: a history write naming another path. Also `location.assign` / `location.href` / `location.replace` with a relative `'#…'`, which resolves against the stamped `<base href>` and reloads the app.

    Split the severity of a finding the way the failure does:
    - **broken** where the call sits outside a `try`. It throws, and anywhere in the render or boot path the app renders nothing.
    - **stale** where a `try` / `catch` wraps it. The URL stops reflecting state, so deep links out of the app break while nothing looks wrong.

    The remedy: keep the app's state in its fragment or query, written as above. Inbound deep links arrive on the fragment, so apply `location.hash` at boot and subscribe to `hashchange`. Reference: `system-knowhow/js-sdk/remote-workspace-client-contract.md` § Setup (what the frame is granted) and `system-knowhow/js-sdk/lucidos-ui-ui-control.md` § "fragment: opening at a place inside the app".
  - **A write to `/env-vars`**, through `lucidos.request` or the app's own `fetch`. The route opens `GET` only. A user env var reaches every command the agent runs, so a name the interpreter loads from would be host code execution. Severity: **broken**, since the call answers 403 and the app's settings never persist. § Remediation below carries the replacement.
  - **An app that tries to add trigger command grants.** Category grants have ended. An owner grants a program from a trigger grant review card or the blocked run's permission request. Flag app code that depends on minting either grant as **broken**. The same holds for app code that revokes a folder grant with `revoke_folder_grants`: the engine refuses an app frame, since only the owner revokes one.
  - The generated preference catalog also includes device appearance settings such as `agent-icon-colors`. These are writable; do not flag them as human-only.
  - A generated model fallback describes an unset preference, not a stored owner choice. An older stored model pin is not drift; see `preferences.md`.
  - **A write of a human-only preference**, through `lucidos.preferences.set` or a `PUT /preferences`. The route stays open, but two key classes are refused to an app. One the Lucidos Agent may not write either: `command_guard`, `coding_agent_auto_answer`, `max_tool_calls`, `local_base_url`, the judge and permission-answer settings and the `provider_enabled_*` switches. One the agent may write but an app may not: the coding-agent paths and permission mode, and `opencode_free_enabled`. These would let an app run its own script as the user, read local-model chat, or send chat to an anonymous relay. Severity: **broken** (403); § Remediation below carries the answer.
  - **A read or write of engine bookkeeping**, such as the last backup's outcome in `backup_last_run`. A preference read leaves it out, and an app's `PUT /preferences` naming it is refused. Severity: **broken** (403 on write, a missing key on read); § Remediation below carries the answer.
  - **The app's own bundled file, fetched by a relative path.** `fetch('data/song.json')`, ``fetch(`audio/clips/${name}.json`)``, any `fetch` whose first argument is a relative path. The frame's origin is opaque, so the browser refuses it like an engine call: `Load failed` in WebKit, a `window.TypeError` in Chromium. It reads as safe, and the engine-fetch pattern misses it, since nothing in the string says `/api/v1`. Split the severity the way the failure does:
    - **broken** where the throw escapes setup, or the fetch is the app's only data path. The app renders an error banner, or nothing.
    - **stale** where a `catch` falls back to `lucidos.data` and the app keeps running, minus whatever the bundled file carried.

    The remedy is `lucidos.data.read('apps/<app-id>/<path>')`, which travels the bridge. **`lucidos.data.url()` is not a remedy.** It builds a URL for a `src` or an `href`, and fetching one is refused the same way.
  - **A `<base href>` the app declares itself.** The first base in a document wins, so it replaces the pass the engine stamps for the app's own files. Behind a gateway every relative `src` / `href` then answers **401**. Severity: **broken**. Delete it: relative refs already resolve against the app's own directory. Reference: `system-knowhow/js-sdk/remote-workspace-client-contract.md` § Setup, and [ADR 0238](https://github.com/lucidos-dev/lucidos/blob/main/docs/adr/0238-app-frame-carries-a-capability-to-its-own-files.md).

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

- **Names from before the dashboard rename.** The Home board became two
  dashboards (ADR 0496). The `dashboard-rename` scan finds the old names.
  Judge each hit:
  - **broken**: the widget place `home`, in a `data-widget-place` selector or
    a `lucidos.widget.place()` comparison. A tile's frame now says
    `dashboard`, so the code meant for a tile never runs. Recommend
    `dashboard`.
  - **broken**: the navigate target `home-board`. It no longer exists.
    Recommend `dashboard`.

