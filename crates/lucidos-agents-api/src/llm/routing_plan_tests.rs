/// Apps never reach the plan: its route has no proxy.
#[test]
fn the_plan_route_has_no_app_proxy() {
    assert!(crate::api::proxy_builtin::model_proxy_info("chatgpt-plan", "gpt-6.1-sol").is_none());
}
