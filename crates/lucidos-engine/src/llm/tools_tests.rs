use super::*;

#[test]
fn send_notification_schema_exposes_optional_app_id() {
    let tool = get_notification_tool();
    let props = tool
        .parameters
        .get("properties")
        .expect("schema must have properties");
    let app_id = props
        .get("app_id")
        .expect("send_notification must expose app_id so the LLM can deep-link the push");
    assert_eq!(
        app_id.get("type").and_then(|v| v.as_str()),
        Some("string"),
        "app_id must be a string"
    );
    let required = tool
        .parameters
        .get("required")
        .and_then(|v| v.as_array())
        .expect("schema must have required array");
    let required_names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        !required_names.contains(&"app_id"),
        "app_id must be optional, got required: {:?}",
        required_names
    );
}
