//! Per-release upgrade pins: which notices a workspace owes when it moves from
//! one named release to the next. Each test names real releases on purpose,
//! so `scripts/lib/version_sources_test.sh` excludes this file. Production code
//! stays in `release_notices.rs`, where the guard still scans every line.

use super::tests::{v, CLASSIC, TREE};
use super::*;

/// The ids a workspace on `running` answers from `cursor`, in order, each
/// accepted by the same write the endpoint makes.
fn walk(running: &Version, facts: &NoticeFacts, cursor: Option<&str>) -> Vec<String> {
    let notices = parse(RELEASE_NOTICES_TOML).unwrap();
    let mut cursor = cursor.map(str::to_string);
    let mut walked = Vec::new();
    while let Some(next) = owed(&notices, running, facts, cursor.as_deref()) {
        let id = next.id.clone();
        cursor = advanced_cursor(&notices, running, facts, cursor.as_deref(), &id);
        assert_eq!(cursor.as_deref(), Some(id.as_str()));
        walked.push(id);
    }
    walked
}

/// The last notice a 0.48.1 workspace could answer. Notice ids are the
/// stored cursor, so these literals pin that contract.
const ANSWERED_ON_0_48_1: &str = "every-workspace-has-home";

/// What 0.49.0 asks of a workspace that answered everything on 0.48.1.
#[test]
fn a_workspace_upgrading_from_0_48_1_owes_the_0_49_0_notices() {
    let on_0_49_0 = v("0.49.0");
    assert_eq!(
        walk(&on_0_49_0, &CLASSIC, Some(ANSWERED_ON_0_48_1)),
        [
            "mcp-servers-reach-only-their-threads",
            "home-board",
            "move-to-tree-memory",
            "workspace-database-handle"
        ]
    );
    assert_eq!(
        walk(&on_0_49_0, &TREE, Some(ANSWERED_ON_0_48_1)),
        [
            "mcp-servers-reach-only-their-threads",
            "home-board",
            "workspace-database-handle"
        ]
    );
}

/// Why those notices say `since = "0.48.2"` and not 0.49.0: a release-dirty
/// 0.48.1 build reports 0.48.2, so the author reads the real cards first.
#[test]
fn a_dirty_0_48_1_build_already_shows_the_0_49_0_notices() {
    let dirty = reported_release("0.48.1", true).unwrap();
    assert_eq!(
        walk(&dirty, &CLASSIC, Some(ANSWERED_ON_0_48_1)),
        walk(&v("0.49.0"), &CLASSIC, Some(ANSWERED_ON_0_48_1))
    );
}

#[test]
fn a_fresh_0_49_0_workspace_owes_no_notice() {
    let on_0_49_0 = v("0.49.0");
    let notices = parse(RELEASE_NOTICES_TOML).unwrap();
    let stamp = fresh_workspace_stamp(&notices, &on_0_49_0);
    assert!(walk(&on_0_49_0, &CLASSIC, stamp.as_deref()).is_empty());
    assert!(walk(&on_0_49_0, &TREE, stamp.as_deref()).is_empty());
}

/// A workspace that skipped 0.49.0 never reads the Home board notice: the
/// two dashboards notice replaced it, and contradicts it.
#[test]
fn a_workspace_upgrading_from_0_48_1_to_0_49_2_skips_the_home_board() {
    let walked = walk(&v("0.49.2"), &TREE, Some(ANSWERED_ON_0_48_1));
    assert!(!walked.iter().any(|id| id == "home-board"), "{walked:?}");
    assert!(walked.iter().any(|id| id == "two-dashboards"), "{walked:?}");
}

/// A workspace that answered the Home board notice on 0.49.0 keeps its place.
#[test]
fn a_workspace_that_answered_the_home_board_still_owes_what_follows_it() {
    assert_eq!(
        walk(&v("0.49.2"), &TREE, Some("home-board")),
        [
            "workspace-database-handle",
            "two-dashboards",
            "intents-run-in-the-thread"
        ]
    );
}

#[test]
fn the_shipped_home_board_notice_is_superseded_by_two_dashboards() {
    let notices = parse(RELEASE_NOTICES_TOML).unwrap();
    let home_board = notices.iter().find(|n| n.id == "home-board").unwrap();
    assert_eq!(home_board.superseded_by.as_deref(), Some("two-dashboards"));
}
