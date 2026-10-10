//! The font catalog: every font Lucidos offers, one entry each.
//!
//! Every other font surface derives from this list:
//!
//! - the `font-family` preference values;
//! - the SDK's generated `font-catalog.ts`, and through it the boot scripts
//!   and the Settings options;
//! - the host's generated `font-faces.css`;
//! - the stylesheets and files `api/fonts.rs` serves to app frames;
//! - `GET /api/v1/fonts`.
//!
//! Adding a font is one entry here plus its files, then the regenerate command
//! the staleness test prints. Every font is vendored or already on the device,
//! so none makes a third-party request and a theme may name any of them (ADR
//! 0303).
//!
//! A workspace can add its own fonts beside these (`core::workspace_fonts`,
//! ADR 0308). Only the `font-family` preference, themes and `GET /api/v1/fonts`
//! see them. The generated files stay the catalog alone.

use serde::{Deserialize, Serialize};

use super::workspace_fonts::{WorkspaceFontFace, WorkspaceFonts};

/// The `font-family` value that follows the active theme's suggestion.
pub const FOLLOW_THEME: &str = "theme";

/// The font a device paints when neither the user nor the theme names one. It
/// is vendored, so it renders offline (ADR 0077).
pub const FALLBACK_FONT: &str = "fira-code";

/// What a font is fit for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FontKind {
    /// Proportional: fit for UI text, not for code.
    Ui,
    /// Fit only for code.
    Mono,
    /// Monospaced and fit for UI text too. Lucidos is mono-first.
    Both,
}

impl FontKind {
    pub fn fits_ui(self) -> bool {
        self != FontKind::Mono
    }

    pub fn fits_code(self) -> bool {
        self != FontKind::Ui
    }
}

/// The generic family a font falls back to. Settings groups fonts by it, and a
/// workspace font declares one in its `font.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FontGroup {
    Sans,
    Serif,
    Mono,
}

/// One woff2 file of a vendored font.
#[derive(Debug)]
pub struct FontFace {
    /// The file name under `crates/lucidos-app/src/assets/fonts/`.
    pub asset: &'static str,
    /// The name `api/fonts.rs` serves it under. It carries the upstream
    /// version, so the bytes can be cached as immutable.
    pub served: &'static str,
    pub bytes: &'static [u8],
    /// A `font-weight` descriptor: the range a variable file covers, or the one
    /// weight of a static file.
    pub weight: &'static str,
    pub unicode_range: Option<&'static str>,
}

/// Size and vertical metric overrides for a face whose design sits off the
/// rest of the catalog. Each value is an `@font-face` percentage, measured from
/// the font file. The browser scales the three line metrics by `size_adjust`.
#[derive(Debug)]
pub struct FontMetrics {
    pub size_adjust: &'static str,
    pub ascent_override: &'static str,
    pub descent_override: &'static str,
    pub line_gap_override: &'static str,
}

/// The lightest weight that counts as bold. Below it the browser draws a real
/// face; at or above it, with no such face, it fakes one by smearing.
const BOLD_WEIGHT: u16 = 600;

/// Whether a face's `font-weight`, one value or a range, reaches bold at its
/// heaviest end.
pub fn weight_reaches_bold(weight: &str) -> bool {
    weight
        .split_whitespace()
        .last()
        .and_then(|heaviest| heaviest.parse::<u16>().ok())
        .is_some_and(|heaviest| heaviest >= BOLD_WEIGHT)
}

/// The weight range a font with no bold face declares its faces over. Bold text
/// then paints with the real outlines instead of a smeared fake.
const EVERY_WEIGHT: &str = "100 900";

/// Where a font's glyphs come from.
#[derive(Debug)]
pub enum FontSource {
    /// Served by the local engine and bundled into the host build.
    Vendored {
        /// The family name the faces declare.
        family: &'static str,
        faces: &'static [FontFace],
    },
    /// Fonts the device already has. Nothing loads.
    Device,
}

#[derive(Debug)]
pub struct FontSpec {
    /// The `font-family` preference value and the theme `fonts` value.
    pub id: &'static str,
    pub label: &'static str,
    /// The CSS `font-family` value. It must pass the theme token value gate,
    /// because a theme's `fonts.mono` lays it as a token. It ends in the generic
    /// family that decides the font's group.
    pub stack: &'static str,
    pub kind: FontKind,
    pub source: FontSource,
    /// An SPDX license id, or `none` for the device's own fonts.
    pub license: &'static str,
    /// Ships programming ligatures, which apply to code and never to prose.
    pub ligatures: bool,
    pub metrics: Option<FontMetrics>,
}

impl FontSpec {
    pub fn source_name(&self) -> &'static str {
        match self.source {
            FontSource::Vendored { .. } => "vendored",
            FontSource::Device => "device",
        }
    }

    /// Read from the generic family that ends the stack, so the two cannot
    /// disagree.
    pub fn group(&self) -> FontGroup {
        match self.stack.rsplit(',').next().map(str::trim) {
            Some("serif") => FontGroup::Serif,
            Some("monospace") => FontGroup::Mono,
            _ => FontGroup::Sans,
        }
    }

    pub fn faces(&self) -> &'static [FontFace] {
        match self.source {
            FontSource::Vendored { faces, .. } => faces,
            FontSource::Device => &[],
        }
    }

    /// Whether any face reaches a bold weight. The device's own fonts are
    /// assumed to have one.
    pub fn has_bold(&self) -> bool {
        match self.source {
            FontSource::Vendored { faces, .. } => {
                faces.iter().any(|face| weight_reaches_bold(face.weight))
            }
            FontSource::Device => true,
        }
    }

    /// The `@font-face` rules of a vendored font, with `url_for` choosing each
    /// face's URL. Empty for a device font.
    pub fn font_face_rules(&self, url_for: impl Fn(&FontFace) -> String) -> String {
        let FontSource::Vendored { family, faces } = &self.source else {
            return String::new();
        };
        let has_bold = self.has_bold();
        let mut out = String::new();
        for face in *faces {
            let weight = if has_bold { face.weight } else { EVERY_WEIGHT };
            out.push_str("@font-face {\n");
            out.push_str(&format!("    font-family: '{family}';\n"));
            out.push_str(&format!(
                "    src: url('{}') format('woff2');\n",
                url_for(face)
            ));
            out.push_str(&format!("    font-weight: {weight};\n"));
            out.push_str("    font-style: normal;\n");
            out.push_str("    font-display: swap;\n");
            if let Some(range) = face.unicode_range {
                out.push_str(&format!("    unicode-range: {range};\n"));
            }
            if let Some(metrics) = &self.metrics {
                out.push_str(&format!("    size-adjust: {};\n", metrics.size_adjust));
                out.push_str(&format!(
                    "    ascent-override: {};\n",
                    metrics.ascent_override
                ));
                out.push_str(&format!(
                    "    descent-override: {};\n",
                    metrics.descent_override
                ));
                out.push_str(&format!(
                    "    line-gap-override: {};\n",
                    metrics.line_gap_override
                ));
            }
            out.push_str("}\n");
        }
        out
    }
}

/// The two subsets every Google-sourced vendored font ships. Other scripts
/// fall back to the rest of the stack.
const LATIN: &str = "U+0000-00FF, U+0131, U+0152-0153, U+02BB-02BC, U+02C6, U+02DA, U+02DC, U+0304, U+0308, U+0329, U+2000-206F, U+20AC, U+2122, U+2191, U+2193, U+2212, U+2215, U+FEFF, U+FFFD";
const LATIN_EXT: &str = "U+0100-02BA, U+02BD-02C5, U+02C7-02CC, U+02CE-02D7, U+02DD-02FF, U+0304, U+0308, U+0329, U+1D00-1DBF, U+1E00-1E9F, U+1EF2-1EFF, U+2020, U+20A0-20AB, U+20AD-20C0, U+2113, U+2C60-2C7F, U+A720-A7FF";

macro_rules! asset_bytes {
    ($file:expr) => {
        include_bytes!(concat!("../../../lucidos-app/src/assets/fonts/", $file))
    };
}

macro_rules! face {
    ($asset:expr, $served:expr, $weight:expr, $range:expr) => {
        FontFace {
            asset: $asset,
            served: $served,
            bytes: asset_bytes!($asset),
            weight: $weight,
            unicode_range: $range,
        }
    };
}

/// The latin and latin-ext faces of a variable font.
macro_rules! latin_faces {
    ($stem:literal, $served:literal, $weight:literal) => {
        &[
            face!(
                concat!($stem, "-latin.woff2"),
                concat!($served, "-latin.woff2"),
                $weight,
                Some(LATIN)
            ),
            face!(
                concat!($stem, "-latin-ext.woff2"),
                concat!($served, "-latin-ext.woff2"),
                $weight,
                Some(LATIN_EXT)
            ),
        ]
    };
}

/// The latin and latin-ext faces of a font with no variable version, one pair
/// per static weight.
macro_rules! static_latin_faces {
    ($stem:literal, $served:literal, [$($weight:literal),+]) => {
        &[$(
            face!(
                concat!($stem, "-", $weight, "-latin.woff2"),
                concat!($served, "-", $weight, "-latin.woff2"),
                stringify!($weight),
                Some(LATIN)
            ),
            face!(
                concat!($stem, "-", $weight, "-latin-ext.woff2"),
                concat!($served, "-", $weight, "-latin-ext.woff2"),
                stringify!($weight),
                Some(LATIN_EXT)
            ),
        )+]
    };
}

/// The catalog, grouped sans, serif, mono, in the order Settings lists it.
pub const FONT_CATALOG: &[FontSpec] = &[
    FontSpec {
        id: "system",
        label: "System",
        stack: "system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
        kind: FontKind::Ui,
        source: FontSource::Device,
        license: "none",
        ligatures: false,
        metrics: None,
    },
    FontSpec {
        id: "geist",
        label: "Geist",
        stack: "'Geist', system-ui, -apple-system, 'Segoe UI', sans-serif",
        kind: FontKind::Ui,
        source: FontSource::Vendored {
            family: "Geist",
            faces: latin_faces!("Geist", "geist-v5", "100 900"),
        },
        license: "OFL-1.1",
        ligatures: false,
        metrics: None,
    },
    FontSpec {
        id: "atkinson-hyperlegible-next",
        label: "Atkinson Hyperlegible Next",
        stack: "'Atkinson Hyperlegible Next', system-ui, -apple-system, 'Segoe UI', sans-serif",
        kind: FontKind::Ui,
        source: FontSource::Vendored {
            family: "Atkinson Hyperlegible Next",
            faces: latin_faces!(
                "AtkinsonHyperlegibleNext",
                "atkinson-hyperlegible-next-v7",
                "200 800"
            ),
        },
        license: "OFL-1.1",
        ligatures: false,
        metrics: None,
    },
    FontSpec {
        id: "inter",
        label: "Inter",
        stack: "'Inter', system-ui, -apple-system, 'Segoe UI', sans-serif",
        kind: FontKind::Ui,
        source: FontSource::Vendored {
            family: "Inter",
            faces: latin_faces!("Inter", "inter-v20", "100 900"),
        },
        license: "OFL-1.1",
        ligatures: false,
        metrics: None,
    },
    FontSpec {
        id: "roboto",
        label: "Roboto",
        stack: "'Roboto', system-ui, -apple-system, 'Segoe UI', sans-serif",
        kind: FontKind::Ui,
        source: FontSource::Vendored {
            family: "Roboto",
            faces: latin_faces!("Roboto", "roboto-v51", "100 900"),
        },
        license: "OFL-1.1",
        ligatures: false,
        metrics: None,
    },
    FontSpec {
        id: "open-sans",
        label: "Open Sans",
        stack: "'Open Sans', system-ui, -apple-system, 'Segoe UI', sans-serif",
        kind: FontKind::Ui,
        source: FontSource::Vendored {
            family: "Open Sans",
            faces: latin_faces!("OpenSans", "open-sans-v44", "300 800"),
        },
        license: "OFL-1.1",
        ligatures: false,
        metrics: None,
    },
    FontSpec {
        id: "manrope",
        label: "Manrope",
        stack: "'Manrope', system-ui, -apple-system, 'Segoe UI', sans-serif",
        kind: FontKind::Ui,
        source: FontSource::Vendored {
            family: "Manrope",
            faces: latin_faces!("Manrope", "manrope-v20", "200 800"),
        },
        license: "OFL-1.1",
        ligatures: false,
        metrics: None,
    },
    FontSpec {
        id: "source-serif-4",
        label: "Source Serif 4",
        stack: "'Source Serif 4', Georgia, 'Times New Roman', serif",
        kind: FontKind::Ui,
        source: FontSource::Vendored {
            family: "Source Serif 4",
            faces: latin_faces!("SourceSerif4", "source-serif-4-v14", "200 900"),
        },
        license: "OFL-1.1",
        ligatures: false,
        metrics: None,
    },
    FontSpec {
        id: "lora",
        label: "Lora",
        stack: "'Lora', Georgia, 'Times New Roman', serif",
        kind: FontKind::Ui,
        source: FontSource::Vendored {
            family: "Lora",
            faces: latin_faces!("Lora", "lora-v37", "400 700"),
        },
        license: "OFL-1.1",
        ligatures: false,
        metrics: None,
    },
    FontSpec {
        id: "literata",
        label: "Literata",
        stack: "'Literata', Georgia, 'Times New Roman', serif",
        kind: FontKind::Ui,
        source: FontSource::Vendored {
            family: "Literata",
            faces: latin_faces!("Literata", "literata-v40", "200 900"),
        },
        license: "OFL-1.1",
        ligatures: false,
        metrics: None,
    },
    FontSpec {
        id: "fira-code",
        label: "Fira Code",
        // The full system-mono chain, because this is the fallback and must
        // paint well before the web font decodes (ADR 0077).
        stack: "'Fira Code', ui-monospace, SFMono-Regular, 'SF Mono', Menlo, 'JetBrains Mono', Monaco, Consolas, monospace",
        kind: FontKind::Both,
        source: FontSource::Vendored {
            family: "Fira Code",
            faces: &[face!(
                "FiraCode-VF.woff2",
                "fira-code-6.2.woff2",
                "300 700",
                None
            )],
        },
        license: "OFL-1.1",
        ligatures: true,
        metrics: None,
    },
    FontSpec {
        id: "monospace",
        label: "Monospace",
        stack: "ui-monospace, SFMono-Regular, 'SF Mono', Menlo, 'Fira Code', 'JetBrains Mono', Monaco, Consolas, monospace",
        kind: FontKind::Both,
        source: FontSource::Device,
        license: "none",
        ligatures: false,
        metrics: None,
    },
    FontSpec {
        id: "geist-mono",
        label: "Geist Mono",
        stack: "'Geist Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
        kind: FontKind::Both,
        source: FontSource::Vendored {
            family: "Geist Mono",
            faces: latin_faces!("GeistMono", "geist-mono-v6", "100 900"),
        },
        license: "OFL-1.1",
        ligatures: false,
        metrics: None,
    },
    FontSpec {
        id: "atkinson-hyperlegible-mono",
        label: "Atkinson Hyperlegible Mono",
        stack: "'Atkinson Hyperlegible Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
        kind: FontKind::Both,
        source: FontSource::Vendored {
            family: "Atkinson Hyperlegible Mono",
            faces: latin_faces!(
                "AtkinsonHyperlegibleMono",
                "atkinson-hyperlegible-mono-v8",
                "200 800"
            ),
        },
        license: "OFL-1.1",
        ligatures: false,
        metrics: None,
    },
    FontSpec {
        id: "jetbrains-mono",
        label: "JetBrains Mono",
        stack: "'JetBrains Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
        kind: FontKind::Both,
        source: FontSource::Vendored {
            family: "JetBrains Mono",
            faces: latin_faces!("JetBrainsMono", "jetbrains-mono-v24", "100 800"),
        },
        license: "OFL-1.1",
        ligatures: true,
        metrics: None,
    },
    FontSpec {
        id: "ibm-plex-mono",
        label: "IBM Plex Mono",
        stack: "'IBM Plex Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
        kind: FontKind::Both,
        source: FontSource::Vendored {
            family: "IBM Plex Mono",
            faces: static_latin_faces!("IBMPlexMono", "ibm-plex-mono-v20", [400, 500, 600, 700]),
        },
        license: "OFL-1.1",
        ligatures: false,
        metrics: None,
    },
    FontSpec {
        id: "source-code-pro",
        label: "Source Code Pro",
        stack: "'Source Code Pro', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
        kind: FontKind::Both,
        source: FontSource::Vendored {
            family: "Source Code Pro",
            faces: latin_faces!("SourceCodePro", "source-code-pro-v31", "200 900"),
        },
        license: "OFL-1.1",
        ligatures: false,
        metrics: None,
    },
    FontSpec {
        id: "commit-mono",
        label: "Commit Mono",
        stack: "'Commit Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
        kind: FontKind::Both,
        source: FontSource::Vendored {
            family: "Commit Mono",
            faces: static_latin_faces!("CommitMono", "commit-mono-1.143", [400, 700]),
        },
        license: "OFL-1.1",
        ligatures: false,
        metrics: None,
    },
    FontSpec {
        id: "cascadia-code",
        label: "Cascadia Code",
        stack: "'Cascadia Code', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
        kind: FontKind::Both,
        source: FontSource::Vendored {
            family: "Cascadia Code",
            // The release file, whole: its licence reserves the name, so a
            // subset could not keep it (ADR 0303).
            faces: &[face!(
                "CascadiaCode.woff2",
                "cascadia-code-2407.24.woff2",
                "200 700",
                None
            )],
        },
        license: "OFL-1.1",
        ligatures: true,
        metrics: None,
    },
    FontSpec {
        id: "vt323",
        label: "VT323",
        stack: "'VT323', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
        kind: FontKind::Both,
        source: FontSource::Vendored {
            family: "VT323",
            faces: static_latin_faces!("VT323", "vt323-v18", [400]),
        },
        license: "OFL-1.1",
        ligatures: false,
        // Measured from the file: 1000 units per em, ascender 800, descender
        // 200, x-height 400, and ink from 560 (caps and ascenders) down to
        // -160. Fira Code's x-height is 0.5385 em, so 134.6% matches it. The
        // line box keeps Fira's total height, 1.2308 em. It centres on the ink
        // at 0.20 em, which puts the text caret level with the glyphs.
        metrics: Some(FontMetrics {
            size_adjust: "134.6%",
            ascent_override: "65.7%",
            descent_override: "25.7%",
            line_gap_override: "0%",
        }),
    },
];

/// Every value the `font-family` preference accepts: follow the theme, then
/// each catalog id.
pub const FONT_PREFERENCE_VALUES: [&str; FONT_CATALOG.len() + 1] = {
    let mut values = [FOLLOW_THEME; FONT_CATALOG.len() + 1];
    let mut i = 0;
    while i < FONT_CATALOG.len() {
        values[i + 1] = FONT_CATALOG[i].id;
        i += 1;
    }
    values
};

pub fn find(id: &str) -> Option<&'static FontSpec> {
    FONT_CATALOG.iter().find(|font| font.id == id)
}

/// One font as `GET /api/v1/fonts` serves it.
#[derive(Debug, Serialize)]
struct FontView<'a> {
    id: &'a str,
    label: &'a str,
    stack: &'a str,
    kind: FontKind,
    group: FontGroup,
    source: &'static str,
    license: &'a str,
    /// Always true now (ADR 0303). Apps filter on it, so it stays.
    theme_nameable: bool,
    ligatures: bool,
    bold: bool,
    /// A workspace font's family and faces, which a client registers itself.
    #[serde(skip_serializing_if = "Option::is_none")]
    family: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    faces: Option<&'a [WorkspaceFontFace]>,
}

/// The catalog, then the workspace fonts, as JSON for `GET /api/v1/fonts`.
/// Workspace fonts that failed a check are listed apart, with the reason.
pub fn catalog_json(workspace: &WorkspaceFonts) -> serde_json::Value {
    let catalog = FONT_CATALOG.iter().map(|font| FontView {
        id: font.id,
        label: font.label,
        stack: font.stack,
        kind: font.kind,
        group: font.group(),
        source: font.source_name(),
        license: font.license,
        theme_nameable: true,
        ligatures: font.ligatures,
        bold: font.has_bold(),
        family: None,
        faces: None,
    });
    let installed = workspace.fonts.iter().map(|font| FontView {
        id: &font.id,
        label: &font.label,
        stack: &font.stack,
        kind: font.kind,
        group: font.group,
        source: "workspace",
        license: &font.license,
        theme_nameable: true,
        ligatures: font.ligatures,
        bold: font.has_bold(),
        family: Some(&font.family),
        faces: Some(&font.faces),
    });
    let fonts: Vec<FontView> = catalog.chain(installed).collect();
    serde_json::json!({
        "fonts": fonts,
        "invalid": workspace.invalid,
        "follow_theme": FOLLOW_THEME,
        "fallback": FALLBACK_FONT,
    })
}

#[cfg(test)]
#[path = "fonts_tests.rs"]
mod tests;
