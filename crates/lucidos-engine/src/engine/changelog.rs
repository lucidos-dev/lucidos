//! INVARIANT: The user-facing changelog, served to the *What's New* panel (Settings >
//! System).
//!
//! **The panel answers "what is NEW", so this cannot answer only "what shipped
//! with me".** The copy baked into the binary tops out at the release that built
//! it. That is older than reality on a dev engine behind its own checkout, and
//! on any install with a newer release published. Both showed the running
//! release as the newest thing in existence.
//!
//! So there are three sources, tried newest-first by [`select_releases`]: the
//! **published** `CHANGELOG.md` on the public mirror's `main`, the local
//! **checkout's** copy, and the **baked** one. One rule picks between them, and
//! it is what makes reaching outward safe. It takes a candidate only when that
//! candidate still contains the release this binary reports running. A checkout
//! on an old branch fails it. So does the HTML a captive portal or a soft-404
//! answers with. Neither can empty the panel.
//!
//! The baked copy is the floor, and is why the panel works offline and on an
//! install with no checkout. It is also why `CHANGELOG.md` stays in
//! `git_ops::restart_detection::files_require_restart`'s engine-bundled-asset
//! list: it is still `include_str!`'d.
//!
//! The fetch runs only while answering a request, never on a timer. That is what
//! keeps `PRIVACY.md`'s promise that the engine does not poll for releases. It
//! is best-effort in every direction: a failure falls through to the next source
//! and logs. One cache in front of it holds the answer, so repeated opens make
//! no repeated requests.
//!
//! One thing is still not here: the notes for a release the *updater* is
//! offering. That release can postdate even the published changelog, so its
//! notes come from the updater manifest and are handled client-side. The
//! manifest's copy is written from this same file by
//! `scripts/lib/release_notes.sh`.

use serde::Serialize;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const CHANGELOG_MD: &str = include_str!("../../../../CHANGELOG.md");

const RELEASE_HEADING_PREFIX: &str = "## v";

const PUBLISHED_CHANGELOG_URL: &str =
    "https://raw.githubusercontent.com/lucidos-dev/lucidos/main/CHANGELOG.md";

const PUBLISHED_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// INVARIANT: Refuse a body larger than this. The real file is a few hundred kilobytes, and
/// nothing downstream bounds what a wrong origin could hand us.
const PUBLISHED_MAX_BYTES: usize = 4 * 1024 * 1024;

const PUBLISHED_CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

const PUBLISHED_FAILURE_CACHE_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChangelogRelease {
    pub version: String,
    pub date: Option<String>,
    pub notes: String,
}

pub async fn changelog_releases() -> Vec<ChangelogRelease> {
    let baked = parse_changelog(CHANGELOG_MD);
    let checkout = checkout_changelog().map(|src| parse_changelog(&src));
    let published = published_releases().await;
    select_releases(published, checkout, baked, crate::build_identity().release)
}

/// INVARIANT: The first candidate that still knows `running`, else `baked`.
///
/// Pure, and the whole of the trust rule. "Still knows the release you are
/// running" is a single check that rejects every way a candidate can be wrong:
/// an old branch's changelog, a truncated download, and a page of HTML from
/// whatever answered instead of the file. `baked` needs no check, since a binary
/// always contains the release it reports.
///
/// A release the chosen copy left undated takes its date from any other copy.
/// The published copy is frozen at its release, so a heading dated on `main`
/// afterwards would otherwise stay blank until the next one. Only a blank is
/// filled, so no copy can overwrite the chosen one's date.
fn select_releases(
    published: Option<Vec<ChangelogRelease>>,
    checkout: Option<Vec<ChangelogRelease>>,
    baked: Vec<ChangelogRelease>,
    running: &str,
) -> Vec<ChangelogRelease> {
    let mut candidates: Vec<Vec<ChangelogRelease>> =
        [published, checkout].into_iter().flatten().collect();
    let trusted = candidates
        .iter()
        .position(|candidate| candidate.iter().any(|r| r.version == running));
    candidates.push(baked);
    let mut chosen = candidates.remove(trusted.unwrap_or(candidates.len() - 1));
    for release in chosen.iter_mut().filter(|r| r.date.is_none()) {
        release.date = candidates
            .iter()
            .flatten()
            .filter(|other| other.version == release.version)
            .find_map(|other| other.date.clone());
    }
    chosen
}

fn checkout_changelog() -> Option<String> {
    let path = crate::paths::repo_root().ok()?.join("CHANGELOG.md");
    std::fs::read_to_string(path).ok()
}

struct CachedPublished {
    at: Instant,
    releases: Option<Vec<ChangelogRelease>>,
}

impl CachedPublished {
    fn is_fresh(&self) -> bool {
        let ttl = if self.releases.is_some() {
            PUBLISHED_CACHE_TTL
        } else {
            PUBLISHED_FAILURE_CACHE_TTL
        };
        self.at.elapsed() < ttl
    }
}

fn published_cache() -> &'static Mutex<Option<CachedPublished>> {
    static CACHE: OnceLock<Mutex<Option<CachedPublished>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// INVARIANT: The published releases, from the cache when it is fresh and from the network
/// otherwise. `None` means the fetch did not produce a usable changelog.
///
/// **The lock covers the fetch, deliberately.** It is what makes a cache miss
/// cost ONE download rather than one per client: a second open arriving mid-fetch
/// waits, then reads what the first stored. Two devices opening the panel
/// together are the ordinary case, so without it the cache would only coalesce
/// requests that never overlapped anyway. A `tokio` mutex, not a `std` one,
/// precisely because the guard is held across an await.
async fn published_releases() -> Option<Vec<ChangelogRelease>> {
    let mut cache = published_cache().lock().await;
    if let Some(cached) = cache.as_ref().filter(|c| c.is_fresh()) {
        return cached.releases.clone();
    }
    let releases = fetch_published_changelog()
        .await
        .map(|src| parse_changelog(&src));
    *cache = Some(CachedPublished {
        at: Instant::now(),
        releases: releases.clone(),
    });
    releases
}

/// INVARIANT: GET the published changelog. `None` on any failure, having logged it.
///
/// Best-effort telemetry's sibling: this enriches a panel that renders
/// completely without it, so it logs a failure rather than surfacing one. The
/// request is a bare GET of a fixed public URL and carries nothing local, which
/// is the claim `PRIVACY.md` makes for it.
async fn fetch_published_changelog() -> Option<String> {
    let client = match reqwest::Client::builder()
        .timeout(PUBLISHED_FETCH_TIMEOUT)
        .user_agent("lucidos")
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            crate::log_legacy!("[Changelog] could not build the HTTP client: {e}");
            return None;
        }
    };
    let mut response = match client.get(PUBLISHED_CHANGELOG_URL).send().await {
        Ok(response) => response,
        Err(e) => {
            crate::log_legacy!("[Changelog] {PUBLISHED_CHANGELOG_URL} unreachable: {e}");
            return None;
        }
    };
    if !response.status().is_success() {
        crate::log_legacy!(
            "[Changelog] {PUBLISHED_CHANGELOG_URL} answered {}",
            response.status()
        );
        return None;
    }
    let mut body: Vec<u8> = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body.len() + chunk.len() > PUBLISHED_MAX_BYTES {
                    crate::log_legacy!(
                        "[Changelog] published changelog is over the size cap; ignored"
                    );
                    return None;
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            // INVARIANT: A truncated body still parses, and its newest releases are intact,
            // so nothing downstream would notice the missing tail. Refuse here.
            Err(e) => {
                crate::log_legacy!("[Changelog] download failed part-way through: {e}");
                return None;
            }
        }
    }
    match String::from_utf8(body) {
        Ok(text) => Some(text),
        Err(_) => {
            crate::log_legacy!("[Changelog] published changelog is not UTF-8; ignored");
            None
        }
    }
}

fn parse_changelog(src: &str) -> Vec<ChangelogRelease> {
    let mut releases: Vec<ChangelogRelease> = Vec::new();
    let mut body = String::new();

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

fn parse_release_heading(line: &str) -> Option<(String, Option<String>)> {
    let rest = line.strip_prefix(RELEASE_HEADING_PREFIX)?;
    if !rest.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
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

    const EM_DASH: char = '\u{2014}';

    fn release(version: &str) -> ChangelogRelease {
        ChangelogRelease {
            version: version.to_string(),
            date: None,
            notes: "body".to_string(),
        }
    }

    fn versions(releases: &[ChangelogRelease]) -> Vec<&str> {
        releases.iter().map(|r| r.version.as_str()).collect()
    }

    #[test]
    fn an_rc_uses_a_changelog_that_knows_its_full_version() {
        let checkout = parse_changelog("## v9.2.0-rc.2\n\n- Local test build\n");
        let chosen = select_releases(
            Some(vec![release("9.1.0")]),
            Some(checkout),
            vec![release("9.2.0-rc.1")],
            "9.2.0-rc.2",
        );
        assert_eq!(versions(&chosen), ["9.2.0-rc.2"]);
        assert_eq!(chosen[0].notes.trim(), "- Local test build");
    }

    #[test]
    fn the_published_changelog_wins_when_it_knows_the_running_release() {
        let chosen = select_releases(
            Some(vec![release("2.0.0"), release("1.0.0")]),
            Some(vec![release("1.5.0"), release("1.0.0")]),
            vec![release("1.0.0")],
            "1.0.0",
        );
        assert_eq!(versions(&chosen), ["2.0.0", "1.0.0"]);
    }

    #[test]
    fn the_checkout_answers_when_there_is_nothing_published() {
        let chosen = select_releases(
            None,
            Some(vec![release("1.5.0"), release("1.0.0")]),
            vec![release("1.0.0")],
            "1.0.0",
        );
        assert_eq!(versions(&chosen), ["1.5.0", "1.0.0"]);
    }

    #[test]
    fn the_baked_copy_answers_when_no_other_source_exists() {
        let chosen = select_releases(None, None, vec![release("1.0.0")], "1.0.0");
        assert_eq!(versions(&chosen), ["1.0.0"]);
    }

    #[test]
    fn a_candidate_that_has_forgotten_the_running_release_is_refused() {
        let chosen = select_releases(
            None,
            Some(vec![release("0.9.0")]),
            vec![release("1.0.0"), release("0.9.0")],
            "1.0.0",
        );
        assert_eq!(versions(&chosen), ["1.0.0", "0.9.0"]);
    }

    fn dated(version: &str, date: &str) -> ChangelogRelease {
        ChangelogRelease {
            date: Some(date.to_string()),
            ..release(version)
        }
    }

    #[test]
    fn a_date_the_chosen_copy_lacks_is_filled_from_another_copy() {
        let chosen = select_releases(
            Some(vec![dated("2.0.0", "2026-02-01"), release("1.0.0")]),
            Some(vec![release("1.0.0")]),
            vec![dated("1.0.0", "2026-01-01")],
            "1.0.0",
        );
        assert_eq!(chosen[0].date.as_deref(), Some("2026-02-01"));
        assert_eq!(chosen[1].date.as_deref(), Some("2026-01-01"));
    }

    #[test]
    fn a_date_the_chosen_copy_has_is_never_overwritten() {
        let chosen = select_releases(
            Some(vec![dated("1.0.0", "2026-01-02")]),
            Some(vec![dated("1.0.0", "2026-01-01")]),
            vec![release("1.0.0")],
            "1.0.0",
        );
        assert_eq!(chosen[0].date.as_deref(), Some("2026-01-02"));
    }

    #[test]
    fn html_from_a_wrong_origin_is_refused_by_the_same_rule() {
        let html = parse_changelog("<!DOCTYPE html>\n<html><body>nope</body></html>\n");
        assert!(html.is_empty());
        let chosen = select_releases(Some(html), None, vec![release("1.0.0")], "1.0.0");
        assert_eq!(versions(&chosen), ["1.0.0"]);
    }

    #[test]
    fn the_baked_changelog_parses_into_every_section_it_contains() {
        let releases = parse_changelog(CHANGELOG_MD);
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
        let releases = parse_changelog(CHANGELOG_MD);
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

    #[test]
    fn the_newest_baked_release_is_the_one_this_binary_reports_running() {
        let releases = parse_changelog(CHANGELOG_MD);
        assert_eq!(releases[0].version, crate::build_identity().release);
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
