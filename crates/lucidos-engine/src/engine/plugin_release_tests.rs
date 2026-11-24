use crate::core::plugins::check_engine_requirement;

/// A dirty build of RELEASE 0.46.1 is the dev build of 0.46.2, so it meets a
/// 0.46.2 floor. `reported_release` is the rule `running_release` applies.
#[test]
fn a_dirty_dev_build_counts_as_the_release_it_is_becoming() {
    let m = serde_json::json!({"engine": ">=0.46.2"});
    let clean = crate::engine::release_notices::reported_release("0.46.1", false);
    let dirty = crate::engine::release_notices::reported_release("0.46.1", true);
    assert!(check_engine_requirement(&m, &clean).is_err());
    assert_eq!(check_engine_requirement(&m, &dirty), Ok(()));
}
