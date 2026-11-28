use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CaptureFormat {
    Png,
    Jpeg,
    Webp,
}

impl CaptureFormat {
    const BY_EXTENSION: &'static [(&'static str, Self)] = &[
        ("png", Self::Png),
        ("jpg", Self::Jpeg),
        ("jpeg", Self::Jpeg),
        ("webp", Self::Webp),
    ];

    pub fn from_extension(extension: &str) -> Option<Self> {
        Self::BY_EXTENSION
            .iter()
            .find(|(ext, _)| *ext == extension)
            .map(|(_, format)| *format)
    }

    pub fn extensions() -> String {
        let names: Vec<&str> = Self::BY_EXTENSION.iter().map(|(ext, _)| *ext).collect();
        match names.split_last() {
            Some((last, [])) => last.to_string(),
            Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
            None => String::new(),
        }
    }

    pub fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }
}
