use axum::extract::Request;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
pub use lucidos_gateway_stamp::workspace_client::ClientContract;
use lucidos_gateway_stamp::workspace_client::VERSION;

pub enum EngineClientProbe {
    Ready(ClientContract),
    Incompatible(Option<PeerContractMismatch>),
    Starting(String),
    Stopped,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct RemoteStartup {
    pub label: String,
}

pub(crate) const MAX_CLIENT_HEALTH_BYTES: usize = 64 * 1024;

#[derive(serde::Deserialize)]
pub struct InstalledClient {
    version: u32,
    client_version: u32,
    files: std::collections::BTreeMap<String, String>,
}

impl InstalledClient {
    pub fn load(dir: &std::path::Path) -> Option<Self> {
        let bytes = std::fs::read(dir.join("workspace-client-assets.json")).ok()?;
        let manifest: Self = serde_json::from_slice(&bytes).ok()?;
        (manifest.version == 1
            && manifest.client_version == lucidos_gateway_stamp::workspace_client::VERSION
            && manifest.files.contains_key("index.html"))
        .then_some(manifest)
    }

    pub fn contains(&self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    pub fn verifies(&self, path: &str, bytes: &[u8]) -> bool {
        use sha2::{Digest, Sha256};
        self.files
            .get(path)
            .is_some_and(|hash| *hash == format!("{:x}", Sha256::digest(bytes)))
    }
}

pub fn contract() -> ClientContract {
    ClientContract::gateway()
}

pub fn advertised_contract(health: &serde_json::Value) -> Option<ClientContract> {
    health
        .get("workspace_client")
        .and_then(|v| serde_json::from_value::<ClientContract>(v.clone()).ok())
}

pub fn contract_binding(gateway: &ClientContract, engine: &ClientContract) -> String {
    serde_json::to_string(&(gateway, engine)).unwrap_or_default()
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct PeerContractMismatch {
    pub gateway: ClientContract,
    pub engine: Option<ClientContract>,
    pub binding_changed: bool,
}

impl PeerContractMismatch {
    pub fn response(engine: Option<ClientContract>, binding_changed: bool) -> Response {
        (
            StatusCode::UPGRADE_REQUIRED,
            [(header::CACHE_CONTROL, "no-store")],
            axum::Json(Self {
                gateway: contract(),
                engine,
                binding_changed,
            }),
        )
            .into_response()
    }
}

pub struct ContractMismatch<'a> {
    pub slug: &'a str,
    pub remote: &'a str,
    pub gateway: Option<ClientContract>,
    pub engine: Option<ClientContract>,
    pub binding_changed: bool,
}

#[derive(Clone, Copy)]
pub enum MismatchAudience {
    Document,
    Data,
    Public,
}

impl MismatchAudience {
    pub fn for_request(req: &Request, navigation: bool) -> Self {
        let extensions = req.extensions();
        if extensions
            .get::<crate::auth::AuthenticatedDevice>()
            .is_none()
            && extensions
                .get::<crate::auth::AuthenticatedLocalProcess>()
                .is_none()
        {
            return Self::Public;
        }
        if navigation {
            Self::Document
        } else {
            Self::Data
        }
    }
}

impl ContractMismatch<'_> {
    pub fn response(self, audience: MismatchAudience) -> Response {
        if matches!(audience, MismatchAudience::Public) {
            return (
                StatusCode::UPGRADE_REQUIRED,
                [(header::CACHE_CONTROL, "no-store")],
                axum::Json(serde_json::json!({"error": "workspace_client_mismatch"})),
            )
                .into_response();
        }
        let (side, theirs) = match self.gateway.as_ref().filter(|c| c.supports_gateway()) {
            Some(_) => ("remote-engine", self.engine.as_ref()),
            None => ("remote-gateway", self.gateway.as_ref()),
        };
        let behind = if theirs.is_some_and(|c| c.min > VERSION) {
            "this-gateway"
        } else {
            side
        };
        let binding_changed = self.binding_changed
            && self
                .gateway
                .as_ref()
                .is_some_and(ClientContract::supports_gateway)
            && self
                .engine
                .as_ref()
                .is_some_and(ClientContract::supports_engine);
        let theirs = theirs.map(|c| format!("{}-{}", c.min, c.max));
        let incompatible = format!("{}@{}", self.slug, self.remote);
        if matches!(audience, MismatchAudience::Document) {
            let mut target = reqwest::Url::parse("http://localhost/~/").expect("static picker URL");
            target.set_query(Some("pick"));
            target
                .query_pairs_mut()
                .append_pair("incompatible", &incompatible)
                .append_pair("behind", behind)
                .append_pair("ours", &VERSION.to_string())
                .append_pair("theirs", theirs.as_deref().unwrap_or(""))
                .append_pair("release", crate::LUCIDOS_RELEASE);
            if binding_changed {
                target.query_pairs_mut().append_pair("binding-changed", "1");
            }
            let location = format!("{}?{}", target.path(), target.query().unwrap_or("pick"));
            return (
                StatusCode::SEE_OTHER,
                [
                    (header::LOCATION, location.as_str()),
                    (header::CACHE_CONTROL, "no-store"),
                ],
            )
                .into_response();
        }
        (
            StatusCode::UPGRADE_REQUIRED,
            [(header::CACHE_CONTROL, "no-store")],
            axum::Json(serde_json::json!({
                "error": "workspace_client_mismatch",
                "incompatible": incompatible,
                "behind": behind,
                "ours": VERSION,
                "theirs": theirs,
                "release": crate::LUCIDOS_RELEASE,
                "binding_changed": binding_changed,
            })),
        )
            .into_response()
    }
}

pub fn preview_unavailable(peer: Option<&str>, needs_trust: bool) -> Response {
    // Remote names are validated, but escape here so this page remains inert
    // if the registry ever admits display labels.
    let peer = peer
        .unwrap_or("the remote gateway")
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace(char::from(39), "&#39;");
    let reason = if needs_trust {
        "Enable trusted pairing for this gateway in Settings → Access → Remote gateways to open its frontend preview."
    } else {
        "Please re-pair or update the remote gateway, and check that its frontend preview is running."
    };
    let status = if needs_trust {
        StatusCode::FORBIDDEN
    } else {
        StatusCode::NOT_FOUND
    };
    (
        status,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8"),
         (header::CACHE_CONTROL, "no-store"),
         (header::CONTENT_SECURITY_POLICY, "default-src 'none'; frame-ancestors 'none'")],
        format!(r#"<!doctype html><html lang="en"><meta charset="utf-8"><title>Frontend preview unavailable</title><h1>Frontend preview unavailable</h1><p>Cannot open the frontend preview from {peer}.</p><p>{reason}</p></html>"#),
    ).into_response()
}

pub fn is_data_path(path: &str) -> bool {
    ["/api/v1/", "/~cap/", "/data/", "/app/"]
        .iter()
        .any(|prefix| path.starts_with(prefix))
}

pub fn foreign_private_request(req: &Request) -> bool {
    let headers = req.headers();
    let shell_path = req
        .uri()
        .path()
        .strip_prefix('/')
        .and_then(|path| path.split_once('/'));
    if req.method() == axum::http::Method::GET
        && headers
            .get("sec-fetch-mode")
            .is_some_and(|value| value == "navigate")
        && headers
            .get("sec-fetch-dest")
            .is_some_and(|value| value == "document")
        && shell_path.is_some_and(|(workspace, rest)| {
            (rest.is_empty() || rest == "index.html")
                && (crate::registry::is_valid_id(workspace)
                    || crate::relay::remote_route(workspace).is_some())
        })
    {
        return false;
    }
    if let Some(site) = headers.get("sec-fetch-site") {
        let opaque = headers
            .get(header::ORIGIN)
            .is_some_and(|origin| origin == "null");
        return opaque
            || !site
                .to_str()
                .is_ok_and(|v| matches!(v, "same-origin" | "none"));
    }
    if headers.contains_key(header::ORIGIN) {
        let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
        let host = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .or_else(|| req.uri().authority().map(|authority| authority.as_str()));
        return !matches!((origin, host), (Some(origin), Some(host))
            if crate::proxy::origin_authority_matches_host(origin, host));
    }
    false
}

pub fn scoped_location(location: &str, request: &str, route: &str) -> Option<String> {
    if location.contains('\\') {
        return None;
    }
    let origin = reqwest::Url::parse("https://relay.invalid").ok()?;
    let base = origin.join(request).ok()?;
    let target = base.join(location).ok()?;
    if target.origin() != origin.origin()
        || !target.username().is_empty()
        || target.password().is_some()
        || !target.path().strip_prefix(route).is_some_and(is_data_path)
    {
        return None;
    }
    Some(format!(
        "{}{}{}",
        target.path(),
        target.query().map(|q| format!("?{q}")).unwrap_or_default(),
        target
            .fragment()
            .map(|f| format!("#{f}"))
            .unwrap_or_default()
    ))
}

pub fn data_response(
    response: reqwest::Response,
    request: &str,
    route: &str,
    cors_resource: bool,
) -> Response {
    let location = match response.headers().get(header::LOCATION) {
        None => None,
        Some(value) => match value
            .to_str()
            .ok()
            .and_then(|v| scoped_location(v, request, route))
        {
            Some(location) => Some(location),
            None => {
                return (
                    StatusCode::BAD_GATEWAY,
                    "The remote workspace returned an unsafe redirect",
                )
                    .into_response()
            }
        },
    };
    let mut out = crate::relay::streamed_response(response);
    let headers = out.headers_mut();
    let remote_csp: Vec<HeaderValue> = headers
        .get_all(header::CONTENT_SECURITY_POLICY)
        .iter()
        .cloned()
        .collect();
    let remove: Vec<_> = headers
        .keys()
        .filter(|name| {
            !matches!(
                name.as_str(),
                "content-type"
                    | "content-length"
                    | "content-encoding"
                    | "content-disposition"
                    | "content-range"
                    | "accept-ranges"
                    | "etag"
                    | "last-modified"
                    | "location"
                    | "retry-after"
                    | "x-lucidos-workspace-state"
                    | "x-lucidos-boot-splash"
            )
        })
        .cloned()
        .collect();
    for name in remove {
        headers.remove(name);
    }
    if cors_resource {
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("*"),
        );
    }
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static(
            "sandbox allow-scripts allow-forms allow-downloads allow-modals allow-popups; object-src 'none'",
        ),
    );
    for policy in remote_csp {
        headers.append(header::CONTENT_SECURITY_POLICY, policy);
    }
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Some(location) = location {
        match HeaderValue::from_str(&location) {
            Ok(value) => {
                headers.insert(header::LOCATION, value);
            }
            Err(_) => return StatusCode::BAD_GATEWAY.into_response(),
        }
    }
    out
}

pub fn worker_request(headers: &HeaderMap) -> bool {
    headers.contains_key("service-worker")
        || headers
            .get("sec-fetch-dest")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| matches!(v, "serviceworker" | "sharedworker" | "worker"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_inventory_requires_a_supported_client_generation() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("workspace-client-assets.json");
        assert!(InstalledClient::load(dir.path()).is_none());
        for value in [
            serde_json::json!({"version": 2, "client_version": lucidos_gateway_stamp::workspace_client::VERSION, "files": {"index.html": "hash"}}),
            serde_json::json!({"version": 1, "client_version": lucidos_gateway_stamp::workspace_client::VERSION + 1, "files": {"index.html": "hash"}}),
            serde_json::json!({"version": 1, "client_version": lucidos_gateway_stamp::workspace_client::VERSION, "files": {}}),
        ] {
            std::fs::write(&file, serde_json::to_vec(&value).unwrap()).unwrap();
            assert!(InstalledClient::load(dir.path()).is_none());
        }
    }

    #[test]
    fn contract_negotiates_versions_and_capabilities_not_builds() {
        let mut health =
            serde_json::json!({"workspace_client": contract(), "build": "different-build"});
        assert!(advertised_contract(&health)
            .filter(ClientContract::supports_gateway)
            .is_some());
        let version = lucidos_gateway_stamp::workspace_client::VERSION;
        health["workspace_client"]["min"] = 0.into();
        health["workspace_client"]["max"] = (version + 1).into();
        assert!(advertised_contract(&health)
            .filter(ClientContract::supports_gateway)
            .is_some());
        health["workspace_client"]["min"] = (version + 1).into();
        assert!(advertised_contract(&health)
            .filter(ClientContract::supports_gateway)
            .is_none());
        health["workspace_client"]["min"] = version.into();
        health["workspace_client"]["capabilities"] = serde_json::json!([]);
        assert!(advertised_contract(&health)
            .filter(ClientContract::supports_gateway)
            .is_none());
        assert!(advertised_contract(&serde_json::json!({})).is_none());
    }

    #[tokio::test]
    async fn gateway_mismatch_identifies_the_older_side_for_documents_and_data() {
        let older = ClientContract {
            min: VERSION - 1,
            max: VERSION - 1,
            ..contract()
        };
        let newer = ClientContract {
            min: VERSION + 1,
            max: VERSION + 2,
            ..contract()
        };
        let missing_capability = ClientContract {
            capabilities: vec![],
            ..contract()
        };
        for (gateway, behind) in [
            (Some(older), "remote-gateway"),
            (Some(newer), "this-gateway"),
            (Some(missing_capability), "remote-gateway"),
            (None, "remote-gateway"),
        ] {
            let mismatch = || ContractMismatch {
                slug: "dev",
                remote: "cloud",
                gateway: gateway.clone(),
                engine: None,
                binding_changed: false,
            };
            let data = mismatch().response(MismatchAudience::Data);
            assert_eq!(data.status(), StatusCode::UPGRADE_REQUIRED);
            let bytes = axum::body::to_bytes(data.into_body(), 4096).await.unwrap();
            let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["error"], "workspace_client_mismatch");
            assert_eq!(body["incompatible"], "dev@cloud");
            assert_eq!(body["behind"], behind);
            assert_eq!(body["ours"], VERSION);
            assert_eq!(body["release"], crate::LUCIDOS_RELEASE);
            let theirs = gateway.as_ref().map(|c| format!("{}-{}", c.min, c.max));
            assert_eq!(body["theirs"], serde_json::json!(theirs));

            let document = mismatch().response(MismatchAudience::Document);
            assert_eq!(document.status(), StatusCode::SEE_OTHER);
            let url = reqwest::Url::parse(&format!(
                "https://local.test{}",
                document.headers()[header::LOCATION].to_str().unwrap()
            ))
            .unwrap();
            assert_eq!(url.path(), "/~/");
            let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
            assert!(query.contains_key("pick"));
            assert_eq!(query["incompatible"], "dev@cloud");
            assert_eq!(query["behind"], behind);
            assert_eq!(query["theirs"], theirs.as_deref().unwrap_or(""));
        }
    }

    #[test]
    fn redirects_resolve_only_within_selected_data_scope() {
        let route = "/demo@remote";
        let request = "/demo@remote/api/v1/files/redirect";
        for target in ["next?q=1#part", "/demo@remote/api/v1/files/next?q=1#part"] {
            assert_eq!(
                scoped_location(target, request, route).as_deref(),
                Some("/demo@remote/api/v1/files/next?q=1#part")
            );
        }
        assert_eq!(
            scoped_location("../document.html", request, route).as_deref(),
            Some("/demo@remote/api/v1/document.html")
        );
        for target in [
            "/~/api/v1/control/workspaces",
            "//evil.test/demo@remote/api/v1/x",
            "https://evil.test/demo@remote/api/v1/x",
            "../../api/v1/x",
            "/demo@remote/%2e%2e/~/api/v1/x",
            "/demo@remote/assets/x.js",
            r"/demo@remote/..\api/v1/x",
        ] {
            assert!(
                scoped_location(target, request, route).is_none(),
                "{target}"
            );
        }
    }

    #[test]
    fn private_browser_sources_fail_closed_before_authentication() {
        for (site, origin, refused) in [
            ("same-origin", "https://gateway.test", false),
            ("same-origin", "https://public.example", false),
            ("same-origin", "null", true),
            ("cross-site", "https://evil.test", true),
            ("same-site", "https://sibling.gateway.test", true),
            ("", "http://gateway.test", true),
            ("", "https://gateway.test", false),
            ("none", "", false),
        ] {
            let mut request = Request::builder()
                .uri("/~/api/v1/control/workspaces")
                .header(header::HOST, "gateway.test");
            if !site.is_empty() {
                request = request.header("sec-fetch-site", site);
            }
            if !origin.is_empty() {
                request = request.header(header::ORIGIN, origin);
            }
            assert_eq!(
                foreign_private_request(&request.body(axum::body::Body::empty()).unwrap()),
                refused,
                "{site} {origin}"
            );
        }
    }
}
