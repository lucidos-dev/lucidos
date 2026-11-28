---
name: Themes
description: Switch, make or ship themes: colours, headers, focus cues. "Nord theme", "flat header", "theme plugin".
---

# Themes

A *theme* is a named set of design-token values: colours, header chrome, focus
cues, radii and shadows. It can also suggest fonts. Light, dark and system are
the *theme mode*, set by the `theme-mode` preference. A theme styles one mode or
both.

A theme carries **tokens and theme parts**, never CSS. It can recolour Lucidos
and add capped effects, such as a glow on chat text. It cannot move a control,
hide a card, write a selector, or load anything from a third-party origin.

## Parts

This doc is split into parts so each one fits a coding agent's tool output. Load the part that covers what you need.

| Part | Covers |
|---|---|
| `system-knowhow/themes/switch-themes` | Switch themes; Make a theme; Three seeds are enough; Fonts; Headers and focus; Actor icons; Theme parts (Text glow; Theme effects; Retro effects); Protected surfaces |
| `system-knowhow/themes/themes-in-a-plugin` | Themes in a plugin; For apps; The theme token catalog (Surfaces; Text; Accent; Brand marks; Status; Header; Focus; Shape; Shadows; Syntax; Typography) |
