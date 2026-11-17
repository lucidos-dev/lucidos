use semver::Version;

/// Whether a feed version can participate in release precedence.
pub fn is_valid_version(version: &str) -> bool {
    Version::parse(version.trim()).is_ok()
}

/// Invalid versions fail closed. Build metadata does not change precedence.
pub fn is_upgrade(current: &str, offered: &str) -> bool {
    let (Ok(current), Ok(offered)) = (
        Version::parse(current.trim()),
        Version::parse(offered.trim()),
    ) else {
        return false;
    };
    offered.cmp_precedence(&current).is_gt()
}

/// Strictly increasing semver precedence, shared with TypeScript through codegen.
pub const ORDERED_VERSIONS: &[&str] = &[
    "0.0.0",
    "9.1.0",
    "9.2.0-alpha",
    "9.2.0-alpha.1",
    "9.2.0-alpha.beta",
    "9.2.0-beta.2",
    "9.2.0-beta.11",
    "9.2.0-nightly.20261116.1",
    "9.2.0-nightly.20261116.2",
    "9.2.0-nightly.20261116.10",
    "9.2.0-nightly.20261117.1",
    "9.2.0-rc.1",
    "9.2.0",
    "9.2.1",
    "9.3.0-nightly.20261117.1",
    "9.3.0",
    "9.100.0",
    "10.0.0",
    "11.0.0",
    "20.0.0",
];
