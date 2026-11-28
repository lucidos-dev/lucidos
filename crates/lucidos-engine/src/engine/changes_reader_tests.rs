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
    for (path, source) in &readers[1..] {
        assert!(
            !source.contains(".list_pending()"),
            "{path} lists pending changes without their thread state"
        );
    }
}
