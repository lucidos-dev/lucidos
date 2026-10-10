---
name: Workspace fonts
description: Install, fix or remove the user's own fonts, or ship one in a plugin: "install this font", "my brand font", "custom font", "woff2 file".
---

# Workspace fonts

A *workspace font* is a font installed into the workspace, beside the fonts
Lucidos bundles. The user installs one in Settings → Appearance → Workspace
fonts, you write its files, or a plugin ships one. It then works like a catalog
font: the user picks it under Font, and a theme can name it.

The engine serves it from the workspace, never from the internet: `font.json`
has no URL field. Decision record: [ADR 0308](https://github.com/lucidos-dev/lucidos/blob/main/docs/adr/0308-workspace-fonts-install-under-data-fonts.md).

## The layout

One directory per font, `data/fonts/<slug>/`:

```text
data/fonts/brand-sans/
  font.json
  BrandSans-Regular.woff2
  BrandSans-Italic.woff2
```

```json
{
  "label": "Brand Sans",
  "group": "sans",
  "ligatures": false,
  "license": "OFL-1.1",
  "faces": [
    { "file": "BrandSans-Regular.woff2", "weight": "100 900", "style": "normal" },
    { "file": "BrandSans-Italic.woff2", "weight": "400", "style": "italic" }
  ]
}
```

- The id is `ws-` and the directory name: `ws-brand-sans`.
- `group` decides where the font fits, as it does for a catalog font:

  | `group` | UI font | Code font (`fonts.mono`) |
  |---|---|---|
  | `sans`, `serif` | yes | no |
  | `mono` | yes | yes |

  It is not checked against the file, so say `mono` only for a monospaced font.
- `weight` is one number, or the `min max` range a variable file covers.
  Default `400`. `style` is `normal` (the default) or `italic`.
- `ligatures` marks a mono font with programming ligatures. They apply on code
  surfaces only, as for Fira Code.
- `license` is optional text for the user to read.

## The limits

| Field | Rule |
|---|---|
| Directory name | Up to 40 lowercase letters and digits, joined by single hyphens |
| Face files | woff2, woff, ttf or otf, each at most 10 MiB. The first bytes must match the extension's container: a `.ttf` or `.otf` may hold either outline type. A font collection (`.ttc`) is refused. |
| File names | Letters, digits, `.`, `_` and `-`, in the font's own directory. No folders and no URLs. |
| Faces | 1 to 16 per font |
| Fonts | 100 per workspace. Installing one more is refused until one is removed. |
| Label | 1 to 60 characters |

## Install one

Write the face files first and `font.json` last, so the font appears only when
complete:

```bash
lucidos data write fonts/brand-sans/BrandSans-Regular.woff2 --from /tmp/BrandSans-Regular.woff2
lucidos data write fonts/brand-sans/font.json --from /tmp/font.json
```

The engine checks each write before it reaches disk. A refused write exits
non-zero with the reason. If the user gave you a URL, download the file once to
`/tmp` and write it from there.

Only use a font the user has the right to use. When you fetch one yourself,
check the licence and put its SPDX id in `license`.

## Check it

`GET /api/v1/fonts` lists the catalog, then every valid workspace font with
`source: "workspace"`, its `family`, and its `faces` as `data/`-relative paths.
A font that fails a check appears on no surface. It is listed under `invalid`
with the reason, such as a missing face file. Fix the file, or remove the font.

## Use it

- **As the UI font:** `set_preference("font-family", "ws-brand-sans")` for the
  calling device, or the user picks it under Settings → Font.
- **From a theme:** name it in `fonts.ui`, or in `fonts.mono` for a `mono` font.
  The theme write is refused unless the font is installed and fits the slot. See
  `system-knowhow/themes.md` § Fonts. Approval cards still show a command in a
  catalog code font (`system-knowhow/themes.md` § Protected surfaces).

If the font is removed later, the preference or theme quietly paints as if it
named nothing.

## Remove one

The user removes one in Settings → Appearance → Workspace fonts, or you delete
the directory. Open clients see the change the next time they read the font
list.

## Ship one in a plugin

Put the directory under the plugin's own `fonts/`, as in the layout above.
Staging checks every font the same way. It refuses a plugin whose new fonts
would take the workspace past 100. A theme in the plugin may name a workspace
font only if the same plugin ships it. See `system-knowhow/plugins.md`.
