---
name: Building an App
description: Use when the user wants to build, scaffold, edit, or extend an app: "make me an app", "build a tracker", "dashboard for X", "habit app". Also covers widgets (small inline answers), motion, live data. Covers when an app is the wrong fit, what to clarify before scaffolding, and how to iterate.
---

# Building an App

How to guide a user from "I want X" to a working Lucidos app. File layout, SDK, and frontmatter rules live in `system-knowhow/best-practices.md`, `system-knowhow/js-sdk.md`, and `docs/taxonomy.md`. Load those when you need them.

## When an app is the right answer

An app is a persistent UI the user opens repeatedly. Push back on "make me an app" if something simpler fits:

| User says | Better answer |
|---|---|
| "Notify me when my package ships" | Trigger, not app |
| "Summarize today's emails" | Just answer, or save to `artifacts/` |
| "Track my morning habits" | App: repeated UI interaction |
| "Show me a chart of last week's runs" | If one-off, a widget in this thread. If they'll keep coming back, app. |
| "Compare these fares across three dates" | Widget: easier to scan and click than to read |
| "Show me flights to Stockholm", and they will pick one | Widget: a pick among data items, never question-card options |

When unsure, ask one question, then act: "Do you want to open this regularly, or is it a one-time thing?"

## Widgets: a small answer inline in the thread

A *widget* is an app that lives in the thread that made it. It is not in the apps list. `create_app` with `kind: "widget"` makes one and records this thread as its origin. It shows inline at that turn as a *widget card*: a bar with its name and menu, and the widget under it. The frame is as tall as the content, so the card never scrolls inside. Past about one phone screen, the card clips with a fade and an Expand button that opens the widget full size in Canvas.

A widget has a chip on the thread's *widget shelf*, under the title, only once pinned. Showing one adds no chip.

**When to choose one.**
- A comparison or a dataset that is easier to scan and click than to read: a fare grid, a sortable table, a chart with a toggle.
- **A pick among data items**: flights, hotels, time slots, products. Build a widget the user picks in. Never list the items as question-card options; those are for deciding what you do next.
- Not for prose, and not for something the user will open every day (that is an app).

The system prompt says whether automatic widgets are on. When they are off, make a widget only when the user asks for one. For a pick, offer one as a question option instead. "Just text" means no widget.

**When to pin one.** Pin with `widgets(action="pin")` only when the user will come back to the widget in this thread, such as a picker. A one-off answer stays unpinned, so a thread of ten one-offs keeps an empty shelf.

**Build it exactly like an app.** The same scaffold, theme rules and SDK apply, and data still comes through `lucidos.*` (see *Live data* below). Four differences:

- **Small: one phone screen, no scroll area inside.** Put the answer on the top screen, because a taller card clips. One focused view, at most two actions, no tabs, no navigation, no settings screen. If it needs more, build an app.
- **Named by what it does**, in two or three words: "Flight picker", "Fare grid", "Run chart". The card's bar shows the name, so the trip, dates and other specifics go inside the widget and in its description.
- **Height follows the content.** The host sizes the frame to `document.body`. Never give the body `height: 100vh` or a full-height flex layout, or the frame cannot shrink. Load the SDK, which reports the height. See `system-knowhow/js-sdk.md` § Sizing a widget to its content.
- **Its manifest says so.** `create_app` writes `"kind": "widget"` and `"origin_thread_id"`. Keep both, and `"reusable"`, when you rewrite `manifest.json`.

**What the user can do with it**, from the ⋯ menu on the card or the chip. Each action is also yours:

| Menu action | Tool |
|---|---|
| Open in Canvas, full size (the card's Expand does the same) | `navigate_ui` to the widget's id |
| Make reusable, offering it to every thread | `widgets(action="make_reusable")` |
| Show a reusable widget in another thread | `widgets(action="show", thread_id=…)` |
| Pin to shelf, for a widget the user will come back to in this thread | `widgets(action="pin")` |
| Unpin from shelf (the card stays) | `widgets(action="unpin")` |
| Make app | Build a new app with `create_app`, reading the widget's files. The widget stays as it is. |

`widgets(action="thread")` lists the *thread widgets*: every widget shown in a thread, and which are pinned.

Deleting the thread deletes its own widgets. A reusable widget, and an app made from a widget, stay.

## Questions to settle with the user before creating

Ask at most two questions before scaffolding. Pick the ones the request leaves open:

- **What's the smallest version that's useful?** (Surfaces scope.)
- **What data does it need to remember between visits?** (Surfaces storage shape.)

Don't design on paper. Scaffold the smallest thing that shows the idea (`create_app` with a working `index.html` + `manifest.json`), show it, then iterate.

## Scaffolding defaults

- **Inherit the Lucidos theme, in every new app, by default.** Apps follow the user's light/dark (OS) appearance for free, like the rest of Lucidos. There is no separate theme to configure: the platform exposes it and the app consumes it. The default scaffold does six things:
  1. **Includes the theme assets in `<head>`** (full boilerplate in `system-knowhow/js-sdk.md` § Setup): `<script src="/api/v1/sdk-prefs.js"></script>`, then `<link rel="stylesheet" href="/api/v1/sdk-iframe.css">`, then `<script src="/api/v1/sdk.js"></script>`.
  2. **Calls `lucidos.ui.applyPreferences()`** (applies the theme on load, resolving a `system` preference to the live OS light/dark) **and `lucidos.ui.watchPreferences()`** (re-applies when the user changes it).
  3. **Styles with the theme CSS variables, never hardcoded colors:** `var(--bg-primary)`, `var(--text-primary)`, `var(--accent)`, `var(--border-color)`, etc. (full list in `js-sdk.md` § Theme variables). These flip light↔dark automatically; hex literals do not.

     **For the font, write nothing at all.** `sdk-iframe.css` already sets `body { font-family: var(--font-ui) }`, plus inputs and `.action-btn`, since form controls don't inherit the page font. A `font-family` stack of your own on `body` (`system-ui, sans-serif`, `-apple-system, "Segoe UI", …`) *overrides* it. The app then silently ships the system font instead of the user's. Re-declare `font-family` only for a deliberately different face, and then use a token, such as `var(--font-mono)` for code or numeric columns. The UI-font token is **`--font-ui`**.
  4. **Reuses Lucidos's shared component classes for controls.** `sdk-iframe.css` ships the component layer the host shell uses, so the app looks like part of Lucidos, not a bare HTML form. Primary buttons are `<button class="action-btn">`, with `.action-btn-confirm` / `.action-btn-danger` variants and `.action-btn-secondary` for a neutral button beside a primary CTA. Also available: `.icon-btn`, `.label`, `.title`, `.list-row*`, `.segmented-control`, `.markdown-content`, `.progress-bar`, `.empty-state`, and `.text-input` for a free-text field (full table in `js-sdk.md` § Component classes). A plain unclassed `<button>` does **not** match Lucidos's blue primary button, and a hand-rolled outlined button is not a secondary one. `lucidos.ui.Select` draws from the same layer, so a themed dropdown matches Settings' own.

     **The `.action-btn` variants are *additive*: always keep the base class.** The base `.action-btn` carries *all* the geometry (padding, radius, sizing, font). `-confirm` / `-danger` / `-secondary` only swap the color or fill, like Bootstrap's `btn btn-primary`. So every variant is `class="action-btn action-btn-X"`, never the variant alone:

     ```html
     <button class="action-btn">Save</button>                      <!-- primary (blue) -->
     <button class="action-btn action-btn-confirm">Apply</button>  <!-- green -->
     <button class="action-btn action-btn-danger">Delete</button>  <!-- red -->
     <button class="action-btn action-btn-secondary">Cancel</button> <!-- neutral outline -->
     ```

     The trap: `.action-btn` alone works, because the base **is** the primary button (unlike Bootstrap's neutral bare `.btn`). A lone `.action-btn-secondary` does **not**: it matches no `.action-btn` rule and falls back to a plain grey browser button.
  5. **Sizes in `rem`, never `px`, so it respects the user's font size.** The user's UI-scale preference is the root font-size, so only `rem`/`em` units scale with it. An app sized in `px` ignores the setting. Size spacing and radii with the `--space-*` / `--radius-*` tokens. `1px` borders are the only acceptable `px`.

     **Use a type-scale token for every font-size, never a hand-picked `rem`/`px` number.** Body text is `--font-size-sm`, a label or control `--font-size-md`, a title `--font-size-lg`, and meta `--font-size-xs`. A value like `0.95rem` or `0.78rem` is a magic number that misses the shared scale, so the element reads a hair off. Pick the nearest step. See `js-sdk.md` § "Respect the user's font size" and § "Theme variables" for the token values.
  6. **Lays out fluidly, and declares the viewport.** The scaffold carries `<meta name="viewport" content="width=device-width, initial-scale=1">`, and every container sizes itself against the pane, not a fixed width. Users open apps on a phone as often as on a laptop. The rules, and the three regions the host paints over a fullscreen app, are in *Responsive by default* below. `rem` sizing does not cover this: it tracks the UI-scale preference, not the pane width.

  Minimal scaffold to start from:

  ```html
  <!DOCTYPE html>
  <html>
    <head>
      <meta charset="utf-8">
      <meta name="viewport" content="width=device-width, initial-scale=1">
      <title>My App</title>
      <script src="/api/v1/sdk-prefs.js"></script>
      <link rel="stylesheet" href="/api/v1/sdk-iframe.css">
      <script src="/api/v1/sdk.js"></script>
      <style>
        /* Theme variables only: these follow the user's light/dark setting. */
        .panel {
          background: var(--bg-secondary);
          color: var(--text-primary);
          border: 1px solid var(--border-color);
          border-radius: var(--radius-md);
          padding: var(--space-lg);
        }
        .row { display: flex; gap: var(--space-sm); margin-top: var(--space-md); }
      </style>
    </head>
    <body>
      <div class="panel">
        Hello
        <div class="row">
          <!-- Variants are additive: always keep the base `action-btn` class. -->
          <button class="action-btn">Primary</button>
          <button class="action-btn action-btn-secondary">Secondary</button>
        </div>
      </div>
      <script>
        lucidos.ui.applyPreferences();   // apply the user's theme on load
        lucidos.ui.watchPreferences();   // re-apply when it changes live
        // app code…
      </script>
    </body>
  </html>
  ```

  Opt out only for an app that ships its own complete visual identity (a game, a full-bleed chart canvas, an embedded third-party UI). Everything else inherits: a light-mode workspace must never get a dark-only app.

- **An app that loads data at startup opts into the ready signal.** By
  default the host reveals the app on page load. An app that reads data with
  `lucidos.data`, `lucidos.events` or `lucidos.proxy` then shows an empty
  screen until the data arrives. Add `"reveal": "on-ready"` to
  `manifest.json`, and call `lucidos.ui.ready()` once the first render is
  done, on the error and empty paths too. The host keeps its loading cover and
  progress bar up until the call. Rules: `js-sdk.md` § Showing the app once
  its content is ready.

## Responsive by default: a phone is a first-class target

An app opens in the *content pane*. On a laptop that is one column of a split layout; on a phone it is a full-width swipe pane. The same markup serves both, so build for the narrow case and let it grow. **The shared stylesheet does not do this for you**: `sdk-iframe.css` has exactly two width breakpoints, both for markdown tables. An app that reads well only at desk width is a defect, like a hardcoded hex.

Six rules cover almost every app:

- **One column first.** Start with a single stacked column, and add columns only where the width allows. `grid-template-columns: repeat(auto-fit, minmax(14rem, 1fr))` collapses on its own, with no breakpoint to maintain.
- **No fixed container width.** Never set `width` in `px` on the body, a panel, or a card. Constrain with `max-width` in `rem` and let the element be narrower when the pane is.
- **Rows wrap.** A flex row of controls takes `flex-wrap: wrap` and a `--space-*` gap. For a row of buttons, use `.button-group`, which already wraps and ellipsizes.
- **Tables and images use the wrappers, inside a `.markdown-content` container.** Wrap a table in `.table-scroll-wrapper` and an image in `.image-scroll-wrapper`. From about four columns up, stamp `data-stack` on the table and `data-label` on every cell, and each row becomes a card at 768px and under. Every rule is scoped, as in `.markdown-content table[data-stack]`, so a wrapper outside that container silently does nothing. See `js-sdk.md` § Component classes.
- **Text wraps, it does not overflow.** A long identifier, URL, or file path needs `overflow-wrap: break-word` on its container. Otherwise one unbreakable token widens the whole layout.
- **Controls are tappable.** `.action-btn` is sized for a cursor, and the host's mobile stylesheet is **not** served to apps. Extend the hit area without changing the look, as the host shell does:

  ```css
  @media (pointer: coarse) {
    .action-btn { position: relative; }
    .action-btn::before { content: ''; position: absolute; inset: -0.375rem -0.25rem; }
  }
  ```

  Give neighbouring controls at least a `--space-sm` gap, so one thumb cannot hit both.

### The host paints over a fullscreen app: keep three regions clear

The user can send any app fullscreen from the content header. Where the browser has no Fullscreen API (iOS, so the phone case), Lucidos falls back to a CSS *pseudo-fullscreen* overlay. It draws its own controls on top of the app, and your app cannot see, style, or move them:

| Region | What is there | When |
|---|---|---|
| **Top-right corner.** A `1.375rem` button inset `0.5rem` from the top and right, inside the safe area. Reserve about `2.5rem` square. | The exit-fullscreen button. On iOS it is the only way out, since a phone has no Escape key. | Pseudo-fullscreen, any device |
| **Left edge**, `2.5rem` wide, full height | A back-gesture guard. It exists to beat the app iframe to the touch, so a tap here never reaches your app. | Pseudo-fullscreen on mobile |
| **Right edge**, `1.25rem` wide, full height | The same guard on the other side. | Pseudo-fullscreen on mobile |

Two rules follow. Both are unconditional, because your markup cannot ask whether it is fullscreen:

- **Never put a control in the top-right corner.** Not a settings gear, a close button, or an overflow menu. Put it top-left, or in a row under the title. A corner control looks fine while you build it, then becomes unreachable in fullscreen on a phone.
- **Never pin an interactive control to the extreme left or right edge.** The left strip is `2.5rem` and the right is `1.25rem`, so `--space-lg` container padding alone is not enough on the left. A control that belongs at an edge sits at least `2.5rem` in.

## Visual quality bar: make it look sleek and native to Lucidos

An agent-built app should be **polished and visually indistinguishable from the host shell**. A cheap-looking app is a defect, like a broken one. The mechanics below are **non-negotiable**. The design principles after them separate "functional" from "a product designer would approve".

**Non-negotiable mechanics.** The six *Scaffolding defaults* above, with *Responsive by default*, are not optional: a violation is a bug. Two of their lists are partial:

- **Every color is a theme variable**: `var(--bg-primary)`, `var(--bg-secondary)`, `var(--text-primary)`, `var(--text-secondary)`, `var(--text-muted)`, `var(--border-color)`, `var(--accent)`, `var(--accent-green/yellow/red)`, `var(--shadow-sm/md/lg)`. A single hex literal is a bug.
- **More shared component classes**: `.label` (+ its status tones), `.pill-bar` (tabs and filter pills), `.toggle-switch`, `.mini-spinner`. They are the host's own components, so the app updates with the host. Never draw your own tabs, chips, switch or spinner.

**Design principles for a result that looks designed, not assembled:**

- **Generous whitespace.** Crowding reads as cheap. Pad containers with `--space-lg`; separate sections with `--space-xl`. When in doubt, add space, not borders.
- **Clear typographic hierarchy.** One obvious title (`.title` or an `h1`–`h6`), body text at the chat's size (`--font-size-sm`, the default), quiet meta (`--text-muted`, `--font-size-xs`). Size and color carry the hierarchy, so don't bold everything.
- **Restrained color.** Build structure from `--bg-secondary` panels and `--border-color` hairlines. Reserve `--accent` for the **one** primary action per screen, and `--accent-green/yellow/red` for genuine status only.
- **One clear focal point per screen.** One main CTA, one headline number, or one list, not a dense grid of competing panels. If two things both shout, neither does.
- **Consistent spacing rhythm.** Pick the space tokens and reuse them. Don't mix `--space-sm` here and an ad-hoc `0.6rem` there.
- **Rounded corners + subtle depth.** `--radius-md` on cards/inputs/buttons (the host's default), `--radius-lg` for large surfaces. Lift a card with `--shadow-sm`/`--shadow-md`, sparingly.
- **Calm and confident over busy.** Fewer, better-spaced elements beat a wall of controls. Lean on Lucidos's defaults instead of custom chrome.

### Smell test before finishing: self-apply every time

A "no" on any line means the app is not finished:

- Does it follow **light *and* dark**? Mentally flip the theme.
- Any **hardcoded hex / `rgb()` / named color**? → replace with a theme variable.
- Any **`px` sizing** beyond `1px` borders? → convert to `rem` / `--space-*` / `--radius-*`.
- Any **plain `<button>`** (or a hand-rolled outlined button) instead of `.action-btn` / `.action-btn-secondary`?
- Does **every** `.action-btn-confirm` / `.action-btn-danger` / `.action-btn-secondary` still carry the base `.action-btn`?
- Any **magic font-size number** (`0.95rem`, `0.78rem`, `18px`) instead of a `--font-size-*` token? → snap it to the nearest scale step.
- Any **`font-family`** declaration whose value isn't a token? → delete it. Keep one only for a deliberately different face, set to `var(--font-mono)` or `var(--font-ui)`.
- Only **real theme tokens**? The token set is closed: `--space-{xs,sm,md,lg,xl}`, `--radius-{sm,md,lg}`, `--font-size-{3xs,2xs,xs,sm,md,lg,xl,2xl,3xl,display}`, `--font-ui` / `--font-mono` (full list in `js-sdk.md` § Theme variables). Don't invent tokens like `--space-2xl`, `--font-size-4xs`, or `--sans` / `--mono`. An undefined `var()` silently drops the rule. Write **`--font-ui`** for the UI font; `--font-family` / `--font` are only tolerated aliases.
- Does it hold at **phone width**? Narrow the pane to about `20rem` in your head. Does a row of controls run off the edge, does a grid still demand three columns, does a long URL widen the page?
- Any **fixed `px` width** on the body, a panel, or a card? Any grid with a hardcoded column count instead of `auto-fit` + `minmax`? Any table outside `.table-scroll-wrapper`, or one whose wrapper has no `.markdown-content` ancestor?
- Anything in the **top-right corner**, or an interactive control pinned to the left or right edge?
- Is there **one clear focal point**, generous whitespace, and a consistent spacing rhythm, or a cramped grid of equal-weight panels?
- Next to the host UI, **would it look like it belongs**, or like a bolted-on web page?
- **Inline `<script>` for small apps.** Split into `app.js` only when the script grows past ~100 lines or you want to share it with another script.
- **Inline `<style>` likewise.** External CSS is for shared design across apps.
- **Use the SDK for everything stateful.** `lucidos.data.read` / `lucidos.data.write` are the right primitives, and `lucidos.storage` holds per-device state. Inside the host shell, a direct `fetch` to `/api/v1/*` has no network at all. An app frame's origin is opaque, so the engine is cross-origin to it and CORS refuses the answer. `localStorage` and `new EventSource` fail the same way, and the SDK carries all three over a bridge to the host. Where no SDK method covers the endpoint, see `system-knowhow/js-sdk.md` § `lucidos.apiUrl` for what a frame can and cannot reach.
- **External APIs: call `lucidos.proxy(name).fetch(path, init)`. Always.** Configure the backend in `data/config/apis.json`; the engine forwards server-side and injects the configured auth header. Never paste credentials into iframe code.

  **Model providers are built in**: `lucidos.proxy('openai' | 'vertex' | 'openrouter' | 'xai' | 'anthropic' | 'local').fetch(...)` needs no `apis.json` entry. The engine reuses its own provider credential from Settings → Models, and an `apis.json` entry of the same name overrides it. See `system-knowhow/js-sdk.md` § `lucidos.proxy` → "Built-in model-provider proxies". Two wrong shapes look right:
  - `fetch('https://<external-host>/...')` from the iframe: mixed-content / CORS blocks it, and any credential sits in the iframe.
  - `fetch('/api/v1/proxy/<name>/...')` from the iframe: the helper's own URL, written by hand. Inside the host shell the frame's own `fetch` cannot reach the engine at all. Elsewhere it makes the proxy name a magic string and skips future SDK-side concerns (timeouts, retries, response parsing, error shape).
- **Manifest description matters.** The user finds the app in the launcher by it, and the engine LLM learns what the app is for. Write it like a one-line README, not a tagline.

## Interaction primitives: use the shell's, not the browser's

`window.confirm` / `window.alert` / `window.prompt` and a native `<select>` all draw **OS chrome** that ignores the user's Lucidos theme, font, and scale. The dialogs also throw a system modal that doesn't sit above your app correctly. `lucidos.ui.*` gives themed equivalents that the **host shell** renders *above* the iframe, so they look native. Use them by default. Every signature, option, and return type is in [`system-knowhow/js-sdk.md`](./js-sdk.md) § lucidos.ui. The four you'll use most:

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

See `system-knowhow/js-sdk.md` § Reduced motion, and § Theme variables for the tokens.

## Live data

This applies to apps and widgets alike. Data written into the HTML freezes the moment you write it: a flight picker with its fares in the markup shows last week's prices.

- **Keep data out of the HTML.** Read it at load through `lucidos.data` (workspace files), `lucidos.events` (the event store) or `lucidos.proxy` (an outside API).
- **Subscribe to the event that changes it.** Call `lucidos.sse.on` with that event's name, and re-render in place: update the rows and numbers, and keep the user's scroll position and selection.
- **For outside data such as fares, let a trigger refresh it.** The trigger writes fresh data to a file under `artifacts/` and emits an event. The widget listens for that event and reads the file again, so it never polls.

```js
async function render() {
  const fares = JSON.parse(await lucidos.data.read('artifacts/fare-watch/fares.json'));
  // Update the existing rows; do not rebuild the page.
}
lucidos.sse.connect();
lucidos.sse.on('FaresRefreshed', render);
render();
```

See `system-knowhow/js-sdk.md` § lucidos.sse, and `system-knowhow/triggers.md` for the trigger.

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
- **They reach one screen: the turn's *last used device*,** the same one `navigate_ui` sends to. Only that device reloads and answers. A failure names it: `Error from "<device>", …` when it has no app open, or a timeout naming the device that did not answer. A timeout adds a hint when Lucidos is not visible there. Tell the user which device you looked at.
- **On "No app UI is currently open", prefer the visible path: ask the user to open the app.** That's the error's first suggested recovery. Calling `navigate_ui(target=app-ui)` yourself is acceptable **only** mid app-iteration, when opening *that* app is clearly the next step. Don't yank an app onto the user's screen out of the blue.
- **Never retry the same `capture_app` / `refresh_app` call after it fails.** The identical call trips the circuit breaker. On "No app UI is currently open", **switch strategy**: ask the user, or `navigate_ui` once.
- **In background / trigger / cron threads, don't attempt `capture_app` / `refresh_app` (or navigate-to-app) at all.** Such a turn has no *last used device*, so no single screen takes the navigate. The capture goes to every connected page, and the first answer wins, usually "No app UI is currently open".

## Common mistakes to avoid

- **Storing data in `apps/{id}/`.** App data goes in `artifacts/{app-id}/`. The app code is git-tracked source; the data is user state. (See `best-practices.md`.)
- **Forgetting the `artifacts/` prefix in `lucidos.data.*` paths.** Paths are relative to `data/`, not `data/artifacts/`. App data lives at `artifacts/{app-id}/data.json`, *not* `{app-id}/data.json`. Without the prefix, `read` returns a 404 `SdkError` and `write` fails silently. It usually surfaces as "the checkbox toggles back" or "state doesn't persist".
- **Hardcoding colors, so the app is light-only or dark-only.** This is the most common theming regression: a light-mode workspace gets a jarring dark app. See *Scaffolding defaults* items 1 to 3.
- **Hardcoding a `font-family` stack on `body`.** It looks like ordinary boilerplate, but it replaces the user's chosen font with the system one, with no error. **Delete the declaration; don't fix the stack.** See *Scaffolding defaults* item 3.
- **Sizing in `px`, or picking magic `font-size` numbers instead of the type scale.** `px` ignores the user's UI-scale preference. A hand-dialed `0.95rem` drifts off the shared scale, so a title ends up subtly off from the same element elsewhere. See *Scaffolding defaults* item 5.
- **Building for the desktop pane only, and never checking phone width.** A fixed multi-column layout looks fine in a wide preview pane and overflows on a phone, where the user meets it just as often. See *Responsive by default*.
- **Putting a control in the top-right corner, or pinning one to a side edge.** The exit-fullscreen button covers the corner, and it is the user's only way out on iOS. On mobile, the back-gesture guards take edge taps first. See *The host paints over a fullscreen app*.
- **Hand-rolling a secondary button, *or* writing `.action-btn-secondary` without the base class.** Don't invent an outlined or ghost button with `var(--accent)`. Write `class="action-btn action-btn-secondary"`: a lone variant falls back to a plain grey browser button. See *Scaffolding defaults* item 4.
- **Using the browser's dialogs or a native `<select>` instead of `lucidos.ui.*`.** That includes a hand-rolled toast banner. Use `lucidos.ui.toast` / `lucidos.ui.confirm` / `lucidos.ui.prompt` / `lucidos.ui.Select` (or `enhanceSelects()`). See *Interaction primitives* above and `system-knowhow/js-sdk.md` § lucidos.ui.
- **Creating an app for a one-shot.** If the user only wants the answer once, just give it: in text, or as a widget when it is easier to scan than to read.
- **Inventing SDK calls, or guessing the surface and checking later.** Load `system-knowhow/js-sdk.md` **before** you write the first SDK call or theme-CSS `var()`. A scaffold written from memory tends to ship several guesses at once. A later check catches the JS but misses CSS class and token errors. Two common guesses:
  - There is no `lucidos.chat` namespace, so `lucidos.chat.send` doesn't exist. Open a chat with the typed `lucidos.ui.startThread({ prompt })`, preferred over the low-level `lucidos.ui.navigate('new-chat', …)`.
  - `lucidos.data.read()` returns a **string**: `JSON.parse` it on read and `JSON.stringify` on write.
- **Hand-rolling the proxy URL with raw `fetch`.** Both shapes in *External APIs* above are wrong. Use `lucidos.proxy(name).fetch(path, init)` and configure the backend in `data/config/apis.json`.
- **Calling the engine with your own `fetch`.** Inside the host shell CORS refuses it, as *Use the SDK for everything stateful* explains. Use an SDK method. A standalone app tab does reach the engine, and there the two obvious URLs both 404:
  - `new URL('api/v1/events/query', document.baseURI)` resolves under the app's own directory, since an app page has no `<base href>`.
  - `fetch('/api/v1/events/query')` hands the gateway `api` as a workspace name.

  Markup is the exception. The engine rewrites `src` / `href` attributes it serves, so `<script src="/api/v1/sdk.js">` works in `index.html`. Nothing rewrites a string JS builds at runtime: **the same path is correct in markup and broken in JS.** Either way it fails *silently*, landing in a `catch` that goes on rendering stale data. See `system-knowhow/js-sdk.md` § `lucidos.apiUrl`.
- **Hardcoding credentials in iframe code.** The credential belongs in the engine's credential store, referenced by name from `apis.json`. The SDK never sees the secret.
- **Batching large rewrites.** After scaffold, prefer small visible changes the user can react to per round.
