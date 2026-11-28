---
name: Lucidos JavaScript SDK, Remote workspace client contract
description: Part of Lucidos JavaScript SDK. Its index lists the sections it covers.
---

One part of `system-knowhow/js-sdk`, which lists every part.

## Remote workspace client contract

The connecting gateway serves its own installed frontend for remote workspaces.
It checks both the remote gateway and its running engine for a compatible client contract.
The health response advertises `workspace_client`; this is separate from SDK method availability.
The generated client version adds no `lucidos.*` method or permission.

Remote app documents remain opaque, including standalone remote app tabs.
Their scoped file passes bind the serving workspace while URLs retain the remote alias.
Use the SDK bridge for private operations; public SDK assets and fonts remain readable.

## Setup

The engine serves app HTML as static content and injects nothing, except `?thread_id=` rewriting on WIP-preview requests. Apps opt into each piece they want.

The standard boilerplate:

```html
<!DOCTYPE html>
<html>
  <head>
    <meta charset="utf-8">
    <title>My App</title>
    <script src="/api/v1/sdk-prefs.js"></script>
    <link rel="stylesheet" href="/api/v1/sdk-iframe.css">
    <script src="/api/v1/sdk-iframe-audio.js"></script>
    <script src="/api/v1/sdk.js"></script>
    <script>
      lucidos.ui.applyPreferences();
      lucidos.ui.watchPreferences();
    </script>
  </head>
  <body>
    <!-- app content -->
  </body>
</html>
```

What each piece does (include only what you need):

| Tag | Provides | Skip if |
|---|---|---|
| `<title>` | Tab title | Never: browsers require it |
| `<script src="/api/v1/sdk-prefs.js"></script>` | Synchronous prefs script. Sets `data-theme-mode`, `--bg-primary` and `--font-ui` on `<html>` (plus `--user-ui-scale` when set) *before* any later stylesheet evaluates. The engine resolves this device's theme mode, theme, font and scale and serves them inside the script, so the frame needs no access to the shell's storage. It stamps `?device=` onto this one `src` and adds nothing to your document. It also carries the device's Autocorrect switch, so `sdk.js` knows it before any field takes focus. It sets `data-motion` (§ Reduced motion), `data-theme-effects` (§ Theme parts) and `data-font-bold` (§ Theme variables). This removes the flash of default theme before `applyPreferences()`. **Place it as early in `<head>` as possible: before `sdk-iframe.css`, any other `<link rel="stylesheet">`, and any inline `<style>` that reads theme vars.** The inline `--bg-primary` makes `background: var(--bg-primary, …)` paint even when stylesheets load asynchronously (JS-injected, dynamic `import()`, Vite dev mode). | App doesn't use `sdk-iframe.css` (no FOUC to fix) |
| `<link rel="stylesheet" href="/api/v1/sdk-iframe.css">` | Theme tokens (`--bg-primary`, `--accent`, etc.), dark/light variables, default body/input/scrollbar styling, **and Lucidos's shared component classes** (`.action-btn` + `.action-btn-confirm`/`.action-btn-danger`, `.button-group`, `.icon-btn`, `.label`, `.title`, `.segmented-control`/`.segmented-btn`, `.list-row*`, `.markdown-content`, `.progress-bar`, `.empty-state`, `.accent-link`). With these classes your controls render like the host shell. The body is set to `--font-size-sm`, the chat prose step. Inputs and buttons get `--font-ui` at `--font-size-md`, the host's step for labels and controls. The root font-size is the user's UI scale, and `1rem` is `--font-size-xl`, a section heading. Without these defaults, unsized text would read two steps larger than body. The page never scrolls sideways: `<html>` and `<body>` clip on the x axis, and images and video cap at `max-width: 100%` (`system-knowhow/building-an-app/scaffolding-defaults.md` § Responsive by default). | App ships its own complete stylesheet and doesn't want Lucidos theming |
| `<script src="/api/v1/sdk-iframe-audio.js"></script>` | Monkey-patches `AudioContext` so app code reuses a gesture-unlocked instance that survives iOS PWA background cycles. **Must be in `<head>` before any code that creates an `AudioContext`.** | App doesn't play audio |
| `<script src="/api/v1/sdk.js"></script>` | The `lucidos.*` API, plus iframe-only side effects that need no call from you. **Link interceptor:** `target="_blank"` links resolve in-frame, and external `http(s)://` and `//host` links go through `lucidos.ui.openExternal()`, whatever the case of the scheme. A link press that turned into a swipe never opens the link: the SDK cancels the click that follows a touch drag. **Shortcut forwarder:** host shortcuts (focus/hide a pane, narrow/widen, new thread, search, Escape) keep working while the app has focus. Apply and the voice call never run from an app, since an app's script could send their chords. A chord bound to a host shortcut has its browser default cancelled, so ⌘P opens file search, not print. Your handlers still get that key, marked `defaultPrevented`. Only modifier chords, Escape and the F-keys are forwarded. Plain typing stays in the app, and so does Ctrl plus a bare letter in a Mac text field. **Scale forwarder:** Cmd/Ctrl + scroll over the app steps the UI scale, as it does over the shell. To keep the gesture (say, to zoom a map), call `preventDefault()` on the wheel event. **Per-app scroll memory** across an app switch or reload. **Pull to refresh** (§ Pull to refresh). **Pane swipe** (§ Pane swipe). **Dynamic bars** (§ Dynamic bars). The Lucidos **tooltip** on any `data-tooltip` element (§ Tooltips, under lucidos.ui). The device's **Autocorrect switch** plus a key-code guard on text fields (§ Text fields and autocorrect). | App doesn't use `lucidos.*` |
| `lucidos.ui.applyPreferences()` | Reads the user's theme mode/theme/font/scale (resolving `system` to the live OS light/dark) and sets `data-theme-mode`, `data-font-bold` and CSS vars on `<html>`. Pairs with `sdk-iframe.css`. | **Don't skip if you include `sdk-iframe.css`**: without it the app ignores a light or system setting and stays dark. Skip only when opting out of Lucidos theming entirely. |
| `lucidos.ui.watchPreferences()` | Re-applies preferences live: on SSE `PreferencesChanged`, when the active theme's file or plugin changes, and under `system` when the OS appearance flips. The OS half watches `prefers-color-scheme` and the frame's own resume, on every platform. Inside the host shell the app also repaints with the shell, mid-drag included (§ lucidos.ui) | Static apps that have opted out of Lucidos theming |

**Hold the loading cover until your data is drawn.** An app that fetches
its first data after page load declares `"reveal": "on-ready"` in its
`manifest.json` and calls `lucidos.ui.ready()`. See § Showing the app once its
content is ready, under lucidos.ui.

**Inherit the theme by default.** A normal app includes the theme assets, calls `applyPreferences()` + `watchPreferences()`, and styles with the theme variables (below). It then follows the user's theme and appearance like the rest of Lucidos. The engine never injects these tags. An app that omits both `sdk-prefs.js` and `sdk-iframe.css` gets no `data-theme-mode`, no CSS variables and no default styling.

Opt out only for an app with its own complete visual identity (charts, games, embedded third-party UIs). Otherwise **hardcoding colors is a bug**: a light-mode workspace gets a dark-only app, or the reverse.

**The tab icon is the one tag the engine does add.** An app in its own browser
tab is a top-level document, and without a `<link rel="icon">` it shows a blank
glyph. So the engine stamps the Lucidos mark into the served `<head>` when your
HTML names no icon. Ship a `<link rel="icon" href="…">` (or
`rel="shortcut icon"`) and the engine keeps yours. Inside the host shell the
iframe has no tab, so nothing changes there.

**Reach the engine through the SDK, never through a bare `fetch`.** Inside the
host shell an app runs in its own renderer process, so a busy main thread slows
only that app. That isolation gives the frame an opaque origin. CORS refuses a
direct `fetch('/api/v1/…')` and `new EventSource(…)`, and `localStorage` throws.
`lucidos.*` carries the first two over a bridge to the host, so every call in
this document works unchanged. An app that goes around the SDK loses its network.

An endpoint with no namespace goes through `lucidos.request`, over the same
bridge. The engine says which endpoints an app may call and refuses the rest
(§ `lucidos.request`). Per-device state goes in `lucidos.storage.local` or
`.session`, which stand in for `localStorage` and `sessionStorage`
(§ `lucidos.storage`). State every device should see goes in `lucidos.data`.

**The bridge runs three calls at a time, fairly across apps.** A burst of calls
(a `lucidos.proxy` per list row, say) only queues your own work, so a click is
never stuck behind it. A `lucidos.ui` call, such as a navigation, goes first, past the queue. A call
that waits past its `timeout` is never sent. Fetch lists in a few calls rather
than one per row, because the slots stay three however you write it.

**Your own files load normally: a separate `app.js`, `style.css` or image is
fine.** The opaque origin costs your frame the device credential on every
subresource, and the gateway asks for it. So the engine gives your document a
short-lived pass to its own files and stamps it into a `<base href>`. Every
relative ref resolves through that, and `lucidos.data.url(path)` carries it too.
The host keeps the pass fresh while your app is open. See
[ADR 0238](https://github.com/lucidos-dev/lucidos/blob/main/docs/adr/0238-app-frame-carries-a-capability-to-its-own-files.md).

**The user's workspace font loads by itself.** When the user picks a font they
installed (`system-knowhow/workspace-fonts.md`), `sdk-prefs.js` and
`applyPreferences()` register its faces and set `--font-ui`. You write no code.

**A bundled font and an ES module load too.** Ship an `@font-face` pointing at
your own `.woff2`, or a `<script type="module">` with relative `import`s. Both
work everywhere, except that Chromium refuses the module on a dev engine opened
with no gateway. A `fetch()` of your own files never works: the engine grants a
load, never a read, so use `lucidos.data.read`. See
[ADR 0289](https://github.com/lucidos-dev/lucidos/blob/main/docs/adr/0289-app-frames-load-fonts-and-modules-across-origins.md).

**Do not declare your own `<base href>`.** The first base wins, so yours would
replace the pass and your files would stop loading behind a gateway. Relative
refs already resolve against your app's directory.

**Your app may write its own fragment and query.** These stay on your document:

- `history.replaceState(null, '', '#30d')` and `pushState`, with a relative
  `#…` or `?…`. The SDK resolves them against your frame's URL, not the base.
- An in-page link, `<a href="#section">`.
- `location.hash = '30d'`.

Avoid `location.assign('#30d')` and `location.href = '#30d'`. Both resolve
against the base and reload your app, so set `location.hash` instead. WebKit
refuses a write that names another path, because the frame is sandboxed.

The pass reaches your app's files and the workspace's `data/` tree, and nothing
else. A `system-knowhow/` path from `lucidos.data.url` routes through
`/api/v1/data/…`, an engine API route, so it still answers **401** behind a
gateway. Read those with `lucidos.data.read`.

**The shell delegates a short list of browser features, and denies the rest.**
A permissions-policy feature defaults to an allowlist of `self`, and an opaque
origin is not `self`. Today the shell delegates `autoplay`, `fullscreen`,
`encrypted-media` and `clipboard-write`. Media plays, fullscreen works, and
`navigator.clipboard.writeText()` works from a Copy button.

Every full screen request in a frame also writes `[Client/app-fullscreen]`
lines to the engine log: the request, the open or close, and a
`never-opened` line when WebKit accepts a request and shows nothing. Look there
when full screen does nothing. Your app calls nothing for this.

What your frame does not get, and why:

- **Reading the clipboard** (a choice). `navigator.clipboard.readText()` is
  refused, because a read would hand your app whatever the user last copied.
- **The camera and the microphone** (a browser limit). `getUserMedia` fails in
  a frame, because both browsers refuse media capture to an opaque origin. An
  app that needs either must run in its own tab.
- **The OS share sheet** (a browser limit). `navigator.share` is refused, and
  iOS refuses the delegation anyway. Call `lucidos.ui.openExternal(url)`, which
  opens the link in a new tab, or through the host where the host must choose.

Popups and OAuth are untouched. In its own browser tab an app is a top-level
document, not a frame, so it keeps every direct path.

**One link shape needs `sdk.js`:** `<a href="report.pdf" download>` on one of
your bundled files. A browser ignores `download` on a cross-origin link, and
your files are cross-origin to the frame, so the click would navigate the frame.
`sdk.js` intercepts it and asks the engine for the file as an attachment, at
`/<slug>/app/<id>/<file>?download=1`, which the pass reaches behind a gateway.
Without the SDK, use a `blob:` or `data:` URL, which downloads from any frame.

### Text fields and autocorrect

**`sdk.js` turns autocorrect off on your text fields while the device's
Autocorrect switch is off.** Every text `<input>` and every `<textarea>` gets
`autocorrect="off"`, including fields you add later. The stamp lands as each
field mounts, before its first focus, which is when iOS reads it. You write
nothing.

The reason is an iOS bug: while autocorrect holds a correction, iOS can swallow
a tap on a button below the text. A Save under a notes field then does nothing
until the keyboard closes. See
[ADR 0262](https://github.com/lucidos-dev/lucidos/blob/main/docs/adr/0262-ios-autocorrect-eats-the-send-tap.md).

The switch is the `autocorrect` preference, per device (§ lucidos.preferences).
Unset, it is on, on every device. A user who keeps hitting the dead tap turns
it off under **Settings → System → Debugging**, on iPhone and iPad only.

Three rules hold:

- **Only `autocorrect` changes.** `autocapitalize` and `spellcheck` stay yours.
- **Your own attribute wins.** A field that declares `autocorrect` keeps it, in
  either direction. Turning the switch back on removes only the SDK's stamps.
- **A change reaches a running app through `watchPreferences()`.** Without it,
  the app keeps the value it read at load until it reloads.

`sdk.js` reads the switch from `sdk-prefs.js` when you include it, otherwise
it starts on. One preference read at load then corrects it.

**`sdk.js` also refuses key codes typed as text.** In the desktop app, an arrow
key with nowhere to move the caret would otherwise type a square into your
field. The SDK cancels any insertion made entirely of control codes (tab and
line breaks excepted) or macOS function-key codes. Typing, pasting and emoji
are untouched, and you write nothing.

### Reduced motion

**Key your animations on `data-motion`, never on the media query.** Every app
frame gets `data-motion="reduce"` or `data-motion="full"` on `<html>`.
`sdk-prefs.js` sets it before first paint, and `sdk.js` sets it again from the
device's `motion` preference (§ lucidos.preferences). It already folds in the OS
switch: under `system` it follows the OS, and `reduce` or `full` override it.

```css
:root[data-motion="reduce"] .card { animation: none; transition: none; }
```

`@media (prefers-reduced-motion: reduce)` reads only the OS, so it ignores a
user who picked Reduce or Full in Lucidos. A change reaches a running app
through `watchPreferences()`.

The shared component classes already stand still under `reduce`: their
transitions run on the `--duration-*` tokens, which collapse to a single frame.
Your own transitions on those tokens collapse with them.

### Theme parts

An app that loads `sdk-iframe.css` paints the active theme's three frame parts
(`system-knowhow/themes/switch-themes.md` § Theme parts):

| Part | Paints | Properties |
|---|---|---|
| `app-text` | the `body` text | `text-shadow`, `letter-spacing` |
| `app-link` | every `a` | `text-decoration-color`, `text-shadow` |
| `app-control` | `input`, `textarea`, `select`, `button` and the SDK select | `border-color`, `box-shadow` |

The tokens arrive through your own tags: `sdk-prefs.js` at first paint, and
`applyPreferences()` live. An app that loads the SDK without `sdk-iframe.css`
gets the tokens and no visible effect. An app without the SDK gets nothing,
because the engine adds no stylesheet or script to your page.

**Opt out for the whole app** with `data-theme-parts="off"` on your `<html>`.
Every part token then resets, and the rest of the SDK styling stays. There is
no opt-in per part: the theme decides which parts it styles.

```html
<html data-theme-parts="off">
```

An element of your own may read a part token by name, with the fallback you
would paint without it: `text-shadow: var(--part-app-text-text-shadow, none)`.
`GET /api/v1/themes/parts` lists every token, and the names are a stable
contract.

**Key your own glows on `data-theme-effects`.** A frame that loads
`sdk-prefs.js` or calls `applyPreferences()` gets
`data-theme-effects="reduce"` or `"full"` on `<html>`, from the device's
`theme-effects` preference, before first paint and live. Under `reduce` the SDK
stylesheet drops every part shadow and filter:

```css
:root[data-theme-effects="reduce"] .badge { box-shadow: none; }
```

### Pull to refresh

**A pull past the top of your app reloads it, with no code from you.** On a
touch screen, `sdk.js` watches for a downward drag once the page is scrolled to
the top. A scroll up that reaches the top and keeps going becomes a pull from
that point. Lucidos draws the arrow and runs the header's Refresh reload, so
your app starts fresh and a WIP preview stays on its WIP.

It never blocks scrolling: every listener is passive. It stays out of the way
of a gesture that is not a pull:

- a drag while the page or a scrolled inner list is still below its top;
- a mostly sideways drag, such as a carousel swipe;
- a second finger, such as a pinch.

**To own the gesture yourself, claim it.** Either of these works:

- call `preventDefault()` on the `touchstart` or `touchmove`;
- give the element a `touch-action` that keeps the vertical pan, such as
  `touch-action: none` on a canvas or `pan-x` on a horizontal strip.

A drag that starts on a claimed element is never a pull. That covers a map, a
drawing surface or a drag handle built on Pointer Events, which never cancel
the touch.

### Pane swipe

**A sideways drag anywhere in your app swipes between Lucidos panes, with no
code from you.** On the phone layout, `sdk.js` watches for a mostly sideways
drag and posts it to Lucidos. Lucidos moves the panes as it does over a thread.
It ignores the drag on the desktop layout and while your app is fullscreen.
Your app may re-render the element under the finger mid-drag. The swipe still
follows the touch to its end.

Vertical scrolling stays native. Once a drag has locked sideways, `sdk.js`
cancels its `touchmove` events so the page does not drift under the swipe. It
stays out of the way of a drag that is not a pane swipe:

- a drag on a range input, or inside an element that scrolls sideways;
- a drag on an XL widget that Lucidos clamped and scrolls sideways;
- a drag while a text field has focus;
- a second finger, such as a pinch.

**To own a sideways drag yourself, claim it.** Either of these works:

- call `preventDefault()` on the `touchstart` or `touchmove`;
- give the element a `touch-action` that keeps the sideways pan, such as
  `touch-action: pan-y` on a carousel or `none` on a canvas.

On `<html>` or `<body>`, `touch-action: pan-y` is no claim. Page-wide, it
only stops the page panning sideways, and the pane swipe still works.
`none` there claims every drag in the app.

