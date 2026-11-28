---
name: Lucidos JavaScript SDK, Setup (continued)
description: Part of Lucidos JavaScript SDK. Its index lists the sections it covers.
---

One part of `system-knowhow/js-sdk`, which lists every part.

## Setup (continued)

### Component classes

`sdk-iframe.css` also ships Lucidos's shared component layer: the **same CSS the
host shell uses, from one source**. The engine appends
`crates/lucidos-app/src/styles/global/shared-components.css` (which the host
imports via `global.css`) to the served stylesheet. Apply these class names and
your controls render like the rest of Lucidos and track the theme and UI scale.
The class names are the contract.

**A button is an action; a picked option is state.** Use the `.action-btn`
family for something the user presses to make a thing happen. Show a current
view, sort or filter with `.segmented-control` or `.pill-bar`, never by swapping
`.action-btn` variants. The shared classes gate their hovers inside
`@media (hover: hover)`, and a hover you write needs the same gate
(`system-knowhow/building-an-app/scaffolding-defaults.md` § Scaffolding defaults).

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
| `.pill-bar` + `.pill-bar-btn` | A row of pills with one picked, as in Settings → Appearance & Behavior → Theme. The markup tells two uses apart. **Tabs** that switch views: `<div class="pill-bar" role="tablist">` holding `<button class="pill-bar-btn" role="tab" aria-selected="true">`. A **filter** that narrows one list: `role="group"` with `aria-pressed` on each button. Set the attribute to `true` on the picked pill, and the bar marks it. It stays on one line and scrolls sideways when the pills do not fit. On a touch screen each pill takes taps `--pill-bar-hit-slop` (0.5rem) above and below itself without growing. The bar pads out room for that reach and for focus rings (`--focus-ring-width`). A negative margin hands back the room above and below, so the pills sit `--focus-ring-width` in from the bar's sides. If you pad the bar yourself, keep both allowances, or a ring or the reach is cut off. **Use this instead of drawing your own tabs or filter pills.** |
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
| `.mini-spinner` | The spinner the host shows for a working state, such as a save in flight: `<span class="mini-spinner" aria-hidden="true"></span>` beside text that says what is happening. A faint ring stays still and a solid arc turns on it. It stops under reduced motion and stays drawn. Recolour it with `--spinner-color`, for example `style="--spinner-color: currentColor"` inside a button. Size it with `--spinner-size` (default `0.75rem`), never `width` or `height`. Its stroke follows its size, so `--spinner-weight` no longer exists and setting it does nothing. Keep the span empty, and leave its `::before` and `::after` alone: they draw it. A busy button is a disabled `.action-btn` holding the spinner and its label. Loading data draws a skeleton or nothing, never a spinner. |
| `.empty-state`, `.error-text` | Empty/error placeholders |
| `data-tooltip` (an attribute, plus the `#tooltip` rules that paint it) | A themed Lucidos tooltip on any element. Write the attribute, and `sdk.js` builds, positions and paints the box (§ Tooltips, under lucidos.ui). |
| `.ring`, `.bar-run`, `.choice-row` + `.choice` | The widget shapes: a ring gauge, a run of bars for a short series, and a wrapping row of choices to pick from. App frames only, since the host draws none. See § Widget shapes below. |

Prefer these over hand-rolled buttons and rows. A plain unclassed `<button>`
gets a neutral default that does **not** match Lucidos's primary blue button.

**Four of those rows are the overflow half of a wider rule.** `.button-group`,
`.table-scroll-wrapper`, `.image-scroll-wrapper` and `data-stack` each contain
one thing that would otherwise widen the page. They do not make an app
responsive. `data-stack` is one of the stylesheet's two width breakpoints, and
the other only tightens markdown-table type, so every other width decision is
yours. The rules, and the three regions the host paints over a fullscreen app,
are in `system-knowhow/building-an-app/scaffolding-defaults.md` § Responsive by default.

`lucidos._capture()` needs nothing from your app. The SDK draws the page through an SVG `foreignObject`. So the browser renders the snapshot, and any CSS it paints comes out right, CSS Color 4 included. It embeds your fonts and images by reading them over the host bridge. One it cannot read (another host, or a route outside the app reach table) paints empty.

When the screenshot fails for any reason, the capture degrades to **DOM-only** rather than throwing. It returns an empty `screenshot` plus a `dom` layout snapshot (element positions + classes), prefixed with the failure reason. The caller still receives the layout and failure reason.

### Widget shapes

`sdk-iframe.css` ships three shapes for what a widget shows instead of a
table. They draw from the theme tokens only, so they follow the user's theme,
corners and motion setting with no code from you. Each carries its own fill,
so it reads on any ground. When to use them is in
`system-knowhow/building-an-app/when-an-app-is-the-right-answer.md` § Widgets, under "How a widget looks".

| Class | What it draws | You set |
|---|---|---|
| `.ring` (+ `.ring-value`, `.ring-label`) | A ring gauge. The accent arc fills a `--bg-tertiary` track, and the children sit in the hole. It stays round in a square theme. | `--ring-value`, from 0 to 1. Optional: `--ring-size` (default `6rem`), `--ring-weight` (`0.5rem`), `--ring-color` (`--accent`), `--ring-track`. |
| `.bar-run` > `.bar-run-item` (+ `.bar-run-bar`, `.bar-run-label`) | A run of rounded bars, one per item, for a short series such as seven days. `.is-accent` on an item paints its bar in the accent. | `--bar-value` on each item, from 0 to 1. Optional: `--bar-run-height` (default `3.5rem`), `--bar-color` on an item. |
| `.choice-row` > `.choice` (+ `.choice-meta`) | A wrapping row of round tap targets, at least `2.75rem` square, for a pick among data items: a day, a slot, a fare. A `.choice-meta` puts a small second line under the label. `aria-pressed="true"` (or `.active`) fills the picked choice with the accent. | Nothing. Use a `<button>` per choice, and set `aria-pressed` when the user picks. |

A new `--ring-value` or `--bar-value` slides to its place, and both fill in on
first paint. Under `data-motion="reduce"` they show the value at once. A choice squeezes a
little on press, and paints no fill under a finger.

A choice is not a `.pill-bar-btn`. The pill bar switches a view or narrows a list,
on one line. A choice row picks a data item, wraps, and can hold two lines.

```html
<div class="ring" style="--ring-value: 0.86">
  <span class="ring-value">86</span><span class="ring-label">today</span>
</div>

<div class="bar-run" role="img" aria-label="Readiness, last 7 days">
  <div class="bar-run-item" style="--bar-value: 0.62">
    <span class="bar-run-bar"></span><span class="bar-run-label">F</span>
  </div>
  <!-- one item per day -->
  <div class="bar-run-item is-accent" style="--bar-value: 0.86">
    <span class="bar-run-bar"></span><span class="bar-run-label">T</span>
  </div>
</div>

<div class="choice-row" role="group" aria-label="Long run day">
  <button class="choice">Fri<span class="choice-meta">3 events</span></button>
  <button class="choice" aria-pressed="true">Sat<span class="choice-meta">free</span></button>
</div>
```

Set a value from script with `el.style.setProperty('--ring-value', 0.9)`.

## Error Handling

An async method that reaches the engine and gets a non-2xx answer throws
`SdkError`:

```js
class SdkError extends Error {
  httpCode: number;
  reason: string;
}
```

GET requests retry a browser transport failure once, unless the caller canceled or the workspace generation changed.
HTTP errors and body parsing errors are not transport retries.

A call that never gets an answer rejects with a `DOMException` instead, and the
`name` says which kind of nothing you got:

| `err.name` | Meaning | What to do |
|---|---|---|
| `TimeoutError` | The SDK's own 10s deadline fired. | Treat as retryable. The request may or may not have reached the engine. |
| `AbortError` | Something cancelled the request: an `AbortSignal` browser type you passed in `init`, or the browser tearing down an in-flight fetch. | If you did not cancel it yourself, treat as retryable. |

The `AbortError` case is routine on an installed iOS PWA: WebKit aborts every
in-flight fetch when it suspends the page. Retry an idempotent call rather than
reporting a failure. Retry when the page comes back (`visibilitychange`,
`pageshow`, `focus`), not immediately, because a suspended page cannot reach
the engine either.

WebKit rejects an aborted fetch with a generic `AbortError` rather than the
signal's reason. So the SDK re-stamps a fired deadline as `TimeoutError`, as
Chrome and Firefox deliver it. A cancel you requested stays an `AbortError`,
even when the deadline fired in the same instant.

## lucidos.configure: SDK endpoint

`lucidos.configure(opts: { baseUrl?: string; token?: string }): void`

Apps loaded by Lucidos use the host bridge automatically. Standalone clients can
configure the SDK before making calls:

```js
lucidos.configure({ baseUrl: 'https://example.com', token: 'your-access-token' });
lucidos.sse.onOpen(load); // Load workspace data after the generation hello.
```

`baseUrl` overrides the auto-derived workspace base path for requests and event
streams. In-app iframes read the gateway prefix from `<base href>` or their
`/app/` URL, so they need no override. `token` adds a Bearer Authorization header
to SDK fetch requests, including bridged requests. Omitted options keep their current
values. The endpoint still enforces its authentication and app-route policy.

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

`write` and `delete` wait up to 60 seconds for the response. If the workspace
repo lock stays busy for 20 seconds, the engine refuses the mutation.
The SDK throws an HTTP 503 `SdkError` with a message that it is safe to retry.
A client timeout leaves the outcome unknown.

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

