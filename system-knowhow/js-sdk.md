---
name: Lucidos JavaScript SDK
description: API reference for the `lucidos` JS SDK that app UIs call: the data, events, proxy, apiUrl, oauth, triggers, apps, preferences, notifications, threads, ui and sse namespaces, plus the component classes and theme variables apps style with.
---

# Lucidos JavaScript SDK

The SDK is available as the `lucidos` global in app UIs (loaded via `<script src="/api/v1/sdk.js">`). The host frontend imports it from the `@lucidos/sdk` package directly.

> From a coding-agent subprocess, prefer the `lucidos` CLI for `data.*` and `events.*` operations — see [`lucidos-cli.md`](./lucidos-cli.md).

## Setup

App HTML is served as static content — the engine doesn't inject anything (except `?thread_id=` rewriting on WIP-preview requests). Apps opt into each piece they want.

The standard Lucidos app boilerplate:

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

What each piece does — include only what you need:

| Tag | Provides | Skip if |
|---|---|---|
| `<title>` | Tab title | (always include — browsers require it) |
| `<script src="/api/v1/sdk-prefs.js"></script>` | Synchronous prefs script — reads the user's theme/font/scale from `localStorage` (shared with the parent shell via same-origin sandboxing) and sets `data-theme`, `--bg-primary`, and `--font-ui` on `<html>` (plus `--user-ui-scale` when the user has set one) *before* any subsequent stylesheet evaluates. Eliminates the flash-of-default-theme between iframe load and `applyPreferences()`. **Place as early in `<head>` as possible — before `sdk-iframe.css`, before any other `<link rel="stylesheet">`, and before any inline `<style>` that reads theme vars.** Inlining `--bg-primary` directly (not just `data-theme`) is what makes the body's `background: var(--bg-primary, …)` paint correctly even when stylesheets are loaded asynchronously (JS-injected, dynamic `import()`, dev-mode bundlers like Vite that ship CSS as JS modules). | App doesn't use `sdk-iframe.css` (no FOUC to fix) |
| `<link rel="stylesheet" href="/api/v1/sdk-iframe.css">` | Theme tokens (`--bg-primary`, `--accent`, etc.), dark/light variables, default body/input/scrollbar styling, **and Lucidos's shared component classes** (`.action-btn` + `.action-btn-confirm`/`.action-btn-danger`, `.button-group`, `.icon-btn`, `.label`, `.title`, `.segmented-control`/`.segmented-btn`, `.list-row*`, `.markdown-content`, `.progress-bar`, `.empty-state`, `.accent-link`). Use these class names and the app's buttons/lists/etc. render identically to the host shell. The body is set to `--font-size-md`, the type scale's body step, and inputs and buttons are set to `--font-ui` at the same step, so text and controls you do not size yourself land where the host shell's body text lands. Note that the body step is NOT the root font-size: the root is the user's UI scale, and `1rem` is `--font-size-xl`, a section heading. Text that names no size at all therefore comes out a step and a half larger than body, which is why the defaults above exist. | App ships its own complete stylesheet and doesn't want Lucidos theming |
| `<script src="/api/v1/sdk-iframe-audio.js"></script>` | Monkey-patches `AudioContext` so app code reuses a gesture-unlocked instance, survives iOS PWA background cycles. **Must be in `<head>` before any code that creates an `AudioContext`.** | App doesn't play audio |
| `<script src="/api/v1/sdk.js"></script>` | The `lucidos.*` API. Also installs iframe-only side effects, none of which needs a call from you: a link interceptor (`target="_blank"` links resolve in-frame; external `http(s)://` links route through `lucidos.ui.openExternal()`); a keyboard-shortcut forwarder (host shortcuts like focus/hide a pane, narrow/widen, new thread, search, and Escape keep working while the app has focus, because iframe keydowns otherwise never reach the host); per-app scroll memory (the app returns to where the user left it after an app switch or a reload); and the Lucidos **tooltip** on any `data-tooltip` element (see § Tooltips, under lucidos.ui). Only modifier-bearing chords and Escape are forwarded; plain typing stays in the app. | App doesn't use `lucidos.*` |
| `lucidos.ui.applyPreferences()` | Reads the user's theme/font/scale (resolving a `system` preference to the live OS light/dark) and sets `data-theme` + CSS vars on `<html>`. Pairs with `sdk-iframe.css` to apply the right palette. | **Don't skip if you include `sdk-iframe.css`** — without it the app ignores the user's light/system setting and stays on the default dark palette. Skip only when opting out of Lucidos theming entirely. |
| `lucidos.ui.watchPreferences()` | Re-applies preferences live: when the user changes one (SSE `PreferencesChanged`), and, under a `system` preference, when the OS light/dark appearance flips. The OS half watches `prefers-color-scheme` and the frame's own resume, on every platform, matching the host shell | Static apps that have opted out of Lucidos theming |

**Inherit the theme by default.** A normal app includes the theme assets, calls `applyPreferences()` + `watchPreferences()`, and styles with the theme variables (below) — so it follows the user's light/dark (OS) appearance just like the rest of Lucidos. Theme integration is *technically* opt-in: the engine never auto-injects these tags, so an app that omits both `<script src="/api/v1/sdk-prefs.js">` and `<link rel="stylesheet" href="/api/v1/sdk-iframe.css">` gets no `data-theme` attribute, no CSS variables, and no Lucidos default styling. Opt out only for an app that ships its own complete visual identity (charts, games, embedded third-party UIs) — otherwise inheriting is the default, and **hardcoding colors is a bug** (a light-mode workspace gets a dark-only app, or vice versa).

### Theme variables

`sdk-iframe.css` defines these CSS custom properties on `<html>` and flips their values between light and dark automatically — driven by the `data-theme` attribute, which `applyPreferences()` sets (resolving `system` to the OS setting) and `watchPreferences()` keeps in sync. Style your app with `var(--name)` and it tracks the user's appearance for free. The canonical values live in the engine's `sdk-iframe.css`; **the names are the contract**:

| Group | Variables |
|---|---|
| Backgrounds | `--bg-primary`, `--bg-secondary`, `--bg-tertiary`, `--bg-quaternary`, `--bg-hover`, `--bg-selected` |
| Text | `--text-primary`, `--text-secondary`, `--text-muted`, `--text-on-accent` |
| Border | `--border-color` |
| Accents | `--accent`, `--accent-light`, `--accent-green`, `--accent-yellow`, `--accent-red` |
| Focus | `--focus-ring` — a ready-made `box-shadow` value (a soft accent band) for focus indicators; the `.action-btn`/`.icon-btn` classes use it, and your own controls match the host with `:focus-visible { box-shadow: var(--focus-ring); }` |
| Shadows | `--shadow-sm`, `--shadow-md`, `--shadow-lg` |
| Layout (theme-independent) | `--font-ui`, `--font-mono`, `--font-features-text`, `--font-features-code`, `--transition`, `--user-ui-scale`, plus the spacing / radius / motion scales below |
| Stacking | `--z-tooltip` (`10000`), the layer the built-in tooltip paints on. Keep your own overlays under it, so a tooltip is never covered. |

The user's UI font is **`--font-ui`** — that's the canonical token, set live to the
user's font choice. You rarely need to apply it yourself: `sdk-iframe.css` already
sets `body { font-family: var(--font-ui) }` — plus inputs and `.action-btn`, since
form controls don't inherit the page font on their own — so any element that
inherits gets the right font for free. (A *bare* unclassed `<button>` is the gap:
it keeps the browser's own control font. One more reason to use `.action-btn`.)
Only re-declare `font-family` when you deliberately override it, and then use
`var(--font-ui)`. As a safety net,
`--font-family` and `--font` are tolerated **aliases** of `--font-ui` (so the
intuitive guess still resolves to the user's font instead of silently dropping to a
hardcoded fallback) — but `--font-ui` is the name to write.

**`--font-features-text` and `--font-features-code` carry programming ligatures,
and only code gets them.** Fira Code is the default UI font, so unless the user
picked another one they resolve to `"liga" 0, "calt" 0` and `"liga" 1, "calt" 1`;
for every other font both are `normal`. `sdk-iframe.css` applies them for you, the text one on `html, input,
textarea, select, button` and the code one on `code, pre, kbd, samp`, so a code
block in your app ligatures `=>` and `!=` while your prose and your form fields
render literally.

Apply one yourself only on an element that shows code but is none of those tags:
`font-feature-settings: var(--font-features-code, normal)`. Never put the CODE
one on `:root`, `html` or `body`: `font-feature-settings` is inherited, so that
reaches every character in your app, and Fira Code's `calt` re-spaces dot runs
tightly enough that a typed `...` reads as two dots.

Two things to know if you write your own rule, because both look fine in
DevTools and neither shows up in the computed value:

- **`normal` does not mean "ligatures off".** `liga` and `calt` are default-ON
  features in CSS, so `normal` renders identically to `"liga" 1, "calt" 1`.
  Use `var(--font-features-text, normal)` when you want them off, never a bare
  `normal`, and never expect deleting a declaration to disable anything.
- **Form controls do not inherit this property.** The UA stylesheet's `font`
  shorthand resets it on `input` / `textarea` / `select` / `button`, which is why
  they are named explicitly above. A custom control of your own needs the same
  treatment.

The spacing, radius, motion, icon, and type scales are theme-independent and have
fixed values — **use the token, not a magic number, and never a `px` fallback
that disagrees with the real value** (`var(--space-xl, 28px)` is a latent bug —
`--space-xl` is `1.5rem` = 24px). When you include `sdk-iframe.css` these are
always defined, so a fallback is dead noise at best:

| Token | Value | | Token | Value |
|---|---|---|---|---|
| `--space-xs` | `0.25rem` (4px) | | `--radius-sm` | `0.25rem` (4px) |
| `--space-sm` | `0.5rem` (8px) | | `--radius-md` | `0.375rem` (6px) |
| `--space-md` | `0.75rem` (12px) | | `--radius-lg` | `0.5rem` (8px) |
| `--space-lg` | `1rem` (16px) | | `--icon-size-sm` | `0.875rem` (14px) |
| `--space-xl` | `1.5rem` (24px) | | `--icon-size-md` | `1rem` (16px) |
| `--duration-fast` | `0.15s` | | `--icon-size-lg` | `1.25rem` (20px) |
| `--duration-normal` | `0.2s` | | `--duration-slow` | `0.3s` |
| `--duration-emphasis` | `0.5s` | | `--duration-scale` | `1` |

Each `--duration-*` above is its listed value times `--duration-scale`, which is
always `1` inside an app. The host multiplies its own copy by a debugging slider
that slows every transition down for inspection, and a custom property does not
cross into an iframe, so your chrome always animates at the listed durations.
Set `--duration-scale` on your own `:root` if you want the same knob for your
app's transitions.

**Type scale — `--font-size-*`.** The sanctioned font sizes; the host shell and
this SDK stylesheet both size text from these, so use the token instead of a raw
`rem`. All `rem`, so every step scales with the user's UI-scale preference.

| Token | Value | Role | | Token | Value | Role |
|---|---|---|---|---|---|---|
| `--font-size-3xs` | `0.5625rem` (9px) | micro-label / tiny badge | | `--font-size-lg` | `0.875rem` (14px) | emphasis |
| `--font-size-2xs` | `0.625rem` (10px) | dots, micro-meta | | `--font-size-xl` | `1rem` (16px) | section heading |
| `--font-size-xs` | `0.6875rem` (11px) | dense metadata | | `--font-size-2xl` | `1.125rem` (18px) | larger heading |
| `--font-size-sm` | `0.75rem` (12px) | labels, secondary | | `--font-size-3xl` | `1.25rem` (20px) | large heading |
| `--font-size-md` | `0.8125rem` (13px) | body default | | `--font-size-display` | `2.25rem` (36px) | hero |

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

#### Respect the user's font size — size in `rem`, never `px`

The user's UI-scale preference is applied as the **root font-size**
(`html { font-size: var(--user-ui-scale, 100%) }`), so **only `rem`/`em` units
scale with it.** An app that sizes text, padding, gaps, and radii in `px`
renders at a fixed size and silently ignores the user's font-size setting — the
single most common "the app doesn't respect my font size" bug. Size everything
in `rem` (divide px by 16: 14px → `0.875rem`, 24px → `1.5rem`), and prefer the
`--space-*` / `--radius-*` tokens above for spacing and corners. For text, prefer
the `--font-size-*` type-scale tokens over a raw `rem` — body text is
`--font-size-md` (13px), small/meta `--font-size-xs` (11px), emphasis
`--font-size-lg` (14px), headings `--font-size-xl`+ (or the `h1`–`h6` defaults
`sdk-iframe.css` already ships). (`1px` borders are the one acceptable `px`
exception, same as the host shell.)

**The body step is already the default.** `sdk-iframe.css` sets
`body { font-size: var(--font-size-md) }`, so a paragraph you never size
explicitly still lands on Lucidos's body text size instead of the raw root
(`1rem`), which is a size nothing in the host shell renders at. So you don't
need to declare it, and if you do (a text-heavy report might want
`--font-size-lg`), it takes another type-scale step. Never reset it to `1rem` or
a `px` value: that is exactly what makes an app read a whole scale step larger
than the rest of Lucidos, with looser line spacing to match.

### Component classes

`sdk-iframe.css` also ships Lucidos's shared component layer — literally the
**same CSS the host shell uses, from one source**: the engine appends
`crates/lucidos-app/src/styles/global/shared-components.css` (which the host
itself imports via `global.css`) to the served stylesheet. There is no copy and
nothing to keep in sync — apply these class names and your app's controls render
exactly like the rest of Lucidos (and track the theme + UI scale for free). The
one exception is the app-facing `.action-btn-secondary` below, which lives in the
engine's `sdk-iframe.css` (the host has no equivalent, so it isn't in the shared
file). The class names are the contract:

| Class | Use for |
|---|---|
| `.action-btn` (+ `.action-btn-confirm` green, `.action-btn-danger` red) | The filled primary CTA button — blue, with the confirm/danger variants additive (`class="action-btn action-btn-danger"`) |
| `.action-btn-secondary` | A neutral, outlined secondary button for a lower-emphasis action beside a primary CTA — additive: `class="action-btn action-btn-secondary"`. **Use this instead of hand-rolling an off-palette outlined button.** |
| `.button-group` | Wrap a **row of buttons** in this instead of a bare flex row. It keeps the row bound by its container: buttons that do not fit stack onto a second row rather than overflowing, and a single button whose label is wider than the row ellipsizes instead of being sliced by whatever ancestor hides its overflow. Set your own `justify-content` on the same element (the class deliberately sets none) and the buttons keep their natural widths. |
| `.icon-btn` | A small borderless icon button (wrap an SVG sized via `--icon-size-sm`) |
| `.accent-link` | An inline text link/button in the accent color |
| `.label` | A small uppercase badge |
| `.title` | A list/panel/modal title |
| `.segmented-control` + `.segmented-btn` (`.active`) | A toggle button group |
| `.list-rows`, `.list-row`, `.list-row-info`, `.list-row-name`, `.list-row-actions`, `.list-section-title`, … | List/row layouts |
| `.list-row-add-card` (+ `.list-row-add-icon`, `.list-row-add-label`) | The "+ Add <thing>" row that closes a list. **Put it on a `<button type="button">`**, not a clickable `<div>`: the class carries the UA button reset and a `:focus-visible` ring, so on a button the card is in the tab order and answers Enter and Space, and on a div it is reachable by pointer only. Markup is `<button class="list-row-add-card"><span class="list-row-add-icon">+</span><span class="list-row-add-label">Add Thing</span></button>`. |
| `.list-row-details` (+ `.list-row-details-prose`) | The small muted line under a row title. The base class is a flex row of metadata fields whose 0.75rem gap IS the separator between them, so a **sentence** takes the additive prose variant (`class="list-row-details list-row-details-prose"`): under the bare flex class every inline `<strong>`/`<code>` becomes its own flex item, which opens gaps mid-sentence and strands the punctuation after the element at the start of the next line. |
| `.markdown-content` | A container for rendered markdown (headings, tables, code, blockquotes) |
| `.table-scroll-wrapper` | Wrap a `<table>` inside `.markdown-content` in this. A table always fits its container and wraps its cells, at every viewport width, so this is a safety net rather than the normal path: it catches the one overflow that cannot be designed away, a single token wider than the container, and scrolls it inside the wrapper instead of widening your iframe body. Cells are also capped at a readable line length (`60ch`), so one prose column cannot swallow the table and starve the key column beside it. |
| `.image-scroll-wrapper` | Wrap an `<img>` inside `.markdown-content` in this. Unlike a table an image cannot reflow, so this one is the normal path rather than a safety net: the wrapper stays within your container width and pans an oversized image sideways inside itself, at every viewport width, instead of widening the body. The image keeps its natural width (no `max-width` cap, which would shrink a wide screenshot to a thumbnail) and is capped at `24rem` tall with the aspect ratio preserved. An image smaller than the container renders unchanged, with no scrollbar. A bare `<img>` with no wrapper around it is untouched by these rules. |
| `data-stack` + `data-label` (attributes, not classes) | Opt a wide table into the stacked mobile layout: put `data-stack` on the `<table>` and `data-label="<column header>"` on every `<td>`. At 768px and under each row becomes a card, the header row is hidden, and each cell shows its `data-label` above its value. Worth it from about 4 columns up; below that the scroll wrapper reads better. |
| `.progress-bar` + `.progress-bar-fill`, `.progress-label` | A progress indicator |
| `.empty-state`, `.error-text` | Empty/error placeholders |
| `data-tooltip` (an attribute, plus the `#tooltip` rules that paint it) | A themed Lucidos tooltip on any element. You write the attribute and nothing else: `sdk.js` builds, positions and paints the box. Full contract in § Tooltips, under lucidos.ui. |

Prefer these over hand-rolling buttons and rows — a plain unclassed `<button>`
gets a neutral default that does **not** match Lucidos's primary blue button.

**Four of those rows are the overflow half of a wider rule.** `.button-group`,
`.table-scroll-wrapper`, `.image-scroll-wrapper` and `data-stack` each contain
one thing that would otherwise widen the page, and that is all they do. They do
not make an app responsive. `data-stack` is one of the stylesheet's two width
breakpoints and the other only tightens markdown-table type, so every other
width decision is yours to write. The rules, and the three regions the host
paints over a fullscreen app, are in `system-knowhow/building-an-app.md`
§ Responsive by default.

Apps using `lucidos._capture()` don't need to include `html2canvas` — the SDK loads it on demand from `/api/v1/static/html2canvas.min.js`. `html2canvas` can't rasterize CSS Color 4 functions (`color()`, `oklab()`, `oklch()`, `color-mix()`); when the screenshot fails for any reason the capture degrades to **DOM-only** — it returns an empty `screenshot` plus a `dom` layout snapshot (element positions + classes) prefixed with the failure reason, rather than throwing. The agent still sees the rendered layout instead of going blind.

External-host apps point `baseUrl` at the Lucidos instance with `lucidos.configure`:

```ts
lucidos.configure(opts: { baseUrl?: string; token?: string }): void
```

```js
lucidos.configure({ baseUrl: 'https://your-lucidos.example' });
```

`baseUrl` overrides the auto-derived workspace base path (in-app iframes don't
need it — the SDK reads the gateway prefix from `<base href>` / the `/app/` URL).
`token`, when set, is sent as an `Authorization: Bearer <token>` header on every
SDK request — for embedders calling a remote engine that requires auth. Both are
optional and each call merges into the existing config.

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
in-flight fetch when it suspends the page, which says nothing about your
request. Retry an idempotent call rather than reporting it as a failure, and
prefer retrying when the page comes back (`visibilitychange`, `pageshow`,
`focus`) over retrying immediately, because a suspended page cannot reach the
engine either.

The two are deliberately distinguishable. WebKit rejects an aborted fetch with
its own generic `AbortError` rather than the signal's reason, so the SDK
re-stamps a fired deadline as `TimeoutError` to match what Chrome and Firefox
deliver. A cancel you requested stays an `AbortError` even when the deadline
fired in the same instant.

## lucidos.data — File Operations

Read, write, and manage files in the workspace `data/` directory.

> **Paths are relative to `data/`, not `data/artifacts/`.** App code lives in `apps/{id}/`, but app *data* must be written under `artifacts/` explicitly — e.g. `artifacts/{app-id}/data.json`. Omitting the prefix gives a 404 `SdkError` from `read` and a silent failure from `write`.

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

Use **quoted keys** whenever a key contains characters that aren't a bare identifier — dates (`"2026-05-04"`), slugs with dots (`"foo.bar"`), or anything with spaces. Inside a quoted key, `\` escapes the next character. RFC 6901 escaping (`~` → `~0`, `/` → `~1`) is applied automatically.

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

`lucidos.data.url(path)` normally returns a `/data/...` URL, which always serves from the live workspace. When the SDK is loaded inside an app iframe (`/app/<id>/...`) and `path` points at the app's own bundled folder (`apps/<id>/<rest>`), it instead returns a `/app/<id>/<rest>` URL and carries over `?thread_id=` from the iframe. This makes JS-set asset URLs (e.g. `img.src = lucidos.data.url('apps/my-app/icon.png')`) load correctly in WIP-preview — without it, the engine's HTML rewriter only covers markup `src` / `href` attributes and JS-set sources silently 404 against the live workspace. Cross-app references (`apps/<other>/...`) and non-app paths (`artifacts/...`, `knowhow/...`) keep the `/data/` route unchanged.

One other special case: a `system-knowhow/...` path is routed through the engine's `/api/v1/data/...` endpoint (these files live in the engine repo, not the workspace, so the static `/data` mount can't serve them).

## lucidos.events — Event Store

Emit domain events, and query the workspace's event store.

**`query` reads the whole store, not just what your app emitted.** Workspace
domain events (`HabitCompleted`) and the engine's own thread / system events
(`ChildThreadCompleted`, `ResponseGenerated`, `ChangeApplied`, `TriggerCompleted`)
are rows in one `events` table and come back from one call, filtered by
`event_type`, time, `thread_id`, or a paging cursor. There is no second stream
to reach for. See
`system-knowhow/thread-events.md` § "One table, two enums" for what the
`ThreadEvent` / `SystemEvent` distinction actually is.

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
  /** Skip persistence — broadcast on SSE only. */
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

// Emit a transient coordination signal — reaches SSE consumers but
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
    e.payload.status,                 // success | failure | no_changes | canceled
    e.payload.summary
  );
}
```

### Paging with `before_event_id` / `after_event_id`

Rows come back **newest first** (`created DESC, id DESC`) and `limit` is clamped to 1000, so anything longer than one page needs a cursor rather than a bigger `limit`. Pass the id of the oldest row you received as `before_event_id` to get the next page backwards; pass the newest id you already stored as `after_event_id` to tail-follow what has arrived since. Both cursors are exclusive, and both are ids from `LucidosEvent.id` (not `sequence`).

The two are mutually exclusive: set both and the engine answers 400, since "strictly older than X AND strictly newer than Y" has no coherent paging meaning. A cursor id matching no event is a 404, never a silently unfiltered page. `after_event_id` still returns newest-first, so a tail longer than `limit` gives you the most recent slice, not the rows immediately after the cursor.

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

## lucidos.proxy — Call External APIs

Call backends configured in `data/config/apis.json` through the engine. The engine injects the configured auth header from the credential store and strips `Cookie`/`Origin`/`Referer`/`Host` from the forwarded request — **the credential never enters the iframe**.

This is the preferred way for app UIs to talk to external HTTP APIs. Direct `fetch` from the iframe runs into two walls:

- **Mixed content** — apps load over HTTPS, so `fetch('http://localhost:5005/...')` is blocked by the browser.
- **CORS** — the upstream rarely whitelists the engine's origin, so cross-origin XHR fails.

`lucidos.proxy` sidesteps both: the request goes to the same-origin engine, which forwards server-side.

```ts
lucidos.proxy(name: string): ProxyClient

interface ProxyClient {
  fetch(path: string, init?: RequestInit): Promise<Response>;
}
```

`fetch` returns the raw `Response` so the caller picks how to read the body (`.json()`, `.text()`, `.blob()`, …). The auth header is added server-side; do not set `Authorization` from the iframe.

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

Authentication is configured per-API and applied server-side — the iframe never sees credentials, and the URL pattern (`/api/v1/proxy/<name>/<path>`) is identical regardless of auth mode. See `system-knowhow/lucidos-cli.md` § `lucidos proxy` for the full `apis.json` schema (bearer / api_key / basic / query_param / hmac_signed / script_handshake). Omit `auth` for unauthenticated backends (e.g. local services).

### Examples

```js
// GET — unauthenticated local backend
const res = await lucidos.proxy('sonos').fetch('/living-room/play');
if (!res.ok) throw new Error(`Sonos: HTTP ${res.status}`);

// POST JSON — auth header injected by engine
const res = await lucidos.proxy('comfort').fetch('/api/v1/devices', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ deviceGuid: 'abc' }),
});
const data = await res.json();
```

### Built-in model-provider proxies (no `apis.json` entry needed)

The engine already holds working credentials + routing for every model provider in the model registry (Settings → Models). Those are exposed as **built-in provider proxies** under the SAME route, so an app can call an LLM / image provider without the workspace re-entering the credential in `apis.json`. When `<name>` matches a model-registry provider and has no `apis.json` entry, the engine forwards to that provider's API root and injects its credential server-side:

| `proxy(name)` | Base URL | Injected server-side | You send |
|---|---|---|---|
| `openai` | `https://api.openai.com/v1` | `Authorization: Bearer <key>` | path as-is, e.g. `/chat/completions`, `/images/generations` |
| `openrouter` | `https://openrouter.ai/api/v1` | `Authorization: Bearer <key>` | path as-is, e.g. `/chat/completions` |
| `xai` | `https://api.x.ai/v1` | `Authorization: Bearer <key>` | path as-is, e.g. `/chat/completions` |
| `anthropic` | `https://api.anthropic.com/v1` | `x-api-key: <key>` (or `Authorization: Bearer` for an OAuth credential) | path as-is, e.g. `/messages` — set your own `anthropic-version` header |
| `local` | your configured local base (Ollama default `http://localhost:11434/v1`) | `Authorization: Bearer <key>` (omitted if keyless) | path as-is, e.g. `/chat/completions` |
| `vertex` | `https://<region>-aiplatform.googleapis.com/v1/projects/<project>/locations/<region>` (engine-owned prefix) | `Authorization: Bearer <access-token>` (minted + refreshed server-side) | ONLY the suffix, e.g. `/publishers/anthropic/models/claude-opus-4-8@default:rawPredict` |

- **Only the credential is injected.** The layer adds just the auth header (the secret the iframe must never see). `Content-Type`, `anthropic-version`, and any attribution headers stay yours to set in `init`.
- **`apis.json` overrides the builtin.** An entry with the same name in `data/config/apis.json` is used instead — so you can still point `openai` at a mock/gateway or add extra auth layers.
- **Vertex is addressed by suffix.** The engine owns the `…/projects/<project>/locations/<region>` prefix (project + region from its own Vertex config, region default `europe-west1`) and mints the OAuth token — so the app never needs the project id or a token. Send only `/publishers/<publisher>/models/<model>:<method>`. The region is fixed to the engine's configured region; a model that must run in another location (e.g. a `global`-only Gemini variant) needs an `apis.json` override.
- **Not configured → 404.** If the provider has no credential/config (and no `apis.json` entry), the call returns 404 naming what to set.

```js
// Chat via the built-in OpenAI proxy — no apis.json, no key in the app
const res = await lucidos.proxy('openai').fetch('/chat/completions', {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ model: 'gpt-5.6-sol', messages: [{ role: 'user', content: 'hi' }] }),
});

// Claude on Vertex — the app sends only the publisher/model suffix
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
| Call a model provider the engine already has (LLM / image) | `lucidos.proxy('openai' \| 'vertex' \| 'openrouter' \| 'xai' \| 'anthropic' \| 'local').fetch(...)`, no `apis.json` needed |
| Call any other external HTTP API | `lucidos.proxy(name).fetch(path, init)` + an `apis.json` entry |
| Hit an engine endpoint no SDK method covers | `fetch(lucidos.apiUrl('/<suffix>'))`. Same origin, no proxy needed, but the URL **must** be built with `apiUrl`: a hand-written `/api/v1/…` is a 404. See § `lucidos.apiUrl` directly below. |

If the iframe needs a model provider the engine already has, use its built-in proxy name above — no config. For any other external API the workspace doesn't have a proxy entry for, add one to `data/config/apis.json` rather than embedding the credential in the app.

## lucidos.apiUrl

```ts
lucidos.apiUrl(suffix: string): string   // synchronous, returns URL
```

Builds an absolute URL onto the engine's `/api/v1` surface, carrying the
**workspace address** (the `/<slug>` path prefix) this app is served under.
Pass the path *after* `/api/v1`.

```js
// The endpoint has no SDK method, so build the URL rather than writing one.
const res = await fetch(lucidos.apiUrl('/notifications'), {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({ title: 'Done', message: 'Import finished.' }),
});
```

**Reach for it only when no SDK method covers the endpoint.** `lucidos.data.*`,
`lucidos.events.*`, `lucidos.threads.*` and the rest already resolve the prefix,
and they carry the timeout, error shape and response parsing this raw `fetch`
does not.

### Why a hand-written `/api/v1/…` does not work

An app iframe is served at `/<workspace>/app/<app-id>/`, and the engine's HTTP
surface lives at `/<workspace>/api/v1/…`. Both of the URLs an author reaches for
first resolve somewhere else:

| Written in JS | Resolves to | Answer |
|---|---|---|
| `new URL('api/v1/events/query', document.baseURI)` | `/<workspace>/app/<app-id>/api/v1/events/query` | `404` |
| `fetch('/api/v1/events/query')` | `/api/v1/events/query` | `404 unknown workspace 'api'` |

The relative form fails because **an app iframe has no `<base href>`**. The SPA
shell gets one stamped in (`<base href="/<workspace>/">`), an app page does not,
so `document.baseURI` is the app's own directory and every relative path hangs
off it. The root-absolute form fails because the gateway reads the **first path
segment as a workspace name**, and there is no workspace called `api`.

**Markup is rewritten on the way out, runtime JS is not.** This is the
non-obvious part, and it is why the boilerplate in § Setup works at all: the
engine rewrites root-absolute `src` / `href` **attributes** in the HTML it
serves, so the `<script src="/api/v1/sdk.js">` sitting in the app's
`index.html` reaches the browser as
`<script src="/<workspace>/api/v1/sdk.js">`. Nothing does that for a string your
JavaScript builds at runtime. **The same `/api/v1/…` string is correct in markup
and broken in JS.**

`apiUrl` derives the prefix the way the SDK derives it internally: the
`<base href>` when the document has one, otherwise everything before `/app/` in
the path. Don't re-derive it in app code, and never hardcode a slug: the
workspace name is not the app's to know. (`lucidos.configure({ baseUrl })` is
the one override, for an app hosted outside the engine.)

**The failure mode is silence.** A wrong URL is a plain 404, so an app that
wraps the call in a `try` / `catch`, warns to the console and falls back to a
second data source goes on looking healthy: it renders plausible, stale numbers
and nothing on screen changes. That is how this survived weeks in a real app.
If a fetch of yours has a fallback path, surface the failure in the UI as well
as the console.

## lucidos.oauth — OAuth Token Access

Fetch a short-lived OAuth access token for a connected provider, for in-browser SDKs that need a bearer token in JavaScript (e.g. the Spotify Web Playback SDK). The engine looks up the connected account, refreshes the token if it's expired or expiring within 60s, and returns ONLY the access token — the refresh token never leaves the engine.

```ts
lucidos.oauth.getAccessToken(provider: string): Promise<AccessToken>

interface AccessToken {
  accessToken: string;
  expiresAt: Date | null;  // null when the upstream provider didn't include an expiry
}
```

### When to use

- **You need a bearer token in the iframe**: a third-party SDK like `Spotify.Player` calls a `getOAuthToken` callback expecting a raw token string. There is no other way to hand it the credential — `lucidos.proxy(...)` can't help because the SDK initiates the request itself, not through your code.
- **You are NOT making ordinary HTTP calls to the upstream API**: for those, use `lucidos.proxy(<provider>).fetch(...)` instead — the engine attaches the bearer header server-side and the iframe never sees the token. Only fall back to `getAccessToken` when something forces you to hand a raw token to in-browser code.

### Example — Spotify Web Playback SDK

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

The SDK calls `getOAuthToken` on first init and again when it detects the token has expired — each call hits the engine, which refreshes from the stored refresh token if needed.

### Errors

- `404` — the provider is not connected for this workspace. Ask the user to connect it via the OAuth account settings (or through the LLM `connect_oauth_account` tool).
- `502` — the engine could not refresh the token (missing client credentials, upstream rejected the refresh, network failure).

### Security note

The refresh token, client_id, client_secret, and PKCE state stay on the server. The iframe receives ONLY the short-lived access token, scoped to the connected account. Apps therefore must NOT cache the access token in `localStorage` / `sessionStorage` — re-call `getAccessToken` whenever you need a fresh one (the engine handles caching and refresh).

## lucidos.triggers — Scheduled Tasks

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
`go_to_review` routing, indistinguishable downstream from a scheduled fire. Use
it for a "Sync now" button in an app rather than re-implementing the trigger's
work in the app.

It resolves when the run is **admitted**, not when it finishes, so a truthy
`success` is not "the work is done". Branch on `status`:

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
// to that event. A trigger may carry several entries — it fires when an
// incoming event matches *any* entry's event_type AND that entry's
// condition (if set) evaluates true against the payload. Conditions are
// per-entry so different events with different payload shapes never
// constrain each other.
interface EventSubscription {
  event_type: string;
  condition?: Record<string, unknown>;
}

// Irreversible-side-effect category a trigger can be granted. Only enforced
// when the workspace's command guard is on (Settings → Permissions → Command
// Safety). A trigger that hits an irreversible command whose category isn't in
// its grant is failed (it can't be asked to approve — it runs unattended).
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
  // Side-effect grant — irreversible categories this trigger may perform
  // unattended. Omitted when empty (= no grant).
  side_effect_grant?: SideEffectCategory[];
  // Chat model this trigger's intent fires on, and its thinking budget. Both
  // omitted when the trigger follows the account default (Settings → Models →
  // Chat & triggers). Intent triggers only: a script trigger runs no LLM.
  model?: string;
  reasoning_effort?: string;
}

interface CreateTrigger {
  name: string;
  run: TriggerRun;
  cron_expressions: string[];
  on?: EventSubscription[];
  /** Optional *trigger group* id; omit for ungrouped. */
  group_id?: string;
  /** Side-effect grant — irreversible categories this trigger may perform
   *  unattended. Omit / `[]` = none granted (the safe default). */
  side_effect_grant?: SideEffectCategory[];
  /** Pin the intent to a chat model and a thinking budget
   *  (`none|low|medium|high|xhigh|max`). Omit either for the account default. */
  model?: string | null;
  reasoning_effort?: string | null;
}

interface UpdateTrigger {
  name?: string;
  run?: TriggerRun;
  cron_expressions?: string[];
  paused?: boolean;
  // Full replacement for the subscription list. Send the complete new set —
  // there is no partial edit. Pass `[]` to clear all subscriptions.
  on?: EventSubscription[];
  /** Move into a group (string id), clear membership (null), or leave it
   *  unchanged (absent). */
  group_id?: string | null;
  /** Full replacement for the side-effect grant; pass `[]` to clear all. */
  side_effect_grant?: SideEffectCategory[];
  /** Pin the intent's model / thinking budget (string), clear it back to the
   *  account default (null), or leave it unchanged (absent). */
  model?: string | null;
  reasoning_effort?: string | null;
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

Each entry's `condition` only applies to its own `event_type` — the `from: 'partner'` filter on `MessageReceived` does NOT block `EmailReceived` from firing on its own filter.

### Cron validation on create and update

Within one cron expression the fields are **ANDed**; across the array they are **ORed**. So `0 0 9 1 * Mon` fires only when the 1st IS a Monday (roughly 1.7 times a year), not on the 1st and every Monday, which is two expressions. See `system-knowhow/triggers.md` § "Writing cron expressions" for the nth-weekday and last-weekday recipes.

An expression that can **never** fire (`0 0 9 31 2 *`, Feb 31, and its relatives) is rejected: `success: false` with an `error` naming the offending fields. Do not retry it, and do not present it to the user as a transient failure; the expression itself is wrong.

Every accepted create / update returns `cron_preview`. Show `next_runs` in your app's confirmation so the user sees what they actually scheduled, and surface each entry of `warnings` (currently the day-of-month/day-of-week AND footgun) rather than dropping it.

Trigger groups are user-visible folders shown in the triggers panel. Pure organizational labels — they have no schedule, run no code, and don't coordinate firing. Apps that organize the triggers they create can pass `group_id` to `create` / `update`; the engine validates the id against the workspace's group registry and rejects unknown values. The SDK does not expose group CRUD today — group management lives behind the engine's HTTP and LLM-tool surfaces.

## lucidos.apps — App Management

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
  /** Optional icon from the app's manifest.json (emoji or asset path).
   *  Omitted when the manifest has none. */
  icon?: string;
}
```

The shape mirrors the app's `manifest.json` (`name` / `description` / `icon`) plus
the `id` derived from its folder. `list()` hits `GET /api/v1/apps`; `get(id)`
hits `GET /api/v1/app?id=<id>` and throws a `404` `SdkError` for an unknown id.

### Example

```js
const apps = await lucidos.apps.list();
const me = await lucidos.apps.get('habit-tracker');
console.log(me.name, me.icon ?? '(no icon)');
```

## lucidos.preferences — User Settings

```ts
lucidos.preferences.get(deviceId?: string | null): Promise<Preferences>
lucidos.preferences.set(key: string, value: string, deviceId?: string): Promise<void>
```

`get()` defaults to the parent device id, so iframes see the same merged view as
the parent UI. The device id is per-workspace (each workspace has its own device
identity); the SDK reads it from the workspace-scoped `lucidos-device-id`
(`ws:<slug>:lucidos-device-id`) so the iframe and the parent agree. Pass `null`
to fetch only globally-scoped preferences.

### Types

```ts
type Preferences = Record<string, string>;
```

### Common keys

| Key | Values | Description |
|-----|--------|-------------|
| `theme` | `dark`, `light`, `system` | UI theme |
| `font-family` | `monospace`, `system`, `inter`, `jetbrains-mono`, `ibm-plex-mono`, `fira-code` | Font (`fira-code` also enables programming ligatures, on code and `pre` blocks only, via `--font-features-text` / `--font-features-code`) |
| `ui-scale` | Number in 12.5% steps from 75 to 200 (`75`, `87.5`, `100`, `112.5`, `125`, `137.5`, `150`, `162.5`, `175`, `187.5`, `200`); or the legacy strings `small` / `medium` / `large` (= `100` / `112.5` / `125`). Off-grid numbers snap to the nearest valid step. | Scale |

## lucidos.notifications — Notification Center

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

// Settings sub-section for `target: 'settings'`. Every top-level Settings
// category plus the System subpanels: no category is platform-gated, so none
// has to be withheld from a caller with no platform signal.
type SettingsViewTarget =
  | 'models' | 'permissions' | 'coding-agents' | 'accounts' | 'locale' | 'marketplaces'
  | 'access' | 'devices' | 'system' | 'appearance' | 'keyboard-shortcuts'
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
 *  notification read on tap. Every notification is openable — the old passive
 *  `none` kind is retired; a historical `{kind:'none'}` is coerced to `modal`. */
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
  /** Specific event UUID inside `thread_id` that raised this notification —
   *  the §4 in-app matrix uses it to silently mark-read when the user is
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

### Tap shapes — examples

The SDK only exposes `list` / `markRead` / `markAllRead` for reading the inbox. Creating a notification from app code goes through the engine HTTP API directly (`POST /api/v1/notifications`, the same wire shape the `lucidos notify` CLI and the `send_notification` LLM tool produce). This is the ordinary case for § `lucidos.apiUrl`: build the URL with it rather than writing `/api/v1/notifications` into the `fetch`, which 404s from an app iframe.

```js
// Default: open the inbox detail showing the message body. Use this for any
// info-only notification too ("OAuth completed", "Build succeeded") — every
// notification is openable; there is no separate passive kind. For ephemeral
// status that should NOT land in the inbox at all, use a plain `showToast`.
await fetch(lucidos.apiUrl('/notifications'), {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    title: 'Daily summary',
    message: 'Here is your summary…',
    tap: { kind: 'modal' },
  }),
});

// Navigate to a panel.
await fetch(lucidos.apiUrl('/notifications'), {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    title: '5 changes ready to apply',
    message: 'Review the Changes panel.',
    tap: { kind: 'navigate', to: { target: 'changes' } },
  }),
});

// Navigate to a thread, optionally scroll-and-pulse a specific event row.
await fetch(lucidos.apiUrl('/notifications'), {
  method: 'POST',
  headers: { 'Content-Type': 'application/json' },
  body: JSON.stringify({
    title: 'Coding agent is asking',
    message: 'Permission needed.',
    thread_id: 't-9',
    event_id: 'e-7',
    tap: { kind: 'navigate', to: { target: 'thread', id: 't-9', event_id: 'e-7' } },
  }),
});

// Navigate to an app's UI, at the one place the notification is about.
// `fragment` arrives as the app's location.hash (§ Navigation targets).
await fetch(lucidos.apiUrl('/notifications'), {
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

From scripts (Python/bash), use the `lucidos notify` CLI — it constructs the same body. From LLM threads, use the `send_notification` tool.

## lucidos.threads — Thread Management

```ts
lucidos.threads.list(opts?: ThreadsListOptions): Promise<ThreadSummary[]>
lucidos.threads.count(opts?: Omit<ThreadsListOptions, 'limit'>): Promise<number>
```

`list()` calls `GET /api/v1/threads/list` and returns a newest-first array of `ThreadSummary` rows from the projection. `count()` calls `GET /api/v1/threads/count` and resolves to the integer count under the same filter — cheaper on big workspaces than reading `(await list()).length`.

Same canonical surface as the `lucidos threads list` / `lucidos threads count` CLI and the `list_threads` / `count_threads` LLM tools. Use this when an app needs to render or react to thread state (counts, status indicators) without subscribing to the full SSE stream.

**`active` is a union, `status` is precise.** `active: true` selects `running` OR `waiting_for_user_answer`, and those two are opposites: `running` is the workspace working, `waiting_for_user_answer` is the workspace stopped and waiting on a person. An app asking "is anything busy?" wants `status: 'running'`; an app rendering "N threads I have something invested in" wants `active: true`. Passing both is a 400.

### Types

```ts
interface ThreadsListOptions {
  /** The UNION of 'running' and 'waiting_for_user_answer'. true selects it,
   *  false inverts it, omitting it filters nothing. For "is the workspace
   *  busy?" pass status: 'running' instead: a thread awaiting a user answer is
   *  blocked on the human, not working. 'waiting' is in neither. Nothing
   *  writes it now, so it only appears on older rows; a thread carrying
   *  changes to review is 'idle', or the verdict its turn ended on.
   *  Mutually exclusive with status. */
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
  /** Thread id. Restrict to that thread's DIRECT children only, never its
   *  grandchildren. Same filter as the `--parent` CLI flag and the
   *  `list_threads` tool's `my_children` (which resolves it from the calling
   *  thread; an app has no calling thread, so it names one). A malformed
   *  uuid is a 400, never a silently unfiltered list. */
  parent?: string;
}

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
  /** When the agent (or trigger) last did something — streaming, a terminal
   *  response, an idle, a trigger fire/complete, or asking the user. */
  last_agent_action: string;
  message_count: number;
  /** Whether the user parked this thread in the Saved section (stored in
   *  thread_summaries.is_saved). */
  saved: boolean;
  /** 'inbox' | 'archived' — stored in thread_summaries.archive_state. */
  section: string;
  active_children_count: number;
  total_children_count: number;
  /** Transitive descendants currently in a state that blocks this thread from
   *  being archived (Running / WaitingForUserAnswer / pending in-workspace
   *  coding-agent changes). `> 0` ⇒ "N sub-threads still busy". */
  blocking_descendant_count: number;
  /** Strict subset of `blocking_descendant_count` that drops the Running case —
   *  descendants needing *user attention* (WaitingForUserAnswer, or pending
   *  changes). Drives REVIEW bubbling up the ancestor chain. */
  attention_descendant_count: number;
  /** 'idle' | 'running' | 'waiting' | 'paused' | 'failed' | 'waiting_for_user_answer'.
   *  The same values the `status` filter above accepts, so you can filter on
   *  what you read. `running` is the workspace working; `waiting_for_user_answer`
   *  is it stopped and waiting on a person (the `active` union covers both).
   *  `paused` = the user's own version switch interrupted the turn and the engine
   *  is resuming it, so nothing is being asked of anyone. Any OTHER interruption
   *  (a crash, or a switch whose resume the boot could not deliver) is `failed`
   *  and offers a Continue button. */
  status: string;
  coding_agent_has_diff: boolean;
  coding_agent_proposed: boolean;
  coding_agent_requires_restart: boolean;
  coding_agent_is_external_repo: boolean;
  coding_agent_applying: boolean;
  last_revived_at: string | null;
  parent_thread_id?: string | null;
  parent_thread_title?: string | null;
  trigger_id?: string | null;
  trigger_name?: string | null;
  cc_repo_id?: string | null;
  cc_repo_name?: string | null;
  /** Coding-agent thread flavor — `'lucidos' | 'app' | 'external'`. Omitted for
   *  non-coding-agent threads (and legacy rows, which consumers default to
   *  `'lucidos'`). */
  coding_agent_kind?: string;
  /** Canonical folder the coding agent operates on — `<ws>/data/apps/<id>/` for
   *  an app thread, the repo root otherwise. Omitted for non-coding-agent threads. */
  coding_agent_folder?: string;
  /** Which backend drives the thread — `'claude-code' | 'codex'`. Omitted for
   *  non-coding-agent threads (legacy rows default to `'claude-code'`). */
  coding_agent?: string;
  /** Compose state machine — `composing` | `active` | `discarded`. The
   *  archive flag is on the separate `section` field; an archived thread
   *  carries `state: 'active'` and `section: 'archived'`. */
  state: 'composing' | 'active' | 'discarded';
  compose_text: string;
  compose_images: string[];
  compose_mode?: 'lucidos' | 'claude_code' | null;
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

## lucidos.ui — UI Control

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
lucidos.ui.prompt(options: PromptOptions): Promise<string | null>
lucidos.ui.Select.create(opts: SelectCreateOptions): SelectInstance
lucidos.ui.enhanceSelects(root?: ParentNode): SelectInstance[]
lucidos.ui.disableTooltips(): void
```

`applyPreferences()` fetches user preferences and applies theme, font, and scale as CSS variables (resolving a `system` theme to the live OS light/dark). Call once on app load, and style your app with the theme variables (§ Theme variables, under Setup) so it follows the user's appearance — don't hardcode colors. For each setting it prefers the server value, then the value the synchronous `sdk-prefs.js` script already applied from the parent shell's `localStorage`, and only then a default — so a device with no server-scoped value (e.g. only `ui-scale` stored, no `theme`) keeps the user's appearance instead of resetting to dark.

`applyPreferences()` also applies the user's **style overrides**: the
`style_overrides` preference holds a map of CSS custom property to value, which
it writes onto `<html>` after theme, font and scale (so an override of one of
those wins). That is what keeps an app's chrome matching a host the user has
retuned. Only custom properties are honoured, and a value containing `;`, `{`,
`}`, `<`, `>`, `@`, a backslash, `url(`, `image-set(`, `expression(` or a
comment opener is dropped, because the map is writable by any app and must not
be able to inject a declaration or fetch from another origin. Nothing is
required of an app beyond calling `applyPreferences()`.

`watchPreferences()` subscribes to live preference changes (SSE `PreferencesChanged`) and re-applies them automatically. Call it once alongside `applyPreferences()` so the app reacts without a reload: when the user toggles light/dark, when the OS appearance changes under a `system` preference, or when a value is retuned from the Style Remote.

Under a `system` preference the OS appearance is watched two ways, because neither alone is enough on every client. The `prefers-color-scheme` media query covers a flip while the app is on screen. The frame's resume (`visibilitychange`, `focus`, `pageshow`) covers one announced while it was not. That is the normal case in an installed iOS PWA, which is resumed rather than reloaded. Both are sampled a moment after the event and only re-apply when the resolved theme actually moved, so a wake that changed nothing costs nothing. Your app needs to do none of this: it is inside `watchPreferences()`.

`navigate()` sends a navigation request to the Lucidos frontend via SSE. `target`
and `params` (`NavigateParams` = `NavigateUi` minus `target`) are typed against the
generated navigation contract, so valid `target`s and `settings_view`s are
discoverable and type-checked (§ Types, under lucidos.notifications).

### Navigation targets

| Target | Params | Description |
|--------|--------|-------------|
| `thread` | `id` | Focus a specific thread |
| `app` | `id` (or `app_id`), `fragment` (optional) | Open an app UI, optionally at a place inside it. See the fragment param below. |
| `settings` | `settings_view` (optional) | Open Settings, optionally a sub-section: `models`, `permissions`, `coding-agents`, `accounts`, `locale`, `marketplaces`, `access`, `devices`, `appearance`, `keyboard-shortcuts`, or a System subpanel (`system`, `release-notices`, `whats-new`, `backup`, `memory`, `disk-usage`, `environment-variables`, `thread-queue`, `debugging`). Omit `settings_view` for the Settings home list. |
| `new-chat` | `prompt` (optional) | Open a fresh chat thread, optionally prefilling the compose textarea. Prefer `lucidos.ui.startThread()` — it's the typed wrapper around this target. |
| `plugins` | `id` (optional) | Open the Plugins panel's Installed tab. With `id` (a plugin id), scroll to and pulse-highlight that plugin's row — used by the plugin-update notification so a tap lands on the plugin that has the pending update. |
| `app-store` | — | Open the Plugins panel's Store (marketplace) tab. |
| `file` | `file_path`, `line` (optional), `line_end` (optional) | Open a file in the preview pane, optionally at a line. See the two accepted path forms and the line params below. |
| _other panels_ | — | `files`, `apps`, `triggers`, `thread-queue`, `changes`, `notifications`; plus `trigger` (`id`), `url` (`url`), `new-app`, `new-trigger`. |

#### `fragment`: opening at a place inside the app

**Whenever you name a specific item, pass it.** The app opens with that string as
its `location.hash`, so an app that routes on the hash lands on the item. Without
it the app opens on whatever the reader last looked at. On a sorted board that
can be several cards away from the one you meant.

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

#### `file_path` — workspace data vs a registered repository

`file_path` takes one of two forms:

- **A workspace data path** — `artifacts/…`, `knowhow/…`, `apps/…`, `triggers/…`, or `system-knowhow/…`. A path with none of those prefixes is treated as an artifact, so `notes.md` opens `artifacts/notes.md`.
- **A repo-encoded path**: `repo:<repoId>:file:<repo-relative path>`, which opens a file from a **registered repository** (a local clone added under Settings → Coding Agents) instead of the workspace data tree. `<repoId>` is that Repository's id, as returned by `GET /api/v1/repositories`; the file is read at the clone's current `HEAD`.

```js
// Open src/main/resources/transforms/order.jslt from a registered repo clone.
await lucidos.ui.navigate('file', {
  file_path: `repo:${repoId}:file:src/main/resources/transforms/order.jslt`,
});
```

The preview pane binds itself to that repository, so the Files panel behind it and the preview's changed-files sidebar stay on the same repo. A malformed `repo:…` string is not a repo path — it falls back to the artifact rule above.

##### Naming a revision: `repo:<repoId>:file#<ref>:<path>`

The bare form reads the clone's `HEAD`, which is often not where the interesting content is: a file a coding agent has edited lives on that agent's worktree branch, and a citation into a released version means a tag or a sha. Add `#<ref>` to the `file` segment to say which revision you mean.

```js
// The file as it stands on a coding agent's branch, not as it stands on HEAD.
await lucidos.ui.previewFile({
  file_path: `repo:${repoId}:file#${branchName}:src/main.rs`,
  line: 510,
});
```

- `<ref>` is anything `git show` accepts as a revision: a branch, a tag, a full or short sha.
- It works on both calls and in the href form below, since it is part of the path string rather than a separate parameter.
- Omit it and you get `HEAD`, exactly as before.
- A ref that does not exist (or a file that does not exist at it) shows the preview's normal "failed to load" state, not a thrown error.
- **Every segment must be non-empty.** `repo:<repoId>:file#:<path>` names no revision and is not a repo path at all, so it falls back to the artifact rule like any other malformed `repo:…` string. Leave the `#` off instead.

A ref cannot contain `:` (git forbids it), which is what keeps the `:`-separated form unambiguous; a `/` in a branch name is fine.

`diff` locators do not take a `#<ref>`: `repo:<repoId>:diff#<changeId>:<path>` already names its revisions through the change.

#### `line` / `line_end`: opening at a cited line

**Whenever you cite a specific line, pass it.** The preview then scrolls that line into view and highlights it, exactly as if the reader had clicked its line number. Without it the file opens at the top and a `file.rs:510` citation leaves the reader to find line 510 by hand, which is the whole value of the citation lost at the last step.

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

A line the file can't honour never costs the reader the file: `0`, a negative or fractional number, a line past the end of the file, and a format with no source view at all (PDF, an image) are all ignored, and the file opens at the top as it would with no `line` at all. A citation's line number is the part that goes stale, so this is deliberate rather than an error.

#### Linking to a repo file from an HTML artifact

An `<a href>` inside a **previewed HTML or markdown artifact** can use the repo-encoded path directly, with a GitHub-style line suffix:

```html
<a href="repo:REPO_ID:file:src/main.rs#L510-L520">src/main.rs:510-520</a>
```

The host routes that click through the same navigation this section describes, so a report full of citations works as a plain artifact and does not have to be published as an app to reach `lucidos.ui.navigate`. `#L510` is a single line; `#L510-L520` (or `#L510-520`) is a range. The suffix exists only for hrefs, since an anchor has no other way to carry a param: from JavaScript, use `line` / `line_end` above.

The revision form composes with it. The two `#` never compete: the line suffix is the trailing one, and the ref is the one inside the `file` segment.

```html
<a href="repo:REPO_ID:file#release/2.4:src/main.rs#L510-L520">src/main.rs:510-520 on release/2.4</a>
```

### Showing a cited file without leaving your app

`navigate('file', …)` takes the whole shell into the Files panel. For a report or a dashboard full of citations that is the wrong motion: the reader loses their place and has to navigate back. `lucidos.ui.previewFile(params)` shows the file in a **file preview modal** over your app instead, so they glance at the code and carry on.

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

Everything the two sections above specify applies unchanged: every `file_path` form (the named-revision one included), `line` / `line_end` 1-based and inclusive, and the same degradation for a line the file cannot honour. The modal shows the same rendering the Files panel shows, with the same highlight and line numbers, and it carries an **Open in Files** link that escalates the glance into exactly the `navigate('file', …)` you would otherwise have called, at the same lines.

Naming the revision matters more here than anywhere else: the modal may be showing a repository the Files panel is not bound to, so it cannot fall back to whatever branch that panel happens to be on. Without a `#<ref>` it reads `HEAD`.

| Want to … | Use |
|---|---|
| Let the reader check a citation and keep reading | `lucidos.ui.previewFile({ file_path, line })` |
| Send the reader to the file to work with it (edit, pick a range for chat, browse the tree) | `lucidos.ui.navigate('file', { file_path, line })` |

Three things to know:

- **It resolves when the preview is on screen, not when the reader dismisses it.** A glance can stay open for minutes and your app is not blocked while it is. It rejects when the host cannot put it on screen, which makes the escalation a natural fallback:

  ```js
  try { await lucidos.ui.previewFile(at); }
  catch { await lucidos.ui.navigate('file', at); }
  ```

  Two things make it reject, and both mean "nothing would have appeared". Your app is running with **no host shell around it**: opened in its own tab, or the SDK loaded in a plain page. Or **something is fullscreen that the host cannot render over**, which in practice means your app called `requestFullscreen` on its own content. Fullscreen taken from the Lucidos content header is fine, and so is everything else: the preview appears over your app there like anywhere else. Write the `catch` and you are covered in all of them.

- **Read-only.** There is no editing in the modal; `navigate('file', …)` is the way to the editable preview. A second `previewFile` replaces a showing one.
- **A `repo:…:diff#…:…` locator previews the file, not the diff.** The diff view belongs to the Files panel; use `navigate` for it. The change is not thrown away though: the file is shown at that change's end state, so a citation into a coding agent's work shows the work. Because it is a file view, its lines ARE honoured, unlike the same locator through `navigate`.

Since one of those two reject causes is about fullscreen, the ordinary case is worth stating plainly: `previewFile`, `confirm`, `prompt` and `toast` are all rendered by the host, and all of them appear over your app when the reader has put it in fullscreen from the content header. Escape closes what is in front: with the app pseudo-fullscreen (iOS, and anywhere the Fullscreen API is unavailable) one Escape closes the modal and the app stays fullscreen; with real fullscreen the browser claims that first Escape to leave fullscreen, so the modal stays up in the normal layout and the next Escape closes it.

### Opening a link outside Lucidos

`lucidos.ui.openExternal(url)` sends a URL out of the app. **Use it instead of
`window.open` for any link that leaves Lucidos.**

Plain anchors are already handled for you: the SDK's link interceptor catches
`<a href="https://…">` clicks and routes them here automatically. Reach for
`openExternal` when you open a URL from JavaScript instead (a button handler, a
row action, a redirect after a fetch).

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
  cannot leave the app: WebKit renders it in an in-app web view with no address
  bar, no tabs and no shared Safari session. That overlay is exactly what the
  user's `external_link_target` preference (`safari` / `ask` / `in-app`, default
  `safari`) exists to control, so falling back to it overrides their choice.

Non-http(s) URLs (`mailto:`, `tel:`) are handed to the platform unchanged. The
promise resolves once the open has been dispatched; a user dismissing the share
sheet resolves normally rather than rejecting.

### Starting a fresh chat with a prefilled prompt

`lucidos.ui.startThread()` opens a new chat thread. If you pass a `prompt`, it lands in the compose textarea **prefilled** — the user reviews, edits, and clicks Send. It is never auto-submitted, so the user always stays in control of what gets sent on their behalf.

```js
// "Set this up for me" button — pops a fresh chat with a ready-to-send prompt.
document.querySelector('#setup-trigger').addEventListener('click', () => {
  lucidos.ui.startThread({
    prompt: 'Create a daily 9am trigger that summarizes my unread email.',
  });
});
```

Call with no arguments (`lucidos.ui.startThread()`) to just open a blank fresh chat — equivalent to the user pressing the "new thread" shortcut.

### Confirmation dialogs

`lucidos.ui.confirm` shows a modal rendered by the Lucidos shell (not inside your app iframe), so it inherits the user's theme and sits above all app content. Use it instead of `window.confirm()`.

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

Resolves `true` on OK click or Enter; `false` on Cancel, Esc, or backdrop click.

If a second `confirm` is called while one is visible, the previous one resolves `false` and the new one replaces it.

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

`lucidos.ui.toast` shows a transient status banner rendered by the Lucidos shell
(above all app content, themed by the user's preferences). It's **fire-and-forget**
— no return value, no result to await. Use it for success/error feedback instead
of hand-rolling your own banner.

```ts
type ToastType = 'success' | 'info' | 'warning' | 'error';

interface ToastOptions {
  /** Auto-dismiss after this many ms. Omit for the host default: errors and
   *  warnings stay until dismissed; success/info auto-close. */
  durationMs?: number;
  /** false = hide the close (X) button. Default true. */
  dismissable?: boolean;
  /** Stable key for in-place replacement. A later toast with the same key
   *  updates the existing toast (message/type/etc.) instead of stacking a new
   *  one — e.g. an 'Opening…' toast becoming 'Opened'. */
  key?: string;
  /** true = show an indeterminate "work in progress" spinner in place of the
   *  severity icon. Pair it with a `key`, so a later keyed toast can replace
   *  the spinner with the outcome. Indeterminate only: no percentage. */
  spinning?: boolean;
}
```

`type` defaults to `'info'`; an unknown value degrades to `'info'`. Only this
serializable subset is exposed — the host's toast action buttons take `onClick`
callbacks, which can't cross the app-iframe boundary, so they aren't available
from an app.

**Example:**

```js
lucidos.ui.toast('Saved', 'success');
lucidos.ui.toast('Could not reach the server', 'error');
lucidos.ui.toast('Working on it…', 'info', { durationMs: 2000 });

// Collapse a two-step status into one toast that updates in place:
lucidos.ui.toast('Opening from Drive…', 'info', { key: 'drive-open' });
lucidos.ui.toast('Opened "Q3 deck"', 'success', { key: 'drive-open' });
```

#### Long-running work: a spinner you can take back down

`spinning: true` swaps the severity icon for a small indeterminate spinner, so a
keyed toast can narrate work that has no honest percentage. Its counterpart is
`lucidos.ui.dismissToast(key)`, which takes that toast back down. That covers the
one case a keyed replacement can't express: work that finishes with nothing left
to say.

`dismissToast` is fire-and-forget like `toast`, and **a key matching nothing is a
no-op**, never an error. Your app can't know whether the toast is still up (the
user may have closed it, or its `durationMs` may have expired), so "already gone"
is the normal case rather than a failure. It reaches toasts by key only, so a
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

Reusing one `key` across every arm is what makes the spinner *become* the outcome
in place instead of stacking a second toast under it. Give a `spinning` toast an
end condition on every path (a keyed replacement or a `dismissToast`), or the
spinner sits there forever.

### Prompts

`lucidos.ui.prompt` shows a single-field text-input modal rendered by the Lucidos
shell (themed, above all app content) — the text-input sibling of `confirm`. Use
it instead of `window.prompt()`.

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

Resolves the entered string on OK click or Enter; `null` on Cancel, Esc, or
backdrop click. (A `multiline` prompt uses Enter for newlines — submit with the
OK button.) If a second `prompt` is called while one is visible, the previous one
resolves `null` and the new one replaces it.

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

Any element with `data-tooltip` gets a themed Lucidos tooltip. There is nothing
to call and nothing to build: `sdk.js` installs one delegated listener on the
document, so an element you add later is covered too. Never hand-roll a tooltip
in an app.

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

The layer stands down on its own whenever your page owns a `#tooltip` element.
So an app that hand-rolled a tooltip before this existed never shows two, and
neither opt-out below is needed for that case.

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

### lucidos.ui.Select — themed dropdown

Replaces native `<select>` (whose popup the OS draws and CSS can't reach) with a fully themed dropdown that uses the same tokens as the rest of Lucidos. Supports keyboard nav, type-to-select, light + dark mode.

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

#### Declarative usage — enhance existing `<select>` elements

`enhanceSelects()` walks `root` (default `document`) and replaces every
`<select class="lucidos-select">` it finds. The native element stays in the DOM
(hidden) — its `value` mirrors the user's selection and `change` events still
fire on it, so existing form code keeps working unchanged. Already-enhanced
selects are skipped, so it's safe to call again after adding new ones.

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

## lucidos.sse — Real-time Events

Subscribe to server-sent events for live updates.

```ts
lucidos.sse.connect(): void
lucidos.sse.disconnect(): void
lucidos.sse.on(eventType: string, callback: (data: unknown, raw: SseEvent) => void): () => void
```

`on()` returns an unsubscribe function. Subscribe by inner event name — the SDK unwraps the wire format.

### One stream per workspace

`connect()` is idempotent, and every `on()` listener in your app is fanned out from one connection. Ten subscriptions cost one stream.

The connection is also shared **across documents**. Where the browser has `SharedWorker`, every document of a workspace attaches to one holder: the Lucidos shell, each app iframe, and each app opened in its own tab. So opening more apps does not open more connections.

You do not opt in, and there is nothing to configure. Two things follow for an app author:

- **A frame is identical either way.** A relayed frame is the same payload a private connection would deliver, so nothing in your handler changes.
- **`disconnect()` detaches this document only.** It never takes the stream from another app or from the shell.

Where `SharedWorker` is missing (Chromium on Android, and Android WebView), the SDK opens a private `EventSource` instead. Same events, same order, one connection per document. Nothing to handle.

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

// Wildcard — all events
lucidos.sse.on('*', (raw) => {
  console.log('Event:', raw);
});

// Cleanup
unsub();
lucidos.sse.disconnect();
```

## lucidos.utils — Utilities

```ts
lucidos.utils.timeAgo(iso: string): string      // "5m ago", "2d ago", "just now"
lucidos.utils.escapeHtml(str: string): string    // HTML-escape
lucidos.utils.formatDate(iso: string): string    // Locale-formatted date string
```

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
