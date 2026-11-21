use super::*;
use crate::llm::tool_names as tn;

/// The description names every builtin, so a row added to the catalog
/// cannot stay invisible to the agent.
#[test]
fn proxy_request_names_every_builtin_provider_proxy() {
    let tools = proxy_tools();
    let proxy_request = tools
        .iter()
        .find(|t| t.name == tn::PROXY_REQUEST)
        .expect("proxy_request is defined");
    for proxy in &crate::api::proxy_builtin::BUILTIN_PROXIES {
        assert!(
            proxy_request.description.contains(proxy.name),
            "proxy_request must name the builtin '{}'",
            proxy.name
        );
    }
}
