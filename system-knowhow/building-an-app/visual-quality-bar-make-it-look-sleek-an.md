---
name: Building an App, Visual quality bar: make it look sleek and native to Lucidos
description: Part of Building an App. Its index lists the sections it covers.
---

<!-- domain-event: FaresRefreshed: Example domain event emitted by a fare-data refresh trigger. -->

One part of `system-knowhow/building-an-app`, which lists every part.

## Visual quality bar: make it look sleek and native to Lucidos

An agent-built app should be **polished and visually indistinguishable from the host shell**. A cheap-looking app is a defect, like a broken one. The mechanics below are **non-negotiable**. The design principles after them separate "functional" from "a product designer would approve".

**Non-negotiable mechanics.** The six *Scaffolding defaults* above, with *Responsive by default*, are not optional: a violation is a bug. Two of their lists are partial:

- **Every color is a theme variable**: `var(--bg-primary)`, `var(--bg-secondary)`, `var(--text-primary)`, `var(--text-secondary)`, `var(--text-muted)`, `var(--border-color)`, `var(--accent)`, `var(--accent-green/yellow/red)`, `var(--shadow-sm/md/lg)`. A single hex literal is a bug.
- **More shared component classes**: `.label` (+ its status tones), `.pill-bar` (tabs and filter pills), `.toggle-switch`, `.mini-spinner`. They are the host's own components, so the app updates with the host. Never draw your own tabs, chips, switch or spinner.

**Design principles for a result that looks designed, not assembled:**

- **Generous whitespace.** Crowding reads as cheap. Pad containers with `--space-lg`; separate sections with `--space-xl`. When in doubt, add space, not borders.
- **Clear typographic hierarchy.** One obvious title (`.title` or an `h1`–`h6`), body text at the chat's size (`--font-size-sm`, the default), quiet meta (`--text-muted`, `--font-size-xs`). Size and color carry the hierarchy, so don't bold everything.
- **Restrained color.** Build structure from space first, then `--bg-secondary` panels. Use a `--border-color` hairline only where space cannot separate two parts. Reserve `--accent` for the **one** primary action per screen, and `--accent-green/yellow/red` for genuine status only.
- **One clear focal point per screen.** One main CTA, one headline number, or one list, not a dense grid of competing panels. If two things both shout, neither does.
- **Consistent spacing rhythm.** Pick the space tokens and reuse them. Don't mix `--space-sm` here and an ad-hoc `0.6rem` there.
- **Rounded corners + subtle depth.** Round with the theme's corner steps: `--radius-control` on inputs, buttons and rows, `--radius-surface` on cards and large surfaces, `--radius-round` on pills and round buttons. A square theme then squares your app too. Lift a card with `--shadow-sm`/`--shadow-md`, sparingly.
- **Calm and confident over busy.** Fewer, better-spaced elements beat a wall of controls. Lean on Lucidos's defaults instead of custom chrome.

### Smell test before finishing: self-apply every time

A "no" on any line means the app is not finished:

- Does it follow **light *and* dark**? Mentally flip the theme.
- Any **hardcoded hex / `rgb()` / named color**? → replace with a theme variable.
- Any **`px` sizing** beyond `1px` borders? → convert to `rem` / `--space-*` / `--radius-*`.
- Any **plain `<button>`** (or a hand-rolled outlined button) instead of `.action-btn` / `.action-btn-secondary`?
- Does **every** `.action-btn-confirm` / `.action-btn-danger` / `.action-btn-secondary` still carry the base `.action-btn`?
- Does a **view switch, sort or filter** show its current choice with `.action-btn` variants? → use `.segmented-btn.active`, or `.pill-bar-btn` with `aria-pressed`.
- Is **every `:hover`** you wrote inside `@media (hover: hover)`?
- Any **left-edge stripe** on a card, callout, banner or notice? That means a coloured `border-left`, a `::before` bar or an inset `box-shadow`. It is forbidden. Use bold text, a raised `--bg-tertiary` surface, spacing or an inline icon.
- For a **widget**: does it fit one phone screen, with about 5 rows, one control row and the rest behind "Show more"? Does it lead with one shape instead of a table, with no title and no border inside it?
- Any **magic font-size number** (`0.95rem`, `0.78rem`, `18px`) instead of a `--font-size-*` token? → snap it to the nearest scale step.
- Any **`font-family`** declaration whose value isn't a token? → delete it. Keep one only for a deliberately different face, set to `var(--font-mono)` or `var(--font-ui)`.
- Only **real theme tokens**? The token set is closed: `--space-{xs,sm,md,lg,xl}`, `--radius-{sm,md,lg}`, the theme's corner steps `--radius-{control,surface,round}`, `--font-size-{3xs,2xs,xs,sm,md,lg,xl,2xl,3xl,display}`, `--font-ui` / `--font-mono` (full list in `system-knowhow/js-sdk/setup.md` § Theme variables). Don't invent tokens like `--space-2xl`, `--font-size-4xs`, or `--sans` / `--mono`. An undefined `var()` silently drops the rule. Write **`--font-ui`** for the UI font; `--font-family` / `--font` are only tolerated aliases.
- Does it hold at **phone width**? Narrow the pane to about `20rem` in your head. Does a row of controls run off the edge, does a grid still demand three columns, does a long URL widen the page?
- Any **fixed `px` width** on the body, a panel, or a card? Any grid with a hardcoded column count instead of `auto-fit` + `minmax`? Any table outside `.table-scroll-wrapper`, or one whose wrapper has no `.markdown-content` ancestor?
- Anything in the **top-right corner**, or an interactive control pinned to the left or right edge?
- Is there **one clear focal point**, generous whitespace, and a consistent spacing rhythm, or a cramped grid of equal-weight panels?
- Next to the host UI, **would it look like it belongs**, or like a bolted-on web page?
- **Inline `<script>` for small apps.** Split into `app.js` only when the script grows past ~100 lines or you want to share it with another script.
- **Inline `<style>` likewise.** External CSS is for shared design across apps.
- **Use the SDK for everything stateful.** `lucidos.data.read` / `lucidos.data.write` are the right primitives, and `lucidos.storage` holds per-device state. Inside the host shell, a direct `fetch` to `/api/v1/*` has no network at all. An app frame's origin is opaque, so the engine is cross-origin to it and CORS refuses the answer. `localStorage` and `new EventSource` fail the same way, and the SDK carries all three over a bridge to the host. Where no SDK method covers the endpoint, see `system-knowhow/js-sdk/lucidos-apiurl.md` § `lucidos.apiUrl` for what a frame can and cannot reach.
- **Model selection:** use `lucidos.ui.ModelPicker.create` for the host's stepped
  model, reasoning effort and provider picker. Set `filter: { callableOnly: true }`
  for app model calls, await `ready`, and persist the returned value in app data.
  Use `lucidos.modelRequest(value, { messages })` to build the provider's request,
  then send it through `lucidos.proxy(req.proxy).fetch(req.path, ...)`. Replies
  retain their provider protocol. Call `refresh()` to reload registry/setup
  changes and `destroy()` when removing the field. See `js-sdk.md` for the contract.
- **External APIs: call `lucidos.proxy(name).fetch(path, init)`. Always.** Configure the backend in `data/config/apis.json`; the engine forwards server-side and injects the configured auth header. Never paste credentials into iframe code.

  **Model providers are built in**: `lucidos.proxy('openai' | 'vertex' | 'openrouter' | 'xai' | 'anthropic' | 'local' | 'typesafe').fetch(...)` needs no `apis.json` entry. The engine reuses its own provider credential from Settings → Providers, and an `apis.json` entry of the same name overrides it. See `system-knowhow/js-sdk/lucidos-media.md` § `lucidos.proxy` → "Built-in model-provider proxies". Two wrong shapes look right:
  - `fetch('https://<external-host>/...')` from the iframe: mixed-content / CORS blocks it, and any credential sits in the iframe.
  - `fetch('/api/v1/proxy/<name>/...')` from the iframe: the helper's own URL, written by hand. Inside the host shell the frame's own `fetch` cannot reach the engine at all. Elsewhere it makes the proxy name a magic string and skips future SDK-side concerns (timeouts, retries, response parsing, error shape).
- **Manifest description matters.** The user finds the app in the launcher by it, and the engine LLM learns what the app is for. Write it like a one-line README, not a tagline.

## Interaction primitives: use the shell's, not the browser's

`window.confirm` / `window.alert` / `window.prompt` and a native `<select>` all draw **OS chrome** that ignores the user's Lucidos theme, font, and scale. The dialogs also throw a system modal that doesn't sit above your app correctly. `lucidos.ui.*` gives themed equivalents that the **host shell** renders *above* the iframe, so they look native. Use them by default. Every signature, option, and return type is in [`system-knowhow/js-sdk.md`](.././js-sdk.md) § lucidos.ui. The four you'll use most:

- **Toast: transient feedback** (instead of a hand-rolled banner). `lucidos.ui.toast(message, type?, opts?)` is fire-and-forget: a brief themed banner above the app, for example after a save, a failed request, or a copy-to-clipboard. `type` ∈ `'success' | 'info' | 'warning' | 'error'` (default `'info'`). `opts` is `{ title?, durationMs?, dismissable?, key?, spinning? }`, where `title` is a bold line over the message. It returns nothing, and **action-button callbacks can't cross the iframe boundary**: a toast is a message, not a prompt.

  ```js
  lucidos.ui.toast('Saved', 'success');
  lucidos.ui.toast('Could not reach the server', 'error');
  ```

- **Confirm: yes/no** (instead of `window.confirm()`). `lucidos.ui.confirm({ title?, message, okLabel?, cancelLabel?, danger? })` → `Promise<boolean>` (`true` on OK/Enter, `false` on Cancel/Esc/backdrop). Set `danger: true` to render the OK button red for a destructive action.

  ```js
  if (await lucidos.ui.confirm({
    message: 'Delete this board and its 3 cards?',
    okLabel: 'Delete', danger: true,
  })) {
    // proceed with deletion
  }
  ```

- **Prompt: one line of text** (instead of `window.prompt()`). `lucidos.ui.prompt({ message, title?, defaultValue?, placeholder?, okLabel?, cancelLabel?, multiline? })` → `Promise<string | null>` (the entered string, or `null` on cancel). Pass `multiline: true` for a textarea.

  ```js
  const name = await lucidos.ui.prompt({ message: 'New name:', defaultValue: 'Untitled' });
  if (name === null) return;  // user cancelled
  ```

- **Select: a themed dropdown** (instead of a native `<select>`). The OS draws a native `<select>`'s popup, and it **cannot be themed**. Two ways in:
  - **Declarative**: give your `<select>` the `lucidos-select` class and call `lucidos.ui.enhanceSelects()` once. The native element stays in the DOM (hidden), so its `value` and `change` events keep firing and existing form code still works.
  - **Programmatic**: `lucidos.ui.Select.create({ options, value?, onChange? })` returns an instance; insert `instance.element` into the DOM and drive it with `setValue` / `setOptions` / `destroy`.

  ```html
  <select class="lucidos-select" data-placeholder="Status…">
    <option value="todo">To do</option>
    <option value="done">Done</option>
  </select>
  <script>lucidos.ui.enhanceSelects();</script>
  ```

The rest of `lucidos.ui` covers navigation (`lucidos.ui.navigate`), opening a fresh chat (`lucidos.ui.startThread`), and theme application (`applyPreferences` / `watchPreferences`).

## Motion that explains

This applies to apps and widgets alike. Motion confirms cause and effect: the user taps, and the thing that changed moves. It is never decoration that delays the answer.

- **Animate what a tap changes.** A picked row settles into its selected state, a total moves to its new value, a re-sorted list slides each row into place.
- **A short staggered entrance is fine.** Rows fade and lift in one after another, done well under a second. Never hold the answer back until the entrance ends.
- **CSS transitions and animations only, on the `--duration-*` tokens.** Write `var(--duration-fast)`, never a literal `0.2s`. The tokens collapse to a single frame under reduced motion, so your motion collapses with them.
- **Key the calm rules on `:root[data-motion="reduce"]`**, never on the `prefers-reduced-motion` media query. The media query reads only the OS and ignores the user's choice in Lucidos. Under `reduce`, stop what the tokens do not cover, such as a transform or a looping animation.
- **No animation libraries.** CSS covers all of this, and a library is one more script to load before the answer shows.

```css
.row { transition: background var(--duration-fast), transform var(--duration-normal); }
.row.picked { background: var(--bg-secondary); }
.row.entering {
  animation: rise var(--duration-slow) both;
  animation-delay: calc(var(--i) * var(--duration-fast) / 3);
}
@keyframes rise { from { opacity: 0; transform: translateY(0.5rem); } }
:root[data-motion="reduce"] .row.entering { animation: none; }
```

See `system-knowhow/js-sdk/remote-workspace-client-contract.md` § Reduced motion, and `system-knowhow/js-sdk/setup.md` § Theme variables for the tokens.

## Live data

This applies to apps and widgets alike. **An app shows the current state of its data, whoever changed it**: the user in this app, the agent, a trigger, a script, or another device. The shell keeps itself correct the same way, from events.

- **Keep data out of the HTML.** Markup freezes the moment you write it: a flight picker with its fares in the markup shows last week's prices. Read the data through `lucidos.data` (workspace files), `lucidos.events` (the event store) or `lucidos.proxy` (an outside API).
- **Keep the app's data in a file under `artifacts/<app-id>/`**, so one path names it.

### The events a file change emits

The engine announces each file write it makes. Listen for the event, check it names your file, and read the file again.

| Where the file is | Events | Payload field |
|---|---|---|
| Under `artifacts/` | `ArtifactCreated`, `ArtifactUpdated`, `ArtifactDeleted` | `artifact_path`, relative to `artifacts/`: `fare-watch/fares.json` |
| Anywhere else in `data/` | `DataFileWritten`, `DataFileEdited`, `DataFileDeleted` | `path`, relative to `data/`: `config/fares.json` |

These come from the agent's file tools (`write_file`, `edit_file`, `copy_file`, `delete_file`), from `run_python`, and from the data API. The data API carries `lucidos.data.*`, so your app's own writes, and `lucidos data write`.

A data API write to an `artifacts/` path emits both: the `Artifact*` event, and a `DataFile*` event whose `path` keeps the `artifacts/` prefix. Listen to the `Artifact*` events for a file under `artifacts/`, and to the `DataFile*` events for any other file. Never both, or each write renders twice.

### Some writes announce nothing

No event, so no open app sees the change until it reloads:

- `run_bash`, and anything it starts.
- `run_python_background`. It writes straight to `data/`, with no staging.
- A script that writes with a plain `open()` outside `run_python`, however it was started.

**A script that feeds an app writes through `lucidos data write`.** The CLI is on `PATH` in every subprocess Lucidos starts, and it goes through the data API, so the write is announced like any other:

```bash
python fetch_fares.py > "${TMPDIR:-/tmp}/fares.json"
lucidos data write artifacts/fare-watch/fares.json --from "${TMPDIR:-/tmp}/fares.json"
```

See `system-knowhow/lucidos-cli/subcommands.md` § `lucidos data write`.

### One `render()`, called from every place the data can change

```js
const PATH = 'artifacts/fare-watch/fares.json';
const ARTIFACT = PATH.slice('artifacts/'.length);   // what artifact_path carries
let shown = null;   // the text on screen now
let reads = 0;      // so a slow read never paints over a newer one

async function render() {
  const read = ++reads;
  let text;
  try {
    text = await lucidos.data.read(PATH);
  } catch (err) {
    if (err.httpCode !== 404) throw err;   // a real failure: show it, never hide it
    text = '';   // deleted, or not written yet: draw the empty state
  }
  if (read !== reads || text === shown) return;
  shown = text;
  // Update the rows and numbers in place. Keep the scroll position and the
  // selection, and leave alone any field that has focus or an unsaved edit.
}

function renderIfMine(data) {
  if (data.artifact_path === ARTIFACT) render();
}

lucidos.sse.onOpen(render);   // the first load, and every reconnect
lucidos.sse.on('ArtifactCreated', renderIfMine);
lucidos.sse.on('ArtifactUpdated', renderIfMine);
lucidos.sse.on('ArtifactDeleted', renderIfMine);
document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'visible') render();
});
addEventListener('pageshow', (event) => {
  if (event.persisted) render();
});
```

- **Call `render` from `lucidos.sse.onOpen`, not on load.** It runs on the first open, and at once if the stream is already open. A second call on load reads everything twice at startup.
- **The reconnect is not optional.** The stream does not replay what landed while it was down: a network drop, an engine restart, a phone that slept. `onOpen` runs again on each reopen, and the read catches the app up.
- **The resume listeners cover a dead stream that looks alive.** iOS can resume a suspended PWA with its stream silently stalled. A read when the page comes back is cheap, and the `shown` check makes a read that finds nothing new cost no repaint.
- **Check the path.** Every file write in the workspace reaches every listener.
- **Re-render in place.** Update what changed, never rebuild the page. A rebuild loses the scroll position, the selection and the user's place in a form.

### Your own write

The event for your own write comes back to you. Handle it so the screen never flickers and never fights the user's typing:

```js
async function save(fares) {
  const text = JSON.stringify(fares, null, 2);
  shown = text;   // the screen already shows this: its echo is now a no-op
  await lucidos.data.write(PATH, text);
}
```

- **Update the screen when the user acts, then write.** Do not wait for the event to show the user their own change.
- **Set `shown` before the write.** The echo then reads the same text and stops at the `shown` check.
- **Never overwrite a field the user is editing.** Skip a field that has focus or holds an unsaved edit, and render it when the user leaves it.

### Outside data: a trigger keeps it fresh

For data from outside, such as fares, a trigger refreshes it on a schedule so the app never polls. Use the same `render`:

- **The trigger writes the file.** Its write is announced, so the listeners above pick it up with nothing more.
- **Or the trigger emits a domain event**, such as `FaresRefreshed`, when the data lives in the event store rather than in a file. Then `render` reads it with `lucidos.events.query`, and the app calls `lucidos.sse.on('FaresRefreshed', render)` beside `lucidos.sse.onOpen(render)`.

See `system-knowhow/js-sdk/lucidos-sse-real-time-events.md` § lucidos.sse, and `system-knowhow/triggers.md` for the trigger.

