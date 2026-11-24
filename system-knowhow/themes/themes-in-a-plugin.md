---
name: Themes, Themes in a plugin
description: Part of Themes. Its index lists the sections it covers.
---

One part of `system-knowhow/themes`, which lists every part.

## Themes in a plugin

A plugin ships themes in a `themes/` folder, one `<id>.json` per theme. Install
copies them to `data/themes/`. Staging validates each one, so a plugin with a
broken theme never installs. See `plugins.md`.

## For apps

App frames follow the active theme too, for the tokens they define. An app that
loads the SDK stylesheet also paints the `app-*` parts, unless it sets
`data-theme-parts="off"` on its `<html>`. An app without the SDK gets no part.
See `js-sdk.md`. A theme-building app reaches these routes through
`lucidos.request`:

| Route | Returns |
|---|---|
| `GET /api/v1/themes` | every theme, built-ins first, each with `resolved.dark` and `resolved.light`, which include the theme's `--protected-*` palette |
| `GET /api/v1/theme?id=<id>` | one theme, resolved |
| `GET /api/v1/themes/tokens` | the theme token catalog below, with defaults per mode |
| `GET /api/v1/themes/parts` | the theme part catalog: each part, its properties with their grammar, caps, part token and default, and the protected surfaces |
| `POST /api/v1/themes/resolve` | a draft theme file resolved without saving it: `{resolved, modes}`, or a 422 with the refusal a save would give. It resolves workspace fonts against the installed ones. |
| `GET /api/v1/fonts` | the font catalog, then the workspace fonts: each font's `id`, `label`, `stack`, `kind`, `group` (`sans`, `serif` or `mono`), `source`, `license`, `ligatures`, `bold` (whether it has a bold face), and `theme_nameable` (true for every font). A workspace font adds `family` and `faces`. `invalid` lists workspace fonts that failed a check, with the reason. |

It saves with `lucidos.data.write('themes/<id>.json', …)` and deletes with
`lucidos.data.delete`, which run the same validation. It switches themes with
`lucidos.preferences.set('theme', id)`. On save, keep every field you did not
edit, `parts`, `dark.parts` and `light.parts` included.

For a live preview while the user tunes, write `style_overrides` and clear it on
save. A parts preview writes the draft's resolved part tokens there. They pass
the same grammar, so a bad one is refused at the write. `style_overrides` refuses
`--part-screen-background-image`, so preview scanlines by saving the theme.

## The theme token catalog

Every token a theme can tune. The engine serves the same list, with each token's
default per mode, at `GET /api/v1/themes/tokens`.

### Surfaces

The backgrounds everything sits on, from the page to raised panels.

| Token | What it paints | Filled in from |
|---|---|---|
| `--bg-primary` | The page background. A seed: set it and the other surfaces, the text greys and the header follow. |  |
| `--bg-secondary` | Panels, cards and the default floating surface. | `--bg-primary`, `--text-primary` |
| `--bg-tertiary` | Chips, question options and controls on a surface. | `--bg-primary`, `--text-primary` |
| `--bg-quaternary` | The strongest fill, for pressed and nested controls. | `--bg-primary`, `--text-primary` |
| `--bg-hover` | The fill a row or control takes under the pointer. | `--bg-primary`, `--text-primary` |
| `--bg-selected` | The fill of the selected row. | `--bg-primary`, `--text-primary` |
| `--border-color` | Hairlines between rows and around surfaces. | `--bg-primary`, `--text-primary` |
| `--surface-bg` | Toasts, menus, popovers and dialogs. |  |
| `--picked-surface` | The user's own message bubble and a chosen answer. |  |
| `--scrim` | The dimming layer behind a blocking dialog. |  |

### Text

Foreground colours for prose, labels and anything drawn on the accent.

| Token | What it paints | Filled in from |
|---|---|---|
| `--text-primary` | Body text. A seed: set it and the secondary and muted greys follow. |  |
| `--text-secondary` | Labels and supporting text. | `--bg-primary`, `--text-primary` |
| `--text-muted` | Hints, timestamps and placeholders. | `--bg-primary`, `--text-primary` |
| `--text-on-accent` | Labels on filled accent buttons and badges. | `--accent` |
| `--text-strong` | Bold text, where the UI font has no bold face and draws bold with its regular strokes. A step brighter than the text by default. |  |
| `--text-glow` | Kept for themes written before parts: the text-shadow of the chat-text, actor-label, header-title and composer-text parts. None by default. See § Theme parts. |  |

### Accent

The brand and action colours: links, active states, filled buttons.

| Token | What it paints | Filled in from |
|---|---|---|
| `--accent` | Links, active states and highlights. A seed: set it and the action colour, the header tint and the focus cues follow. |  |
| `--accent-light` | A lighter step of the accent, for hover and emphasis. | `--accent` |
| `--accent-action` | The fill of primary buttons. | `--accent` |
| `--brand-blue` | The Lucidos brand blue, behind the default action colour and light header. |  |
| `--brand-blue-deep` | The deeper brand blue, the light header's lower stop and the header badge text. |  |
| `--initiator-coding-agent` | The coding-agent brand orange. The Claude mark takes it unless a theme sets --claude-mark. |  |

### Brand marks

The Lucidos tile, the Claude logo and the Codex logo, on the chat's actor chips and wherever else they draw in their own colours. They keep their brand colours until a theme sets them.

| Token | What it paints | Filled in from |
|---|---|---|
| `--lucidos-mark-bg-top` | The Lucidos tile's gradient at its upper-left light source. |  |
| `--lucidos-mark-bg-bottom` | The Lucidos tile's gradient at the far corner. Set both stops to one colour for a flat tile. |  |
| `--lucidos-mark-fg` | The three squares and the spark on the Lucidos tile. |  |
| `--claude-mark` | The Claude logo on the chat's actor chips and in navigation history. Follows --initiator-coding-agent by default. |  |
| `--codex-mark` | The Codex logo's fill. Follows the primary text colour by default. |  |

### Status

Colours that carry meaning: success, caution, error, and neutral notable state.

| Token | What it paints | Filled in from |
|---|---|---|
| `--accent-green` | Success, applied, confirm. |  |
| `--accent-green-soft` | A status word in a pill, such as Installed or applied: the success green pulled toward secondary text. |  |
| `--accent-yellow` | Caution only. Never a neutral marker. |  |
| `--accent-red` | Errors and destructive actions. |  |
| `--accent-orange` | A warm step between caution and error. |  |
| `--accent-notable` | A neutral state worth marking that is not a caution: waiting, trimmed, changed. |  |

### Header

The bar across the top of every pane: its fill, its foreground, badges and control veils.

| Token | What it paints | Filled in from |
|---|---|---|
| `--header-bar-top` | The header gradient's top stop. The macOS title-bar band takes it too. | `--accent`, `--bg-primary` |
| `--header-bar-bottom` | The header gradient's bottom stop. | `--accent`, `--bg-primary` |
| `--header-gradient` | The header's whole fill. Built from the two stops by default; set it to a flat colour for a flat bar. |  |
| `--titlebar-strip-bg` | The reclaimed macOS title-bar band above the header. |  |
| `--titlebar-strip-continues` | 1 paints the header fill up through the macOS title-bar band, lined up with the header. 0 keeps the band solid. | `--header-gradient` |
| `--header-fg` | Every glyph and label on the header. | `--accent`, `--bg-primary`, `--text-primary` |
| `--header-fg-muted-alpha` | How strong muted header glyphs are, from 0 to 1. |  |
| `--header-fg-muted` | Muted header glyphs. Built from the header text and its strength by default. |  |
| `--header-control-veil-hover` | How much header text colour a control mixes in under the pointer. |  |
| `--header-control-veil-active` | The veil of a header control that is toggled on. |  |
| `--header-control-veil-active-hover` | The veil of a toggled-on control under the pointer. |  |
| `--header-divider` | The line between pane headers. By default the pane divider colour, so each divider runs unbroken to the window top. |  |
| `--header-badge-bg` | The fill of a count badge on the header. | `--accent`, `--bg-primary`, `--text-primary` |
| `--header-badge-fg` | The text of a count badge on the header. | `--accent`, `--bg-primary`, `--text-primary` |

### Focus

How Lucidos shows where you are: the focused pane's header, the mobile pane dot, the navigation spotlight and the keyboard focus ring.

| Token | What it paints | Filled in from |
|---|---|---|
| `--focus-header-tint` | The wash over the focused pane's header segment. Set it transparent to turn the wash off. | `--accent`, `--bg-primary`, `--text-primary` |
| `--focus-header-underline` | A line under the focused pane's header segment. Transparent (off) by default. |  |
| `--focus-header-underline-width` | The thickness of the focused header underline, in px. |  |
| `--focus-pill-bg` | The active pane dot on the phone header. | `--accent` |
| `--focus-pill-glow` | The bloom around the active pane dot. | `--accent` |
| `--nav-focus-glow` | The neutral spotlight on the item you navigated to. Keep it hue-free: it marks a place, not a state. | `--bg-primary`, `--text-primary` |
| `--nav-focus-border-alpha` | How strong the spotlight's hairline border is. |  |
| `--focus-ring` | The ring around the control that has keyboard focus. Keep its band within `0.1875rem`: that is the room a scrolling or clipping box leaves for it, and a wider ring is cut there. |  |

### Shape

Corner radii of controls, surfaces, and circles and pills.

| Token | What it paints | Filled in from |
|---|---|---|
| `--radius-control` | Corner radius of buttons, inputs and rows. |  |
| `--radius-surface` | Corner radius of toasts, menus, popovers and dialogs. |  |
| `--radius-round` | Corner radius of circles and pills: round buttons, dots and chips. Set it to 0 to square them. Badges on icons stay round in every theme. |  |

Every corner in Lucidos follows one of the three. A smaller or larger corner,
like a chip or the composer box, is a multiple of the control or surface
radius. So it scales with them. For a fully square look, set all three to `0`.
Spinners and the badges on icons stay round.

### Shadows

Elevation of floating surfaces.

| Token | What it paints | Filled in from |
|---|---|---|
| `--shadow-sm` | Low elevation. |  |
| `--shadow-md` | Menus and popovers. |  |
| `--shadow-lg` | Dialogs. |  |
| `--shadow-up` | A surface docked under the content it lifts off. |  |

### Syntax

Code highlighting in previews and tool output.

| Token | What it paints | Filled in from |
|---|---|---|
| `--syntax-key` | Code highlighting: keys. |  |
| `--syntax-string` | Code highlighting: strings. |  |
| `--syntax-number` | Code highlighting: numbers. |  |
| `--syntax-keyword` | Code highlighting: keywords. |  |
| `--syntax-comment` | Code highlighting: comments. |  |
| `--syntax-control` | Code highlighting: control flows. |  |
| `--syntax-type` | Code highlighting: types. |  |
| `--syntax-function` | Code highlighting: functions. |  |

### Typography

The code font. A theme suggests the UI font with `fonts.ui`, and the user's own font pick wins over it.

| Token | What it paints | Filled in from |
|---|---|---|
| `--font-mono` | The font for code, paths and tool output. |  |
