# 0309: Protected surfaces read an engine-clamped palette; a look may set catalog tokens only

- **Status**: Accepted
- **Date**: 2026-09-27

## Context

Looks are token maps the engine validates and resolves (ADR 0296). The engine
never checked what the tokens paint, and any plugin can ship a look. So a
token-only look could set the card text to the card fill and blank a
permission card. It could swap `--accent-green` and `--accent-red`, so Deny
read as Allow. It could also make the scrim transparent, so a blocking dialog
stopped looking blocking.

Three further holes turned up. A look could set any custom property, not just
catalog tokens, so it reached `--z-modal`, the type scale and spacing. A shadow
token could paint over a neighbour, or hide a dialog's text with an inset
shadow. And `style_overrides`, which any app can write, had the same reach.

Plan: `docs/plans/2026-09-27-protected-approval-surfaces.md`.

## Decision

A *protected surface* is any surface where the user grants, denies, answers or
confirms. It reads a `--protected-*` palette and nothing else for text, fills
and confirm or deny colours. The engine derives that palette from the look at
resolve time and clamps it to WCAG AA. A look far enough off to read as
hostile is refused on write, with a reason.

## Rationale

**The clamp is what protects, so it is not optional.** Refusal alone misses a
look already on disk, and it never sees `style_overrides`. The clamp holds for
any input, including values validation would refuse.

**The engine clamps, not the client.** The shell, its boot script and app
frames must agree, as ADR 0296 requires for derivation. So the resolver emits
the palette as literal colours inside each resolved map. A literal matters: a
`var()` of an ordinary token would let an override flow through.

**One remap, not a rewrite of every rule.** `.protected-surface` redeclares
every catalog paint token to its protected twin. A look writes inline on
`<html>`, and a declaration on the surface beats inheritance, so the existing
rules follow unchanged. A guard test fails when the catalog gains a paint
token the remap does not map.

**Refusal is what informs.** An author learns at write time that their text is
1.0:1 on the page, instead of watching the clamp silently repaint their
dialogs. It lives in the one validator the data route, plugin staging and load
share. The bar is deliberately low: 3:1 for page text, a hue swap for green
and red. Anything between that and AA is accepted and repaired.

**Confirm and deny are pinned by hue, not ordered.** Green is clamped into an
OKLCH hue band around green, red into one around red, each with a chroma
floor. Checking only that two colours differ would let a look paint Allow red
and Deny green.

**A look may set catalog tokens only.** ADR 0296 already promised a look
"cannot change layout, spacing or type scale". The validator now makes that
true, and the same rule closes `--z-*` and `--protected-*`.

## Consequences

- Protected surfaces follow a look's hues and fills closely, but not exactly.
  A faint or clashing look shows its repaired palette there. The default dark
  Allow button got darker, since white on the old green was about 3:1.
- A look naming a non-catalog token is refused. No built-in did. An older
  workspace or plugin look that now fails stops painting. The shell says why
  in a warning toast, once per page, so it never falls back in silence.
- `style_overrides` can no longer set `--protected-*`, `--z-*`, the UI font
  tokens or `--user-ui-scale`. Inside a protected surface, the type and
  spacing scales are pinned, and the root size follows only the user's own
  UI scale preference.
- A shadow token reaches at most 2rem past its box, counting its offset,
  spread and half its blur together. It may call colour functions only. A
  shadow in a style override is held to the same reach, since it can paint
  over a protected surface from a neighbour outside it.
- A new catalog paint token needs a remap entry in the same change. A new
  catalog *kind* fails the guard until someone decides whether it can hide
  content. This applies to the icon and text-glow tokens, and to any future
  paint-only styling layer.
- Toasts still paint above modals. That is shell content, not look paint.

## Alternatives considered

- **Enforce AA on the whole look and repair it.** Every surface would get the
  repaired colours, and the chat's look would drift from what the author
  wrote. It still would not cover `style_overrides`.
- **Refuse any look below AA.** Too strict for a theme: many well-loved
  palettes put muted text below AA. It also protects nothing already on disk.
- **Clamp in TypeScript at the apply site.** The boot script, the shell and
  the SDK would each need the colour maths, and they would drift. One Rust
  resolver serves all of them, as with derivation.
- **Hardcode approval colours.** Safe, but every look would lose its identity
  exactly where the user looks hardest. The clamp keeps the look's hue and
  moves only lightness.
