use serde::{Deserialize, Serialize};

use super::workspace_fonts::{WorkspaceFontFace, WorkspaceFonts};

pub const FOLLOW_THEME: &str = "theme";

pub const FALLBACK_FONT: &str = "fira-code";

pub const NATIVE_MONO_STACK: &str = "ui-monospace, SFMono-Regular, 'SF Mono', Menlo, 'Cascadia Mono', Consolas, 'Ubuntu Mono', 'DejaVu Sans Mono', 'Liberation Mono', 'Noto Sans Mono', monospace";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FontKind {
    Ui,
    Mono,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FontGroup {
    Sans,
    Serif,
    Mono,
}

#[derive(Debug)]
pub struct FontFace {
    pub asset: &'static str,
    /// INVARIANT: The name `api/fonts.rs` serves it under. It carries the upstream
    /// version, so the bytes can be cached as immutable.
    pub served: &'static str,
    pub bytes: &'static [u8],
    pub weight: &'static str,
    pub unicode_range: Option<&'static str>,
}

#[derive(Debug)]
pub struct FontMetrics {
    pub size_adjust: &'static str,
    pub ascent_override: &'static str,
    pub descent_override: &'static str,
    pub line_gap_override: &'static str,
}

const BOLD_WEIGHT: u16 = 600;

pub fn weight_reaches_bold(weight: &str) -> bool {
    weight
        .split_whitespace()
        .last()
        .and_then(|heaviest| heaviest.parse::<u16>().ok())
        .is_some_and(|heaviest| heaviest >= BOLD_WEIGHT)
}

const EVERY_WEIGHT: &str = "100 900";

#[derive(Debug)]
pub enum FontSource {
    Vendored {
        family: &'static str,
        faces: &'static [FontFace],
    },
    Device,
}

#[derive(Debug)]
pub struct FontSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// INVARIANT: The CSS `font-family` value. It must pass the theme token value gate,
    /// because a theme's `fonts.mono` lays it as a token. It ends in the generic
    /// family that decides the font's group.
    pub stack: &'static str,
    pub kind: FontKind,
    pub source: FontSource,
    pub license: &'static str,
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

    pub fn has_bold(&self) -> bool {
        match self.source {
            FontSource::Vendored { faces, .. } => {
                faces.iter().any(|face| weight_reaches_bold(face.weight))
            }
            FontSource::Device => true,
        }
    }

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
        stack: NATIVE_MONO_STACK,
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
        metrics: Some(FontMetrics {
            size_adjust: "134.6%",
            ascent_override: "65.7%",
            descent_override: "25.7%",
            line_gap_override: "0%",
        }),
    },
];

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

#[derive(Debug, Serialize)]
struct FontView<'a> {
    id: &'a str,
    label: &'a str,
    stack: &'a str,
    kind: FontKind,
    group: FontGroup,
    source: &'static str,
    license: &'a str,
    theme_nameable: bool,
    ligatures: bool,
    bold: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    family: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    faces: Option<&'a [WorkspaceFontFace]>,
}

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

#[path = "fonts_codegen.rs"]
pub mod codegen;

#[cfg(test)]
#[path = "fonts_tests.rs"]
mod tests;
