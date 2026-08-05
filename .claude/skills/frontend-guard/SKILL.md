---
name: frontend-guard
description: Use when modifying Lucidos frontend code (CSS, TSX, components) — checks changes against design tokens, component classes, and conventions to prevent drift
---

When modifying Lucidos frontend code (CSS, TSX, components), check every change against these rules. Flag any violation before committing.

## Design Tokens (`:root` in `global.css`)

Use these — never hardcode the raw values:

| Token | Value | Use for |
|-------|-------|---------|
| `--bg-primary` | `#07172e` | Page/app background |
| `--bg-secondary` | `#0d2244` | Cards, elevated surfaces |
| `--bg-tertiary` | `#163052` | Inputs, hover states, code blocks |
| `--border-color` | `#2c456a` | All borders |
| `--text-primary` | `#e6edf3` | Headings, important text |
| `--text-secondary` | `#8b949e` | Supporting text |
| `--text-muted` | `#6e7681` | Timestamps, hints |
| `--text-on-accent` | `#fff` | White text on colored backgrounds |
| `--accent` | `#58a6ff` | Links, highlights, default action |
| `--accent-green` | `#3fb950` | Success, confirm actions |
| `--accent-yellow` | `#d29922` | Warnings, high importance |
| `--accent-red` | `#f85149` | Errors, danger actions |

### Z-Index Scale

| Token | Value | Use for |
|-------|-------|---------|
| `--z-dropdown` | `100` | Dropdowns, picker menus |
| `--z-sticky` | `200` | Sticky headers, drawer overlays |
| `--z-drawer` | `300` | Side drawers |
| `--z-control-panel` | `2200` | Floating header items (brand, route panel, collapsed thread actions) |
| `--z-app-fullscreen` | `2250` | A pseudo-fullscreen app panel: covers the header chrome, and is covered by every host overlay |
| `--z-modal` | `2300` | Modal overlays — sits above the header so the dim backdrop blocks it |
| `--z-toast` | `2400` | Toast notifications (must sit above `.ui-blocking-overlay` at `--z-control-panel + 100`) |
| `--z-tooltip` | `10000` | Tooltips |

Raw z-index values 1-10 are fine for local stacking contexts within a component. Anything higher must use a token.

### Transitions

| Token | Value | Use for |
|-------|-------|---------|
| `--duration-fast` | `0.15s` | Hover feedback, small toggles |
| `--duration-normal` | `0.2s` | State changes, color transitions |
| `--duration-slow` | `0.3s` | Layout animations, slide-ins |
| `--duration-emphasis` | `0.5s` | Deliberate state-change cues users must register (rare — reach for normal/slow first) |

### Icon Sizes

| Token | Value | Use for |
|-------|-------|---------|
| `--icon-size-sm` | `0.875rem` | List row actions, inline icons |
| `--icon-size-md` | `1rem` | Utility buttons, standalone icons |
| `--icon-size-lg` | `1.25rem` | Header-level icons, nav buttons |

### Shadows

Use `var(--shadow-sm)`, `var(--shadow-md)`, `var(--shadow-lg)`. Never hardcode `box-shadow` values.

### Syntax Highlighting

Use `--syntax-key`, `--syntax-string`, `--syntax-number`, `--syntax-keyword`, `--syntax-comment`, `--syntax-control`.

## Component Classes

> **Where component CSS lives (three files, split by audience — put each rule in the right one):**
> - **Reusable (host + app iframes)** → `styles/global/shared-components.css`. SINGLE SOURCE OF TRUTH: the engine `include_str!`s this exact file and appends it to `/api/v1/sdk-iframe.css` (`crates/lucidos-engine/src/api/sdk.rs`), so a class added here ships to the host AND every opted-in app at once. Never copy these into the engine's `sdk_iframe.css`. When you add an app-facing class here, also add it to the component-class table in `system-knowhow/js-sdk.md` (the app-author contract).
> - **Host-chrome only (never served to apps)** → `styles/global/host-components.css` (custom `<Dropdown>`/`.nav-history-*`, `.send-cancel-*`, `.icon-btn.header-icon`/`.filter-active`/`.pinned`, `.list-row.flip-animating`).
> - **Iframe-only (apps, not host)** → the engine's `crates/lucidos-engine/src/api/sdk_iframe.css` (`.action-btn-secondary`, `.lucidos-select`).

### Buttons

- **`.action-btn`** — filled text button. Default blue, `.action-btn-confirm` green, `.action-btn-danger` red (in `shared-components.css`). For a neutral secondary button in an app iframe, `.action-btn-secondary` (engine `sdk_iframe.css`).
- **`.icon-btn`** — icon-only button. SVGs inside are auto-sized via `.icon-btn svg { width: var(--icon-size-sm) }`. Do NOT set inline `width`/`height` on SVGs inside `.icon-btn`.
- Never create per-component button styles. Use the shared classes.

### Lists

All list items use `.list-row` / `.list-row-info` / `.list-row-actions` (in `shared-components.css`). Never create custom list item layouts.

### Dropdowns

Use the `Dropdown` component from `components/shared/Dropdown.tsx`. Never use native `<select>`.

### Textareas

Multi-line fields in modals use `AutoTextarea` from `components/shared/AutoTextarea.tsx`. Never use `<textarea rows={N}>`.

### Tooltips

Use `data-tooltip="text"`. Never use the HTML `title` attribute. Tooltips are desktop-only (disabled globally on touch devices) — no per-element opt-out needed.

### Dialogs

Use `showToast(message, type)` for notifications and `await showConfirm(message, okLabel)` for confirmations. Never use `alert()`, `confirm()`, or `prompt()`.

## Rules

1. **All sizes in `rem`** — all padding, margin, gap, width, height, min/max sizes, positioning, border-radius, line-height, translateX/Y, and icon sizes must use `rem` (divide px by 16). Only `1px` borders/outlines, `0px` in `env()` fallbacks, `@media` breakpoints, and `box-shadow` blur/spread may use `px`.
   - **`font-size` specifically must use a `--font-size-*` token, not a raw `rem`** — the closed type scale is `--font-size-{3xs,2xs,xs,sm,md,lg,xl,2xl,3xl,display}` (defined in `styles/global/base.css`, mirrored into the engine's `api/sdk_iframe.css` for app iframes). A raw `font-size: N rem` literal is a drift finding — snap to the nearest token. `em` / percentage / `inherit` / `var(--user-ui-scale)` font-sizes are legitimate and stay literal (deliberately relative / the root scale), as is a value **deliberately pinned to a computed-px threshold** (e.g. the mobile prompt textarea's `0.9rem`, kept ≥16px to avoid iOS input zoom) — such a literal must carry a comment saying why it isn't a token.
2. **All icon buttons need `aria-label`** — if the button has no visible text, add `aria-label`.
3. **No hardcoded colors** — use CSS variables. Exception: `#fff` / `rgba(255,255,255,*)` on translucent overlays over image content.
4. **No dead code** — delete unused styles, don't comment them out.
5. **Reusable component styles in `styles/global/shared-components.css`** (the single source shared with app iframes); host-chrome-only in `host-components.css`; domain styles in their own file under `styles/`.
6. **`.error-text`** for inline error messages — defined in `shared-components.css`.
7. **Links always underlined** — use subtle `text-decoration-color` at 40% opacity, full on hover.
8. **Loading states use `useDelayedLoading`** — 300ms delay before showing spinner text.
9. **`Loadable<T>` for all async data** — handle all four states (not-loaded, loading, loaded, failed). Never render not-loaded/loading as empty array. Failed must look different from empty.
