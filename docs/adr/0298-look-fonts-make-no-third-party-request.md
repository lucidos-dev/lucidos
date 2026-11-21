# 0298: A look may suggest only a font that makes no third-party request: vendored or device, never CDN

- **Status**: Accepted
- **Date**: 2026-09-27
- **Builds on**: [ADR 0077](0077-default-ui-font-is-vendored.md)
- **Extended by**: [ADR 0303](0303-bundle-every-catalog-font.md), which vendors every catalog font and removes the `cdn` source. The rule here now holds by construction, and the alternative "Vendor every catalog font" below was taken.

## Context

A look can now suggest a UI font and a code font. The `font-family` preference
gained the value `look`, which is the new default: a device that never picked a
font paints whatever its look suggests. The rule is "look suggests, user wins".

ADR 0077 drew one line: the default font must render with no third-party
request, and an opt-in font may take the Google Fonts CDN. A look sits between
the two. Nobody picks the font it suggests, yet it paints on every device that
follows the look. A look is also a file any app or plugin can write.

Plan: `docs/plans/2026-09-27-font-catalog-and-look-fonts.md`.

## Decision

A look may name only two kinds of font:

- `vendored`: served by the local engine, with its license text in the tree;
- `device`: the device's own fonts.

A `cdn` font stays available as an explicit user pick only.

The engine refuses any other id when a look is written, through the data route
and plugin staging. Every client re-checks a suggested id against the font
catalog before it uses it.

## Rationale

ADR 0077's premise extends to looks. Applying a look is not consent to tell
Google about the device. A CDN font would announce every boot of every device on
that look, and render in the fallback stack offline.

The refusal sits at write time, where the author reads the reason. The client
re-check costs one lookup and covers what write-time validation cannot: a look
cache in local storage is data, and data can be forged.

## Consequences

- Five new fonts are vendored so looks have real choices: Geist, Geist Mono,
  Atkinson Hyperlegible Next, Atkinson Hyperlegible Mono and Source Serif 4.
  All are SIL OFL 1.1.
- Each ships only the latin and latin-ext subsets, as variable-weight woff2,
  with no italic file. Other scripts fall back to the stack, and the browser
  synthesises oblique.
- The cost is about 258 KB of woff2, carried twice on every install on every
  platform: in the engine binary (`include_bytes!`, for app frames) and in the
  frontend build (Vite, for the host). About 0.5 MB per install.
- Nothing downloads a face until text uses it. The host declares every face,
  and the service worker caches assets on fetch rather than precaching them.
- A look also cannot set `--font-ui` or its aliases as tokens. A token would be
  laid over the user's explicit pick, so the UI font reaches a look only
  through `fonts.ui`.
- Making a CDN font nameable by looks means vendoring it first.

## Alternatives considered

- **Let a look name any catalog font.** The simplest rule. It loses because the
  look would request a font from a third party that the user never chose. ADR
  0077 forbids exactly that for the default.
- **Let a look name a CDN font, but load it only after a prompt.** A consent
  dialog for a font is friction nobody asked for. A look that does not paint as
  designed until the user answers is a worse look.
- **Vendor every catalog font, the CDN ones included.** Rejected for the reason
  ADR 0077 gave: bytes shipped to every install for a minority who opted in.
  Inter, JetBrains Mono and IBM Plex Mono stay on the CDN.
- **Vendor the full character sets.** Source Serif 4 alone would grow from 93
  KB to several hundred. The two subsets cover the Latin-script languages.
- **Commit Mono and Monaspace.** Both are OFL, but neither is on Google Fonts,
  so neither can be a CDN pick. Vendoring one would add a third new monospaced
  font to every install.
