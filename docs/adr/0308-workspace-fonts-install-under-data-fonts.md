# 0308: Users, agents and plugins install workspace fonts under data/fonts, served through the gated /data mount, never a third party

- **Status**: Accepted
- **Date**: 2026-09-27
- **Builds on**: [ADR 0077](0077-default-ui-font-is-vendored.md), [ADR 0238](0238-app-frame-carries-a-capability-to-its-own-files.md), [ADR 0289](0289-app-frames-load-fonts-and-modules-across-origins.md), [ADR 0296](0296-looks-are-tokens-the-engine-resolves.md), [ADR 0298](0298-look-fonts-make-no-third-party-request.md), [ADR 0303](0303-bundle-every-catalog-font.md)

## Context

The user asked that users can install their own fonts. Until now every font
came from the font catalog, compiled into the engine and the host build. ADR
0303 made every catalog font local, so none makes a third-party request.

A user's font is different from a catalog font in two ways. It is workspace
data, possibly a licensed commercial font. And it arrives at run time, from a
person, an agent or a plugin, with a name and a file nobody reviewed.

Plan: `docs/plans/2026-09-27-workspace-fonts.md`.

## Decision

A *workspace font* is a directory `data/fonts/<slug>/` holding a `font.json`
manifest and the font files it names. Its id is `ws-<slug>`. It is served
through the gated `/data` mount, and it fits the same slots as a catalog font
of the same kind:

| `group` | Kind | UI font | Code font |
|---|---|---|---|
| `sans`, `serif` | `ui` | yes | no |
| `mono` | `both` | yes | yes |

## Rationale

**No third-party request, by construction.** The manifest has no URL field. A
face names a leaf file name in its own directory. Every face URL is built by
Lucidos from the `/data` path, so the only origin a workspace font can reach is
the local engine. ADR 0077's premise holds without a check to forget.

**The gated mount, not the public font tree.** `/api/v1/fonts/*` needs no
credential, because an app frame cannot send one. That is safe only because
the tree holds the catalog, which ships on the public mirror. A workspace font
is the user's file. The `/data` mount is gated, and an app frame already
reaches it with its frame capability (ADR 0238). The engine already grants a
font load across the frame's opaque origin (ADR 0289).

**No user string reaches CSS.** The CSS family is `'ws-<slug>'`, and the stack
is that family plus a fixed fallback chain per group. The label reaches UI
text only. Faces load through the CSS Font Loading API, which takes structured
values, so nothing is escaped and nothing can be injected. A look still names
a font by id only (ADR 0296).

**Listing validates, whatever wrote the files.** The data route and plugin
staging check a font for a readable error. But the agent's shell and a git
pull write files too. So the engine validates again each time it lists
`data/fonts/`. A font that fails is left out of every surface and reported
with its reason. That check is the authority.

**Same slots as the catalog.** A mono workspace font is both a UI font and a
code font, because Lucidos is mono-first and every catalog mono font is. The
group is declared, not measured. A wrong claim lays out a code block
badly, which harms nobody.

**The `ws-` prefix is reserved.** A later catalog font can never shadow a
workspace font on upgrade, and every surface tells the two apart from the id.

## Consequences

- The `font-family` preference accepts a `ws-` id. A look may name one in
  `fonts.ui` or `fonts.mono`, if it exists and fits the slot when the look is
  written. A font removed later falls back as an unknown id always has.
- A client rebuilds a workspace font's stack from its id and group. It does not
  trust the one on the wire, because an entry also reaches it from local
  storage. The three fallback chains are generated into `font-catalog.ts`.
- Formats: woff2, woff, TrueType and OpenType, told apart by magic bytes that
  must match the extension. Collections are refused. The caps are 10 MiB per
  file, 16 faces per font and 100 fonts per workspace.
- Installs and removals are data writes, so they emit `DataFileWritten` and
  `DataFileDeleted`. There is no new event.
- `fonts` joins the plugin content directories. A plugin look may name only a
  workspace font the same plugin ships. So a plugin never depends on a font the
  user happened to install. Staging also runs without a workspace, for a
  marketplace listing.
- `sdk-prefs.js` is credential-free and carries the workspace fonts in use for
  first paint. A caller who guesses a device uuid learns their slugs and data
  paths. The preference value already says the slug, and the bytes stay gated.
- Browsers sanitise every web font before use. A plugin font adds no trust a
  plugin did not already have, since a plugin already ships app code.
- The agent may fetch a font file from the internet when a user asks it to.
  That is one download the user requested, not a request every render makes.

## Alternatives considered

- **Serve workspace fonts under the public `/fonts/` tree.** App frames load it
  with no credential, so the user's files would sit behind a guessable URL with
  no gate.
- **Content-hashed capability URLs under `/fonts/`.** It works, but it invents a
  second capability scheme next to ADR 0238's, which already covers `/data`.
- **A font arriving through a look or a style override.** ADR 0296 bans `url()`
  in tokens for exactly this reason.
- **Generate `@font-face` CSS from the manifest.** A user string would reach
  CSS, and every field would need escaping that must never be wrong.
- **A dedicated multipart install endpoint.** It is atomic, but it is a new
  surface. The data route with the manifest written last gives the same result,
  and it reuses the route's gate, its events and the agent's own tools.
- **Measure monospace from the font file.** It needs woff2 decompression in the
  engine to catch a claim whose only cost is layout.
- **An unprefixed id with a collision check.** A later catalog font with the same
  id would silently shadow the user's font on upgrade.
