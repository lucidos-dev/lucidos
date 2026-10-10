# Vendored fonts

Every font here is an entry in the font catalog,
`crates/lucidos-engine/src/core/fonts.rs`, and ships under the SIL Open Font
License 1.1. Each font's license text sits beside it as `LICENSE-<Family>.txt`,
which OFL requires for redistribution (a unit test fails if one is missing).

**Why every font is checked in rather than fetched from Google Fonts.** A
vendored font works on a workspace with no internet and makes no request to a
third-party origin. Every catalog font works that way, so a theme may name any of
them (ADR 0077, ADR 0298, ADR 0303).

## Provenance

| File | Font | Source |
|---|---|---|
| `FiraCode-VF.woff2` | Fira Code 6.2, weights 300 to 700 | `https://github.com/tonsky/FiraCode/releases/download/6.2/Fira_Code_v6.2.zip` (`woff2/FiraCode-VF.woff2`) |
| `CascadiaCode.woff2` | Cascadia Code 2407.24, weights 200 to 700 | `https://github.com/microsoft/cascadia-code/releases/download/v2407.24/CascadiaCode-2407.24.zip` (`woff2/CascadiaCode.woff2`, SHA-256 `3ec1f7e7…0dccc`) |
| `CommitMono-{400,700}-*.woff2` | Commit Mono 1.143, weights 400 and 700 | `https://github.com/eigilnikolajsen/commit-mono/releases/download/v1.143/CommitMono-1.143.zip` (`CommitMono-{400,700}-Regular.otf`) |
| `Geist-*.woff2` | Geist, Google Fonts v5, weights 100 to 900 | `https://fonts.googleapis.com/css2?family=Geist:wght@100..900` |
| `GeistMono-*.woff2` | Geist Mono, Google Fonts v6, weights 100 to 900 | `https://fonts.googleapis.com/css2?family=Geist+Mono:wght@100..900` |
| `AtkinsonHyperlegibleNext-*.woff2` | Atkinson Hyperlegible Next, Google Fonts v7, weights 200 to 800 | `https://fonts.googleapis.com/css2?family=Atkinson+Hyperlegible+Next:wght@200..800` |
| `AtkinsonHyperlegibleMono-*.woff2` | Atkinson Hyperlegible Mono, Google Fonts v8, weights 200 to 800 | `https://fonts.googleapis.com/css2?family=Atkinson+Hyperlegible+Mono:wght@200..800` |
| `SourceSerif4-*.woff2` | Source Serif 4, Google Fonts v14, weights 200 to 900 | `https://fonts.googleapis.com/css2?family=Source+Serif+4:wght@200..900` |
| `Inter-*.woff2` | Inter, Google Fonts v20, weights 100 to 900 | `https://fonts.googleapis.com/css2?family=Inter:wght@100..900` |
| `Roboto-*.woff2` | Roboto, Google Fonts v51, weights 100 to 900 | `https://fonts.googleapis.com/css2?family=Roboto:wght@100..900` |
| `OpenSans-*.woff2` | Open Sans, Google Fonts v44, weights 300 to 800 | `https://fonts.googleapis.com/css2?family=Open+Sans:wght@300..800` |
| `Manrope-*.woff2` | Manrope, Google Fonts v20, weights 200 to 800 | `https://fonts.googleapis.com/css2?family=Manrope:wght@200..800` |
| `Lora-*.woff2` | Lora, Google Fonts v37, weights 400 to 700 | `https://fonts.googleapis.com/css2?family=Lora:wght@400..700` |
| `Literata-*.woff2` | Literata, Google Fonts v40, weights 200 to 900 | `https://fonts.googleapis.com/css2?family=Literata:wght@200..900` |
| `JetBrainsMono-*.woff2` | JetBrains Mono, Google Fonts v24, weights 100 to 800 | `https://fonts.googleapis.com/css2?family=JetBrains+Mono:wght@100..800` |
| `IBMPlexMono-{400,500,600,700}-*.woff2` | IBM Plex Mono, Google Fonts v20, four static weights | `https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500;600;700` |
| `SourceCodePro-*.woff2` | Source Code Pro, Google Fonts v31, weights 200 to 900 | `https://fonts.googleapis.com/css2?family=Source+Code+Pro:wght@200..900` |
| `VT323-400-*.woff2` | VT323, Google Fonts v18, one static weight (400) | `https://fonts.googleapis.com/css2?family=VT323` |

For each Google-sourced font, the files are the `latin` and `latin-ext` subsets
that stylesheet names, fetched with a current Chrome user agent and unmodified.
The `unicode-range` of each subset is in `core/fonts.rs`. The license texts come
from `https://github.com/google/fonts/tree/main/ofl`.

The files stay unmodified even where a face renders off the rest of the
catalog. Its `@font-face` rules adjust it instead: VT323's catalog entry in
`core/fonts.rs` carries `size-adjust` and the three line-metric overrides,
measured from the file. A font with no bold face declares its one face over
every weight, so browsers never smear it into a fake bold.

The two fonts from GitHub releases differ:

- **Commit Mono** ships no variable file, so it has the two static weights of
  its release. Each was cut to the same two subsets with
  `pyftsubset --layout-features='*' --flavor=woff2`. Its license has no Reserved
  Font Name, so a subset may keep the name. The license text is the release's
  `license.txt`.
- **Cascadia Code** has the Reserved Font Name "Cascadia Code". Under OFL a
  subset is a Modified Version, which may not use that name. So the file is the
  release's own woff2, byte for byte: the full character set, with no
  `unicode-range`. The license text is the repository's `LICENSE`.

## Two consumers, one copy

The host bundle declares every face in the generated
`src/styles/generated/font-faces.css` with a relative `url()`, so Vite hashes the
files into `assets/`. App iframes are outside that bundle, so `core/fonts.rs`
`include_bytes!`s **these same files** and `api/fonts.rs` serves them at
`/api/v1/fonts/<id>-<version>….woff2`. Moving or renaming a file breaks the
engine build, which is the intended failure mode.

## Adding or upgrading a font

1. Put the woff2 files and the license text here.
2. Add or edit the entry in `core/fonts.rs`. The served name carries the
   upstream version, because the bytes go out as `immutable` for a year: new
   bytes at an old URL would leave every warm client on the old glyphs.
3. Run `cargo test -p lucidos-engine --lib generate_font_catalog_files -- --ignored`,
   then rebuild the SDK boot scripts (`cd packages/lucidos-sdk && npm run build`).
4. Name the new id in `system-knowhow/preferences.md` and `system-knowhow/js-sdk.md`
   (a unit test fails otherwise).
