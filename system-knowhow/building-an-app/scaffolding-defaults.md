---
name: Building an App, Scaffolding defaults
description: Part of Building an App. Its index lists the sections it covers.
---

One part of `system-knowhow/building-an-app`, which lists every part.

## Scaffolding defaults

- **Inherit the Lucidos theme, in every new app, by default.** Apps follow the user's light/dark (OS) appearance for free, like the rest of Lucidos. There is no separate theme to configure: the platform exposes it and the app consumes it. The default scaffold does six things:
  1. **Includes the theme assets in `<head>`** (full boilerplate in `system-knowhow/js-sdk/remote-workspace-client-contract.md` § Setup): `<script src="/api/v1/sdk-prefs.js"></script>`, then `<link rel="stylesheet" href="/api/v1/sdk-iframe.css">`, then `<script src="/api/v1/sdk.js"></script>`.
  2. **Calls `lucidos.ui.applyPreferences()`** (applies the theme on load, resolving a `system` preference to the live OS light/dark) **and `lucidos.ui.watchPreferences()`** (re-applies when the user changes it).
  3. **Styles with the theme CSS variables, never hardcoded colors:** `var(--bg-primary)`, `var(--text-primary)`, `var(--accent)`, `var(--border-color)`, etc. (full list in `system-knowhow/js-sdk/setup.md` § Theme variables). These flip light↔dark automatically; hex literals do not.

     **For the font, write nothing at all.** `sdk-iframe.css` already sets `body { font-family: var(--font-ui) }`, plus inputs and `.action-btn`, since form controls don't inherit the page font. A `font-family` stack of your own on `body` (`system-ui, sans-serif`, `-apple-system, "Segoe UI", …`) *overrides* it. The app then silently ships the system font instead of the user's. Re-declare `font-family` only for a deliberately different face, and then use a token, such as `var(--font-mono)` for code or numeric columns. The UI-font token is **`--font-ui`**.
  4. **Reuses Lucidos's shared component classes for controls.** `sdk-iframe.css` ships the component layer the host shell uses, so the app looks like part of Lucidos, not a bare HTML form. Primary buttons are `<button class="action-btn">`, with `.action-btn-confirm` / `.action-btn-danger` variants and `.action-btn-secondary` for a neutral button beside a primary CTA. Also available: `.icon-btn`, `.label`, `.title`, `.list-row*`, `.segmented-control`, `.markdown-content`, `.progress-bar`, `.empty-state`, and `.text-input` for a free-text field (full table in `system-knowhow/js-sdk/setup-2.md` § Component classes). A plain unclassed `<button>` does **not** match Lucidos's blue primary button, and a hand-rolled outlined button is not a secondary one. `lucidos.ui.Select` draws from the same layer, so a themed dropdown matches Settings' own.

     **The `.action-btn` variants are *additive*: always keep the base class.** The base `.action-btn` carries *all* the geometry (padding, radius, sizing, font). `-confirm` / `-danger` / `-secondary` only swap the color or fill, like Bootstrap's `btn btn-primary`. So every variant is `class="action-btn action-btn-X"`, never the variant alone:

     ```html
     <button class="action-btn">Save</button>                      <!-- primary (blue) -->
     <button class="action-btn action-btn-confirm">Apply</button>  <!-- green -->
     <button class="action-btn action-btn-danger">Delete</button>  <!-- red -->
     <button class="action-btn action-btn-secondary">Cancel</button> <!-- neutral outline -->
     ```

     The trap: `.action-btn` alone works, because the base **is** the primary button (unlike Bootstrap's neutral bare `.btn`). A lone `.action-btn-secondary` does **not**: it matches no `.action-btn` rule and falls back to a plain grey browser button.

     **A choice is state, not an action.** A view switch, a sort or a filter shows the current choice. It is not a button to press. Never show "selected" by swapping `.action-btn` variants: users read the blue one as the button to press.
     - One of two or three options: `.segmented-control`, with `.active` on the current `.segmented-btn`.
     - Tabs, or one of more options: `.pill-bar` (see `system-knowhow/js-sdk/setup-2.md` § Component classes for its markup).
     - An on/off filter: `.pill-bar-btn` with `aria-pressed="true"` while it is on.

     ```html
     <div class="segmented-control" role="group" aria-label="Sort by">
       <button class="segmented-btn active">Cost</button>
       <button class="segmented-btn">Date</button>
     </div>
     ```

     **Put every `:hover` you write inside `@media (hover: hover)`.** On a touch screen an ungated hover stays on after a tap. The last control tapped then looks greyed or disabled. The shared classes gate their own hovers already.

     ```css
     @media (hover: hover) {
       .card:hover { background: var(--bg-tertiary); }
     }
     ```

     **The engine checks these rules when you write an app file.** After `create_app`, `write_file`, `edit_file` or `run_python` writes HTML, CSS or JS under `apps/<id>/`, the result can end with an `[APP LINT]` note. The note names each line with an ungated `:hover`, an `.action-btn` variant swapped to show a choice, or a variant without its base class. The file is saved. Fix every finding in the same turn.
  5. **Sizes in `rem`, never `px`, so it respects the user's font size.** The user's UI-scale preference is the root font-size, so only `rem`/`em` units scale with it. An app sized in `px` ignores the setting. Size spacing and radii with the `--space-*` / `--radius-*` tokens. `1px` borders are the only acceptable `px`.

     **Use a type-scale token for every font-size, never a hand-picked `rem`/`px` number.** Body text is `--font-size-sm`, a label or control `--font-size-md`, a title `--font-size-lg`, and meta `--font-size-xs`. A value like `0.95rem` or `0.78rem` is a magic number that misses the shared scale, so the element reads a hair off. Pick the nearest step. See `system-knowhow/js-sdk/setup.md` § "Respect the user's font size" and § "Theme variables" for the token values.
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
        /* sdk-iframe.css stops sideways page scroll; rows still wrap. */
        .row { display: flex; flex-wrap: wrap; gap: var(--space-sm); margin-top: var(--space-md); }
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
  progress bar up until the call. Rules: `system-knowhow/js-sdk/lucidos-ui-ui-control.md` § Showing the app once
  its content is ready.

## App icon

Every app and every widget gets an icon. Lucidos draws it on the apps panel, the menu drawer, Search Everywhere, the history menu and the coding-agent App row. Widgets need one most: a dashboard's Add lists them side by side.

- **Pass `icon_svg` to `create_app`.** It saves the SVG as `assets/icon.svg` and sets the manifest's `"icon": "assets/icon.svg"`.
- **The name is words only.** Never put an emoji or other pictograph in an app or widget name. Lucidos draws the icon beside the name, so a leading emoji shows two icons side by side. Draw the motif in the SVG icon instead.
- **Draw for 18px.** A square `viewBox`, one bold motif in full colour, no text and no fine detail. Lucidos clips it to a rounded square and adds a faint theme hairline inside its edge. Give it its own background or leave a little margin; Lucidos supplies the tile's border.
- **The field has one meaning:** a relative path inside the app folder to an SVG, PNG or WebP file of at most 512 KiB. An emoji, a glyph name, a `..` path or a missing file counts as no icon.
- **Without a valid icon, a monogram tile stands in:** the first letter of the name on a tile coloured from the theme. It is a fallback, not a design choice.
- **To add or change an icon later,** write the file under `assets/` and set `icon` in `manifest.json`.

Lucidos renders the icon as an `<img>`, so script inside an SVG never runs.

## Responsive by default: a phone is a first-class target

An app opens in the *content pane*. On a laptop that is one column of a split layout; on a phone it is a full-width swipe pane. The same markup serves both, so build for the narrow case and let it grow. **The shared stylesheet does not lay it out for you**: `sdk-iframe.css` has exactly two width breakpoints, both for markdown tables. An app that reads well only at desk width is a defect, like a hardcoded hex.

**The page never scrolls sideways.** `sdk-iframe.css` clips `<html>` and `<body>` on the x axis and caps images and video at `max-width: 100%`. So a too-wide element is **cut off** at the right edge, not scrollable. The clip hides the bug; it does not fix it. When something is cut off, find the element and fix it:

- **Text that can grow wraps or shrinks.** Never put `flex: none` and `white-space: nowrap` on text that holds user data or a free-text note: it grows to the text's width. Use `flex: 0 1 auto; min-width: 0;` and let it wrap, or ellipsize it with `overflow: hidden; text-overflow: ellipsis`. A widget never ellipsizes data text: it shows less (§ The widget box, rule 8).
- **A flex or grid child that should shrink takes `min-width: 0`.** Its default minimum is its content width, so one long word holds the whole row open.
- **Never size with `100vw`.** It ignores the pane's padding. Use `width: 100%`.
- **A sideways scroller is its own element.** A chip row or a wide table goes in a container with `overflow-x: auto`, and only that container scrolls.
- **Lock page scroll on `<html>`, never `<body>`.** For a modal, set `document.documentElement.style.overflow = 'hidden'`. A `body { overflow: hidden }` lock no longer reaches the page, so the page scrolls on behind the modal.
- **Check at 320px.** That is the narrowest phone. Look for anything cut off at the right edge.

Six rules cover almost every app:

- **One column first.** Start with a single stacked column, and add columns only where the width allows. `grid-template-columns: repeat(auto-fit, minmax(14rem, 1fr))` collapses on its own, with no breakpoint to maintain.
- **No fixed container width.** Never set `width` in `px` on the body, a panel, or a card. Constrain with `max-width` in `rem` and let the element be narrower when the pane is.
- **Rows wrap.** A flex row of controls takes `flex-wrap: wrap` and a `--space-*` gap. For a row of buttons, use `.button-group`, which already wraps and ellipsizes.
- **Tables and images use the wrappers, inside a `.markdown-content` container.** Wrap a table in `.table-scroll-wrapper` and an image in `.image-scroll-wrapper`. From about four columns up, stamp `data-stack` on the table and `data-label` on every cell, and each row becomes a card at 768px and under. Every rule is scoped, as in `.markdown-content table[data-stack]`, so a wrapper outside that container silently does nothing. See `system-knowhow/js-sdk/setup-2.md` § Component classes.
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

