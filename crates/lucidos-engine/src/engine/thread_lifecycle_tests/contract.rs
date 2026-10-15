use super::codegen::all_statuses;
use super::*;

#[test]
fn every_status_sql_literal_quotes_its_wire_text() {
    for status in ThreadStatus::ALL {
        assert_eq!(status.sql_literal(), format!("'{}'", status.as_str()));
    }
}

/// [`ThreadStatus::ALL`] drives the fixture's status dimension, the generated
/// TS union, the status filter's accepted values, the CLI help and the LLM tool
/// schema, so an entry that is wrong shrinks or corrupts all five at once. Every
/// entry must round-trip through `as_str` / `parse` / `try_parse`, and no two
/// may share a wire string.
///
/// What this test does NOT catch, stated plainly because a guard nobody has is
/// less dangerous than one they think they have: a `ThreadStatus` variant added
/// to the enum and never added to `ALL`. Nothing here can see it, because every
/// enumeration available to a test is `ALL` itself. The two forcing functions
/// are elsewhere and both are the compiler, not this file. Widening `ALL`
/// without widening `all_statuses`'s return type is a type error, and adding a
/// variant at all is a non-exhaustive-match error in `ThreadStatus::as_str`,
/// which is where the instruction to update `ALL` lives. The count assertion
/// this test used to carry looked like a third guard and was not one: `.len()`
/// on a `[Self; 6]` is a compile-time constant, so it could never fail.
#[test]
fn all_statuses_covers_the_enum() {
    let mut seen: Vec<&str> = Vec::new();
    for (wire, status) in all_statuses() {
        assert_eq!(
            status.as_str(),
            wire,
            "ThreadStatus::ALL wire string disagrees with ThreadStatus::as_str",
        );
        assert_eq!(
            ThreadStatus::parse(wire),
            status,
            "ThreadStatus::parse does not round-trip '{wire}'; a status missing \
             from `parse` falls back to Idle and the projection would silently \
             read the wrong state off the column",
        );
        assert_eq!(
            ThreadStatus::try_parse(wire),
            Some(status),
            "ThreadStatus::try_parse does not round-trip '{wire}'; a status it \
             rejects cannot be named by the `status` filter on threads \
             list / count",
        );
        assert!(
            !seen.contains(&wire),
            "'{wire}' appears twice in ThreadStatus::ALL; a duplicate widens the \
             fixture's status dimension with a redundant case and makes the \
             generated TS union and the tool schema enum list it twice",
        );
        seen.push(wire);
    }
}
