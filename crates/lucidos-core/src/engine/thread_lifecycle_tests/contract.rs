use super::codegen::all_statuses;
use super::*;

#[test]
fn every_status_sql_literal_quotes_its_wire_text() {
    for status in ThreadStatus::ALL {
        assert_eq!(status.sql_literal(), format!("'{}'", status.as_str()));
    }
}

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
