//! Backup filenames shared by the engine and the gateway restore picker.

const PREFIX: &str = "lucidos-backup-";
const ORIGIN_MARKER: char = 'o';
pub const AGE_SUFFIX: &str = ".age";
pub const LEGACY_SUFFIX: &str = ".enc";

pub fn format_marked(
    workspace: &str,
    timestamp: impl std::fmt::Display,
    origin: &str,
    suffix: &str,
) -> String {
    format!("{PREFIX}{workspace}-{timestamp}-{ORIGIN_MARKER}{origin}{suffix}")
}

pub fn is_origin(token: &str) -> bool {
    token.len() == 12
        && token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub struct ArchiveName<'a> {
    pub workspace: &'a str,
    pub origin: Option<&'a str>,
}

pub fn parse(filename: &str) -> Option<ArchiveName<'_>> {
    let rest = filename.strip_prefix(PREFIX)?;
    let stem = rest
        .strip_suffix(AGE_SUFFIX)
        .or_else(|| rest.strip_suffix(LEGACY_SUFFIX))?;
    let (base, tail) = stem.rsplit_once('-')?;
    let (stem, origin) = match tail.strip_prefix(ORIGIN_MARKER) {
        Some(origin) if is_origin(origin) => (base, Some(origin)),
        Some(_) => return None,
        None => (stem, None),
    };
    let (before_time, time) = stem.rsplit_once('-')?;
    let (workspace, date) = before_time.rsplit_once('-')?;
    if workspace.is_empty()
        || date.len() != 8
        || time.len() != 6
        || !date.bytes().chain(time.bytes()).all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some(ArchiveName { workspace, origin })
}
