---
name: Building an App
description: Use when the user wants to build, scaffold, edit, or extend an app: "make me an app", "build a tracker", "dashboard for X", "habit app". Covers when an app is the wrong fit, what to clarify before scaffolding, and how to iterate.
---

# Building an App

How to guide a user from "I want X" to a working Lucidos app. File layout, SDK, and frontmatter rules live in `system-knowhow/best-practices.md`, `system-knowhow/js-sdk.md`, and `docs/taxonomy.md` — load those when you need them.

## When an app is the right answer

An app is a persistent UI the user opens repeatedly. Push back on "make me an app" if something simpler fits:

| User says | Better answer |
|---|---|
| "Notify me when my package ships" | Trigger, not app |
| "Summarize today's emails" | Just answer, or save to `artifacts/` |
| "Track my morning habits" | App — repeated UI interaction |
| "Show me a chart of last week's runs" | If one-off, render in chat. If they'll keep coming back, app. |

When unsure, ask: "Do you want to open this regularly, or is it a one-time thing?" — one question, then act.

## Questions to settle with the user before creating

Two questions max before scaffolding — pick the ones the user's request leaves open. Good ones:

- **What's the smallest version that's useful?** (Surfaces scope.)
- **What data does it need to remember between visits?** (Surfaces storage shape.)

Don't design on paper. Once the questions are answered, scaffold the smallest thing that demonstrates the idea (`create_app` with a working `index.html` + `manifest.json`), show it, then iterate.

## Scaffolding defaults

- **Inherit the Lucidos theme, in every new app, by default.** Apps follow the user's light/dark (OS) appearance for free, exactly like the rest of Lucidos. There is no separate "theme" to configure: the platform exposes it, the app just consumes it. The default scaffold does six things:
  1. **Includes the theme assets in `<head>`** (full boilerplate in `system-knowhow/js-sdk.md` § Setup): `<script src="/api/v1/sdk-prefs.js"></script>`, then `<link rel="stylesheet" href="/api/v1/sdk-iframe.css">`, then `<script src="/api/v1/sdk.js"></script>`.
  2. **Calls `lucidos.ui.applyPreferences()`** (applies the theme on load — resolves a `system` preference to the live OS light/dark) **and `lucidos.ui.watchPreferences()`** (re-applies when the user changes it).
  3. **Styles with the theme CSS variables — never hardcoded colors:** `var(--bg-primary)`, `var(--text-primary)`, `var(--accent)`, `var(--border-color)`, etc. (full list in `js-sdk.md` § Theme variables). These flip light↔dark automatically; hex literals do not.

     **The same goes for the font — and here the best move is to write nothing at all.** `sdk-iframe.css` already sets `body { font-family: var(--font-ui) }` (plus inputs and `.action-btn`, since form controls don't inherit the page font on their own), so an app that never mentions `font-family` inherits the user's chosen font for free. Declaring your own stack on `body` — `font-family: system-ui, sans-serif`, `-apple-system, "Segoe UI", …` — *overrides* that default and ships the system font instead, silently: nothing errors, it just isn't the user's font. Only re-declare `font-family` when you deliberately want a different face (e.g. `var(--font-mono)` for code or numeric columns), and then use the token. The UI-font token is **`--font-ui`**.
  4. **Reuses Lucidos's shared component classes for controls** — `sdk-iframe.css` ships the same component layer the host shell uses, so the app looks like part of Lucidos instead of a bare HTML form: primary buttons are `<button class="action-btn">` (with `.action-btn-confirm` / `.action-btn-danger` variants, and `.action-btn-secondary` for a neutral secondary button beside a primary CTA), plus `.icon-btn`, `.label`, `.title`, `.list-row*`, `.segmented-control`, `.markdown-content`, `.progress-bar`, `.empty-state` (full table in `js-sdk.md` § Component classes). A plain unclassed `<button>` does **not** match Lucidos's blue primary button — reach for `.action-btn`, and **don't hand-roll an outlined "secondary" button** — use `.action-btn-secondary`.

     **The `.action-btn` variants are *additive* — always keep the base class.** This is the standard base-plus-modifier shape (Bootstrap's `btn btn-primary`, BEM's `block block--modifier`): the base `.action-btn` carries *all* the geometry (padding, radius, sizing, font), and `-confirm` / `-danger` / `-secondary` only swap the color or fill. So every variant is `class="action-btn action-btn-X"`, never the variant alone:

     ```html
     <button class="action-btn">Save</button>                      <!-- primary (blue) -->
     <button class="action-btn action-btn-confirm">Apply</button>  <!-- green -->
     <button class="action-btn action-btn-danger">Delete</button>  <!-- red -->
     <button class="action-btn action-btn-secondary">Cancel</button> <!-- neutral outline -->
     ```

     The trap: `.action-btn` *does* work on its own — because the base **is** the primary button (unlike Bootstrap's neutral bare `.btn`). That's exactly why it's tempting to assume `.action-btn-secondary` also works alone. It does **not** — written by itself it matches no `.action-btn` rule and falls back to a plain grey browser button ("weird-looking button" bug). Pair it.
  5. **Sizes in `rem`, never `px`, so it respects the user's font size.** The user's UI-scale preference is applied as the root font-size, so only `rem`/`em` units scale with it — an app sized in `px` renders at a fixed size and ignores the setting (the "doesn't respect my font size" bug). Size spacing/radii with the `--space-*` / `--radius-*` tokens and text with the `--font-size-*` type-scale tokens (body `--font-size-md`, small `--font-size-xs`). **Use a type-scale token for every font-size — never a hand-picked `rem`/`px` number.** Ad-hoc values like `font-size: 0.95rem` or `font-size: 0.78rem` are magic numbers: they don't line up with the shared scale, so a modal/heading ends up a hair bigger or smaller than the same element elsewhere and reads as "off". Pick the nearest step (`--font-size-lg` for a title, `--font-size-md` for body, `--font-size-sm`/`--font-size-xs` for meta) instead of dialing in a bespoke number. `1px` borders are the only acceptable `px`. See `js-sdk.md` § "Respect the user's font size" and § "Theme variables" for the token values.
  6. **Lays out fluidly, and declares the viewport.** The scaffold carries `<meta name="viewport" content="width=device-width, initial-scale=1">`, and every container sizes itself against the pane instead of a fixed width. A Lucidos app is opened on a phone as often as on a laptop, so phone width is a target rather than an edge case. The rules, and the three regions the host paints over a fullscreen app, are in *Responsive by default* below. Note that `rem` sizing does not cover any of this: it tracks the user's UI-scale preference, not the width of the pane.

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
        /* Theme variables only — these follow the user's light/dark setting. */
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
          <!-- Variants are additive — always keep the base `action-btn` class. -->
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

  Opt out only for an app that ships its own complete visual identity (a game, a full-bleed chart canvas, an embedded third-party UI). For everything else, inheriting is the default — a workspace in light mode must never get a dark-only app.

## Responsive by default: a phone is a first-class target

An app opens in the *content pane*. On a laptop that is one column of a split layout; on a phone it is a full-width swipe pane. The same markup serves both, so build for the narrow case and let it grow. **Nothing in the shared stylesheet does this for you**: `sdk-iframe.css` ships exactly two width breakpoints and both are for markdown tables. An app that reads well only at desk width is a defect, on the same footing as a hardcoded hex.

Six rules cover almost every app:

- **One column first.** Start with a single stacked column, and add columns only where the width allows. `grid-template-columns: repeat(auto-fit, minmax(14rem, 1fr))` collapses on its own, with no breakpoint to maintain.
- **No fixed container width.** Never set `width` in `px` on the body, a panel, or a card. Constrain with `max-width` in `rem` and let the element be narrower when the pane is.
- **Rows wrap.** A flex row of controls takes `flex-wrap: wrap` and a `--space-*` gap. For a row of buttons use `.button-group`, which already wraps and ellipsizes, instead of a bare flex row.
- **Tables and images use the wrappers, inside a `.markdown-content` container.** Wrap a table in `.table-scroll-wrapper` and an image in `.image-scroll-wrapper`. From about four columns up, stamp `data-stack` on the table and `data-label` on every cell, and each row becomes a card at 768px and under. The ancestor is not optional: every rule is scoped, as in `.markdown-content table[data-stack]`, so a wrapper outside that container silently does nothing. See `js-sdk.md` § Component classes.
- **Text wraps, it does not overflow.** A long identifier, URL, or file path needs `overflow-wrap: break-word` on its container. Without it a single unbreakable token widens the whole layout and nothing else can reflow around it.
- **Controls are tappable.** `.action-btn` is sized for a cursor. The host's mobile stylesheet is **not** served to apps, so on a phone your buttons stay at the bare desktop size. Extend the hit area without changing the look, the same trick the host shell uses:

  ```css
  @media (pointer: coarse) {
    .action-btn { position: relative; }
    .action-btn::before { content: ''; position: absolute; inset: -0.375rem -0.25rem; }
  }
  ```

  Give neighbouring controls at least a `--space-sm` gap, so one thumb cannot hit both.

### The host paints over a fullscreen app: keep three regions clear

The user can send any app fullscreen from the content header. Where the browser has no Fullscreen API, Lucidos falls back to a CSS *pseudo-fullscreen* overlay and draws its own controls on top of the app. That fallback is the iOS path, so this is the phone case. Your app cannot see, style, or move any of it:

| Region | What is there | When |
|---|---|---|
| **Top-right corner.** A `1.375rem` button inset `0.5rem` from the top and right, inside the safe area. Reserve about `2.5rem` square. | The exit-fullscreen button. On iOS it is the only way out, since a phone has no Escape key. | Pseudo-fullscreen, any device |
| **Left edge**, `2.5rem` wide, full height | A back-gesture guard. It exists to beat the app iframe to the touch, so a tap here never reaches your app. | Pseudo-fullscreen on mobile |
| **Right edge**, `1.25rem` wide, full height | The same guard on the other side. | Pseudo-fullscreen on mobile |

Two rules follow, and both are unconditional, because your markup cannot ask whether it is fullscreen:

- **Never put a control in the top-right corner.** Not a settings gear, not a close button, not an overflow menu. Put it top-left, or in a row under the title. A corner control looks fine while you build it and becomes unreachable the moment the user goes fullscreen on a phone.
- **Never pin an interactive control to the extreme left or right edge.** The left strip is `2.5rem` and the right is `1.25rem`, so the `--space-lg` container padding is not on its own enough on the left. A control that belongs at an edge sits at least `2.5rem` in.

## Visual quality bar — make it look sleek and native to Lucidos

The default for an agent-built app is **polished and visually indistinguishable from the host shell**, not "generic HTML form that happens to work". A cheap-looking app is a defect on the same footing as a broken one. The mechanics below are **non-negotiable**; the design principles after them are what separate "functional" from "a product designer would approve".

**Non-negotiable mechanics** (these are bugs if violated — full detail in *Scaffolding defaults* above and `js-sdk.md`):

1. **Inherit the theme.** Include the head scaffold (`/api/v1/sdk-prefs.js` → `/api/v1/sdk-iframe.css` → `/api/v1/sdk.js`, in that order) and call `lucidos.ui.applyPreferences()` + `lucidos.ui.watchPreferences()`.
2. **Zero hardcoded colors.** Every color is a theme variable — `var(--bg-primary)`, `var(--bg-secondary)`, `var(--text-primary)`, `var(--text-secondary)`, `var(--text-muted)`, `var(--border-color)`, `var(--accent)`, `var(--accent-green/yellow/red)`, `var(--shadow-sm/md/lg)`. A single hex literal is a bug — the light-mode workspace gets a dark-only app.
3. **Reuse the shared component classes.** `.action-btn` (+ the *additive* variants `.action-btn-confirm` / `.action-btn-danger` / `.action-btn-secondary` — always `class="action-btn action-btn-X"`, never the variant alone), `.icon-btn`, `.label`, `.title`, `.list-row*`, `.segmented-control`, `.markdown-content`, `.progress-bar`, `.empty-state`. A plain unclassed `<button>` does **not** match Lucidos's primary button — never hand-roll one.
4. **Size in `rem` / tokens, never `px`.** Use `rem` and the `--space-*` / `--radius-*` tokens for all spacing and corners, and a **`--font-size-*` type-scale token for every font-size** — never a hand-picked number (no `font-size: 0.95rem`, no `0.78rem`). `1px` borders are the only acceptable `px`. The user's font-size preference is the root font-size, so `px` ignores it, and off-scale `rem` values drift out of alignment with the rest of the UI.
5. **Lay out for a phone.** One column first, no fixed `px` container width, rows that wrap, tables through `.table-scroll-wrapper` or `data-stack`, and tap-sized controls. Keep the **top-right corner** and both side edges clear: the host draws its fullscreen controls there and your app cannot move them. Full detail in *Responsive by default* above.

**Design principles for a result that looks designed, not assembled:**

- **Generous whitespace.** Crowding reads as cheap. Pad containers with `--space-lg`; separate sections with `--space-xl`. When in doubt, add space, not borders.
- **Clear typographic hierarchy.** One obvious title (`.title` or an `h1`–`h6`), readable body (`--font-size-md`), quiet meta (`--text-muted`, `--font-size-xs`). Size and color carry the hierarchy — don't bold everything.
- **Restrained color.** Build structure from `--bg-secondary` panels and `--border-color` hairlines. Reserve `--accent` for the **one** primary action per screen; `--accent-green/yellow/red` only for genuine status. A UI where everything is colored has no focal point.
- **One clear focal point per screen.** A single primary thing the eye lands on — one main CTA, one headline number, one list — not a dense grid of competing widgets. If two things both shout, neither does.
- **Consistent spacing rhythm.** Pick the space tokens and reuse them; don't mix `--space-sm` here and an ad-hoc `0.6rem` there. Even rhythm is most of what "feels designed".
- **Rounded corners + subtle depth.** `--radius-md` on cards/inputs/buttons (the host's default), `--radius-lg` for large surfaces. Lift a card off the background with `--shadow-sm`/`--shadow-md` — sparingly, not on every element.
- **Calm and confident over busy.** Fewer, better-spaced elements beat a wall of controls. Lean on Lucidos's defaults instead of inventing custom chrome — the more it looks like the host shell, the more native it feels.

### Smell test before finishing — self-apply every time

Before you call the app done, check it against this list. A "no" on any line means it's not finished:

- Does it follow **light *and* dark**? (Mentally flip the theme — would it still look right?)
- Any **hardcoded hex / `rgb()` / named color** anywhere? → replace with a theme variable.
- Any **`px` sizing** beyond `1px` borders? → convert to `rem` / `--space-*` / `--radius-*`.
- Any **plain `<button>`** (or a hand-rolled outlined button) instead of `.action-btn` / `.action-btn-secondary`?
- Does **every** `.action-btn-confirm` / `.action-btn-danger` / `.action-btn-secondary` still carry the base `.action-btn`? A lone variant (`class="action-btn-secondary"`) renders as a plain grey browser button — the variants are additive modifiers.
- Any **magic font-size number** — a raw `rem`/`px` on `font-size` (e.g. `0.95rem`, `0.78rem`, `18px`) instead of a `--font-size-*` token? → snap it to the nearest scale step. Every `font-size` should read a token.
- Any **`font-family`** declaration whose value isn't a token? → delete it. `sdk-iframe.css` already sets `body { font-family: var(--font-ui) }`, so a hardcoded stack only *replaces* the user's chosen font with the system one. Keep a declaration only where you mean a different face, and give it `var(--font-mono)` or `var(--font-ui)`.
- Only **real theme tokens**? The token set is closed — `--space-{xs,sm,md,lg,xl}`, `--radius-{sm,md,lg}`, `--font-size-{3xs,2xs,xs,sm,md,lg,xl,2xl,3xl,display}`, `--font-ui` / `--font-mono` (full list in `js-sdk.md` § Theme variables). Don't invent tokens like `--space-2xl`, `--font-size-4xs`, or `--sans` / `--mono` — an undefined `var()` silently drops the rule. The UI font is **`--font-ui`** (`--font-family` / `--font` are tolerated aliases so the common mis-guess still resolves, but write `--font-ui`).
- Does it hold at **phone width**? Narrow the pane to about `20rem` in your head. Does a row of controls run off the edge, does a grid still demand three columns, does a long URL widen the page?
- Any **fixed `px` width** on the body, a panel, or a card? Any grid with a hardcoded column count instead of `auto-fit` + `minmax`? Any table outside `.table-scroll-wrapper`, or one whose wrapper has no `.markdown-content` ancestor to match against?
- Anything in the **top-right corner**, or an interactive control pinned to the left or right edge? Those regions belong to the host's fullscreen controls, and a tap there never reaches your app.
- Is there **one clear focal point**, generous whitespace, and a consistent spacing rhythm — or a cramped grid of equal-weight widgets?
- Dropped into Lucidos next to the host UI, **would it look like it belongs** — or like a bolted-on web page?
- **Inline `<script>` for small apps.** Split into `app.js` only when the script grows past ~100 lines or you want to share it with another script.
- **Inline `<style>` likewise.** External CSS is for shared design across apps.
- **Use the SDK for everything stateful.** `lucidos.data.read` / `lucidos.data.write` are the right primitives. A direct `fetch` to `/api/v1/*` doesn't just bypass the workspace abstraction: the URL as written doesn't resolve at all. An app iframe has no `<base href>`, and the gateway reads a leading `api` as a workspace name, so both the relative and the root-absolute form 404. Where no SDK method covers the endpoint, build the URL with `lucidos.apiUrl(suffix)` (`system-knowhow/js-sdk.md` § `lucidos.apiUrl`).
- **External APIs: call `lucidos.proxy(name).fetch(path, init)`. Always.** Configure the backend in `data/config/apis.json`; the engine forwards server-side and injects the configured auth header. Never paste credentials into iframe code.

  **Model providers are built in**: `lucidos.proxy('openai' | 'vertex' | 'openrouter' | 'xai' | 'anthropic' | 'local').fetch(...)` needs no `apis.json` entry. The engine reuses its own provider credential from Settings → Models, and an `apis.json` entry of the same name overrides it. See `system-knowhow/js-sdk.md` § `lucidos.proxy` → "Built-in model-provider proxies". The two wrong shapes that look right:
  - `fetch('https://<external-host>/...')` from the iframe — mixed-content / CORS will block it, and any credential is sitting in the iframe.
  - `fetch('/api/v1/proxy/<name>/...')` from the iframe — same-origin so it *runs*, but it's the proxy URL the SDK helper builds for you. Constructing it by hand makes the proxy name a magic string and skips any future SDK-side concerns (timeouts, retries, response parsing, error shape). Use the helper.
- **Manifest description matters.** It's how the user finds the app in the launcher and how the engine LLM knows what the app is for. Write it like a one-line README, not a tagline.

## Interaction primitives — use the shell's, not the browser's

An app runs in an iframe, and the browser's built-in interactions don't belong there. `window.confirm` / `window.alert` / `window.prompt` and a native `<select>` all draw **OS chrome** that ignores the user's Lucidos theme, font, and scale — and the dialogs throw a system modal that doesn't sit above your app correctly. `lucidos.ui.*` gives you themed equivalents the **host shell** renders *above* the iframe, so they inherit the user's light/dark + font + scale for free and look native. Reach for these by default; the browser's primitives are a smell. The full reference — every signature, option, and return type — lives in [`system-knowhow/js-sdk.md`](./js-sdk.md) § lucidos.ui; the four you'll reach for most:

- **Toast — transient feedback** (instead of a hand-rolled banner). `lucidos.ui.toast(message, type?, opts?)` is fire-and-forget: a brief themed banner above the app for success/error/info/warning. Reach for it after a save, a failed request, a copy-to-clipboard. `type` ∈ `'success' | 'info' | 'warning' | 'error'` (default `'info'`); `opts` is `{ durationMs?, dismissable? }`. It returns nothing, and **action-button callbacks can't cross the iframe boundary** — a toast is a message, not a prompt.

  ```js
  lucidos.ui.toast('Saved', 'success');
  lucidos.ui.toast('Could not reach the server', 'error');
  ```

- **Confirm — yes/no** (instead of `window.confirm()`). `lucidos.ui.confirm({ title?, message, okLabel?, cancelLabel?, danger? })` → `Promise<boolean>` (`true` on OK/Enter, `false` on Cancel/Esc/backdrop). Set `danger: true` to render the OK button red for a destructive action.

  ```js
  if (await lucidos.ui.confirm({
    message: 'Delete this board and its 3 cards?',
    okLabel: 'Delete', danger: true,
  })) {
    // proceed with deletion
  }
  ```

- **Prompt — one line of text** (instead of `window.prompt()`). `lucidos.ui.prompt({ message, title?, defaultValue?, placeholder?, okLabel?, cancelLabel?, multiline? })` → `Promise<string | null>` (the entered string, or `null` on cancel). Pass `multiline: true` for a textarea.

  ```js
  const name = await lucidos.ui.prompt({ message: 'New name:', defaultValue: 'Untitled' });
  if (name === null) return;  // user cancelled
  ```

- **Select — a themed dropdown** (instead of a native `<select>`). A native `<select>`'s popup is drawn by the OS and **cannot be themed** — it's the one control that breaks the "looks native to Lucidos" bar even when everything else is perfect. Two ways in:
  - **Declarative** — give your `<select>` the `lucidos-select` class and call `lucidos.ui.enhanceSelects()` once. The native element stays in the DOM (hidden), so its `value` and `change` events keep firing — existing form code is untouched.
  - **Programmatic** — `lucidos.ui.Select.create({ options, value?, onChange? })` returns an instance; insert `instance.element` into the DOM and drive it with `setValue` / `setOptions` / `destroy`.

  ```html
  <select class="lucidos-select" data-placeholder="Status…">
    <option value="todo">To do</option>
    <option value="done">Done</option>
  </select>
  <script>lucidos.ui.enhanceSelects();</script>
  ```

These four are the everyday set; navigation (`lucidos.ui.navigate`), opening a fresh chat (`lucidos.ui.startThread`), and theme application (`applyPreferences` / `watchPreferences`) round out `lucidos.ui` — see `js-sdk.md` § lucidos.ui for all of it.

## Updating an app

There is no `update_app` tool. After `create_app`, all changes go through `write_file` / `edit_file` on `apps/{id}/index.html` (and `manifest.json` when the name or description changes). Don't recreate the app to change one button.

### Editing an app with a coding agent

Two paths, picked per request:

- **Chat path (quick edits)** — file tools (`write_file`, `edit_file`) on the live `data/apps/<id>/` files. Best for one-line tweaks, copy fixes, small CSS adjustments. The change lands immediately on workspace `main`. **You don't need to manually refresh:** when your turn finishes, the engine automatically refreshes every app you edited this turn — it reloads the open app UI (`AppUiRefreshRequested`) and refreshes the apps list (`AppUpdated`), once per app, coalesced (not per write). So just edit and finish; avoid spamming `refresh_app` mid-turn. (A brand-new app created this turn via `create_app` already appears via `AppCreated`.) **When you tell the user the app is ready to open, link it** with a markdown link using the `app:<id>` scheme — `[Habit Tracker](app:habit-tracker)` — so it's one click to open; a bare prose mention of the app name is NOT a link. Make this the default whenever you name an app the user can open (whichever path you used), not just for a brand-new one.
- **App coding-agent thread (heavier work)**: `run_coding_agent(folder='data/apps/<id>')` spawns a *coding-agent thread* in an isolated sparse-checkout *worktree* narrowed to that one app folder. Branch name shape: `lucidos-<coding-agent>-app-<id>-<slug>-<thread id>` (for example `lucidos-claude-code-app-habit-tracker-add-streaks-401a2d19`). Best for multi-file refactors, new features, work that needs review before landing. Produces a *change* the user reviews + clicks *Apply*. Apply ff-merges to workspace `main`, emits `AppUiRefreshRequested` so open iframes reload, and does **not** restart the engine. `/harden` is not run for app changes: apps own their hardening (ship a `.claude/commands/harden.md` if you want one).

While the app coding-agent thread is open the user can preview the in-flight app via `?thread_id=<id>` on the app UI URL — the panel-overlay slot swaps from the live workspace copy to the WIP worktree copy. SDK calls (`lucidos.data.*`, `lucidos.events.*`) still hit live workspace data — data-coupled UI edits show their full effect only after Apply.

### Checking your work with `capture_app` / `refresh_app`

`capture_app` / `refresh_app` are **agent self-check tools** — they reload the open app UI and snapshot it back to you so you can see the effect of an edit. Use them *only while actively iterating on an app with the user*, i.e. when that app is the subject of the current turn (the user just asked you to change it). Don't reach for them in threads that aren't about an app (research, data tasks, general chat) — there's nothing to capture.

- **You usually don't need them at all.** The engine already auto-refreshes every app you edited when your turn finishes (see the chat path above), so for an ordinary edit-and-finish turn, just edit and stop. Reserve a manual `capture_app` for when you genuinely need to *look* at the result mid-turn before deciding the next edit; don't spam `refresh_app`.
- **On "No app UI is currently open", prefer the visible path: tell the user / ask them to open the app.** That's the error's first suggested recovery, and it's the default here. Silently calling `navigate_ui(target=app-ui)` to open the app yourself is acceptable **only** when you're mid app-iteration and opening *that* app is clearly the expected next step. Don't open apps out of the blue — a thread yanking an app onto the user's screen while they're doing something else is the friction to avoid.
- **Never retry the same `capture_app` / `refresh_app` call after it fails.** Re-issuing the identical call trips the circuit breaker. If the first call reports "No app UI is currently open", **switch strategy** (ask the user, or `navigate_ui` once) — do not repeat the same call.
- **In background / trigger / cron threads, don't attempt `capture_app` / `refresh_app` (or navigate-to-app) at all.** There's no originating device, so the frontend won't navigate and the capture can't succeed; and pulling an app open on the user's screen from a background task is exactly the out-of-the-blue behavior to avoid. (`navigate_ui` is scoped to the device that sent the prompt in this thread; a deviceless turn navigates nothing.)

## Common mistakes to avoid

- **Storing data in `apps/{id}/`.** App data goes in `artifacts/{app-id}/`. The app code is git-tracked source; the data is user state. (See `best-practices.md`.)
- **Forgetting the `artifacts/` prefix in `lucidos.data.*` paths.** Paths are relative to `data/`, not `data/artifacts/`. App data lives at `artifacts/{app-id}/data.json` — *not* `{app-id}/data.json`. Without the prefix, `read` returns a 404 `SdkError` and `write` fails silently, which usually surfaces as "the checkbox toggles back" or "state doesn't persist".
- **Hardcoding colors / shipping a light-only (or dark-only) app.** New apps inherit the Lucidos theme by default (see *Scaffolding defaults*): include the theme assets, call `lucidos.ui.applyPreferences()` + `lucidos.ui.watchPreferences()`, and style with the theme CSS variables (`var(--bg-primary)`, `var(--text-primary)`, `var(--accent)`, `var(--border-color)`, …) instead of hex literals. Hardcoded colors ignore the user's OS light/dark setting — a light-mode workspace gets a jarring dark app, or vice versa. This is the single most common theming regression.
- **Hardcoding a `font-family` stack on `body`.** The same failure as hardcoded colors, but easier to miss because it looks like ordinary boilerplate: `body { font-family: system-ui, sans-serif }` (or an `-apple-system, "Segoe UI", …` stack) *overrides* the `body { font-family: var(--font-ui) }` that `sdk-iframe.css` already applies, so the app ships the system font instead of the user's chosen font — with no error, and looking fine to whoever built it. **The fix is to delete the declaration, not to fix the stack:** say nothing about fonts and the app inherits the right one. Name a token only for a deliberate override — `var(--font-mono)` for code/numeric cells, `var(--font-ui)` to re-assert the UI font. (See *Scaffolding defaults* item 3.)
- **Sizing in `px`, or picking magic `font-size` numbers instead of the type scale.** Two related smells. (a) `px` values don't scale with the user's font-size / UI-scale preference (root font-size), so the app ignores the setting — size in `rem` and the `--space-*` / `--radius-*` tokens (`1px` borders are the only acceptable exception). (b) Even in `rem`, a hand-dialed `font-size: 0.95rem` / `0.78rem` is a magic number that drifts off the shared scale, so a title or modal ends up subtly bigger/smaller than the same element elsewhere. **Every `font-size` should read a `--font-size-*` token** — snap to the nearest step rather than inventing a value. (See *Scaffolding defaults* item 5.)
- **Building for the desktop pane only, and never checking phone width.** The easiest app to write is a fixed multi-column layout, because the pane you preview it in is wide. The same app on a phone overflows, and the user meets it there just as often. Nothing rescues you: the shared stylesheet has two width breakpoints and both are for markdown tables. Build one column first and let it grow. (See *Responsive by default*.)
- **Putting a control in the top-right corner.** The host draws the exit-fullscreen button there, over your app, and on iOS it is the user's only way out of fullscreen. Your settings gear then sits under it, unreachable. The same goes for a control pinned to the extreme left or right edge on mobile, where the back-gesture guards take the tap first. (See *The host paints over a fullscreen app*.)
- **Hand-rolling a secondary button — *or* writing `.action-btn-secondary` without the base class.** The shared layer ships `.action-btn` (+ `.action-btn-confirm` / `.action-btn-danger`) and `.action-btn-secondary`. Don't invent your own outlined/ghost button with `var(--accent)` — use `.action-btn-secondary`. But the variants are **additive**: write `class="action-btn action-btn-secondary"`, not `class="action-btn-secondary"` alone. A lone variant matches no `.action-btn` rule and falls back to a plain grey browser button — the "weird-looking secondary button" bug. (This is the standard base-plus-modifier shape, like Bootstrap's `btn btn-secondary`; see *Scaffolding defaults* item 4.)
- **Using the browser's dialogs or a native `<select>` instead of `lucidos.ui.*`.** `window.confirm()` / `window.alert()` / `window.prompt()`, a hand-rolled toast banner, and a native `<select>` all draw OS chrome that ignores the user's theme, font, and scale — and the dialogs throw a jarring system modal that doesn't sit above the app correctly. A native `<select>`'s popup in particular **can't be styled at all**, so it breaks the "looks native" bar on its own. Use `lucidos.ui.toast` / `lucidos.ui.confirm` / `lucidos.ui.prompt` / `lucidos.ui.Select` (or `enhanceSelects()`) — themed and rendered by the host shell above your iframe. See *Interaction primitives — use the shell's, not the browser's* above and `system-knowhow/js-sdk.md` § lucidos.ui.
- **Creating an app for a one-shot.** If the user only wants the answer once, just give it.
- **Inventing SDK calls — or guessing the surface, then checking later.** Load `system-knowhow/js-sdk.md` **before** you write the first SDK call or theme-CSS `var()`, not after the scaffold is written. The surface is small and stable but easy to misremember, and a scaffold written from memory tends to ship several guesses at once: e.g. there is no `lucidos.chat` namespace (`lucidos.chat.send` doesn't exist — open a chat with the typed `lucidos.ui.startThread({ prompt })`, the preferred wrapper over the low-level `lucidos.ui.navigate('new-chat', …)`), and `lucidos.data.read()` returns a **string** — `JSON.parse` it on read and `JSON.stringify` on write. Checking *after* you've written the JS catches the calls but tends to miss the CSS (the markup class / token errors hide in a later pass).
- **Hand-rolling the proxy URL with raw `fetch`.** Both `fetch('https://<external-host>/...')` (mixed-content / CORS will block it) and `fetch('/api/v1/proxy/<name>/...')` (same-origin so it runs, but constructs the helper's URL by hand and bypasses the SDK) are wrong. Use `lucidos.proxy(name).fetch(path, init)` and configure the backend in `data/config/apis.json` — one shape, no string-building.
- **Building an engine URL by hand in JavaScript.** `new URL('api/v1/events/query', document.baseURI)` and `fetch('/api/v1/events/query')` both look right and both 404. An app iframe has no `<base href>`, so the first resolves under the app's own directory (`/<workspace>/app/<app-id>/api/v1/…`); the gateway reads the second's first path segment as a workspace name and answers `unknown workspace 'api'`. The engine rewrites `src` / `href` attributes in the markup it serves, which is why `<script src="/api/v1/sdk.js">` works in `index.html`, but it cannot rewrite a string JS builds at runtime: **the same path is correct in markup and broken in JS.** Use an SDK method, or `lucidos.apiUrl(suffix)` where none covers the endpoint. This one is worth extra care because it fails *silently*: the 404 lands in a `catch`, the app falls back to another source, and it keeps rendering plausible stale data. See `system-knowhow/js-sdk.md` § `lucidos.apiUrl`.
- **Hardcoding credentials in iframe code.** The credential belongs in the engine's credential store, referenced by name from `apis.json`. The SDK never sees the secret.
- **Batching large rewrites.** After scaffold, prefer small visible changes the user can react to per round.
