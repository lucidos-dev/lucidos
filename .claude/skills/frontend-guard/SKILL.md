---
name: frontend-guard
description: Use when modifying Lucidos frontend code (CSS, TSX, components) — checks changes against design tokens, component classes, and conventions to prevent drift
---

When modifying Lucidos frontend code (CSS, TSX, components), check every change against these rules. Flag any violation before committing.

## Design Tokens (`:root` in `global.css`)

Use these — never hardcode the raw values:

| Token | Value | Use for |
|-------|-------|---------|
| `--bg-primary` | `#0b2342` | Page/app background |
| `--bg-secondary` | `#122c50` | Cards, elevated surfaces |
| `--bg-tertiary` | `#1b3a60` | Inputs, hover states, code blocks |
| `--border-color` | `#324f76` | All borders |
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

### Buttons

- **`.action-btn`** — bordered text button. Default blue, `.action-btn-confirm` green, `.action-btn-danger` red.
- **`.icon-btn`** — icon-only button. SVGs inside are auto-sized via `.icon-btn svg { width: var(--icon-size-sm) }`. Do NOT set inline `width`/`height` on SVGs inside `.icon-btn`.
- Never create per-component button styles. Use the shared classes.

### Lists

All list items use `.list-row` / `.list-row-info` / `.list-row-actions` from `global.css`. Never create custom list item layouts.

### Dropdowns

Use the `Dropdown` component from `components/shared/Dropdown.tsx`. Never use native `<select>`.

### Textareas

Multi-line fields in modals use `AutoTextarea` from `components/shared/AutoTextarea.tsx`. Never use `<textarea rows={N}>`.

### Tooltips

Use `data-tooltip="text"`. Never use the HTML `title` attribute. Tooltips are desktop-only (disabled globally on touch devices) — no per-element opt-out needed.

### Dialogs

Use `showToast(message, type)` for notifications and `await showConfirm(message, okLabel)` for confirmations. Never use `alert()`, `confirm()`, or `prompt()`.

## Rules

1. **All sizes in `rem`** — all padding, margin, gap, width, height, min/max sizes, positioning, border-radius, font-size, line-height, translateX/Y, and icon sizes must use `rem` (divide px by 16). Only `1px` borders/outlines, `0px` in `env()` fallbacks, `@media` breakpoints, and `box-shadow` blur/spread may use `px`.
2. **All icon buttons need `aria-label`** — if the button has no visible text, add `aria-label`.
3. **No hardcoded colors** — use CSS variables. Exception: `#fff` / `rgba(255,255,255,*)` on translucent overlays over image content.
4. **No dead code** — delete unused styles, don't comment them out.
5. **Shared styles in `global.css`** — domain styles in their own file under `styles/`.
6. **`.error-text`** for inline error messages — defined in `global.css`.
7. **Links always underlined** — use subtle `text-decoration-color` at 40% opacity, full on hover.
8. **Loading states use `useDelayedLoading`** — 300ms delay before showing spinner text.
9. **`Loadable<T>` for all async data** — handle all four states (not-loaded, loading, loaded, failed). Never render not-loaded/loading as empty array. Failed must look different from empty.
