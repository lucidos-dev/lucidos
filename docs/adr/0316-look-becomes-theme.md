# 0316: The palette is a theme and light/dark is the theme mode, renamed in every layer

- **Status**: Accepted, amends ADR 0296
- **Date**: 2026-09-28

## Context

ADR 0296 named the new palette concept a *look*. The stored `theme` preference
already meant light, dark or system. 0296 judged renaming that key "not worth
it for a naming clash a glossary entry resolves".

The clash reached the UI. Settings → Appearance opened with a section called
"Theme" that held Mode, Motion and Look effects. A second section, "Look", held
the actual themes.

The user asked what best practice is. Slack, Chrome, Obsidian and VS Code call
the named palette a theme and the light/dark switch a mode. No user reads the
glossary. Plan: `docs/plans/2026-09-27-look-becomes-theme.md`.

## Decision

The palette is a *theme* and light/dark/system is the *theme mode*, in every
layer. The keys are `theme`, `theme-mode` and `theme-effects`. The attribute is
`data-theme-mode`, and the routes, data folder and plugin folder are `themes`.
Settings → Appearance opens with a Theme section: Mode, the picker, Effects.

## Rationale

- **Use the word users already know.** The glossary entry for *look* had to say
  "what Obsidian and VS Code call a theme". A term that needs a translation
  note is the wrong term.
- **Rename every layer, not the label.** The glossary rule wants one name root
  across the UI, code and docs. A UI that says "Theme" over a `look` key would
  have been the half-renamed concept that rule calls worse than the drift.
- **Now was the cheapest moment.** Looks landed the day before, so no plugin
  and almost no workspace depended on the names.
- **"Mode", not "colour mode".** A theme picks the colours, so "colour" would
  claim the theme's word for the switch between light and dark.

## Consequences

- Two migrations move every stored row, `theme` to `theme-mode` first and then
  `look` to `theme`.
- A write of `theme=dark` is refused with an error naming `theme-mode`, and no
  theme id may be `light`, `dark` or `system`. Code written before the rename
  fails loudly rather than painting the default.
- `/api/v1/looks` and its siblings return 404. No alias routes.
- Three temporary measures carry the upgrade: the legacy `data-theme` stamp in
  app frames, the one-time adoption of the renamed localStorage keys, and the
  startup move of `data/looks/` into `data/themes/`
  (`docs/temporary-measures.md`).
- ADR 0296's decision stands in full. Only its names change.

## Alternatives considered

- **Keep "Look", fix the headings.** UI-only and cheap, but it keeps a term
  that needs explaining, and "theme" would still mean the mode in code.
- **Keep the `data-theme` attribute name for the resolved mode.** No shim, but
  `data-theme` would say light or dark while "theme" means the palette
  everywhere else. The stamp in app frames keeps old app styles working
  instead, for a bounded time.
- **"Colour mode" or "Appearance" for light/dark.** Colour clashes with what a
  theme does, and Appearance is already the name of the Settings page.
