---
name: Lucidos JavaScript SDK
description: API reference for the `lucidos` JS SDK. Namespaces: data, events, proxy, apiUrl, oauth, triggers, apps, preferences, storage, notifications, threads, ui, sse. Also the component classes and theme variables apps style with.
---

# Lucidos JavaScript SDK

The SDK is available as the `lucidos` global in app UIs (loaded via `<script src="/api/v1/sdk.js">`). The host frontend imports it from the `@lucidos/sdk` package directly.

> From a coding-agent subprocess, prefer the `lucidos` CLI for `data.*` and `events.*` operations. See [`lucidos-cli.md`](./lucidos-cli.md).

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
| `<link rel="stylesheet" href="/api/v1/sdk-iframe.css">` | Theme tokens (`--bg-primary`, `--accent`, etc.), dark/light variables, default body/input/scrollbar styling, **and Lucidos's shared component classes** (`.action-btn` + `.action-btn-confirm`/`.action-btn-danger`, `.button-group`, `.icon-btn`, `.label`, `.title`, `.segmented-control`/`.segmented-btn`, `.list-row*`, `.markdown-content`, `.progress-bar`, `.empty-state`, `.accent-link`). With these classes your controls render like the host shell. The body is set to `--font-size-sm`, the chat prose step. Inputs and buttons get `--font-ui` at `--font-size-md`, the host's step for labels and controls. The root font-size is the user's UI scale, and `1rem` is `--font-size-xl`, a section heading. Without these defaults, unsized text would read two steps larger than body. | App ships its own complete stylesheet and doesn't want Lucidos theming |
| `<script src="/api/v1/sdk-iframe-audio.js"></script>` | Monkey-patches `AudioContext` so app code reuses a gesture-unlocked instance that survives iOS PWA background cycles. **Must be in `<head>` before any code that creates an `AudioContext`.** | App doesn't play audio |
| `<script src="/api/v1/sdk.js"></script>` | The `lucidos.*` API, plus iframe-only side effects that need no call from you. **Link interceptor:** `target="_blank"` links resolve in-frame, and external `http(s)://` links go through `lucidos.ui.openExternal()`. **Shortcut forwarder:** host shortcuts (focus/hide a pane, narrow/widen, new thread, search, Escape) keep working while the app has focus. Apply and the voice call never run from an app, since an app's script could send their chords. A chord bound to a host shortcut has its browser default cancelled, so ⌘P opens file search, not print. Your handlers still get that key, marked `defaultPrevented`. Only modifier chords, Escape and the F-keys are forwarded. Plain typing stays in the app, and so does Ctrl plus a bare letter in a Mac text field. **Per-app scroll memory** across an app switch or reload. **Pull to refresh** (§ Pull to refresh). **Pane swipe** (§ Pane swipe). The Lucidos **tooltip** on any `data-tooltip` element (§ Tooltips, under lucidos.ui). The device's **Autocorrect switch** plus a key-code guard on text fields (§ Text fields and autocorrect). | App doesn't use `lucidos.*` |
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

The pass reaches your app's files and the workspace's `data/` tree, and nothing
else. A `system-knowhow/` path from `lucidos.data.url` routes through
`/api/v1/data/…`, an engine API route, so it still answers **401** behind a
gateway. Read those with `lucidos.data.read`.

**The shell delegates a short list of browser features, and denies the rest.**
A permissions-policy feature defaults to an allowlist of `self`, and an opaque
origin is not `self`. Today the shell delegates `autoplay`, `fullscreen`,
`encrypted-media` and `clipboard-write`. Media plays, fullscreen works, and
`navigator.clipboard.writeText()` works from a Copy button.

What your frame does not get, and why:

- **Reading the clipboard** (a choice). `navigator.clipboard.readText()` is
  refused, because a read would hand your app whatever the user last copied.
- **The camera and the microphone** (a browser limit). `getUserMedia` fails in
  a frame, because both browsers refuse media capture to an opaque origin. An
  app that needs either must run in its own tab.
- **The OS share sheet** (a browser limit). `navigator.share` is refused, and
  iOS refuses the delegation anyway. Call `lucidos.ui.openExternal(url)`, which
  opens the link through the host.

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
(`themes.md` § Theme parts):

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

Vertical scrolling stays native. Once a drag has locked sideways, `sdk.js`
cancels its `touchmove` events so the page does not drift under the swipe. It
stays out of the way of a drag that is not a pane swipe:

- a drag on a range input, or inside an element that scrolls sideways;
- a drag while a text field has focus;
- a second finger, such as a pinch.

**To own a sideways drag yourself, claim it.** Either of these works:

- call `preventDefault()` on the `touchstart` or `touchmove`;
- give the element a `touch-action` that keeps the sideways pan, such as
  `touch-action: pan-y` on a carousel or `none` on a canvas.

### Find in app

**Your app's text is searchable with no code from you.** The header's *Find*
button, or Mod+F while the app is open, rolls the find bar in above it.
Lucidos sends each query to `sdk.js`, which matches the rendered text,
highlights every match and scrolls the current one into view. Enter and
Shift+Enter step through the matches and wrap at the ends.

What it matches:

- visible text only, case-insensitive, across inline elements but not from one
  block into the next;
- not text in `script`, `style`, `template`, or a form field's value;
- not an element hidden with `display: none` or `visibility: hidden`.

It never edits your DOM. The highlights are CSS Custom Highlights named
`lucidos-find` and `lucidos-find-current`, styled by `sdk-iframe.css` from the
theme tokens. Without that stylesheet the current match shows as the selection.
An app that loads no `sdk.js` cannot be searched, and the bar says so.

The count is taken when the reader types or steps. Content your app renders
later is counted on the next keystroke.

### Theme variables

`sdk-iframe.css` defines these CSS custom properties on `<html>` and flips them between light and dark. The `data-theme-mode` attribute drives them: `applyPreferences()` sets it (resolving `system` to the OS setting) and `watchPreferences()` keeps it in sync. Style with `var(--name)` and your app tracks the user's appearance. The canonical values live in the engine's `sdk-iframe.css`. **The names are the contract**:

| Group | Variables |
|---|---|
| Backgrounds | `--bg-primary`, `--bg-secondary`, `--bg-tertiary`, `--bg-quaternary`, `--bg-hover`, `--bg-selected` |
| Text | `--text-primary`, `--text-secondary`, `--text-muted`, `--text-on-accent`, `--text-strong` (bold text where the UI font has no bold face). `<html>` carries `data-font-bold="none"` for such a font and `"face"` otherwise, and `sdk-iframe.css` then paints `strong` and `b` inside `.markdown-content` with it. Key your own bold on the same attribute: `html[data-font-bold="none"] .my-label b { color: var(--text-strong); }` |
| Border | `--border-color` |
| Accents | `--accent`, `--accent-light`, `--accent-green`, `--accent-yellow`, `--accent-red` |
| Focus | `--focus-ring`: a ready-made `box-shadow` value (a soft accent band) for focus indicators. The `.action-btn`/`.icon-btn` classes use it, and your controls match the host with `:focus-visible { box-shadow: var(--focus-ring); }`. `--focus-ring-width` (`0.1875rem`) is the band's width. The band paints outside the control, so a box that scrolls or clips (`overflow` other than `visible`) cuts it at its edge. Give such a box that much edge padding, and hand it back with a negative margin if nothing should move. |
| Shadows | `--shadow-sm`, `--shadow-md`, `--shadow-lg` |
| Shape | `--radius-control` (`0.5rem`), `--radius-surface` (`0.75rem`), `--radius-round` (`999px`): the theme's corner steps. The shared component classes round with them, so a square theme squares them. Scale one with `calc()` to follow the theme in your own CSS. |
| Layout (theme-independent) | `--font-ui`, `--font-mono`, `--font-features-text`, `--font-features-code`, `--transition`, `--user-ui-scale`, plus the spacing / radius / motion scales below |
| Stacking | `--z-tooltip` (`10000`), the layer the built-in tooltip paints on. Keep your own overlays under it, so a tooltip is never covered. |

The user's **theme** retunes these values on top of the light and dark defaults, so style with the variables rather than copying their values. An app that lists or builds themes uses the routes in `themes.md` § For apps.

The user's UI font is **`--font-ui`**, the canonical token. It is set live to the
user's font pick, or to the theme's font when they follow the theme (the
default). `sdk-iframe.css` already sets `body { font-family: var(--font-ui) }`,
plus inputs and `.action-btn`, since form controls don't inherit the page font.
So any inheriting element gets the right font. A *bare* unclassed `<button>`
keeps the browser's control font: one more reason to use `.action-btn`.

Re-declare `font-family` only to override it deliberately, with `var(--font-ui)`.
`--font-family` and `--font` are tolerated **aliases** of `--font-ui`, so a
guess still resolves to the user's font. Write `--font-ui`.

**`--font-features-text` and `--font-features-code` carry programming ligatures,
and only code gets them.** Fira Code (the default), JetBrains Mono and Cascadia
Code ship programming ligatures. With one of them as the UI font, the two
resolve to `"liga" 0, "calt" 0` and `"liga" 1, "calt" 1`. For every other font
both are `normal`. `sdk-iframe.css` applies the text one on `html, input,
textarea, select, button` and the code one on `code, pre, kbd, samp`. So a code
block ligatures `=>` and `!=` while prose and form fields render literally.

Apply one yourself only on an element that shows code but is none of those tags:
`font-feature-settings: var(--font-features-code, normal)`. Never put the CODE
one on `:root`, `html` or `body`. `font-feature-settings` is inherited, and Fira
Code's `calt` re-spaces dot runs so a typed `...` reads as two dots.

Two traps in your own rules. Both look fine in DevTools and neither shows in the
computed value:

- **`normal` does not mean "ligatures off".** `liga` and `calt` are default-ON
  features in CSS, so `normal` renders identically to `"liga" 1, "calt" 1`.
  Use `var(--font-features-text, normal)` when you want them off, never a bare
  `normal`, and never expect deleting a declaration to disable anything.
- **Form controls do not inherit this property.** The UA stylesheet's `font`
  shorthand resets it on `input` / `textarea` / `select` / `button`, which is why
  they are named explicitly above. A custom control of your own needs the same
  treatment.

The spacing, radius, motion, icon and type scales are theme-independent, with
fixed values. **Use the token, not a magic number, and never a `px` fallback
that disagrees with the real value.** `var(--space-xl, 28px)` is a latent bug,
since `--space-xl` is `1.5rem` = 24px. With `sdk-iframe.css` these are always
defined, so a fallback is dead noise at best:

| Token | Value | | Token | Value |
|---|---|---|---|---|
| `--space-xs` | `0.25rem` (4px) | | `--radius-sm` | `0.25rem` (4px) |
| `--space-sm` | `0.5rem` (8px) | | `--radius-md` | `0.375rem` (6px) |
| `--space-md` | `0.75rem` (12px) | | `--radius-lg` | `0.5rem` (8px) |
| `--space-lg` | `1rem` (16px) | | `--icon-size-sm` | `0.875rem` (14px) |
| `--space-xl` | `1.5rem` (24px) | | `--icon-size-md` | `1rem` (16px) |
| `--duration-fast` | `0.15s` | | `--icon-size-lg` | `1.25rem` (20px) |
| `--duration-normal` | `0.2s` | | `--duration-slow` | `0.3s` |
| `--duration-emphasis` | `0.5s` | | `--duration-scale` | `1`, or `0.001` under reduced motion |
| `--spinner-weight` | `0.125rem` (2px), the `.mini-spinner` ring | | | |

Each `--duration-*` above is its listed value times `--duration-scale`, which is
`1` inside an app. The host's debugging slider for its own copy never reaches
you, since a custom property does not cross into an iframe. Under reduced motion
the scale drops to `0.001`, and every transition on these tokens ends inside a
frame. Set `--duration-scale` on your own `:root` for a knob of your own.

**Type scale: `--font-size-*`.** The host shell and this stylesheet both size
text from these, so use the token instead of a raw `rem`. Every step is in
`rem`, so it scales with the user's UI-scale preference.

| Token | Value | Role | | Token | Value | Role |
|---|---|---|---|---|---|---|
| `--font-size-3xs` | `0.5625rem` (9px) | micro-label / tiny badge | | `--font-size-lg` | `0.875rem` (14px) | emphasis |
| `--font-size-2xs` | `0.625rem` (10px) | dots, micro-meta | | `--font-size-xl` | `1rem` (16px) | section heading |
| `--font-size-xs` | `0.6875rem` (11px) | dense metadata | | `--font-size-2xl` | `1.125rem` (18px) | larger heading |
| `--font-size-sm` | `0.75rem` (12px) | body text, the chat prose step | | `--font-size-3xl` | `1.25rem` (20px) | large heading |
| `--font-size-md` | `0.8125rem` (13px) | labels, controls, row titles | | `--font-size-display` | `2.25rem` (36px) | hero |

```css
.card {
  background: var(--bg-secondary);
  color: var(--text-primary);
  border: 1px solid var(--border-color);
  border-radius: var(--radius-md);
  padding: var(--space-lg);
}
.card a { color: var(--accent); }
```

#### Respect the user's font size: size in `rem`, never `px`

The user's UI-scale preference is the **root font-size**
(`html { font-size: var(--user-ui-scale, 100%) }`), so **only `rem`/`em` units
scale with it.** An app sized in `px` ignores the user's font-size setting.
That is the most common "the app doesn't respect my font size" bug. Size
everything in `rem` (px / 16: 14px → `0.875rem`, 24px → `1.5rem`), and prefer
the `--space-*` / `--radius-*` tokens for spacing and corners. `1px` borders are
the one acceptable `px` exception, as in the host shell.

For text, prefer the `--font-size-*` tokens over a raw `rem`. Body text is
`--font-size-sm` (12px), the chat step. Labels, controls and row titles are
`--font-size-md` (13px). Small/meta is `--font-size-xs` (11px), emphasis
`--font-size-lg` (14px), and headings use the `h1`–`h6` defaults
`sdk-iframe.css` ships.

**The body step is already the default.** `sdk-iframe.css` sets
`body { font-size: var(--font-size-sm) }`, so an unsized paragraph reads at the
size of the chat beside it, not at the raw root (`1rem`). Don't raise it for a
report: a larger body reads as too big beside the chat. Never reset it to `1rem`
or a `px` value, which reads two steps larger than the rest of Lucidos.

### Component classes

`sdk-iframe.css` also ships Lucidos's shared component layer: the **same CSS the
host shell uses, from one source**. The engine appends
`crates/lucidos-app/src/styles/global/shared-components.css` (which the host
imports via `global.css`) to the served stylesheet. Apply these class names and
your controls render like the rest of Lucidos and track the theme and UI scale.
The class names are the contract:

| Class | Use for |
|---|---|
| `.action-btn` (+ `.action-btn-confirm` green, `.action-btn-danger` red) | The filled primary CTA button: blue, with the confirm/danger variants additive (`class="action-btn action-btn-danger"`) |
| `.action-btn-secondary` | A neutral, outlined button for a lower-emphasis action beside a primary CTA. Additive: `class="action-btn action-btn-secondary"`. **Use this instead of hand-rolling an off-palette outlined button.** |
| `.button-group` | Wrap a **row of buttons** in this instead of a bare flex row. Buttons that do not fit stack onto a second row rather than overflowing. A single button wider than the row ellipsizes rather than being sliced by an ancestor's hidden overflow. Set your own `justify-content` on the same element (the class sets none), and the buttons keep their natural widths. |
| `.icon-btn` | A small borderless icon button (wrap an SVG sized via `--icon-size-sm`). `disabled` fades it and drops its tooltip. `aria-disabled="true"` only drops the hover wash, so a busy button keeps its tooltip. |
| `.accent-link` | An inline text link/button in the accent color |
| `.label` (+ `.label-success`, `.label-warning`, `.label-error`, `.label-neutral`) | The host's small uppercase chip for a status or a category. The bare class is the accent tone, and the tones are additive: `class="label label-success"`. They use the toast's words, so a status reads the same in a chip and a toast. **Use this instead of drawing your own badge, chip or pill.** A pill the user taps is a button, not a label: see `.pill-bar`. |
| `.title` | A list/panel/modal title |
| `.segmented-control` + `.segmented-btn` (`.active`) | A toggle button group: two or three mutually exclusive options with one picked. Not page navigation, and not a long strip: past a handful of segments it wraps and reads as options to weigh, not places to go. Use `.pill-bar` as tabs to switch views, or a list of rows to go somewhere. It wraps when the segments pass their container, so each label stays on one line. |
| `.pill-bar` + `.pill-bar-btn` | A row of pills with one picked, as in Settings > Theme. The markup tells two uses apart. **Tabs** that switch views: `<div class="pill-bar" role="tablist">` holding `<button class="pill-bar-btn" role="tab" aria-selected="true">`. A **filter** that narrows one list: `role="group"` with `aria-pressed` on each button. Set the attribute to `true` on the picked pill, and the bar marks it. It stays on one line and scrolls sideways when the pills do not fit. On a touch screen each pill takes taps `--pill-bar-hit-slop` (0.5rem) above and below itself without growing. The bar pads out room for that reach and for focus rings (`--focus-ring-width`). A negative margin hands back the room above and below, so the pills sit `--focus-ring-width` in from the bar's sides. If you pad the bar yourself, keep both allowances, or a ring or the reach is cut off. **Use this instead of drawing your own tabs or filter pills.** |
| `.list-rows`, `.list-row`, `.list-row-info`, `.list-row-name`, `.list-row-actions`, `.list-section-title`, … | List/row layouts |
| `.list-row-add-card` (+ `.list-row-add-icon`, `.list-row-add-label`) | The "+ Add <thing>" row that closes a list. **Put it on a `<button type="button">`**, not a clickable `<div>`. The class carries the UA button reset and a `:focus-visible` ring. On a button the card is in the tab order and answers Enter and Space, and on a div only a pointer reaches it. Markup is `<button class="list-row-add-card"><span class="list-row-add-icon">+</span><span class="list-row-add-label">Add Thing</span></button>`. |
| `.list-row-details` (+ `.list-row-details-prose`) | The small muted line under a row title. The base class is a flex row of metadata fields, and its 0.75rem gap IS the separator. So a **sentence** takes the additive prose variant (`class="list-row-details list-row-details-prose"`). Under the bare flex class every inline `<strong>`/`<code>` becomes a flex item, which opens gaps mid-sentence and strands the following punctuation on the next line. |
| `.markdown-content` | A container for rendered markdown (headings, tables, code, blockquotes) |
| `.table-scroll-wrapper` | Wrap a `<table>` inside `.markdown-content` in this. A table always fits its container and wraps its cells, so this is a safety net. It catches a single token wider than the container and scrolls it inside the wrapper instead of widening your iframe body. Cells are capped at `60ch`, so one prose column cannot starve the key column beside it. |
| `.image-scroll-wrapper` | Wrap an `<img>` inside `.markdown-content` in this. An image cannot reflow, so this is the normal path. The wrapper stays within your container width and pans an oversized image sideways inside itself, instead of widening the body. The image keeps its natural width (no `max-width` cap to shrink a screenshot to a thumbnail) and is capped at `24rem` tall, aspect ratio preserved. A smaller image renders unchanged, with no scrollbar. These rules leave a bare unwrapped `<img>` alone. |
| `data-stack` + `data-label` (attributes, not classes) | Opt a wide table into the stacked mobile layout: `data-stack` on the `<table>` and `data-label="<column header>"` on every `<td>`. At 768px and under, each row becomes a card, the header row hides, and each cell shows its `data-label` above its value. Worth it from about 4 columns up. Below that the scroll wrapper reads better. |
| `.progress-bar` + `.progress-bar-fill`, `.progress-label` | A progress indicator |
| `<input type="checkbox">` (element, no class) | A plain checkbox already renders as the Lucidos checkbox: a soft accent-tinted box with a tick that draws on, sized in `em` to its row's text, identical in every browser. The `indeterminate` DOM property shows a dash. Put it in a `<label>` with its text and set no width or height on it. |
| `.text-input` | A free-text field, the box every host text field uses: `<input class="text-input">` or `<textarea class="text-input">`. Gives the themed background, border, radius, placeholder colour and focus ring. |
| `.toggle-switch` + `.toggle-slider` | An on/off switch, the one Settings draws. Markup is `<label class="toggle-switch"><input type="checkbox" role="switch"><span class="toggle-slider"></span></label>`: the real checkbox stays in the markup and carries the state, so read and set `checked` as usual. `disabled` on the input dims it, and `toggle-switch-disabled` on the label adds the not-allowed cursor. Give it an accessible name with an `aria-label` on the input, or a visible `<label for>`. |
| `.mini-spinner` | The spinning ring the host shows for a working state, such as a save in flight: `<span class="mini-spinner" aria-hidden="true"></span>` beside text that says what is happening. It stops under reduced motion and stays drawn. Recolour it with `--spinner-color`, for example `style="--spinner-color: currentColor"` inside a button. A busy button is a disabled `.action-btn` holding the ring and its label. Loading data draws a skeleton or nothing, never a spinner. |
| `.empty-state`, `.error-text` | Empty/error placeholders |
| `data-tooltip` (an attribute, plus the `#tooltip` rules that paint it) | A themed Lucidos tooltip on any element. Write the attribute, and `sdk.js` builds, positions and paints the box (§ Tooltips, under lucidos.ui). |

Prefer these over hand-rolled buttons and rows. A plain unclassed `<button>`
gets a neutral default that does **not** match Lucidos's primary blue button.

**Four of those rows are the overflow half of a wider rule.** `.button-group`,
`.table-scroll-wrapper`, `.image-scroll-wrapper` and `data-stack` each contain
one thing that would otherwise widen the page. They do not make an app
responsive. `data-stack` is one of the stylesheet's two width breakpoints, and
the other only tightens markdown-table type, so every other width decision is
yours. The rules, and the three regions the host paints over a fullscreen app,
are in `system-knowhow/building-an-app.md` § Responsive by default.

`lucidos._capture()` needs nothing from your app. The SDK draws the page through an SVG `foreignObject`. So the browser renders the snapshot, and any CSS it paints comes out right, CSS Color 4 included. It embeds your fonts and images by reading them over the host bridge. One it cannot read (another host, or a route outside the app reach table) paints empty.

When the screenshot fails for any reason, the capture degrades to **DOM-only** rather than throwing. It returns an empty `screenshot` plus a `dom` layout snapshot (element positions + classes), prefixed with the failure reason. So the agent still sees the layout.

External-host apps point `baseUrl` at the Lucidos instance with `lucidos.configure`:

```ts
lucidos.configure(opts: { baseUrl?: string; token?: string }): void
```

```js
lucidos.configure({ baseUrl: 'https://your-lucidos.example' });
```

`baseUrl` overrides the auto-derived workspace base path. In-app iframes don't
need it: the SDK reads the gateway prefix from `<base href>` / the `/app/` URL.
`token`, when set, goes out as an `Authorization: Bearer <token>` header on every
SDK request, for embedders calling a remote engine that requires auth. Both are
optional, and each call merges into the existing config.

## Error Handling

An async method that reaches the engine and gets a non-2xx answer throws
`SdkError`:

```js
class SdkError extends Error {
  httpCode: number;
  reason: string;
}
```

A call that never gets an answer rejects with a `DOMException` instead, and the
`name` says which kind of nothing you got:

| `err.name` | Meaning | What to do |
|---|---|---|
| `TimeoutError` | The SDK's own 10s deadline fired. | Treat as retryable. The request may or may not have reached the engine. |
| `AbortError` | Something cancelled the request: an `AbortSignal` you passed in `init`, or the browser tearing down an in-flight fetch. | If you did not cancel it yourself, treat as retryable. |

The `AbortError` case is routine on an installed iOS PWA: WebKit aborts every
in-flight fetch when it suspends the page. Retry an idempotent call rather than
reporting a failure. Retry when the page comes back (`visibilitychange`,
`pageshow`, `focus`), not immediately, because a suspended page cannot reach
the engine either.

WebKit rejects an aborted fetch with a generic `AbortError` rather than the
signal's reason. So the SDK re-stamps a fired deadline as `TimeoutError`, as
Chrome and Firefox deliver it. A cancel you requested stays an `AbortError`,
even when the deadline fired in the same instant.

## lucidos.data: File Operations

Read, write, and manage files in the workspace `data/` directory.

> **Paths are relative to `data/`, not `data/artifacts/`.** App code lives in `apps/{id}/`, but app *data* goes under `artifacts/` explicitly, e.g. `artifacts/{app-id}/data.json`. Omitting the prefix gives a 404 `SdkError` from `read` and a silent failure from `write`.

```ts
lucidos.data.read(path: string): Promise<string>
lucidos.data.write(path: string, content: string): Promise<WriteResult>
lucidos.data.delete(path: string): Promise<void>
lucidos.data.list(pattern?: string): Promise<string[]>
lucidos.data.url(path: string): string   // synchronous, returns URL
lucidos.data.edit(path: string, operations: EditOperation[]): Promise<void>
lucidos.data.upload(file: File): Promise<UploadResult>  // 120s timeout
```

### Types

```ts
interface WriteResult { success: boolean; commit?: string }
interface UploadResult { success: boolean; filename?: string; error?: string }
interface EditOperation {
  json_path?: string;   // JSON path edit (see syntax below)
  json_value?: unknown;
  find?: string;        // Text find-replace edit
  replace?: string;
}
```

#### `json_path` syntax

Mix any of these forms in a single path:

| Form                              | Example                            | Resolves to                  |
|-----------------------------------|------------------------------------|------------------------------|
| Dot notation                      | `metadata.author.name`             | `/metadata/author/name`      |
| Array index                       | `sections[1]`                      | `/sections/1`                |
| Quoted key (double or single)     | `dailyLog["2026-05-04"]`           | `/dailyLog/2026-05-04`       |
| JSONPath root marker              | `$.streak`                         | `/streak`                    |
| Raw JSON Pointer (RFC 6901)       | `/sections/1/title`                | `/sections/1/title`          |
| Mixed                             | `habits[0].dailyLog["2026-05-04"]` | `/habits/0/dailyLog/2026-05-04` |

Use **quoted keys** for any key that isn't a bare identifier: dates (`"2026-05-04"`), slugs with dots (`"foo.bar"`), or anything with spaces. Inside a quoted key, `\` escapes the next character. RFC 6901 escaping (`~` → `~0`, `/` → `~1`) is applied automatically.

### Examples

```js
// Read a JSON file
const raw = await lucidos.data.read('artifacts/habits/data.json');
const data = JSON.parse(raw);

// Write content
await lucidos.data.write('artifacts/notes.md', '# My Notes\nContent here.');

// List files matching a pattern
const csvFiles = await lucidos.data.list('artifacts/imported/**/*.csv');

// Edit JSON in-place
await lucidos.data.edit('artifacts/habits/data.json', [
  { json_path: '$.streak', json_value: 5 }
]);

// Edit a key whose name isn't a bare identifier (here: an ISO date)
await lucidos.data.edit('artifacts/habits/data.json', [
  { json_path: 'habits[0].dailyLog["2026-05-04"]', json_value: 3 }
]);

// Get a URL for embedding in HTML
const src = lucidos.data.url('artifacts/screenshots/latest.png');
```

### `url` and app-bundled assets

`lucidos.data.url(path)` normally returns a `/data/...` URL, which serves from the live workspace. Inside an app iframe (`/app/<id>/...`), a `path` in the app's own folder (`apps/<id>/<rest>`) instead returns `/app/<id>/<rest>` and carries over the iframe's `?thread_id=`. So JS-set asset URLs (e.g. `img.src = lucidos.data.url('apps/my-app/icon.png')`) load in WIP-preview. The engine's HTML rewriter covers only markup `src` / `href` attributes, so a JS-set source would otherwise 404 against the live workspace. Cross-app references (`apps/<other>/...`) and non-app paths (`artifacts/...`, `knowhow/...`) keep the `/data/` route.

A `system-knowhow/...` path routes through the engine's `/api/v1/data/...` endpoint, because those files live in the engine repo and the static `/data` mount can't serve them.

Behind a gateway, a returned URL carries your frame's pass to its own files, so it loads like any other subresource. The `system-knowhow/` case is the exception: a pass never reaches an `/api/v1` route, so it answers **401** there. Read those with `lucidos.data.read`. See § Setup.

The pass behind a URL lasts an hour, and `url()` reads the current one on every call. So build the URL where you use it rather than caching the string. An `<iframe src>` left open past the hour needs its `src` rebuilt before an in-page link inside it works again.

The same applies to a URL the browser captured at load. A dynamic `import()` resolves against its module's URL, and a stylesheet's `url()` against the stylesheet's. Both keep the pass they loaded with, so a chunk first imported an hour in answers 401. Load what you need up front, or accept that the user reloads.

## lucidos.events: Event Store

Emit domain events, and query the workspace's event store.

**`query` reads the whole store, not just what your app emitted.** Workspace
domain events (`HabitCompleted`) and the engine's own thread / system events
(`ChildThreadCompleted`, `ResponseGenerated`, `ChangeApplied`, `TriggerCompleted`)
are rows in one `events` table. One call returns them, filtered by `event_type`,
time, `thread_id`, or a paging cursor. There is no second stream. See
`system-knowhow/thread-events.md` § "One table, two enums" for the
`ThreadEvent` / `SystemEvent` distinction.

```ts
lucidos.events.emit(type: string, payload: Record<string, unknown>, options?: EmitOptions): Promise<void>
lucidos.events.query(params?: EventQuery): Promise<LucidosEvent[]>
```

### Types

```ts
interface EventQuery {
  event_type?: string;
  since?: string;             // ISO 8601
  until?: string;             // ISO 8601
  limit?: number;             // default 100, clamped to 1..1000
  before_event_id?: string;   // walk backward from this event, exclusive
  after_event_id?: string;    // tail-follow forward from this event, exclusive
  thread_id?: string;         // restrict to one thread
  event_id?: string;          // resolve ONE event by id; uuid or 'evt-<32 hex>'
}

interface LucidosEvent {
  id: string;
  event_type: string;
  payload: Record<string, unknown>;
  created: string;
  /** Engine thread events only (absent, not null, on domain events). */
  thread_id?: string;
  /** Monotonic insertion order across the workspace. Always present. */
  sequence: number;
}

interface EmitOptions {
  /** Skip persistence: broadcast on SSE only. */
  transient?: boolean;
}
```

### Examples

```js
// Emit an event
await lucidos.events.emit('HabitCompleted', {
  summary: 'Completed meditation',
  habit: 'meditation',
  streak: 5
});

// Emit a transient coordination signal: it reaches SSE consumers but
// is not written to the event store. Use for heartbeats and ephemeral
// state broadcasts (e.g. presenter↔remote view sync).
await lucidos.events.emit('SlidePresenterState', {
  slide_index: 3,
  is_paused: false,
}, { transient: true });

// Query recent events
const events = await lucidos.events.query({
  event_type: 'HabitCompleted',
  since: '2026-04-01T00:00:00Z',
  limit: 50
});

// Read the outcome of child threads the workspace has spawned. This is an
// ENGINE event, not one your app emitted, and it comes back from the same
// call: `thread_id` is the PARENT thread, and the payload carries the child.
const completions = await lucidos.events.query({
  event_type: 'ChildThreadCompleted',
  limit: 20
});
for (const e of completions) {
  console.log(
    e.thread_id,                      // parent thread
    e.payload.child_thread_id,
    e.payload.child_thread_title,
    e.payload.status,                 // success | failure | no_changes | canceled | interrupted
    e.payload.summary
  );
}
```

### Paging with `before_event_id` / `after_event_id`

Rows come back **newest first** (`created DESC, id DESC`) and `limit` is clamped to 1000, so a longer read needs a cursor. Pass the oldest id you received as `before_event_id` to page backwards. Pass the newest id you stored as `after_event_id` to tail-follow what arrived since. Both cursors are exclusive, and both take `LucidosEvent.id` (not `sequence`).

The two are mutually exclusive: set both and the engine answers 400. A cursor id matching no event is a 404, never a silently unfiltered page. `after_event_id` still returns newest-first, so a tail longer than `limit` gives the most recent slice, not the rows right after the cursor.

```js
// Walk backwards from newest until we reach an event we already have.
async function eventsNewerThan(knownId) {
  const collected = [];
  let before;
  for (;;) {
    const page = await lucidos.events.query({
      event_type: 'HabitCompleted',
      limit: 1000,
      before_event_id: before,   // omitted on the first call: start at newest
    });
    if (page.length === 0) return collected;
    for (const e of page) {
      if (e.id === knownId) return collected;   // caught up
      collected.push(e);
    }
    before = page[page.length - 1].id;          // oldest row of this page
  }
}
```

## lucidos.proxy: Call External APIs

Call backends configured in `data/config/apis.json` through the engine. The engine injects the configured auth header from the credential store. It strips `Cookie`/`Origin`/`Referer`/`Host` from the forwarded request, plus every `x-lucidos-*` header and the two `x-forwarded-*` ones the gateway owns. So **the credential never enters the iframe**, and no Lucidos credential reaches the upstream.

This is the preferred way for app UIs to call external HTTP APIs. Direct `fetch` from the iframe hits two walls:

- **Mixed content:** apps load over HTTPS, so the browser blocks `fetch('http://localhost:5005/...')`.
- **CORS:** the upstream rarely allows the engine's origin, so cross-origin XHR fails.

`lucidos.proxy` sidesteps both: the engine makes the upstream call server-side. From an app frame the request travels over the host bridge, and from a standalone app tab it goes direct.

```ts
lucidos.proxy(name: string): ProxyClient

interface ProxyClient {
  fetch(path: string, init?: RequestInit): Promise<Response>;
}
```

`fetch` returns the raw `Response`, so you pick how to read the body (`.json()`, `.text()`, `.blob()`, …). The engine adds the auth header, so do not set `Authorization` from the iframe.

**A model call through it records its own cost.** When the upstream is a model provider, the engine reads the usage block from the reply and writes a `ContextCaptured` with `purpose: "proxy"`. An app never reports its own model spend, and the Token Cost app counts it. To read that block, the engine asks a model provider for an uncompressed reply, so `Content-Encoding` is absent there.

**The upstream cannot act on the Lucidos origin.** The engine serves the response from its own origin, so it passes only the headers a caller reads:

- content, caching and range headers, `Location`, `Link` and `Retry-After`
- the `RateLimit-*` family, `Request-Id` and any `x-` header
- the `anthropic-` and `openai-` families of the builtin providers

The engine drops everything else. That covers `Set-Cookie`, `Clear-Site-Data`, `Strict-Transport-Security`, a CSP, CORS headers, `Alt-Svc` and `WWW-Authenticate`, and any other vendor header. Every response carries `X-Content-Type-Options: nosniff`. An HTML, XML or untyped one also carries `Content-Security-Policy: sandbox`, so a proxy URL opened in a tab never runs upstream script.

**A response is buffered, so it does not stream.** The engine reads the whole upstream body before it answers, from a frame or a standalone tab. A token stream arrives complete, so render the finished answer.

**The engine waits 30 seconds on the upstream by default, then answers 504.** A streamed reply counts in full. Raise the wait for every route with the `proxy_timeout_secs` preference, or for one entry with `timeout_secs` in `apis.json`. Both accept 1 to 600 (`system-knowhow/lucidos-cli.md` § Timeouts). One proxied call never runs past 600 seconds in total, and the bridge waits a little longer, so it never gives up first.

### Configure the backend (one-time)

`data/config/apis.json`:

```json
{
  "sonos":   { "base_url": "http://localhost:5005" },
  "comfort": {
    "base_url": "https://accsmart.panasonic.com",
    "auth": { "type": "bearer", "credential": "comfort-cloud" }
  }
}
```

Auth is configured per API and applied server-side. The URL pattern (`/api/v1/proxy/<name>/<path>`) is the same in every auth mode. See `system-knowhow/lucidos-cli.md` § `lucidos proxy` for the full `apis.json` schema (bearer / api_key / basic / query_param / hmac_signed / script_handshake). Omit `auth` for unauthenticated backends (e.g. local services).

### Examples

```js
// GET: unauthenticated local backend
const res = await lucidos.proxy('sonos').fetch('/living-room/play');
if (!res.ok) throw new Error(`Sonos: HTTP ${res.status}`);

// POST JSON: the engine injects the auth header
const res = await lucidos.proxy('comfort').fetch('/api/v1/devices', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ deviceGuid: 'abc' }),
});
const data = await res.json();
```

### Built-in model-provider proxies (no `apis.json` entry needed)

The engine already holds credentials and routing for every model provider in the model registry (Settings → Models), plus TypeSafe. It exposes them as **built-in provider proxies** under the SAME route. So an app can call an LLM / image / judgment provider with no `apis.json` entry. When `<name>` matches one and has no `apis.json` entry, the engine forwards to that provider's API root and injects its credential server-side:

| `proxy(name)` | Base URL | Injected server-side | You send |
|---|---|---|---|
| `openai` | `https://api.openai.com/v1` | `Authorization: Bearer <key>` | path as-is, e.g. `/chat/completions`, `/images/generations` |
| `openrouter` | `https://openrouter.ai/api/v1` | `Authorization: Bearer <key>` | path as-is, e.g. `/chat/completions` |
| `xai` | `https://api.x.ai/v1` | `Authorization: Bearer <key>` | path as-is, e.g. `/chat/completions` |
| `anthropic` | `https://api.anthropic.com/v1` | `x-api-key: <key>` (or `Authorization: Bearer` for an OAuth credential) | path as-is, e.g. `/messages`; set your own `anthropic-version` header |
| `local` | your configured local base (Ollama default `http://localhost:11434/v1`) | `Authorization: Bearer <key>` (omitted if keyless) | path as-is, e.g. `/chat/completions` |
| `vertex` | `https://<region>-aiplatform.googleapis.com/v1/projects/<project>/locations/<region>` (engine-owned prefix) | `Authorization: Bearer <access-token>` (minted + refreshed server-side) | ONLY the suffix, e.g. `/publishers/anthropic/models/claude-opus-4-8@default:rawPredict` |
| `typesafe` | `https://api.typesafe.ai/v1` | `Authorization: Bearer <key>` | path as-is, e.g. `/systemone` |

- **Only the credential is injected.** The layer adds just the auth header. `Content-Type`, `anthropic-version` and any attribution headers stay yours to set in `init`.
- **`apis.json` overrides the builtin.** A same-name entry in `data/config/apis.json` wins, so you can point `openai` at a mock/gateway or add auth layers.
- **Vertex is addressed by suffix.** The engine owns the `…/projects/<project>/locations/<region>` prefix (from its own Vertex config, region default `europe-west1`) and mints the OAuth token. Send only `/publishers/<publisher>/models/<model>:<method>`: any other path answers 400, because the token could manage every resource in the project. The region is the engine's configured one. A model that must run elsewhere (e.g. a `global`-only Gemini variant) needs an `apis.json` override.
- **Not configured → 404.** With no credential/config and no `apis.json` entry, the call returns 404 naming what to set.
- **Every default base already includes `/v1`.** Send `/models`, never `/v1/models`, which doubles the segment and answers 404. A `local` base you configure yourself may lack it.
- **`opencode-free` has no proxy.** The keyless free tier serves chat only, and an app must not build on an anonymous endpoint that can vanish without notice (ADR 0104).
- **The same proxies serve scripts and the agent.** `lucidos proxy <name>` and the `proxy_request` tool resolve names exactly as this route does.

```js
// Chat via the built-in OpenAI proxy: no apis.json, no key in the app
const res = await lucidos.proxy('openai').fetch('/chat/completions', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ model: 'gpt-5.6-sol', messages: [{ role: 'user', content: 'hi' }] }),
});

// Claude on Vertex: the app sends only the publisher/model suffix
const res = await lucidos.proxy('vertex').fetch(
  '/publishers/anthropic/models/claude-opus-4-8@default:rawPredict',
  { method: 'POST', headers: { 'Content-Type': 'application/json' }, body },
);
```

### When to use which

| Want to … | Use |
|---|---|
| Read/write workspace files | `lucidos.data.*` |
| Emit a domain event, or query the event store (domain AND engine events) | `lucidos.events.*` |
| Call a model provider the engine already has (LLM / image / judgment) | `lucidos.proxy('openai' \| 'vertex' \| 'openrouter' \| 'xai' \| 'anthropic' \| 'local' \| 'typesafe').fetch(...)`, no `apis.json` needed |
| Call any other external HTTP API | `lucidos.proxy(name).fetch(path, init)` + an `apis.json` entry |
| Hit an engine endpoint no SDK method covers | `lucidos.request('/<suffix>', init)`. It travels the same bridge, and the engine decides which routes an app may reach. See § `lucidos.request`. |

For any other external API, add an entry to `data/config/apis.json` rather than embedding the credential in the app.

## lucidos.apiUrl

```ts
lucidos.apiUrl(suffix: string): string   // synchronous, returns URL
```

Builds an absolute URL onto the engine's `/api/v1` surface, carrying the
**workspace address** (the `/<slug>` path prefix) this app is served under.
Pass the path *after* `/api/v1`.

```js
// A stylesheet loads from an app frame, and this is the URL it needs.
link.href = lucidos.apiUrl('/fonts/fira-code.css');
```

**A `fetch` of what this returns does NOT work inside the host shell.** The
frame's origin is opaque, so CORS refuses the engine's answer. WebKit reports
`Load failed` and Chromium a `TypeError`, and neither names the cause. A URL
the browser fetches for you still works: a `src`, an `href`, a stylesheet.

**So this builds URLs, and never makes calls.** For an endpoint with no SDK
method, use `lucidos.request` below, which travels the bridge. For everything
else use the SDK method: it resolves the prefix, travels the bridge, and
carries the timeout, error shape and response parsing a raw `fetch` lacks.

### Why a hand-written `/api/v1/…` does not work

An app iframe is served at `/<workspace>/app/<app-id>/`, and the engine's HTTP
surface lives at `/<workspace>/api/v1/…`. The two URLs an author reaches for
first both resolve somewhere else:

| Written in JS | Resolves to | Answer |
|---|---|---|
| `new URL('api/v1/events/query', document.baseURI)` | your app's own directory, plus `api/v1/events/query` | `404` |
| `fetch('/api/v1/events/query')` | `/api/v1/events/query` | `404 unknown workspace 'api'` |

**Inside the host shell the call fails before it gets that far.** Every request
from the opaque-origin frame to the engine is cross-origin, and the engine
grants no CORS. The `fetch` rejects with a `TypeError` and your code never sees
a status. The table shows what a standalone app tab, on the engine's own origin,
resolves.

The relative form fails because **`document.baseURI` is your app's own
directory**. Behind a gateway that directory also carries your frame's pass,
which reaches no engine route, so the URL is wrong twice over. The root-absolute
form fails because the gateway reads the **first path segment as a workspace
name**, and there is no workspace called `api`.

**Markup is rewritten on the way out, runtime JS is not.** The engine rewrites
root-absolute `src` / `href` **attributes** in the HTML it serves. So the
`<script src="/api/v1/sdk.js">` in `index.html` reaches the browser as
`<script src="/<workspace>/api/v1/sdk.js">`, which is why the § Setup
boilerplate works. Nothing rewrites a string your JavaScript builds at runtime.
**The same `/api/v1/…` string is correct in markup and broken in JS.**

`apiUrl` derives the prefix as the SDK does internally: the `<base href>` when
the document has one, minus your frame's pass, otherwise everything before
`/app/` in the path. Don't re-derive it, and never hardcode a slug: the
workspace name is not the app's to know. `lucidos.configure({ baseUrl })` is the
one override, for an app hosted outside the engine.

**The failure mode is silence.** A wrong URL is a plain 404, and a refused one is
`Load failed` or a `TypeError`. An app that catches it, warns to the console and
falls back to a second data source looks healthy while it renders stale numbers.
If a fetch of yours has a fallback path, surface the failure in the UI too.

## lucidos.request: an endpoint no namespace covers

```ts
lucidos.request<T>(suffix: string, init?: RequestInit): Promise<T>
```

Call the engine's `/api/v1` surface directly. Pass the path *after* `/api/v1`,
and an optional `init` of the shape `fetch` takes. The answer is parsed as JSON,
a non-2xx raises `SdkError`, and an empty body resolves to `null`. Same 10s
deadline and `TimeoutError` as every other method.

From an app frame it travels the host bridge, so it works where a raw
`fetch(lucidos.apiUrl(...))` is refused. From its own browser tab it goes
direct, with the same contract.

```js
// Read the workspace's environment variables. They are NOT secret: every app
// can read them, and a value appears in logs and events. Use a credential for
// an API key, a token or a password.
const { env_vars } = await lucidos.request('/env-vars');

// The engine refuses a write. An env var reaches every command the agent runs,
// so a name the interpreter loads from would be host code execution. Store an
// app's own setting with lucidos.preferences or lucidos.data instead.

// Emit a workspace event.
await lucidos.request('/events/emit', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ event_type: 'ReportOpened', payload: { summary: 'x' } }),
});

// Turn a model id into the label the user sees.
const { models } = await lucidos.request('/models');
```

### Not every endpoint is reachable

The engine classifies every route, and an app frame reaches only the ones marked
app-reachable. Anything else answers `SdkError` with a 403, whichever realm the
app runs in. The whole reachable set, with the methods each opens:

| Route | Methods |
|---|---|
| `/data`, `/data/*path` | GET, and PUT plus DELETE on a path |
| `/data/edit`, `/data/upload` | POST |
| `/events/query`, `/events/count`, `/events/types` | GET |
| `/events/emit` | POST |
| `/triggers` | GET, POST, PUT, DELETE |
| `/triggers/run` | POST |
| `/triggers/historical`, `/trigger-groups` | GET |
| `/preferences` | GET, PUT |
| `/notifications` | GET, POST |
| `/notification`, `/notifications/before` | GET |
| `/notification/read`, `/notifications/read-all` | POST |
| `/apps`, `/app` | GET |
| `/threads/list`, `/threads/count` | GET |
| `/oauth/:provider/access-token` | GET |
| `/proxy/:name/*path` | any method |
| `/ui/navigate` | POST |
| `/env-vars` | GET |
| `/models`, `/knowhow`, `/knowhow/read`, `/health` | GET |
| `/themes`, `/theme`, `/themes/tokens`, `/themes/parts` | GET |
| `/themes/resolve` | POST |

Most of those have a namespace of their own, which is the better way to call
them. The engine writes the exact list to
`packages/lucidos-sdk/src/generated/app-reach.ts`, so it cannot drift.

Denied, and why:

| Denied | Why |
|---|---|
| credentials, backup keys, OAuth accounts, the email account | the secret never enters the iframe, which is what `lucidos.proxy` exists for |
| `/chat/stream`, thread creation, follow-ups, compose | `lucidos.ui.startThread()` prefills and never submits, so the user always sends their own prompt |
| answering a question, every consent route, the `/internal/` tree | an app never answers as the user |
| applying a change, restarting, rebuilding, installing a plugin | an app does not change the platform under the user |
| writing an env var (the read is open) | an env var reaches every command the agent runs, so a loader-hook name would be host code execution |
| writing a preference the Lucidos Agent may not write | a security setting such as the command guard or the local model host (`local_base_url`) stays the user's, changed in Settings |
| writing the coding-agent paths or their permission mode | a path could run the app's own script as the user |
| reading or writing engine bookkeeping such as `vapid_keys` | it is engine state, not a setting: a read leaves it out |
| message bodies, history, search, memory | an app sees that a thread exists, never what is in it |
| repositories, `/browse-directories`, `/workspaces` | outside the workspace |

If your app needs a denied route, say so rather than working around it. The
list lives in `crates/lucidos-engine/src/api/app_reach.rs`, and opening a route
is a deliberate decision (ADR 0231).

**Prefer a namespace where one exists.** `lucidos.data.read` gives you text
rather than JSON, `lucidos.triggers.create` validates the cron before it sends,
and `lucidos.proxy` handles a non-JSON body. This is the hatch, not the front
door.

## lucidos.oauth: OAuth Token Access

Fetch a short-lived OAuth access token for a connected provider, for in-browser SDKs that need a bearer token in JavaScript (e.g. the Spotify Web Playback SDK). The engine looks up the connected account and refreshes the token if it is expired or expires within 60s. It returns ONLY the access token: the refresh token never leaves the engine.

```ts
lucidos.oauth.getAccessToken(provider: string): Promise<AccessToken>

interface AccessToken {
  accessToken: string;
  expiresAt: Date | null;  // null when the upstream provider didn't include an expiry
}
```

### When to use

- **You need a bearer token in the iframe**: a third-party SDK like `Spotify.Player` calls a `getOAuthToken` callback expecting a raw token string. `lucidos.proxy(...)` can't help, because the SDK makes the request itself.
- **Not for ordinary HTTP calls to the upstream API**: use `lucidos.proxy(<provider>).fetch(...)`, so the engine attaches the bearer header and the iframe never sees the token.

### Example: Spotify Web Playback SDK

```js
const player = new Spotify.Player({
  name: 'My Sonos App',
  getOAuthToken: async (cb) => {
    const tok = await lucidos.oauth.getAccessToken('spotify');
    cb(tok.accessToken);
  },
  volume: 0.5,
});
await player.connect();
```

The SDK calls `getOAuthToken` on first init and again when the token expires. Each call hits the engine, which refreshes from the stored refresh token if needed.

### Errors

- `404`: the provider is not connected for this workspace. Ask the user to connect it in the OAuth account settings (or through the LLM `connect_oauth_account` tool).
- `502`: the engine could not refresh the token (missing client credentials, upstream rejected the refresh, network failure).

### Security note

The refresh token, client_id, client_secret and PKCE state stay on the server. The iframe receives ONLY the short-lived access token, scoped to the connected account. Do NOT cache it in `localStorage` / `sessionStorage`: re-call `getAccessToken` when you need one, and the engine handles caching and refresh.

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
`last_run` and runs under the trigger's own identity, side-effect grant and
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

// Irreversible-side-effect category a trigger can be granted. Only enforced
// when the workspace's command guard is on (Settings → Permissions → Command
// Safety). A trigger that hits an irreversible command whose category isn't in
// its grant fails, since it runs unattended and cannot ask for approval.
type SideEffectCategory =
  | 'email'
  | 'external_api'
  | 'cloud_cli'
  | 'out_of_workspace_destruction'
  | 'other';

interface Trigger {
  id: string;
  name: string;
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
  // Side-effect grant: irreversible categories this trigger may perform
  // unattended. Omitted when empty (= no grant).
  side_effect_grant?: SideEffectCategory[];
  // Chat model this trigger's intent fires on, and its thinking budget. Both
  // omitted when the trigger follows the account default (Settings → Models →
  // Chat & triggers). Intent triggers only: a script trigger runs no LLM.
  model?: string;
  reasoning_effort?: string;
  // The backend the pinned model runs on. Omitted when the model decides.
  provider?: string;
}

interface CreateTrigger {
  name: string;
  run: TriggerRun;
  cron_expressions: string[];
  on?: EventSubscription[];
  /** Optional *trigger group* id; omit for ungrouped. */
  group_id?: string;
  /** Side-effect grant: irreversible categories this trigger may perform
   *  unattended. Omit / `[]` = none granted (the safe default). */
  side_effect_grant?: SideEffectCategory[];
  /** Pin the intent to a chat model and a thinking budget
   *  (`none|low|medium|high|xhigh|max`). Omit either for the account default. */
  model?: string | null;
  reasoning_effort?: string | null;
  /** The backend for the pinned model. Needs `model`, and must name one of its
   *  providers. A pin to an unconfigured backend refuses the fire. */
  provider?: string | null;
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
  /** Full replacement for the side-effect grant; pass `[]` to clear all. */
  side_effect_grant?: SideEffectCategory[];
  /** Pin the intent's model / thinking budget / backend (string), clear it
   *  back to the default (null), or leave it unchanged (absent). Changing the
   *  model drops a backend pin it cannot use. */
  model?: string | null;
  reasoning_effort?: string | null;
  provider?: string | null;
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

Within one cron expression the fields are **ANDed**. Across the array they are **ORed**. So `0 0 9 1 * Mon` fires only when the 1st IS a Monday (roughly 1.7 times a year). The 1st plus every Monday takes two expressions. See `system-knowhow/triggers.md` § "Writing cron expressions" for the nth-weekday and last-weekday recipes.

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

The shape mirrors the app's `manifest.json` (`name` / `description` / `icon` /
`reveal` / `kind` / `origin_thread_id` or `origin_plugin_id` / `reusable` / `params`) plus the `id` and, for a built-in widget, `built_in` derived from its folder. `list()` hits `GET /api/v1/apps`, which lists apps only, never a *widget*.
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

A *widget*'s params for the place it shows (ADR 0415): the JSON object given to `widgets(action="show")`, a *widget embed*, or a pin. It returns `{}` when the widget was given none, or when the value is not a JSON object.

The host writes the object into the frame URL under the one query key `params`. So a param name never collides with a host name. The widget's `manifest.json` declares the names it takes (`system-knowhow/building-an-app.md` § Widgets). Params are visible in the URL, so never pass a credential. Params over 2 KB are refused.

### Example

```js
const { clip } = lucidos.params();
if (!clip) document.body.textContent = 'No clip given';
```

## lucidos.preferences: User Settings

```ts
lucidos.preferences.get(deviceId?: string | null): Promise<Preferences>
lucidos.preferences.set(key: string, value: string, deviceId?: string): Promise<void>
```

`get()` defaults to the device the app runs on, so it sees the same merged view
as the shell. Theme, font and scale are device-scoped, and a read naming no
device gets only the global rows. The device id is per-workspace, and an app
never handles it. An app frame names its device, and the host substitutes the
id. A standalone app tab reads the id itself.

A popped-out tab follows the shell it left, named by the `?device=` in its URL,
so it keeps that shell's theme. Otherwise the tab reads the workspace-scoped
`ws:<slug>:lucidos-device-id`. Pass `null` to fetch only global preferences.

`set()` refuses any key the Lucidos Agent may not write, such as
`command_guard`, `max_tool_calls`, `network_bind` or `local_base_url`. Those
are security settings the user changes in Settings. It also refuses the keys
that choose what a coding-agent session spawns (`coding_agent_claude_path`,
`coding_agent_codex_path`, `coding_agent_claude_permission_mode`).

Engine bookkeeping, such as the Web Push keypair in `vapid_keys`, is not a
setting: `get()` leaves it out, and `set()` refuses it.

Every refusal rejects the promise with the engine's reason, including a value
the engine will not store, such as a malformed timezone.

### Types

```ts
type Preferences = Record<string, string>;
```

### Common keys

| Key | Values | Description |
|-----|--------|-------------|
| `theme-mode` | `dark`, `light`, `system` | Light or dark. `system` (the default) follows the OS. Read the resolved value as `data-theme-mode` on `<html>` |
| `font-family` | `theme`, `system`, `geist`, `atkinson-hyperlegible-next`, `inter`, `roboto`, `open-sans`, `manrope`, `source-serif-4`, `lora`, `literata`, `fira-code`, `monospace`, `geist-mono`, `atkinson-hyperlegible-mono`, `jetbrains-mono`, `ibm-plex-mono`, `source-code-pro`, `commit-mono`, `cascadia-code`, `vt323` | Font. `theme` (the default) follows the active theme's suggested font, else `fira-code`; `GET /api/v1/fonts` lists the fonts, each with its `group` (`sans`, `serif`, `mono`). `applyPreferences()` resolves it for you into `--font-ui` and loads the font from the local engine. `fira-code`, `jetbrains-mono` and `cascadia-code` also enable programming ligatures, on code and `pre` blocks only, via `--font-features-text` / `--font-features-code` |
| `ui-scale` | Number in 12.5% steps from 75 to 200 (`75`, `87.5`, `100`, `112.5`, `125`, `137.5`, `150`, `162.5`, `175`, `187.5`, `200`); or the legacy strings `small` / `medium` / `large` (= `100` / `112.5` / `125`). Off-grid numbers snap to the nearest valid step. | Scale |
| `autocorrect` | `true`, `false` | Whether text fields autocorrect on this device. Unset, on everywhere. `sdk.js` applies it to your fields (§ Text fields and autocorrect, under Setup) |
| `motion` | `system`, `reduce`, `full` | Whether this device reduces motion. `system` (the default) follows the OS. Read it as `data-motion` on `<html>` (§ Reduced motion, under Setup) |
| `theme-effects` | `system`, `reduce`, `full` | Whether this device shows a theme's part shadows, filters and scanlines. `system` (the default) drops them when the OS asks for more contrast or less transparency. Read it as `data-theme-effects` on `<html>` (§ Theme parts, under Setup) |

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

**A write the host refuses later**, such as when the browser's storage is full,
is rolled back and passed to every `onError` handler. The user also sees a toast
naming the app. With no handler the console logs it.

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
`navigate_ui` tool** (the `NAVIGATE_TARGETS` / `NAVIGABLE_SETTINGS_VIEWS` consts in
`crates/lucidos-engine/src/llm/tools/misc.rs`) into
`packages/lucidos-sdk/src/generated/navigate-targets.ts`, so the SDK and the LLM
tool schema cannot drift. To change the set, edit those Rust consts and run
`cargo test -p lucidos-engine --lib generate_navigate_targets_file -- --ignored`.

```ts
type NavigateTarget =
  | 'files' | 'apps' | 'app-store' | 'plugins' | 'triggers' | 'thread-queue' | 'changes' | 'notifications'
  | 'settings' | 'app' | 'file' | 'trigger' | 'thread'
  | 'new-app' | 'new-trigger' | 'new-chat' | 'url';

// Settings sub-section for `target: 'settings'`. No category is platform-gated,
// so none is withheld from a caller with no platform signal. `system` opens the
// list of System pages; `system-overview` is the one holding versions. Two pages
// are absent because nothing has asked to link them: `webhooks` and
// `communication-surfaces`.
type SettingsViewTarget =
  | 'models' | 'permissions' | 'mcp' | 'coding-agents' | 'accounts' | 'locale' | 'marketplaces'
  | 'access' | 'devices' | 'system' | 'appearance' | 'keyboard-shortcuts'
  | 'system-overview' | 'release-notices' | 'whats-new'
  | 'thread-queue' | 'backup' | 'memory' | 'disk-usage' | 'environment-variables' | 'debugging';

interface NavigateUi {
  target: NavigateTarget;
  settings_view?: SettingsViewTarget;
  app_id?: string;
  /** The place INSIDE the app, delivered as the app iframe's `location.hash`.
   *  Only used with `target: 'app'`. See § Navigation targets. */
  fragment?: string;
  file_path?: string;
  /** 1-based line to open `file_path` at, and the inclusive last line of the
   *  range. See § Navigation targets for the degradation rules. */
  line?: number;
  line_end?: number;
  id?: string;
  url?: string;
  event_id?: string;
  prompt?: string;
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

## lucidos.threads: Thread Management

```ts
lucidos.threads.list(opts?: ThreadsListOptions): Promise<ThreadSummary[]>
lucidos.threads.count(opts?: Omit<ThreadsListOptions, 'limit'>): Promise<number>
```

`list()` calls `GET /api/v1/threads/list` and returns a newest-first array of `ThreadSummary` rows from the projection. `count()` calls `GET /api/v1/threads/count` and resolves to the count under the same filter, cheaper on big workspaces than `(await list()).length`.

Same surface as the `lucidos threads list` / `lucidos threads count` CLI and the `list_threads` / `count_threads` LLM tools. Use it to render thread state (counts, status indicators) without subscribing to the full SSE stream.

**`active` is a union, `status` is precise.** `active: true` selects `running` OR `waiting_for_user_answer`, which are opposites: the workspace working, or waiting on a person. "Is anything busy?" wants `status: 'running'`. "N threads I have something invested in" wants `active: true`. Passing both is a 400.

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
   *  `list_threads` tool's `my_children`, except an app names the thread.
   *  A malformed uuid is a 400, never a silently unfiltered list. */
  parent?: string;
}

/** Why a turn end left a coding agent's work unproposed. */
type UnproposedReason =
  | 'plan_missing'
  | 'plan_awaiting_approval'
  | 'outside_bound'
  | 'hardening_missing'
  | 'turn_incomplete';

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
}
```

### When to use which

| Want to … | Use |
|---|---|
| Render a list of threads in an app UI | `lucidos.threads.list()` |
| Ask "is the workspace busy?" | `lucidos.threads.count({ status: 'running' })` |
| Show "N threads need me" | `lucidos.threads.count({ status: 'waiting_for_user_answer' })` |
| Show "N active threads" badge (working AND asking) | `lucidos.threads.count({ active: true })` |
| Render one thread's children (a fan-out board) | `lucidos.threads.list({ parent: id })` |
| React to thread state changes in real time | Subscribe to `lucidos.sse` instead |
| Spawn a new thread from an app | `lucidos.ui.startThread({ prompt })` |
| Open a link outside Lucidos from JS | `lucidos.ui.openExternal(url)` (never `window.open`) |

## lucidos.ui: UI Control

```ts
lucidos.ui.applyPreferences(): Promise<void>
lucidos.ui.watchPreferences(): void
lucidos.ui.navigate(target: NavigateTarget, params?: NavigateParams): Promise<void>
lucidos.ui.openExternal(url: string): Promise<void>
lucidos.ui.startThread(opts?: { prompt?: string }): Promise<void>
lucidos.ui.previewFile(params: FilePreviewParams): Promise<void>
lucidos.ui.confirm(options: ConfirmOptions): Promise<boolean>
lucidos.ui.toast(message: string, type?: ToastType, opts?: ToastOptions): void
lucidos.ui.dismissToast(key: string): void
lucidos.ui.ready(): void
lucidos.ui.prompt(options: PromptOptions): Promise<string | null>
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

Both sample a moment after the event. A moved theme repaints only if a second read, about a second later, still sees the move. So a brief iOS wake that still reports the app-switcher snapshot's appearance repaints nothing. Your app does none of this itself.

`navigate()` sends a navigation request to the Lucidos frontend via SSE. `target`
and `params` (`NavigateParams` = `NavigateUi` minus `target`) are typed against
the generated navigation contract, so valid `target`s and `settings_view`s are
type-checked (§ Types, under lucidos.notifications).

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

### Sizing a widget to its content

A *widget* shows inline in its thread as a *widget card*, so its frame fits
what it draws instead of filling a pane. Nothing to call: when the host marks a
frame as a widget, the SDK measures `document.body` and reports its height on
every resize. Design the widget to fit one phone screen, with the answer on its
top screen.

- **Inline**, the frame takes the full height you report and never scrolls
  inside. Past about one phone screen, the card clips with a fade and an
  Expand button that opens the widget full size in Canvas.
- **On the shelf**, a pinned widget drops open under the title, capped at 70%
  of the screen height.

- **Never size the body to the viewport.** `height: 100vh`, `min-height:
  100%` or a full-height flex body reports the frame's own height, so the
  frame can never shrink to the content.
- **Load the SDK.** A widget without `<script src="/api/v1/sdk.js">` reports
  nothing and keeps a fixed default height.
- The same widget opened full size in the Canvas pane reports nothing, so
  there it is an ordinary app.

### Navigation targets

| Target | Params | Description |
|--------|--------|-------------|
| `thread` | `id` | Focus a specific thread |
| `app` | `id` (or `app_id`), `fragment` (optional) | Open an app UI, optionally at a place inside it. See the fragment param below. |
| `settings` | `settings_view` (optional) | Open Settings, optionally a sub-section: `models`, `permissions`, `mcp`, `coding-agents`, `accounts`, `locale`, `marketplaces`, `access`, `devices`, `appearance`, `keyboard-shortcuts`, or a System page (`system` is the list of them; `system-overview`, `release-notices`, `whats-new`, `backup`, `memory`, `disk-usage`, `environment-variables`, `thread-queue`, `debugging` are the pages themselves). Omit `settings_view` for the Settings home list. |
| `new-chat` | `prompt` (optional) | Open a fresh chat thread, optionally prefilling the compose textarea. Prefer `lucidos.ui.startThread()`, the typed wrapper around this target. |
| `plugins` | `id` (optional) | Open the Plugins panel's Installed tab. With `id` (a plugin id), scroll to and pulse-highlight that plugin's row. The plugin-update notification uses this, so a tap on one update lands on its plugin. |
| `app-store` | (none) | Open the Plugins panel's Store (marketplace) tab. |
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

- Only used with `target: 'app'`. On any other target it is ignored.
- Lucidos never inspects it: only the app knows what its own targets are.
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
- A ref that does not exist (or a file that does not exist at it) shows the preview's normal "failed to load" state, not a thrown error.
- **Every segment must be non-empty.** `repo:<repoId>:file#:<path>` names no revision and is not a repo path at all, so it falls back to the artifact rule like any other malformed `repo:…` string. Leave the `#` off instead.

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

The host routes that click through the same navigation, so a report full of citations works as a plain artifact, with no app needed. The artifact runs sandboxed and loads no SDK, so a link is its only way to reach the host. See `system-knowhow/best-practices.md` § What a standalone HTML document can do.

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

- **Call it synchronously from the click handler.** When the user has chosen the
  "Ask" external-link target, this opens the OS share sheet, which the browser
  refuses without a live user gesture. An `await` before the call spends that
  gesture. Do async work first, then open from a later interaction.
- **Don't fall back to `window.open`.** Inside an installed iOS PWA `window.open`
  cannot leave the app: WebKit renders an in-app web view with no address bar,
  no tabs and no shared Safari session. The user's `external_link_target`
  preference (`safari` / `ask` / `in-app`, default `safari`) controls that
  overlay, so a fallback overrides their choice.

Non-http(s) URLs (`mailto:`, `tel:`) go to the platform unchanged. The promise
resolves once the open is dispatched. A user dismissing the share sheet resolves
normally rather than rejecting.

### Starting a fresh chat with a prefilled prompt

`lucidos.ui.startThread()` opens a new chat thread. A `prompt` lands in the compose textarea **prefilled**: the user reviews, edits and clicks Send. It is never auto-submitted, so the user controls what is sent on their behalf.

```js
// "Set this up for me" button: pops a fresh chat with a ready-to-send prompt.
document.querySelector('#setup-trigger').addEventListener('click', () => {
  lucidos.ui.startThread({
    prompt: 'Create a daily 9am trigger that summarizes my unread email.',
  });
});
```

Call it with no arguments to open a blank fresh chat, like the "new thread" shortcut.

### Confirmation dialogs

`lucidos.ui.confirm` shows a modal rendered by the Lucidos shell, outside your app iframe, so it inherits the user's theme and sits above all app content. Use it instead of `window.confirm()`.

```ts
interface ConfirmOptions {
  /** Optional heading. Renders above the message. */
  title?: string;
  /** Required. Plain text. A BLANK line (\n\n) starts a new paragraph; a
   *  single \n collapses to a space, the way HTML collapses source wrapping. */
  message: string;
  /** Default: "Confirm". */
  okLabel?: string;
  /** Default: "Cancel". */
  cancelLabel?: string;
  /** Style the OK button as destructive (red). Default: false. */
  danger?: boolean;
}
```

Resolves `true` on OK click or Enter, and `false` on Cancel, Esc or backdrop click. A second `confirm` while one is visible resolves the first `false` and replaces it.

**Example:**

```js
const ok = await lucidos.ui.confirm({
  title: 'Delete node?',
  message: 'Delete "Reduce CPAC by 50%" and its 3 descendants?',
  okLabel: 'Delete',
  danger: true,
});
if (!ok) return;
// proceed with deletion
```

### Toasts

`lucidos.ui.toast` shows a transient status banner rendered by the Lucidos shell,
above all app content and themed. It is **fire-and-forget**, with nothing to
await. Use it for success/error feedback instead of hand-rolling a banner.

```ts
type ToastType = 'success' | 'info' | 'warning' | 'error';

interface ToastOptions {
  /** A bold line over the message. Only the title is bold; without one the
   *  toast is its message alone. A non-string title throws a TypeError. */
  title?: string;
  /** Auto-dismiss after this many ms. Omit for the host default: errors and
   *  warnings stay until dismissed; success/info auto-close. */
  durationMs?: number;
  /** false = never show the close (X) button, and a tap on the toast never
   *  closes it. A toast that leaves on its own timer and has no button shows
   *  no X either way. Default true. */
  dismissable?: boolean;
  /** Stable key for in-place replacement. A later toast with the same key
   *  updates the existing toast (message/type/etc.) instead of stacking a new
   *  one, e.g. an 'Opening…' toast becoming 'Opened'. */
  key?: string;
  /** true = show an indeterminate "work in progress" spinner in place of the
   *  severity icon. Pair it with a `key`, so a later keyed toast can replace
   *  the spinner with the outcome. Indeterminate only: no percentage. */
  spinning?: boolean;
}
```

`type` defaults to `'info'`, and an unknown value degrades to `'info'`. Only this
serializable subset is exposed. The host's toast action buttons take `onClick`
callbacks, which can't cross the app-iframe boundary.

**A tap on an app's toast does nothing**, so it stays up until its X or its
timer. A toast never takes keyboard focus. The user reaches it with the Focus
newest toast shortcut.

**A repeat counts instead of stacking.** Raise an unkeyed toast whose type,
title and message match one still up, and that card shows `×2`, `×3` and so on.
Its timer restarts. Use a `key` when a later toast should replace the words.

**The title is explicit, and the message is plain text.** Pass `opts.title` for
a bold line over the message. A newline in the message is just a line break: the
host reads no heading or list out of it, and shows a `"• "` line as written.

**A toast is a summary, and the host bounds it.** A message longer than 2000
characters is truncated with an ellipsis. An `'error'` toast shows its title and
message as ONE line each, truncated at 200 characters, so a newline becomes a
space. Keep an error to a sentence, and put the detail somewhere the user can
come back to.

**Example:**

```js
lucidos.ui.toast('Saved', 'success');
lucidos.ui.toast('Could not reach the server', 'error');
lucidos.ui.toast('Working on it…', 'info', { durationMs: 2000 });

// A bold title over a longer message:
lucidos.ui.toast('3 habits due today\n• Read\n• Walk', 'info', { title: 'Habit Tracker' });

// Collapse a two-step status into one toast that updates in place:
lucidos.ui.toast('Opening from Drive…', 'info', { key: 'drive-open' });
lucidos.ui.toast('Opened "Q3 deck"', 'success', { key: 'drive-open' });
```

#### Long-running work: a spinner you can take back down

`spinning: true` swaps the severity icon for a small indeterminate spinner, so a
keyed toast can narrate work with no honest percentage. Its counterpart,
`lucidos.ui.dismissToast(key)`, takes that toast back down, for work that
finishes with nothing left to say.

`dismissToast` is fire-and-forget like `toast`, and **a key matching nothing is a
no-op**, never an error. The user may have closed the toast, or its `durationMs`
expired, so "already gone" is normal. It reaches toasts by key only, so a
`toast()` raised without one can't be dismissed this way.

Start the spinner on the user's action, and let the event that reports the work
finished decide how it ends:

```js
document.querySelector('#reindex').addEventListener('click', async () => {
  lucidos.ui.toast('Reindexing your notes…', 'info', {
    key: 'reindex',
    spinning: true,
    dismissable: false,   // work is under way; there's nothing to cancel by closing
  });
  await lucidos.events.emit('ReindexRequested', {});
});

lucidos.sse.on('ReindexCompleted', (data) => {
  if (data.changed === 0) {
    lucidos.ui.dismissToast('reindex');   // nothing worth reporting, just clear it
  } else {
    lucidos.ui.toast(`Reindexed ${data.changed} notes`, 'success', { key: 'reindex' });
  }
});
lucidos.sse.on('ReindexFailed', (data) => {
  lucidos.ui.toast(`Reindex failed: ${data.error}`, 'error', { key: 'reindex' });
});
```

One `key` across every arm makes the spinner *become* the outcome in place,
instead of stacking a second toast. Give a `spinning` toast an end on every path
(a keyed replacement or a `dismissToast`), or the spinner stays forever.

### Prompts

`lucidos.ui.prompt` shows a single-field text-input modal rendered by the Lucidos
shell, themed and above all app content: the text-input sibling of `confirm`.
Use it instead of `window.prompt()`.

```ts
interface PromptOptions {
  /** Required. The question/instruction shown above the input. Plain text,
   *  with the same paragraph rule as `confirm`: a blank line (\n\n) starts a
   *  new paragraph, a single \n collapses to a space. */
  message: string;
  /** Optional heading rendered above the message. */
  title?: string;
  /** Prefilled input value. */
  defaultValue?: string;
  /** Placeholder shown when the input is empty. */
  placeholder?: string;
  /** OK button label. Default "OK". */
  okLabel?: string;
  /** Cancel button label. Default "Cancel". */
  cancelLabel?: string;
  /** Render a multi-line textarea instead of a single-line input. Default false. */
  multiline?: boolean;
}
```

Resolves the entered string on OK click or Enter, and `null` on Cancel, Esc or
backdrop click. A `multiline` prompt uses Enter for newlines, so submit with the
OK button. A second `prompt` while one is visible resolves the first `null` and
replaces it.

**Example:**

```js
const name = await lucidos.ui.prompt({
  title: 'Rename board',
  message: 'New name for this board:',
  defaultValue: 'Untitled',
});
if (name === null) return; // user cancelled
// proceed with `name`
```

### Tooltips

Any element with `data-tooltip` gets a themed Lucidos tooltip, with nothing to
call. `sdk.js` installs one delegated listener on the document, so an element
you add later is covered too. Never hand-roll a tooltip in an app.

```html
<button class="icon-btn" data-tooltip="Delete this row">🗑</button>
```

| Attribute | Effect |
|---|---|
| `data-tooltip` | The tooltip text. For the normal case this is the whole contract. |
| `data-tooltip-title` | A bold heading above the text. |
| `data-tooltip-rows` | A JSON array of `{label, value, tone?}`, rendered as a two-column grid. Use it for a small property list. `tone` paints a leading dot and takes `running`, `changes`, `waiting` or `failed`. |
| `data-tooltip-below` | Always place the tooltip below the element, never above. |
| `data-tooltip-follow-cursor` | Anchor on the pointer instead of the element's border. Worth it only on a very tall element, where the border is far from the hand. |
| `data-tooltip-tap` | On a touch device, reveal on a single tap as well as on a long press. |

Placement: above the element by default, flipped below when there is no room
above. Horizontally centred on the element and clamped inside the viewport, with
the arrow kept on the anchor. It hides on mouseout, on a mousedown, on a scroll,
and when the frame loses focus.

**A pointer waits 300ms before the tooltip appears**, so crossing a toolbar does
not trail one behind the cursor.

**Touch: press and hold for 450ms.** The tooltip appears under the finger and
clears itself two seconds after you lift. Moving the finger cancels it, so a
scroll or a swipe never reveals one. The tap that ends the long press is
swallowed, so revealing a tooltip never also activates what sits under it.

**A redundant tooltip is dropped.** When the text repeats the element's own
visible text, and that text is not truncated, nothing shows. A truncated label
keeps its tooltip, which is what makes a clipped file name readable.

#### Turning it off

The layer stands down on its own whenever your page owns a `#tooltip` element,
so an app with a hand-rolled tooltip never shows two.

To turn it off deliberately, set the attribute in markup:

```html
<html data-lucidos-tooltips="off">
```

or call this once at startup:

```js
lucidos.ui.disableTooltips();
```

The attribute is read on `<html>` or `<body>`, and it applies before any script
runs. `disableTooltips()` sets the same attribute and drops the tooltip node.
Both last for the life of the page.

### lucidos.ui.Select: themed dropdown

Replaces a native `<select>` (whose popup the OS draws and CSS can't reach) with a themed dropdown. Supports keyboard nav, type-to-select, and light and dark mode.

It renders the host's own `.dropdown-trigger` / `.dropdown-option` classes, plus `.surface-box` for the menu's box, so it matches the dropdown in Settings.

#### Types

```ts
interface SelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

interface SelectCreateOptions {
  options: SelectOption[];
  value?: string;
  placeholder?: string;
  disabled?: boolean;
  className?: string;
  onChange?: (value: string, option: SelectOption | undefined) => void;
}

interface SelectInstance {
  element: HTMLElement;          // insert into the DOM
  getValue(): string | undefined;
  setValue(value: string | undefined): void;
  setOptions(options: SelectOption[]): void;
  setDisabled(disabled: boolean): void;
  open(): void;
  close(): void;
  destroy(): void;               // removes listeners + detaches the element
}
```

#### Keyboard

| Key | Action |
|---|---|
| `ArrowDown` / `ArrowUp` | Open the menu, then move focus through options |
| `Home` / `End` | Jump to first / last option (when open) |
| `Enter` / `Space` | Open the menu, or commit the focused option |
| `Escape` | Close the menu without changing the value |
| Letter keys | Jump to the next option whose label starts with the typed prefix (multi-character buffer, resets after 500 ms) |
| `Tab` | Close the menu and move focus to the next focusable element |

#### Programmatic usage

```js
const sel = lucidos.ui.Select.create({
  options: [
    { value: 'apple', label: 'Apple' },
    { value: 'banana', label: 'Banana' },
    { value: 'cherry', label: 'Cherry' },
  ],
  value: 'apple',
  placeholder: 'Pick a fruit…',
  onChange: (v) => console.log('picked', v),
});
document.querySelector('#my-container').appendChild(sel.element);

// Later, mutate it from outside:
sel.setValue('banana');
sel.setOptions([{ value: 'd', label: 'Durian' }]);
sel.setDisabled(true);

// Tear down when the host UI unmounts:
sel.destroy();
```

#### Declarative usage: enhance existing `<select>` elements

`enhanceSelects()` walks `root` (default `document`) and replaces every
`<select class="lucidos-select">` it finds. The native element stays in the DOM,
hidden. Its `value` mirrors the user's selection and `change` events still fire
on it, so existing form code keeps working. It skips already-enhanced selects,
so call it again after adding new ones.

```html
<select class="lucidos-select" data-placeholder="Choose…">
  <option value="todo">To do</option>
  <option value="doing">In progress</option>
  <option value="done">Done</option>
</select>
<script>
  lucidos.ui.enhanceSelects();
</script>
```

## lucidos.sse: Real-time Events

Subscribe to server-sent events for live updates.

```ts
lucidos.sse.connect(): void
lucidos.sse.disconnect(): void
lucidos.sse.on(eventType: string, callback: (data: unknown, raw: SseEvent) => void): () => void
```

`on()` returns an unsubscribe function. Subscribe by inner event name: the SDK unwraps the wire format.

### One stream per workspace

`connect()` is idempotent, and one connection fans out to every `on()` listener in your app.

The connection is also shared **across documents**. An app frame has an opaque origin and can open neither an `EventSource` nor a `SharedWorker` port. So the host relays every frame off the connection it already holds. A document that is not a frame (the Lucidos shell, or an app in its own tab) attaches to the `SharedWorker` holder directly. Opening more apps opens no more connections.

There is nothing to configure. For an app author:

- **A frame is identical either way.** A relayed frame carries the same payload a private connection would, so your handler does not change.
- **`disconnect()` detaches this document only.** It never takes the stream from another app or from the shell.

Where `SharedWorker` is missing (Chromium on Android, and Android WebView), a document that is not an app frame opens a private `EventSource`. Same events, same order, one connection per document.

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

// Connect SSE for live updates
lucidos.sse.connect();

// Load data
const raw = await lucidos.data.read('artifacts/my-app/data.json');
const state = JSON.parse(raw);

// Listen for relevant events
lucidos.sse.on('MyAppDataUpdated', (data) => {
  // Re-render with new data
});
```
