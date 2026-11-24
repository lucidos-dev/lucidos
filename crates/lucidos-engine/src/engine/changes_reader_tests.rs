/// Every surface that serves a pending list reads it through
/// `list_pending_for_readers`. A behavioural test cannot catch a new
/// reader that forgets: it has no flags to assert against, only `false`.
#[test]
fn every_pending_reader_goes_through_the_loader() {
    let readers = [
        (
            "api/changes.rs",
            include_str!("../../../lucidos-agents-api/src/api/changes.rs"),
        ),
        (
            "engine/change_ops_emitters.rs",
            include_str!("change_ops_emitters.rs"),
        ),
        (
            "engine/tools/mod.rs",
            include_str!("../../../lucidos-agents-api/src/engine/tools/mod.rs"),
        ),
    ];
    for (path, source) in readers {
        assert!(
            source.contains("list_pending_for_readers("),
            "{path} serves pending changes without the loader"
        );
    }
    // These two serve nothing but readers, so a bare list is always a
    // list with every flag false. `api/changes.rs` keeps one for Discard
    // All, which filters by the gate itself and serves no list.
    for (path, source) in &readers[1..] {
        assert!(
            !source.contains(".list_pending()"),
            "{path} lists pending changes without their thread state"
        );
    }
}
