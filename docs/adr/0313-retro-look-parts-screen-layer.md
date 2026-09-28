# 0313: Retro look parts: a block caret, scanlines on the screen fill, capped double borders

- **Status**: Accepted (amends [ADR 0307](0307-look-parts-paint-only.md))
- **Date**: 2026-09-27

## Context

A look can glow text and icons through look parts (ADR 0307). A terminal look
also wants three effects no part reaches: a block caret in the composer,
scanlines across the window, and DOS-style double-line boxes. The threat table
in `docs/plans/2026-09-27-look-parts.md` still holds. A look writes no selector,
no `content`, no `position`, no `pointer-events` and no `url(`. No effect may
paint over a protected surface (ADR 0309). Plan:
`docs/plans/2026-09-27-retro-look-parts.md`.

## Decision

Four part properties join the catalog:

| Property | Part | Grammar and caps |
|---|---|---|
| `caret-shape` | `composer-text` | `auto`, `bar`, `block` or `underscore` |
| `background-image` | `screen` (the base fill) | `none`, or a vertical `repeating-linear-gradient` of 2 to 4 literal stops, each at most 0.25 alpha, repeating every 8px or less |
| `border-style` | `surface`, `card` | `solid` or `double` |
| `border-width` | `surface`, `card` | 1px to 4px |

Scanlines paint as a background image on Lucidos's own screen fills,
`.app-shell` and `.mobile-swipe-pane`. They sit above the fill colour and under
everything the fill contains. `look-effects: reduce` turns them off.

## Rationale

**Scanlines under content, because the protected cards are trapped.** On
desktop the transcript's fade mask makes it a stacking context. On mobile every
swipe pane is one. So a question or permission card cannot rise above a
window-wide layer that covers the transcript around it, whatever its z-index.

An ancestor's background, by contrast, paints under all its descendants in
every stacking context. So no protected surface can ever sit beneath the lines.
The layer also adds no element, no z-index and nothing that takes a pointer
event.

**Literal stops with a hard alpha cap.** Each stop's alpha is known at
validation, so the cap holds for any input. 0.25 is well past what reads as
faint, and it keeps the lines from painting a solid colour. The engine also
refuses a look whose stops drop page text, `--text-primary` or a chat part
colour, under the 3:1 page floor.

**The protected clamp counts the bands.** No inline protected root paints a
fill of its own, so its text sits on the screen, bands included. An alpha cap
cannot hold the 4.5:1 floor there: at 0.25, text at the floor falls to about
3.6:1. So the clamp of ADR 0309 treats each band as a fill that protected text
must clear. A look that sets scanlines always gets a clamped palette. A style
override may not set them, since the clamp never sees an override.

**An 8px period and no direction.** The default direction draws horizontal
lines only. No band is tall enough to read as a divider or a row.

**The caret ships where browsers draw it.** `caret-shape` exists in Chromium
144 and later, and in no WebKit or Firefox release. Safari, the iPhone and the
macOS app keep a bar in the look's `caret-color`. The composer stops the blink
under `data-motion="reduce"`.

**Borders only where one is drawn today, at 4px at most.** `double` needs 3px
to draw two lines, and 4px grows each side by 3px at most. `border-width` is
the second layout-affecting part property, after `letter-spacing`. It cannot
move a box out of flow or cover a neighbour.

## Consequences

- ADR 0307's non-goal "background images, including gradients, on parts" now
  has one exception, the `screen` part.
- Scanlines do not cross glyphs, or surfaces with their own fill.
- `--part-screen-background-image` joins the reserved override names. Look
  Studio can preview scanlines only by saving the look.
- The block caret is Chromium-only until WebKit ships `caret-shape`.
- The `surface` part excludes protected surfaces in its selector, and the
  protected reset covers every new token as well.
- The consumer guard allows a part selector to name a floating surface only in
  that excluded form.

## Alternatives considered

- **A fixed overlay with engine-cut holes.** It sits just under `--z-float`,
  and the engine clips it around every protected surface on screen. It draws
  over text. But WebKit scrolls on the compositor. During a fast fling a hole
  trails its card by a frame or more, and the lines cross the card. The
  maintainer chose the screen layer over it.
- **An overlay per stacking context.** One layer inside the transcript, one
  inside each mobile pane, one at the root. It needs z-index changes on
  surfaces that overlap each other, which reorders unrelated UI. One new
  stacking context around a card silently breaks it.
- **Hide the overlay while a protected surface exists.** Answered question
  cards stay in the transcript for good, so the lines would vanish from most
  coding threads.
- **A drawn caret for WebKit.** A fake caret over a textarea must track IME
  composition, selection, right-to-left text, wrapping and scroll. A caret in
  the wrong place is worse than a thin one.
- **Borders on any part.** Adding a border where none is drawn moves layout on
  the default look's neighbours, and changes the default look's pixels.
