# 0077: The default UI font is vendored and served locally; only an opt-in font may take the Google CDN

- **Status**: Accepted
- **Date**: 2026-08-13

## Context

Four selectable web fonts reach the UI through `GOOGLE_FONT_URLS`
(`packages/lucidos-sdk/src/appearance.ts`): Inter, JetBrains Mono, IBM Plex Mono
and, originally, Fira Code. Each was fetched from `fonts.googleapis.com` on
first selection, which is what an opt-in font can afford.

Fira Code then became the `font-family` default. That changed what the CDN
dependency costs, because a default is on the first paint of every install.

## Decision

Fira Code is vendored and served by the local engine, so it is deliberately
ABSENT from `GOOGLE_FONT_URLS`. The other three stay on the CDN.

The rule the asymmetry expresses: the DEFAULT font must render with no request
to a third-party origin and with no internet at all. An opt-in font may take the
CDN and the wait, because the user chose it.

## Rationale

Lucidos is a self-contained local install. A default that depends on a
third-party origin breaks that premise twice over. An offline workspace renders
its whole UI in the browser's generic `monospace`, which is Courier and visibly
worse than the option the default replaced. And every boot of every install
announces itself to Google, for a font nobody asked for.

The engine is local, so a locally-served font works offline by construction.

## Consequences

- One vendored file, `FiraCode-VF.woff2` (variable weight 300-700), plus its SIL
  Open Font License 1.1 text. OFL permits redistribution.
- It lives in the app crate and is consumed twice. Vite's asset graph takes it
  for the host, hashing it into `assets/`, which the service worker's shell
  cache covers offline. The engine takes it by `include_bytes!` for app iframes,
  reaching across crates as it already does for `shared-components.css`.
- The `fira-code` stack is the FULL system-mono chain rather than a bare
  `monospace` tail. A default has to paint acceptably before the web font
  decodes, and on any device where it never arrives.
- Making a different font the default means vendoring that one too.

## Alternatives considered

- **Keep Fira Code on the CDN for consistency with its three siblings.** This is
  the option a reader arrives at, because the map looks incomplete without it.
  It loses because consistency across the four is not the property that matters:
  being the default is. Adding the URL back would silently restore the
  first-paint dependency.
- **Vendor all four.** Rejected as unpaid weight. Only the default has to work
  offline, and three more font files ship to every install to serve a minority
  of devices that opted in.
- **Fall back to bare `monospace` when the CDN is unreachable.** That is what the
  original arrangement already did, and Courier for the entire UI is the failure
  this decision exists to remove.

Prior decision this reverses (for the default only):
`docs/plans/2026-06-26-add-fira-code-font.md`, which chose "Google Fonts CDN
only, no self-hosted font file" while Fira Code was opt-in.
