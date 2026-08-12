# Vendored fonts

## FiraCode-VF.woff2

Fira Code **6.2**, the variable-weight WOFF2 build, taken verbatim from the
upstream release archive:

    https://github.com/tonsky/FiraCode/releases/download/6.2/Fira_Code_v6.2.zip  (woff2/FiraCode-VF.woff2)

One file covers weights **300 to 700**, which is the whole range the app asks
for. License: SIL Open Font License 1.1, full text in `LICENSE-FiraCode.txt`
(copied from the tag), which permits redistribution.

**Why it is checked in rather than fetched from Google Fonts.** Fira Code is the
default UI font, and a Lucidos workspace is a self-contained local install, so
its ordinary appearance must not depend on a third-party origin being reachable.
Off the CDN, an offline or air-gapped workspace would render the entire UI in the
browser's generic `monospace`, and every boot would announce itself to Google.
The other three web fonts (Inter, JetBrains Mono, IBM Plex Mono) are still loaded
from Google on demand, which is fine for a font the user opted into.

**Two consumers, one copy.** The host bundle declares the `@font-face` in
`src/styles/global/base.css` with a relative `url()`, so Vite hashes the file
into `assets/` and the service worker's shell cache covers it. App iframes are
outside that bundle, so `crates/lucidos-engine/src/api/sdk_fonts.rs`
`include_bytes!`s **this same file** and serves it at
`/api/v1/fonts/fira-code-<version>.woff2`. Moving or renaming the file breaks
the engine build, which is the intended failure mode.

**Upgrading the font is a three-line change, and all three lines matter.**
Replace the `.woff2` here, then bump `FIRA_CODE_VERSION` in `sdk_fonts.rs` and
the filename in `sdk_fonts_fira_code.css` (a unit test fails if those two
disagree). The version is in the served URL because the bytes go out as
`immutable` for a year: dropping new bytes at the old URL would leave every
warm client on the old glyphs with no way to invalidate them. The host side
needs no version, since Vite content-hashes its copy already.
