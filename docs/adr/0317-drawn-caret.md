# 0317: A drawn caret where the browser has no caret-shape

- **Status**: Accepted (amends [ADR 0313](0313-retro-look-parts-screen-layer.md))
- **Date**: 2026-09-28

## Context

The `composer-text` part takes `caret-shape` (ADR 0313). Only Chromium 144 and
later draws it. The macOS app, Safari, iOS browsers and Firefox show the thin
bar. So a terminal theme loses its block caret on most clients Lucidos ships.

ADR 0313 rejected a drawn caret: it must track IME, selection, right-to-left
text, wrapping and scroll, and a caret in the wrong place is worse than a thin
one. The maintainer asked for it anyway. Plan:
`docs/plans/2026-09-28-drawn-caret-fallback.md`.

## Decision

Where `CSS.supports('caret-shape', 'block')` is false and the composer's
resolved part asks for `block` or `underscore`, Lucidos draws the caret. An
invisible layout copy of the textarea holds a real inline caret element. The
native caret turns transparent only while the drawn one shows. Chromium keeps
the native property and gets no extra DOM.

## Rationale

**The browser places the caret, not our arithmetic.** The copy has the
textarea's text, font, spacing, padding, width and wrap rules. So the line
breaks and glyph advances come from the same layout engine. The caret element
sits between the text before and after it, and the copy follows the textarea's
scroll. This answers wrapping, scroll, resize and UI scale.

**Each concern in ADR 0313 has an answer:**

| Concern | Answer |
|---|---|
| IME composition | The drawn caret hides and the native caret comes back until composition ends. |
| Selection | A range selection hides the drawn caret, as it hides a native one. |
| Wrapping | The copy wraps the same text at the same width. |
| Scroll | The copy takes the textarea's scroll offsets on every scroll event. |
| Right-to-left text | The copy takes the textarea's `direction` and bidi layout. |

**What stays inexact.** A caret at a soft-wrap point, or between runs of
opposite direction, has two valid visual spots. The drawn caret may take the
other one. It stays on the right line of text and moves correctly with the next
key. That is a smaller cost than losing the shape on every WebKit client.

**Colour comes from the textarea, motion from CSS.** The caret paints the
part's `caret-color`, else the composer text colour. The controller resolves
both from the textarea, since the theme-part guard lets only rules under the
textarea read `composer-text` tokens. A theme switch changes `<html>`, which
redraws the caret. The blink is CSS and stops under `data-motion="reduce"`.

## Consequences

- ADR 0313's "The block caret is Chromium-only" no longer holds.
- The fallback is a temporary measure. It ends when every engine Lucidos ships
  on draws `caret-shape` natively (`docs/temporary-measures.md`).
- App frames get no fallback. The catalog never sends `composer-text` to a
  frame.
- A protected surface never gets a drawn caret: the part token resets there,
  and the decision also checks for a protected ancestor.

## Alternatives considered

- **Keep the thin caret (ADR 0313).** Safe, but the theme's main retro cue is
  gone on the macOS app and the iPhone, where most use happens.
- **Compute the caret position from font metrics.** Canvas `measureText` per
  line needs our own wrap algorithm, which drifts from the browser's on
  kerning, ligatures, tabs and break rules.
- **Replace the textarea with a contenteditable element.** A real DOM
  selection would give an exact caret rectangle. It changes paste, autocorrect,
  IME and the iOS keyboard behaviour the composer depends on (ADR 0262), for a
  cosmetic gain.
