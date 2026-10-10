# 0296: Looks carry design tokens only, never CSS, and the engine alone resolves them

- **Status**: Accepted, amended by ADRs 0307, 0309 and 0316
- **Date**: 2026-09-26

> **Amendment (ADR 0316).** A look is now a *theme* in every layer, and the
> light/dark `theme` preference is `theme-mode`. The rejected alternative below,
> renaming that key, is the one adopted. The decision stands; only names moved.

> **Amendment (ADR 0309).** A look may now set catalog tokens only, which makes
> the "cannot change layout, spacing or type scale" consequence below true. The
> resolver also emits a clamped `--protected-*` palette for protected surfaces.
> It refuses a look whose text hides on its page, whose green and red are
> swapped, or whose shadows reach past 2rem.

## Context

The user asked for a look and feel setup after Obsidian's themes, tunable
enough that a plugin can let users make their own. Every tunable element had to
be listable by a plugin, headers and focus cues included.

Two things were already in place. The design tokens live in the theme blocks of
`base.css`. The `style_overrides` preference already applies a validated token
map inline on `<html>`, at first paint and live. A plugin could not reach any
of it. Plan: `docs/plans/2026-09-26-looks.md`.

## Decision

A *look* is a JSON file of token values, per theme mode. It may contain custom
properties and values that pass the style-override rules, and nothing else. The
engine owns the token catalog, the built-in looks, validation and derivation. It
serves every look already resolved, one map per mode, and every surface only
applies the map it is handed.

## Rationale

**Tokens only.** A plugin is third-party content, and a workspace look is a file
any app can write. Raw CSS can restyle or cover a permission card, and `url()`
leaks the page view to any origin. A token map can recolour a control, never
move or hide one. The value rules already existed for the style remote, so a
look adds no new attack surface.

**The engine resolves.** Derivation (three seeds fill in the rest) must give
the same answer in the shell, its boot script and every app frame. The boot
script cannot fetch, and an isolated app frame shares no storage with the shell.
The engine is the one party that can hand each of them a finished map: the shell
caches it, and the app-frame seed carries it.

**One catalog, pinned to the stylesheet.** "Every tunable element is listable"
is only true while the catalog matches `base.css`. A Vitest test fails when a
theme block declares a token the catalog lacks, or when a default drifts.

## Consequences

- A look cannot change layout, spacing or type scale. Those tokens stay out of
  the catalog on purpose.
- Header tokens moved from `.pane-header` to the theme blocks, so a value on
  `<html>` reaches them. A header token redeclared on a descendant would beat
  the look, and a guard test refuses it.
- A new theme token needs a catalog entry in the same change.
- Surfaces with hardcoded colours (parts of the picker, previews and the drawer)
  keep them under a look until they move to tokens.
- The `theme` preference keeps meaning the theme mode. The new concept took the
  word *look* rather than rename a stored key.

## Alternatives considered

- **Obsidian-style `theme.css`.** Full power, and the reason Obsidian themes are
  so varied. Lost on safety: it hands third-party CSS the whole shell,
  approval cards included, with no way to validate it.
- **Derive in TypeScript.** The shell, boot script and SDK could share a TS
  resolver. The engine would then need a second copy to seed isolated app
  frames, and the two would drift. One Rust resolver serves all of them.
- **New `LookSaved` / `LookDeleted` events and write routes.** Looks are files,
  and `PUT` / `DELETE /api/v1/data/looks/...` already commit and emit
  `DataFileWritten` / `DataFileDeleted`. The data route gained look validation
  instead.
- **Rename the `theme` preference to `theme-mode`.** Clearer, but it touches
  stored rows, the boot scripts, the SDK and every shipped app reading
  `prefs.theme`. Not worth it for a naming clash a glossary entry resolves.
