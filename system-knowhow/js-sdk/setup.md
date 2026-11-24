---
name: Lucidos JavaScript SDK, Setup (continued)
description: Part of Lucidos JavaScript SDK. Its index lists the sections it covers.
---

One part of `system-knowhow/js-sdk`, which lists every part.

## Setup (continued)

### Dynamic bars

**On a phone, the Lucidos header glides away after the reader scrolls down your
app, with no code from you.** It moves once the finger lifts, never during the
drag. It glides back after a scroll up, near the top, or on a pane swipe. This
follows the user's *dynamic bars* setting, which is on by default. With it off,
the header stays put.

`sdk.js` only watches the scroll and the touches, and posts them to Lucidos.
Every listener is passive, so your app scrolls exactly as before. It sees the
page's own scroll and any element that scrolls vertically inside it.

While the header is away, your app's panel moves up into its room and grows
by its height. Your page gets one `resize` as the header leaves, and one as it
returns. An element fixed to the bottom of your page slides with the panel as
the header comes back.

### One sound at a time

**A sound your app starts pauses every other sound in Lucidos, with no code
from you.** When an `<audio>` or `<video>` element starts playing with sound,
`sdk.js` tells Lucidos. Lucidos pauses the media playing in every other app and
widget, and in its own file previews. Your app's media pauses the same way when
a sound starts somewhere else. Two sound player widgets on one question card
therefore never play over each other.

Only audible media takes part. A muted element, such as a background video,
neither pauses others nor gets paused. Nor does a stream with no audio track,
such as a camera preview. An element counts once it plays unmuted, or is
unmuted while playing.

It does not reach:

- Web Audio (`AudioContext`): it has no `play` event to watch;
- a second element in your own app: layer your own sounds as you like.

### Find in app

**Your app's text is searchable with no code from you.** The header's *Find*
button, or Mod+F while the app is open, rolls the find bar in above it.
Lucidos sends each query to `sdk.js`, which matches the rendered text,
highlights every match and scrolls the current one into view. Enter and
Shift+Enter step through the matches and wrap at the ends.

What it matches:

- visible text only, case-insensitive, across inline elements but not from one
  block into the next;
- not text in `script`, `style`, `template`, or a form field's value;
- not an element hidden with `display: none` or `visibility: hidden`.

It never edits your DOM. The highlights are CSS Custom Highlights named
`lucidos-find` and `lucidos-find-current`, styled by `sdk-iframe.css` from the
theme tokens. Without that stylesheet the current match shows as the selection.
An app that loads no `sdk.js` cannot be searched, and the bar says so.

The count is taken when the reader types or steps. Content your app renders
later is counted on the next keystroke.

### Theme variables

`sdk-iframe.css` defines these CSS custom properties on `<html>` and flips them between light and dark. The `data-theme-mode` attribute drives them: `applyPreferences()` sets it (resolving `system` to the OS setting) and `watchPreferences()` keeps it in sync. Style with `var(--name)` and your app tracks the user's appearance. The canonical values live in the engine's `sdk-iframe.css`. **The names are the contract**:

| Group | Variables |
|---|---|
| Backgrounds | `--bg-primary`, `--bg-secondary`, `--bg-tertiary`, `--bg-quaternary`, `--bg-hover`, `--bg-selected` |
| Text | `--text-primary`, `--text-secondary`, `--text-muted`, `--text-on-accent`, `--text-strong` (bold text where the UI font has no bold face). `<html>` carries `data-font-bold="none"` for such a font and `"face"` otherwise, and `sdk-iframe.css` then paints `strong` and `b` inside `.markdown-content` with it. Key your own bold on the same attribute: `html[data-font-bold="none"] .my-label b { color: var(--text-strong); }` |
| Border | `--border-color` |
| Accents | `--accent`, `--accent-light`, `--accent-green`, `--accent-yellow`, `--accent-red` |
| Focus | `--focus-ring`: a ready-made `box-shadow` value (a soft accent band) for focus indicators. The `.action-btn`/`.icon-btn` classes use it, and your controls match the host with `:focus-visible { box-shadow: var(--focus-ring); }`. `--focus-ring-width` (`0.1875rem`) is the band's width. The band paints outside the control, so a box that scrolls or clips (`overflow` other than `visible`) cuts it at its edge. Give such a box that much edge padding, and hand it back with a negative margin if nothing should move. |
| Shadows | `--shadow-sm`, `--shadow-md`, `--shadow-lg` |
| Shape | `--radius-control` (`0.5rem`), `--radius-surface` (`0.75rem`), `--radius-round` (`999px`): the theme's corner steps. The shared component classes round with them, so a square theme squares them. Scale one with `calc()` to follow the theme in your own CSS. |
| Layout (theme-independent) | `--font-ui`, `--font-mono`, `--font-features-text`, `--font-features-code`, `--transition`, `--user-ui-scale`, plus the spacing / radius / motion scales below |
| Stacking | `--z-tooltip` (`10000`), the layer the built-in tooltip paints on. Keep your own overlays under it, so a tooltip is never covered. |

The user's **theme** retunes these values on top of the light and dark defaults, so style with the variables rather than copying their values. An app that lists or builds themes uses the routes in `system-knowhow/themes/themes-in-a-plugin.md` § For apps.

The user's UI font is **`--font-ui`**, the canonical token. It is set live to the
user's font pick, or to the theme's font when they follow the theme (the
default). `sdk-iframe.css` already sets `body { font-family: var(--font-ui) }`,
plus inputs and `.action-btn`, since form controls don't inherit the page font.
So any inheriting element gets the right font. A *bare* unclassed `<button>`
keeps the browser's control font: one more reason to use `.action-btn`.

Re-declare `font-family` only to override it deliberately, with `var(--font-ui)`.
`--font-family` and `--font` are tolerated **aliases** of `--font-ui`, so a
guess still resolves to the user's font. Write `--font-ui`.

`--font-mono` defaults to the native OS monospace stack, matching the host shell. It names macOS, Windows and Linux fonts before the generic fallback, with no user-installed coding fonts or font downloads.

**`--font-features-text` and `--font-features-code` carry programming ligatures,
and only code gets them.** Fira Code (the default), JetBrains Mono and Cascadia
Code ship programming ligatures. With one of them as the UI font, the two
resolve to `"liga" 0, "calt" 0` and `"liga" 1, "calt" 1`. For every other font
both are `normal`. `sdk-iframe.css` applies the text one on `html, input,
textarea, select, button` and the code one on `code, pre, kbd, samp`. So a code
block ligatures `=>` and `!=` while prose and form fields render literally.

Apply one yourself only on an element that shows code but is none of those tags:
`font-feature-settings: var(--font-features-code, normal)`. Never put the CODE
one on `:root`, `html` or `body`. `font-feature-settings` is inherited, and Fira
Code's `calt` re-spaces dot runs so a typed `...` reads as two dots.

Two traps in your own rules. Both look fine in DevTools and neither shows in the
computed value:

- **`normal` does not mean "ligatures off".** `liga` and `calt` are default-ON
  features in CSS, so `normal` renders identically to `"liga" 1, "calt" 1`.
  Use `var(--font-features-text, normal)` when you want them off, never a bare
  `normal`, and never expect deleting a declaration to disable anything.
- **Form controls do not inherit this property.** The UA stylesheet's `font`
  shorthand resets it on `input` / `textarea` / `select` / `button`, which is why
  they are named explicitly above. A custom control of your own needs the same
  treatment.

The spacing, radius, motion, icon and type scales are theme-independent, with
fixed values. **Use the token, not a magic number, and never a `px` fallback
that disagrees with the real value.** `var(--space-xl, 28px)` is a latent bug,
since `--space-xl` is `1.5rem` = 24px. With `sdk-iframe.css` these are always
defined, so a fallback is dead noise at best:

| Token | Value | | Token | Value |
|---|---|---|---|---|
| `--space-xs` | `0.25rem` (4px) | | `--radius-sm` | `0.25rem` (4px) |
| `--space-sm` | `0.5rem` (8px) | | `--radius-md` | `0.375rem` (6px) |
| `--space-md` | `0.75rem` (12px) | | `--radius-lg` | `0.5rem` (8px) |
| `--space-lg` | `1rem` (16px) | | `--icon-size-sm` | `0.875rem` (14px) |
| `--space-xl` | `1.5rem` (24px) | | `--icon-size-md` | `1rem` (16px) |
| `--duration-fast` | `0.15s` | | `--icon-size-lg` | `1.25rem` (20px) |
| `--duration-normal` | `0.2s` | | `--duration-slow` | `0.3s` |
| `--duration-emphasis` | `0.5s` | | `--duration-scale` | `1`, or `0.001` under reduced motion |
| `--spinner-weight` | `0.125rem` (2px), the `.mini-spinner` ring | | | |

Each `--duration-*` above is its listed value times `--duration-scale`, which is
`1` inside an app. The host's debugging slider for its own copy never reaches
you, since a custom property does not cross into an iframe. Under reduced motion
the scale drops to `0.001`, and every transition on these tokens ends inside a
frame. Set `--duration-scale` on your own `:root` for a knob of your own.

**Type scale: `--font-size-*`.** The host shell and this stylesheet both size
text from these, so use the token instead of a raw `rem`. Every step is in
`rem`, so it scales with the user's UI-scale preference.

| Token | Value | Role | | Token | Value | Role |
|---|---|---|---|---|---|---|
| `--font-size-3xs` | `0.5625rem` (9px) | micro-label / tiny badge | | `--font-size-lg` | `0.875rem` (14px) | emphasis |
| `--font-size-2xs` | `0.625rem` (10px) | dots, micro-meta | | `--font-size-xl` | `1rem` (16px) | section heading |
| `--font-size-xs` | `0.6875rem` (11px) | dense metadata | | `--font-size-2xl` | `1.125rem` (18px) | larger heading |
| `--font-size-sm` | `0.75rem` (12px) | body text, the chat prose step | | `--font-size-3xl` | `1.25rem` (20px) | large heading |
| `--font-size-md` | `0.8125rem` (13px) | labels, controls, row titles | | `--font-size-display` | `2.25rem` (36px) | hero |

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

#### Respect the user's font size: size in `rem`, never `px`

The user's UI-scale preference is the **root font-size**
(`html { font-size: var(--user-ui-scale, 100%) }`), so **only `rem`/`em` units
scale with it.** An app sized in `px` ignores the user's font-size setting.
That is the most common "the app doesn't respect my font size" bug. Size
everything in `rem` (px / 16: 14px → `0.875rem`, 24px → `1.5rem`), and prefer
the `--space-*` / `--radius-*` tokens for spacing and corners. `1px` borders are
the one acceptable `px` exception, as in the host shell.

For text, prefer the `--font-size-*` tokens over a raw `rem`. Body text is
`--font-size-sm` (12px), the chat step. Labels, controls and row titles are
`--font-size-md` (13px). Small/meta is `--font-size-xs` (11px), emphasis
`--font-size-lg` (14px), and headings use the `h1`–`h6` defaults
`sdk-iframe.css` ships.

**The body step is already the default.** `sdk-iframe.css` sets
`body { font-size: var(--font-size-sm) }`, so an unsized paragraph reads at the
size of the chat beside it, not at the raw root (`1rem`). Don't raise it for a
report: a larger body reads as too big beside the chat. Never reset it to `1rem`
or a `px` value, which reads two steps larger than the rest of Lucidos.

