use serde::{Deserialize, Serialize};

/// The file format a saved app capture is encoded in. The save path's
/// extension picks it, and the app frame encodes the picture to match.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CaptureFormat {
    Png,
    Jpeg,
    Webp,
}

impl CaptureFormat {
    /// Every extension a save path may end in, and the format it names.
    const BY_EXTENSION: &'static [(&'static str, Self)] = &[
        ("png", Self::Png),
        ("jpg", Self::Jpeg),
        ("jpeg", Self::Jpeg),
        ("webp", Self::Webp),
    ];

    /// The format a lowercase file extension names, if any.
    pub fn from_extension(extension: &str) -> Option<Self> {
        Self::BY_EXTENSION
            .iter()
            .find(|(ext, _)| *ext == extension)
            .map(|(_, format)| *format)
    }

    /// The allowed extensions as a list for prose: "png, jpg, jpeg or webp".
    pub fn extensions() -> String {
        let names: Vec<&str> = Self::BY_EXTENSION.iter().map(|(ext, _)| *ext).collect();
        match names.split_last() {
            Some((last, [])) => last.to_string(),
            Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
            None => String::new(),
        }
    }

    /// The MIME type `core::blobs::sniff_image_mime` reports for this format.
    pub fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }
}
