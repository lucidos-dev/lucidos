---
name: Themes
description: Switch, make or ship themes: colours, headers, focus cues. "Nord theme", "flat header", "theme plugin".
---

# Themes

A *theme* is a named set of design-token values: colours, header chrome, focus
cues, radii and shadows. It can also suggest fonts. Light, dark and system are
the *theme mode*, set by the `theme-mode` preference, and a theme styles one mode
or both.

A theme carries **tokens and theme parts**, never CSS. It can recolour Lucidos
and add capped effects, such as a glow on chat text. It cannot move a control,
hide a card, write a selector, or load anything from a third-party origin.

## Switch themes

Set the device-scoped `theme` preference to a theme's id with `set_preference`.
The user finds the same choice under **Settings → Appearance → Theme**, a
carousel of every theme grouped by family.

A theme with a map for one mode only is *light only* or *dark only*. Picked in
the carousel from the other mode, it asks first, then switches the device's
`theme-mode` to the theme's mode. `set_preference` does not ask: set `theme-mode` too when
the user wants to see the theme now.

A workspace theme's card carries a "Custom" badge, so it reads apart from a
built-in one.

Built-in themes: `lucidos` (the default), `minimal`, `mono` (pure black or
white), `amethyst`, `nord`, `catppuccin`, `rose-pine`, `gruvbox`, `solarized`,
`tokyo-night`, `everforest`, `paper` (light only), `ember`, `harbour`, and
`real-computer` (dark only). An unknown id shows the default.

The picker groups themes by *family*, one section each, in this order:

| Family | Section | Built-in themes |
|---|---|---|
| `blue` | Cool | Lucidos, Nord, Tokyo Night, Solarized, Harbour |
| `violet` | Violet | Amethyst, Catppuccin, Rosé Pine |
| `warm` | Warm | Gruvbox, Everforest, Paper, Ember |
| `neutral` | Neutral | Minimal, Mono, Real computer |

The `blue` section reads "Cool" because only Lucidos has a blue background.
The others share a blue accent on slate, teal or indigo.

A theme that names no family shows last, under "Other".

## Make a theme

A workspace theme is a file, `data/themes/<id>.json`. The id is lowercase letters
and digits joined by single hyphens, and cannot be a built-in id. Write it with
`write_file` at `themes/<id>.json`, or with `lucidos data write themes/<id>.json`.
Then set `theme` to the id.

```json
{
  "name": "Harbour",
  "description": "Deep teal with a brass accent.",
  "author": "Me",
  "credit": "Palette after a harbour at dusk.",
  "family": "blue",
  "tokens": { "--radius-control": "0.375rem" },
  "fonts": { "ui": "geist", "mono": "geist-mono" },
  "dark": {
    "--bg-primary": "#0f2327",
    "--text-primary": "#e3ecea",
    "--accent": "#d4a650"
  },
  "light": {
    "--bg-primary": "#f4f1ea",
    "--text-primary": "#1d2b2e",
    "--accent": "#9a6b1c"
  }
}
```

- `tokens` applies in both modes. A mode map wins over it.
- A theme with no map for a mode leaves that mode at the default, apart from
  `tokens`.
- `fonts` is optional. See § Fonts.
- `parts` is optional, at the top level and in each mode map. See § Theme parts.
- `name` is required. `description`, `author` and `credit` are optional.
  Credit the palette when it comes from someone else.
- `family` is optional: `blue`, `violet`, `warm` or `neutral`. Pick the one the
  theme reads as at a glance, which is not always its accent. Any other value is
  refused.

**Rules every theme must follow.** Every key is a token from § The theme token
catalog, and nothing else: not the spacing or type scale, not `--z-*`, and not
`--protected-*`. Every value is at most 120 characters and uses no `url(`,
`image-set(`, `expression(`, `;`, braces, `<`, `>`, `@`, backslash or `/*`.
`var()`, `color-mix()`, `rgba()`, `calc()` and gradients are fine. A theme never
sets `--font-ui`, `--font-family`, `--font` or the two `--font-features-*`
tokens: it suggests a UI font with `fonts.ui` instead. These rules keep a theme
from reading as hostile. See § Protected surfaces for why:

- `--text-primary` on `--bg-primary` reaches at least 3:1, in both modes.
- `--accent-green` never reads as red, and `--accent-red` never reads as green.
- A shadow token reaches at most `2rem` (`32px`) past its box. Each layer
  counts its larger offset, its spread and half its blur together. Inside a
  shadow, only colour functions such as `rgba()` or `color-mix()` are allowed.
- A colour token holds a colour: a hex, `rgb()`, `hsl()`, `oklch()`,
  `oklab()`, `color-mix()`, `var()` of another colour token, a named colour or
  `transparent`.
- No token map sets a `--part-*` token. Set parts with `parts`.

A write through `write_file`, `copy_file` or `lucidos data write` that breaks a
rule is refused with the reason, and nothing reaches disk. A theme file written
any other way (`run_python`, say) that breaks a rule never shows: the picker
skips it, and setting it paints the default. If the active theme stops passing,
the default theme shows and a warning names the rule it broke.

A theme is always written whole: `edit_file` and the partial-edit route refuse
`themes/`. Read the theme, then write it back whole. Rewriting the active theme's
file through `write_file` or `lucidos data write` repaints every device at once.
After any other kind of write, set `theme` again.

## Three seeds are enough

`--bg-primary`, `--text-primary` and `--accent` are *seeds*. Set one, and the
engine fills in every token that derives from it and that the theme left unset:
the surface ladder, the text greys, the action colour, the header, its badge and
the focus cues. The table below names each token's seeds. Set a token yourself
to override its derivation.

Write seeds as hex literals where you can. A hex `--bg-primary` also paints the
page before any stylesheet loads, so a cold start shows the theme at once.

**The way out.** If a theme makes the UI unreadable, open Lucidos with
`?style-reset` on the URL. It resets the theme to the default, and clears the
style overrides, before the first pixel is painted.

## Fonts

A theme suggests fonts by font id, in its `fonts` field:

- `ui` is the UI font. It paints only on a device whose `font-family`
  preference is `theme` ("Follow the theme" in Settings), which is the default.
  A font the user picked always wins. A theme with no `ui` font leaves the
  default, Fira Code. Of the built-in themes, Paper suggests Source Serif 4,
  Minimal suggests Geist and Mono suggests Geist Mono.
- `mono` is the code font. It sets `--font-mono` in both modes, so it paints on
  every device. Set it with `fonts.mono` or a free-form `--font-mono` token,
  not both. A `mono` font must be monospaced (kind `both` or `mono`).

The font ids and what each is fit for come from `GET /api/v1/fonts`. Every font
is one Lucidos serves itself (`vendored`), one the device already has
(`device`), or one installed in the workspace (`workspace`, id `ws-<slug>`).
None makes a request to the internet, so a theme may name any of them. A write
that names an unknown id, a workspace font that is not installed, or a
proportional code font, is refused with the reason. A workspace font removed
later drops out of the theme, which then paints as if it named nothing. See
`system-knowhow/workspace-fonts.md`.

The fonts, by group:

| Group | Fonts |
|---|---|
| Sans (UI only) | `system`, `geist`, `atkinson-hyperlegible-next`, `inter`, `roboto`, `open-sans`, `manrope` |
| Serif (UI only, for prose themes) | `source-serif-4`, `lora`, `literata` |
| Mono (valid as `fonts.mono` and as a UI font) | `fira-code`, `monospace`, `geist-mono`, `atkinson-hyperlegible-mono`, `jetbrains-mono`, `ibm-plex-mono`, `source-code-pro`, `commit-mono`, `cascadia-code`, `vt323` |

Fira Code, JetBrains Mono and Cascadia Code ship programming ligatures. As a UI
font they apply them on code surfaces only.

VT323 is a DEC terminal face with a small x-height and one weight. Lucidos
scales it to Fira Code's x-height and centres its line box on the glyphs. It
reads at the size of the other fonts, and the caret sits level with the text.
It has no bold face, so bold text keeps its crisp regular strokes. Where the UI
font has no bold, bold text takes `--text-strong` instead, a step brighter than
the text, the way a terminal showed bold. A theme that sets its page or text
colour gets a step away from its own page, in either theme: toward white on a
dark page, toward black on a light one.

## Headers and focus

The header bar is fully tunable:

- **A flat bar:** set `--header-gradient` to `var(--header-bar-top)`, and the
  two stops to one colour, often `--bg-primary`.
- **Dark text on a light bar:** set `--header-fg` to a dark colour.
  `--header-fg-muted` and the control veils follow it.
- **Lines between pane headers:** set `--header-divider` to a colour.
- **A patterned bar:** set `--header-gradient` to a pattern, such as a
  `repeating-linear-gradient`. On macOS the pattern carries on up through the
  title-bar band, lined up with the header, because setting `--header-gradient`
  sets `--titlebar-strip-continues` to 1. `--titlebar-strip-bg` stays the solid
  colour under it and the window colour.

Focus shows in four places, and a theme tunes each one:

- the wash over the focused pane's header, `--focus-header-tint`;
- the underline under it, `--focus-header-underline` and
  `--focus-header-underline-width`, off by default;
- the active pane dot on a phone, `--focus-pill-bg` and `--focus-pill-glow`;
- the spotlight on the item you navigated to, `--nav-focus-glow`.

To swap the wash for an underline, set `--focus-header-tint` to `transparent`
and `--focus-header-underline` to your accent.

## Actor icons

The chat marks each actor with its brand icon, and a theme can recolour all
three:

- **The Lucidos tile:** `--lucidos-mark-bg-top` and `--lucidos-mark-bg-bottom`
  for its gradient, `--lucidos-mark-fg` for the glyph. Set both stops to one
  colour for a flat tile.
- **The Claude logo:** `--claude-mark`. A theme that sets only
  `--initiator-coding-agent` recolours it too; `--claude-mark` wins over it.
- **The Codex logo:** `--codex-mark`. It follows `--accent-light` until set.

These never derive from the seeds. A green-on-black theme that wants green
marks sets them, for example `"--claude-mark": "var(--accent)"`.

## Theme parts

A *theme part* is a named region of the UI that a theme may style with capped
paint-only properties: a glow on chat text, a tinted glow on the actor icons,
wider letter-spacing on header titles. The theme names the part and the
property. It never writes a selector: Lucidos owns every selector.

```json
{
  "name": "Green Screen",
  "parts": {
    "chat-text":    { "text-shadow": "0 0 0.3em var(--accent)" },
    "header-title": { "letter-spacing": "0.08em" }
  },
  "dark": {
    "--bg-primary": "#050805",
    "parts": { "actor-icon": { "filter": "drop-shadow(0 0 3px var(--accent))" } }
  },
  "light": {
    "--bg-primary": "#f2fff2",
    "parts": { "chat-text": { "text-shadow": "none" } }
  }
}
```

- **`parts` applies in both modes.** A mode map's own `parts` wins per part and
  property, the way mode tokens win over `tokens`. Above, dark keeps the chat
  glow and adds an icon glow, and light turns the chat glow off.
- **`none` switches a shared effect off in one mode.** `text-shadow`,
  `box-shadow` and `filter` accept it. A part colour meant for one mode goes in
  that mode's `parts`.
- **A mode map that holds only `parts` does not make a theme dark only or light
  only.** A glow in one mode never makes the user switch modes.
- **Take colours from tokens.** A part colour may name a colour token,
  `var(--accent)`, with no fallback. A mode switch or a seed change then
  repaints the effect for free.

The engine checks each value against its property's grammar and emits one
*part token* per part property, such as `--part-chat-text-text-shadow`. A
refusal names the field, the rule and the limit, for example
`light.parts.chat-text.text-shadow: blur 2em is over the 0.6em cap.`
`GET /api/v1/themes/parts` serves the catalog below with every cap.

| Part | What it paints | Properties |
|---|---|---|
| `chat-text` | Prose in chat messages, yours and the agent's. | `color`, `text-shadow`, `letter-spacing` |
| `chat-heading` | Headings inside chat messages. Unset properties follow chat text. | `color`, `text-shadow`, `letter-spacing` |
| `chat-link` | Links inside chat messages. An unset glow follows chat text. | `color`, `text-shadow` |
| `inline-code` | Inline code spans inside chat messages. | `color`, `background-color` |
| `code-block` | Fenced code blocks inside chat messages. An unset glow follows chat text. | `background-color`, `box-shadow`, `text-shadow` |
| `actor-label` | The name beside each actor icon on a message. | `text-shadow`, `letter-spacing` |
| `actor-icon` | The Lucidos, Claude and Codex marks beside a message. The Brand marks tokens colour them; this part adds a glow. | `filter` |
| `header-icon` | The glyphs on the icon buttons in a pane's header. Their colour follows --header-fg-muted. | `filter` |
| `header-title` | Pane, thread and workspace titles in the header. Their colour follows --header-fg. | `text-shadow`, `letter-spacing` |
| `composer` | The box around the message you type in chat. It can overlap scrolled messages, so it takes inset shadows only. | `background-color`, `border-color`, `box-shadow` |
| `composer-text` | The text you type in the composer. `caret-shape` is native in Chromium and drawn by Lucidos elsewhere. | `color`, `caret-color`, `caret-shape`, `text-shadow`, `letter-spacing` |
| `card` | The bordered blocks a step shows: tool output, its result and its reasoning. | `border-style`, `border-width`, `border-color` |
| `surface` | Every toast, popover, menu and dialog. Protected dialogs keep their own frame. | `border-style`, `border-width`, `border-color` |
| `screen` | The base fill behind every pane. Scanlines paint here, under all content. | `background-image` |
| `app-text` | Body text in an app frame. | `text-shadow`, `letter-spacing` |
| `app-link` | Links in an app frame. An unset glow follows app text. | `text-decoration-color`, `text-shadow` |
| `app-control` | Buttons, inputs and the SDK select in an app frame. | `border-color`, `box-shadow` |

A nested part follows its parent: a glow on `chat-text` reaches headings,
links and code blocks unless the theme sets them apart.

| Property | Values | Caps |
|---|---|---|
| `color`, `caret-color` | a colour | alpha at least 0.6; no bare `transparent` |
| `background-color`, `border-color`, `text-decoration-color` | a colour | none beyond the colour grammar |
| `text-shadow` | `none`, or 1 or 2 layers of `<x> <y> <blur> <colour>` | `em` only; x and y within ±0.1em; blur 0 to 0.6em |
| `box-shadow` | `none`, or 1 or 2 layers of `[inset] <x> <y> <blur> [<spread>] <colour>` | `px` only; x and y within ±4px; blur 0 to 16px; spread within ±2px. The composer takes `inset` only. |
| `filter` | `none`, or one `drop-shadow(<x> <y> <blur> <colour>)` | `px` only; x and y within ±2px; blur 0 to 6px |
| `letter-spacing` | `normal`, or a length | `em` only; −0.02em to 0.12em |
| `caret-shape` | `auto`, `bar`, `block` or `underscore` | none |
| `border-style` | `solid` or `double` | none |
| `border-width` | a length | `px` only; 1px to 4px |
| `background-image` | `none`, or `repeating-linear-gradient()` of 2 to 4 stops | see § Retro effects |

A part colour takes one of these forms:

- a hex of 3, 4, 6 or 8 digits;
- `rgb()`, `rgba()`, `hsl()`, `hsla()`, `oklab()` or `oklch()` with plain
  numbers and percentages;
- `color-mix(in srgb | oklab | oklch, …)`, nested at most two deep;
- `var(--<colour token>)`, a named colour, or `currentColor`.

`transparent` alone may be a shadow colour, a `color-mix()` input or a
scanline stop.
`calc()`, `attr()`, `url()` and anything else the grammar does not name is
refused. A part text colour must reach 3:1 against the background it sits on,
the same floor page text has.

No part reaches a protected surface. Each protected surface resets every part
token and the inherited part properties, and paints above its neighbours.

### Text glow

`--text-glow` is kept for themes written before parts. The engine sets it as the
`text-shadow` of `chat-text`, `actor-label`, `header-title` and
`composer-text`, so it follows every part rule: the caps, colour typing,
protected surfaces and theme effects. An explicit part value wins over it. A
glow sized in `rem` or `px` compiles as `em`. For a new theme, set
the parts instead.

### Theme effects

The device-scoped `theme-effects` preference decides whether part shadows,
filters and scanlines show. `reduce` drops every part `text-shadow`,
`box-shadow` and `filter` and the screen's scanlines. It keeps part colours,
letter-spacing, the caret shape and borders. `system`, the default,
drops them when the OS asks for more contrast or less transparency. `full`
always shows them.

The user finds it under **Settings → Appearance → Theme → Effects**. Offer
`reduce` to a user who finds a glow hard to read, or wants to save battery.

### Retro effects

Three parts give a theme an old-terminal feel: a block caret, scanlines and
double-line boxes.

```json
{
  "name": "Phosphor",
  "dark": { "--bg-primary": "#050805", "--text-primary": "#33ff33" },
  "parts": {
    "composer-text": { "caret-shape": "block", "caret-color": "#33ff33" },
    "screen": {
      "background-image": "repeating-linear-gradient(transparent 0, transparent 2px, rgba(51, 255, 51, 0.08) 2px, rgba(51, 255, 51, 0.08) 3px)"
    },
    "surface": { "border-style": "double", "border-width": "3px", "border-color": "#0f4d0f" },
    "card": { "border-style": "double", "border-width": "3px", "border-color": "#0f4d0f" }
  }
}
```

**The block caret.** `composer-text` takes `caret-shape: block` or
`underscore`. `caret-shape` is native in Chromium and drawn by Lucidos
elsewhere: Safari, the iPhone, the macOS app and Firefox get the same shape. It
paints in the part's `caret-color`, else the composer text colour, and a block
shows the character under it.

The shape hides while you select text, and the thin caret returns while an
input method composes text. Under reduced motion the caret stops blinking.
Protected inputs, such as the credential form, keep the normal caret.

**Scanlines.** `screen` takes a `repeating-linear-gradient()` of faint stops.
Lucidos paints it on the base fill behind every pane, so it shows through
wherever a pane has no fill of its own. It never paints over text, a card, the
composer or a protected surface. The header draws its own lines through
`--header-gradient`. The rules:

- 2 to 4 stops, each a hex, `rgb()`, `rgba()`, `hsl()` or `hsla()` colour, or
  `transparent`. No `var()`, no named colour, no `color-mix()`.
- Each stop has at most 0.25 alpha.
- No direction or angle: the lines always run across, top to bottom.
- Positions in `px`, from 0 to 8px, never going down. The last stop needs a
  position: it sets how often the pattern repeats.
- `--text-primary`, and a chat part colour, must keep 3:1 on `--bg-primary`
  under every stop.

Protected text keeps 4.5:1 on every band: the engine clamps the protected
palette against the scanlines. So only a theme sets them: `style_overrides`
refuses the screen token. `theme-effects: reduce` turns the scanlines off.

**Double-line boxes.** `surface` and `card` take `border-style: double` and a
`border-width` from 1px to 4px. A double line needs at least 3px. The parts
paint only boxes that already draw a border. Protected dialogs keep their solid
frame.

## Protected surfaces

Some surfaces ask the user to decide. They are:

- permission and question cards;
- the credential and email forms;
- the Apply controls and change list;
- plugin install and uninstall panels;
- the confirm, prompt and progress dialogs, `lucidos.ui.confirm` and
  `lucidos.ui.prompt` included.

No theme can make one of them unreadable or misleading.

The engine derives a `--protected-*` palette from each theme and clamps it:

- text and every button label reach at least 4.5:1 on their fill;
- confirm stays green and deny stays red, whatever the theme's accents;
- a neutral action never turns red, and a blocking dialog's scrim always dims.

These surfaces read that palette and nothing else for text, fills and
confirm or deny colours, so the theme's own values never reach them. Most themes
see no change. A faint or clashing palette shows its repaired version there.
A theme cannot set a `--protected-*` token, and neither can a style override.
Protected surfaces also keep a catalog code font for the command being
approved.

`?style-reset` still clears the theme and every override before first paint.

## Themes in a plugin

A plugin ships themes in a `themes/` folder, one `<id>.json` per theme. Install
copies them to `data/themes/`. Staging validates each one, so a plugin with a
broken theme never installs. See `plugins.md`.

## For apps

App frames follow the active theme too, for the tokens they define. An app that
loads the SDK stylesheet also paints the `app-*` parts, unless it sets
`data-theme-parts="off"` on its `<html>`. An app without the SDK gets no part.
See `js-sdk.md`. An app that builds themes (a theme editor, say) reaches these
routes through `lucidos.request`:

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
`lucidos.preferences.set('theme', id)`. For a live preview while the user tunes,
write `style_overrides` and clear it on save. A preview of parts writes the
draft's resolved part tokens there. Part tokens in `style_overrides` pass the
same grammar, so a bad one is refused at the write. Leave out
`--part-screen-background-image`: `style_overrides` refuses it, so preview
scanlines by saving the theme. Keep every field you do not
edit when you save, `parts`, `dark.parts` and `light.parts` included.

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
| `--codex-mark` | The Codex logo's stroke. Follows the light accent by default. |  |

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
| `--header-divider` | The line between pane headers. Transparent by default, for one seamless bar. |  |
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
| `--focus-ring` | The ring around the control that has keyboard focus. |  |

### Shape

Corner radii of controls, surfaces, and circles and pills.

| Token | What it paints | Filled in from |
|---|---|---|
| `--radius-control` | Corner radius of buttons, inputs and rows. |  |
| `--radius-surface` | Corner radius of toasts, menus, popovers and dialogs. |  |
| `--radius-round` | Corner radius of circles and pills: round buttons, dots, badges and chips. Set it to 0 to square them. |  |

Every corner in Lucidos follows one of the three. A smaller or larger corner,
like a chip or the composer box, is a multiple of the control or surface
radius. So it scales with them. For a fully square look, set all three to `0`.
Spinners stay round.

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
