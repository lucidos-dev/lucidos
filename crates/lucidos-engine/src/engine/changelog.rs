//! The user-facing changelog, baked into the binary and served to the *What's
//! New* panel (Settings > System).
//!
//! **Baked, not read from disk**, for the same reason [`crate::LUCIDOS_RELEASE`]
//! is: a packaged install has no repo checkout, so a filesystem read would leave
//! the panel empty exactly where it matters most. `include_str!` also makes the
//! text available offline and removes any question of which checkout answered.
//!
//! The cost of baking is that the running process serves the copy it was BUILT
//! with, so an edit needs a rebuild to show up. `CHANGELOG.md` is therefore in
//! `git_ops::restart_detection::files_require_restart`'s engine-bundled-asset
//! list, alongside the other `include_str!`'d assets. In practice the file moves
//! only at release time, in the same commit that bumps `RELEASE`, which is
//! already baked.
//!
//! This module answers only "what is in the releases you HAVE". The notes for a
//! release being *offered* by the updater are necessarily absent here, since the
//! offered version postdates the binary doing the offering; those come from the
//! updater manifest's `notes` (written by `scripts/lib/release_notes.sh` from
//! this same file) and are handled client-side.

use serde::Serialize;

/// The repo-root `CHANGELOG.md`. See the module docs for why this is baked.
const CHANGELOG_MD: &str = include_str!("../../../../CHANGELOG.md");

/// The marker opening a release section. The version digit is checked
/// separately, so this alone does not identify a header.
const RELEASE_HEADING_PREFIX: &str = "## v";

/// One published release, as the What's New panel shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChangelogRelease {
    /// The version with no leading `v`, e.g. `0.26.3`. Compared against
    /// [`crate::LUCIDOS_RELEASE`] client-side to mark the running release.
    pub version: String,
    /// The release date as written in the heading, or `None` when the heading
    /// carries only a version.
    pub date: Option<String>,
    /// The section body as RAW MARKDOWN, heading excluded, with surrounding
    /// blank lines trimmed. Raw rather than HTML per `.claude/rules/rust.md`:
    /// the frontend converts.
    pub notes: String,
}

/// Every release in the baked changelog, newest first (the file's own order).
pub fn changelog_releases() -> Vec<ChangelogRelease> {
    parse_changelog(CHANGELOG_MD)
}

/// Split a changelog into its release sections.
///
/// **Separator-blind by construction.** The headings are written
/// `## v0.26.3 <separator> 2026-08-11`, and the separator in this repo's file is
/// an em dash, which `.claude/rules/no-em-dashes.md` forbids this source from
/// containing. So the date is not matched against a separator at all: it is
/// whatever remains once the leading non-alphanumeric run is dropped. That
/// tolerates an em dash, an en dash, a hyphen, a comma or nothing, and it is the
/// same posture `release_notes_extract_section` (scripts/lib/release_notes.sh)
/// takes for the same reason.
///
/// A heading counts as a release only when a DIGIT follows `## v`, so a prose
/// heading such as `## various notes` stays body text instead of becoming a
/// release named `arious`.
fn parse_changelog(src: &str) -> Vec<ChangelogRelease> {
    let mut releases: Vec<ChangelogRelease> = Vec::new();
    let mut body = String::new();

    // Close the section under construction, if any, moving `body` into it.
    fn flush(releases: &mut [ChangelogRelease], body: &mut String) {
        if let Some(last) = releases.last_mut() {
            last.notes = body.trim().to_string();
        }
        body.clear();
    }

    for line in src.lines() {
        match parse_release_heading(line) {
            Some((version, date)) => {
                flush(&mut releases, &mut body);
                releases.push(ChangelogRelease {
                    version,
                    date,
                    notes: String::new(),
                });
            }
            // Everything before the first heading (the document's own `#
            // Changelog` title) has no section to belong to and is dropped.
            None if releases.is_empty() => {}
            None => {
                body.push_str(line);
                body.push('\n');
            }
        }
    }
    flush(&mut releases, &mut body);
    releases
}

/// The version and optional date of a release heading, or `None` when `line` is
/// not one. Pure, and the whole of the format's definition.
fn parse_release_heading(line: &str) -> Option<(String, Option<String>)> {
    let rest = line.strip_prefix(RELEASE_HEADING_PREFIX)?;
    // A digit is what separates `## v0.26.3` from a prose heading opening with
    // the same three characters.
    if !rest.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    // `splitn` rather than measuring the version and slicing the remainder by
    // its byte length: `.claude/rules/rust.md` bans byte-index slicing outright,
    // and while a whitespace-delimited prefix does land on a char boundary, a
    // form with no index at all cannot be got wrong by the next person to touch
    // it. The date half is absent for a heading that is only a version.
    let mut parts = rest.splitn(2, char::is_whitespace);
    let version = parts.next()?;
    let date = parts
        .next()
        .unwrap_or_default()
        .trim_start_matches(|c: char| !c.is_alphanumeric())
        .trim_end();
    Some((
        version.to_string(),
        (!date.is_empty()).then(|| date.to_string()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The em dash this repo's changelog separates version from date with,
    /// written as an escape so this file stays clean under
    /// `.claude/rules/no-em-dashes.md`.
    const EM_DASH: char = '\u{2014}';

    #[test]
    fn the_baked_changelog_parses_into_every_section_it_contains() {
        let releases = changelog_releases();
        // Counted independently of the parser, so a parser that silently
        // dropped sections could not agree with itself.
        let headings = CHANGELOG_MD
            .lines()
            .filter(|l| {
                l.starts_with(RELEASE_HEADING_PREFIX)
                    && l[RELEASE_HEADING_PREFIX.len()..].starts_with(|c: char| c.is_ascii_digit())
            })
            .count();
        assert!(headings > 1, "the changelog should carry many releases");
        assert_eq!(releases.len(), headings);
    }

    #[test]
    fn every_baked_release_is_distinct_and_carries_notes() {
        let releases = changelog_releases();
        let mut seen = std::collections::HashSet::new();
        for release in &releases {
            assert!(
                seen.insert(release.version.clone()),
                "duplicate release {}: a heading was mis-split",
                release.version
            );
            assert!(
                !release.notes.trim().is_empty(),
                "release {} parsed with no notes",
                release.version
            );
        }
    }

    /// The panel marks "you are running this" by matching `LUCIDOS_RELEASE`
    /// against a version in this list, so a format drift that stopped them
    /// matching would leave nothing marked and say nothing about why.
    #[test]
    fn the_newest_baked_release_is_the_one_this_binary_reports_running() {
        let releases = changelog_releases();
        assert_eq!(releases[0].version, crate::LUCIDOS_RELEASE);
    }

    #[test]
    fn the_date_is_read_whatever_separates_it_from_the_version() {
        for separator in [format!(" {EM_DASH} "), " \u{2013} ".into(), " - ".into()] {
            let src = format!("## v1.2.3{separator}2026-01-01\n\nbody\n");
            let releases = parse_changelog(&src);
            assert_eq!(releases[0].version, "1.2.3");
            assert_eq!(
                releases[0].date.as_deref(),
                Some("2026-01-01"),
                "separator {separator:?} defeated the date"
            );
        }
    }

    #[test]
    fn a_heading_with_no_date_yields_no_date_rather_than_an_empty_one() {
        let releases = parse_changelog("## v0.7.1\n\nbody\n");
        assert_eq!(releases[0].version, "0.7.1");
        assert_eq!(releases[0].date, None);
    }

    #[test]
    fn notes_stop_at_the_next_release_and_exclude_both_headings() {
        let src =
            "# Changelog\n\n## v2.0.0\n\n### Added\n\n- new thing\n\n## v1.0.0\n\n- old thing\n";
        let releases = parse_changelog(src);
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].notes, "### Added\n\n- new thing");
        assert_eq!(releases[1].notes, "- old thing");
        // The document title precedes every section and belongs to none.
        assert!(!releases[0].notes.contains("# Changelog"));
    }

    #[test]
    fn a_prose_heading_opening_like_a_version_is_body_text() {
        let releases = parse_changelog("## v1.0.0\n\n## various notes\n\n- a thing\n");
        assert_eq!(releases.len(), 1);
        assert!(releases[0].notes.contains("## various notes"));
    }

    #[test]
    fn a_document_with_no_release_headings_yields_no_releases() {
        assert!(parse_changelog("# Changelog\n\nnothing yet\n").is_empty());
        assert!(parse_changelog("").is_empty());
    }
}
