use crate::core::plugins::check_engine_requirement;

#[test]
fn a_dirty_dev_build_counts_as_the_release_it_is_becoming() {
    let m = serde_json::json!({"engine": ">=0.46.2"});
    let clean = crate::engine::release_notices::reported_release("0.46.1", false);
    let dirty = crate::engine::release_notices::reported_release("0.46.1", true);
    assert!(check_engine_requirement(&m, &clean).is_err());
    assert_eq!(check_engine_requirement(&m, &dirty), Ok(()));
}
