# 0307: Look parts are capped paint-only effects on named parts, compiled to tokens

- **Status**: Accepted (amends [ADR 0296](0296-looks-are-tokens-the-engine-resolves.md):
  a look still ships token values only, and the engine alone resolves them),
  amended by ADR 0313
- **Date**: 2026-09-27

> **Amendment (ADR 0313).** Three retro effects join the catalog. The composer
> takes a `caret-shape`. The `screen` part takes capped scanlines as a
> background image, the one exception to the non-goal on gradients. Floating
> surfaces and step cards take `border-style` and a 1px to 4px `border-width`.
> Scanlines paint under all content, because inline protected cards sit in
> stacking contexts no overlay can stack beneath.

## Context

A look can recolour Lucidos, but it cannot add an effect that no token feeds. A
CRT glow on chat text, a glow on an icon and wider letter-spacing on a header
title are all out of reach. The ask is a styling layer for such effects that
still cannot hide, cover or fake anything.

ADR 0296 rejected Obsidian-style CSS. Raw CSS can cover or restyle a permission
card, and `url()` leaks the page view to any origin. Any new layer has to keep
both reasons intact. Plan: `docs/plans/2026-09-27-look-parts.md`.

## Decision

A look gains a `parts` field. It maps a catalog part (`chat-text`,
`header-title`, ...) to a few paint-only properties, each with a whitelist
grammar and caps. The engine validates it and compiles each property to a
*part token*, such as `--part-chat-text-text-shadow`, in the resolved map. A
stylesheet Lucidos ships reads the part token under a selector Lucidos owns.

The maintainer settled four further points:

- **Per-mode parts.** A look may carry `parts` (shared) plus `dark.parts` and
  `light.parts`. A mode map wins over the shared map per (part, property), the
  way mode tokens win over `tokens`. `none` switches a shared effect off in one
  mode.
- **A device switch.** The device-scoped `look-effects` preference (`system`,
  `reduce`, `full`, default `system`) drops part shadows and filters on
  `reduce`. It keeps part colours and letter-spacing. `system` follows
  `prefers-reduced-transparency` and `prefers-contrast: more`, not battery
  hints.
- **App frames.** An app that loads the Lucidos SDK gets every part, with one
  opt-out for the whole app (`data-look-parts="off"` on its `<html>`). There is
  no opt-in per part. An app that does not load the SDK gets nothing.
- **No built-in look uses parts yet.** Parts ship without a showcase look. A
  workspace look is the manual test case, and the browser tests use a test
  fixture look.

- **`--text-glow` folds into parts.** The token stays so a look that sets it
  keeps working. The engine compiles it into the `text-shadow` part tokens of
  `chat-text`, `actor-label`, `header-title` and `composer-text`, and drops it
  from the resolved map. So it follows every part rule, and no glow paints
  twice. An explicit part value wins over it. The Brand marks colour tokens
  stay plain tokens, registered as `<color>` like every colour token.

Protected surfaces have no parts, in the shell or in a frame. They reset every
part token and the inherited paint properties, and paint in their own stacking
layer.

## Rationale

**Compile to tokens, not to CSS text.** A CSS string would need a new sink in
the shell, the boot script and every app frame. The boot script would then
build stylesheet text from stored data. A part token instead reaches the page
through `setProperty` on one custom property, which cannot break out of its
declaration. First paint, live refresh, `?style-reset`, the frame seed and
plugin staging already carry tokens, so parts reuse all of them.

**A look never writes a selector.** Selector power is what makes CSS able to
reach a permission card, a pseudo-element or an attribute value. Without
selectors, overlay, exfiltration and restyling a protected card all need a
capability the look does not have.

**A whitelist grammar per property.** The banned-substring rule only knows what
it bans. A grammar that names every allowed form refuses a future CSS feature
by default. The engine re-serialises each parsed value rather than echoing the
author's string.

**Typed colour tokens.** A part may name a catalog colour token, but only in a
colour slot. `var()` substitutes text, so an untyped `--accent` holding
`red, 9em 9em 5em red` would add a second, uncapped shadow layer. Every
catalog colour token is therefore registered with `@property` as `<color>`.
The browser substitutes a computed colour, and a non-colour value falls back to
the inherited or initial colour.

**Caps by geometry, not only by syntax.** Shadow offsets stay under 0.1em for
text and a few pixels for boxes. So no shadow can draw a legible copy of text
elsewhere, or cover a neighbour. `filter` is allowed only on leaf icons, since
it re-anchors fixed-position descendants. Outer box shadows are allowed only on
parts in normal flow.

**Protected surfaces reset what they inherit.** `color`, `caret-color`,
`text-shadow` and `letter-spacing` inherit, so a card nested in a styled
container would pick them up. Resetting at the protected root closes that path
whatever a part's selector matches.

**Per-mode parts merge like tokens.** Authors already know that a mode map wins
over `tokens`. The same rule for parts needs no new concept. A mode map holding
only `parts` does not make a look single-mode, so a dark-only glow never forces
a mode switch.

**CSS drops the effects, not the apply path.** Under
`[data-look-effects="reduce"]`, one rule per realm resets the shadow and filter
part tokens. The resolved map stays the same on every device, so the cache and
the frame seed need no variant. A toggle repaints without re-applying tokens.
The boot scripts set the attribute before first paint, as they set
`data-motion`.

**No battery hints.** The Battery Status API exists only in Chromium, and no
browser exposes iOS Low Power Mode. A look that changed with the battery would
also repaint mid-session with no user action.

**Frames follow the app's own opt-in.** The engine serves app HTML as written,
apart from a base, prefix rewrites, the device on the app's own `sdk-prefs.js`
tag, and favicon links. Styling has always been opt-in through the app's tags,
and parts keep that. One opt-out per app keeps the contract simple for app
authors. Per-part choice belongs to the look, not to each app.

**No showcase look yet.** A built-in look using parts would ship effects to
every user before the device switch and the iPhone performance check have been
tested. A built-in showcase may follow once both have.

## Consequences

- The look token catalog gains a sibling, `look-parts.json`, served at
  `GET /api/v1/looks/parts`.
- `POST /api/v1/looks/resolve` resolves a draft look without saving it, so Look
  Studio reuses the engine's merge and grammar rather than copying them.
- A mode map becomes a struct: its tokens plus an optional `parts`. Existing
  look files parse unchanged.
- `--part-` becomes a reserved token prefix. A look's token maps refuse it.
- The engine starts validating `style_overrides` at the preference write, for
  part keys. Any app can write that preference, so the apply-site check alone
  would be the only gate.
- A TypeScript twin of the grammar runs at every apply site, pinned to the Rust
  side by one fixture.
- Every catalog colour token gets an `@property` rule, in the shell CSS and in
  `sdk_iframe.css`. Parts then depend on `@property` in every supported
  browser. Without it, a part could take literal colours only.
- A new device preference, `look-effects`, joins the catalog, the frame seed,
  the boot scripts and Settings.
- `sdk_iframe.css` gains the frame parts, the effects rule, the opt-out reset
  and the protected reset.
- Each part property needs one consuming declaration whose fallback equals
  today's value, so the default look still changes no pixel.
- A part text colour must reach 3:1 against its background, the floor page
  text has (ADR 0309). The strict 4.5:1 guarantee covers protected surfaces.
- App frames register only the colour tokens they define, so a frame part may
  name only those, and an app's own `var(--token, fallback)` still falls back.
- `letter-spacing` changes text width. It is the one layout-affecting property
  on the list, kept under tight caps.

## What shipped against the plan

- `text-selection` was dropped. A `::selection` rule replaces the browser's
  own highlight, and no fallback reproduces it, so the default look would
  change.
- `nav-icon` became `header-icon`: Lucidos has no nav rail, and the header's
  icon buttons are the glyphs the part meant.
- `actor-label` was added, so the `--text-glow` alias covers every leaf it
  painted before.
- Colours already painted by a token stay with the token: `color` left
  `header-title` and `header-icon`. `border-color` left `inline-code` and
  `code-block`, whose base CSS draws no border. `text-decoration-color` left
  `chat-link`, where it would override the dashed artifact-link underline.
- Modal protected surfaces keep the `--z-modal` band of the overlay stack
  rather than moving to `dialog.showModal()`. They already paint above every
  part, and parts add no z-index.

## Alternatives considered

- **Compile parts to a CSS string the engine serves.** Simple to explain, and
  closest to the original idea. Lost on the new sink it needs in three realms,
  and on the boot script building stylesheet text from stored data.
- **Obsidian-style `theme.css`, or CSS with a sanitiser.** Rejected by ADR 0296,
  and a sanitiser does not change that. It has to understand selectors and
  every property, and a miss exposes the whole shell.
- **Only more tokens.** A token per element and effect would work for a few
  cases. It bloats the token catalog and gives no place for per-property caps.
  A part property is a token with one consumer and a grammar, so parts keep
  this option's mechanism and add the structure.
- **Structured effect slots** (a glow with separate colour and radius tokens,
  clamped by CSS `min()`). CSS would enforce the caps even with a buggy
  validator. Lost on authoring cost and token count; it stays open if the
  grammar proves hard to keep in sync.
- **Untyped `var()` in colour slots.** The first draft assumed a non-colour
  token would make the declaration invalid. Review showed a comma in the token
  adds a valid, uncapped layer instead. Typing the tokens closes it.
- **Shared parts only, with mode colour through tokens.** Simpler, but it
  cannot give an effect a different shape per mode, or put one in a single
  mode.
- **Drop the effects in the apply path.** The engine or each apply site would
  strip shadow tokens on `reduce`. That makes the resolved map depend on the
  device, and every toggle re-apply the map. The CSS rule does neither.
- **Push parts into every frame.** The engine could inject the frame
  stylesheet. Lost because styling has always been the app's own opt-in, and a
  plain app should come back exactly as written.
- **Opt-in per part for apps.** More control for app authors, and a second
  place to decide which parts apply. The look already decides that.
- **A built-in showcase look now.** A clear demo, but it would ship effects to
  every user ahead of the device switch and the performance check.
