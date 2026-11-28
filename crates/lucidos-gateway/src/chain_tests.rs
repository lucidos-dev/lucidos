//! An app frame's own files, through a real gateway, to a real engine.
//!
//! This is the removal condition of the "Inline your own CSS and JS" entry in
//! `docs/temporary-measures.md`, written as a test: an app that ships a
//! separate `style.css`, opened through a gateway URL, answers 200.

use std::path::{Path, PathBuf};

use axum::http::StatusCode;

use crate::registry::Workspace;
use crate::server::{gateway_router, GatewayState};

const APP_ID: &str = "capability-probe";
const STYLE: &str = ".probe{color:rebeccapurple}\n";
const NESTED: &str = "<!doctype html><title>probe</title><a href=\"./next/index.html\">on</a>";
const HOP: &str = "<!doctype html><title>one hop down</title>";

struct LiveEngine {
    workspace: PathBuf,
    port: u16,
    tls: bool,
}

impl LiveEngine {
    fn from_env() -> Self {
        LiveEngine {
            workspace: PathBuf::from(require("E2E_WORKSPACE")),
            port: require("VITE_PORT")
                .parse()
                .expect("VITE_PORT is the e2e engine's port"),
            tls: require("PROTO") == "https",
        }
    }
}

fn require(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!("{name} is not set. Run this through ./scripts/e2e-api.sh, which starts an engine.")
    })
}

fn stamped_device_headers(device: &str) -> reqwest::header::HeaderMap {
    stamped_device_headers_in(Path::new(&require("E2E_WORKSPACE")), device)
}

fn stamped_device_headers_in(workspace: &Path, device: &str) -> reqwest::header::HeaderMap {
    let text = std::fs::read_to_string(workspace.join(".lucidos/e2e-stamp-key"))
        .expect("the harness writes .lucidos/e2e-stamp-key");
    let mut lines = text.lines();
    let key = lucidos_gateway_stamp::StampKey::read_handover(
        lines.next().expect("a handover key line").as_bytes(),
    )
    .expect("a valid handover key");
    let workspace_id = lines.next().expect("a workspace id line");
    let stamp = key.sign_now(lucidos_gateway_stamp::Claim::Device {
        workspace_id,
        device_id: device,
    });
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "x-lucidos-device-id",
        reqwest::header::HeaderValue::from_str(device).expect("a header-safe device id"),
    );
    headers.insert(
        lucidos_gateway_stamp::HEADER_GATEWAY_STAMP,
        reqwest::header::HeaderValue::from_str(&stamp).expect("a header-safe stamp"),
    );
    headers
}

#[test]
fn stamped_device_headers_carry_a_stamp_the_key_verifies() {
    let key = lucidos_gateway_stamp::StampKey::generate().expect("a fresh key");
    let workspace = tempfile::tempdir().expect("temp workspace");
    std::fs::create_dir(workspace.path().join(".lucidos")).expect("state dir");
    std::fs::write(
        workspace.path().join(".lucidos/e2e-stamp-key"),
        format!("{}e2e-test\n", key.handover_line()),
    )
    .expect("write the key file");

    let headers = stamped_device_headers_in(workspace.path(), "chain-device");

    assert_eq!(headers["x-lucidos-device-id"], "chain-device");
    let stamp = headers[lucidos_gateway_stamp::HEADER_GATEWAY_STAMP]
        .to_str()
        .expect("stamp text");
    assert!(key.verify_now(
        stamp,
        lucidos_gateway_stamp::Claim::Device {
            workspace_id: "e2e-test",
            device_id: "chain-device",
        }
    ));
}

fn write_fixture(workspace: &Path) {
    let app = workspace.join("data/apps").join(APP_ID);
    std::fs::create_dir_all(&app).expect("the e2e workspace is writable");
    std::fs::write(
        app.join("index.html"),
        format!(
            "<!doctype html><html><head><link rel=\"stylesheet\" href=\"style.css\"></head>\
             <body><img src=\"/data/artifacts/{APP_ID}/pixel.txt\"></body></html>"
        ),
    )
    .expect("writing the probe app");
    std::fs::write(app.join("style.css"), STYLE).expect("writing the probe stylesheet");
    std::fs::write(app.join("own.woff2"), "wOF2 stand-in").expect("writing the probe font");
    std::fs::write(app.join("main.js"), "export const ok = 1;\n").expect("writing the module");

    let artifacts = workspace.join("data/artifacts").join(APP_ID);
    std::fs::create_dir_all(artifacts.join("next")).expect("the artifacts tree is writable");
    std::fs::write(artifacts.join("index.html"), NESTED).expect("writing the nested document");
    std::fs::write(artifacts.join("next/index.html"), HOP).expect("writing the hop target");
    std::fs::write(artifacts.join("pixel.txt"), "px").expect("writing the image stand-in");
}

async fn serve_gateway(engine: &LiveEngine, slug: &str, token: &str) -> String {
    let state = GatewayState::for_tests_with(None, token).with_live_engine_sockets();
    state.register_for_test(Workspace {
        id: slug.to_string(),
        name: slug.to_string(),
        dir: engine
            .workspace
            .to_str()
            .expect("workspace path")
            .to_string(),
        port: engine.port,
        database_url: None,
        cleanup: Default::default(),
        autostart: false,
    });
    assert!(
        state.engine_address(slug).is_none(),
        "registration alone cannot route"
    );
    state.adopt_running_engines_for_test().await;
    let socket = lucidos_engine_socket::path_for(&engine.workspace)
        .expect("the e2e workspace has an engine socket");
    assert_eq!(
        state.engine_address(slug),
        Some(crate::engine_hop::EngineAddress::at(socket, engine.port)),
        "the real adoption probe must install the registered engine socket",
    );

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a free port for the gateway");
    let base = format!("http://{}", listener.local_addr().expect("a bound address"));
    let router = gateway_router(state);
    tokio::spawn(crate::server::serve_listener(
        listener,
        router,
        axum_server::Handle::new(),
        lucidos_gateway_tls::ListenerTls::Plain,
    ));
    base
}

fn client() -> reqwest::Client {
    lucidos_local_token::client::client_builder()
        .build()
        .expect("a reqwest client")
}

fn base_href(html: &str) -> String {
    let at = html
        .find("<base href=\"")
        .unwrap_or_else(|| panic!("the engine stamped no capability base:\n{html}"));
    let rest = &html[at + "<base href=\"".len()..];
    rest[..rest.find('"').expect("a closed attribute")].to_string()
}

fn resolve(base: &str, href: &str) -> String {
    let dir = &base[..=base.rfind('/').expect("a directory-shaped base")];
    format!("{dir}{}", href.trim_start_matches("./"))
}

async fn get(client: &reqwest::Client, url: &str) -> (StatusCode, String) {
    let res = client
        .get(url)
        .header("sec-fetch-dest", "empty")
        .send()
        .await
        .unwrap_or_else(|e| panic!("GET {url} failed to send: {e}"));
    let status = res.status();
    (status, res.text().await.unwrap_or_default())
}

#[tokio::test]
#[ignore = "needs a live engine; run through ./scripts/e2e-api.sh"]
async fn an_app_frames_own_files_load_through_a_gateway() {
    let engine = LiveEngine::from_env();
    write_fixture(&engine.workspace);
    let token = lucidos_local_token::read()
        .expect("the machine-local token, which the engine derived its key from");
    let slug = "e2e-test";
    let gateway = serve_gateway(&engine, slug, &token).await;
    let client = client();

    if std::env::var("LUCIDOS_E2E_GATEWAY").as_deref() == Ok("own") {
        gateway_restore_origin_from_spawned_child(&engine, &gateway, &token).await;
    }

    let document = client
        .get(format!("{gateway}/{slug}/app/{APP_ID}/"))
        .header(crate::auth::HEADER_LOCAL_TOKEN, &token)
        .header("sec-fetch-dest", "iframe")
        .send()
        .await
        .expect("the app document");
    assert_eq!(document.status(), StatusCode::OK);
    let html = document.text().await.expect("the document body");
    let base = base_href(&html);
    assert!(
        base.starts_with(&format!("/{slug}/~cap/")) && base.ends_with(&format!("/app/{APP_ID}/")),
        "the base must carry a pass for this app: {base}"
    );

    let (status, body) = get(
        &client,
        &format!("{gateway}{}", resolve(&base, "style.css")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the app's own stylesheet: {body}");
    assert_eq!(body, STYLE);

    let pass = base
        .strip_prefix(&format!("/{slug}/"))
        .and_then(|rest| rest.split('/').nth(1))
        .expect("a pass inside the base");
    let carrier = format!("{gateway}/{slug}/~cap/{pass}");
    let (status, _) = get(
        &client,
        &format!("{carrier}/data/artifacts/{APP_ID}/pixel.txt"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "a root-absolute workspace ref");

    let nested = format!("{carrier}/data/artifacts/{APP_ID}/index.html");
    let (status, body) = get(&client, &nested).await;
    assert_eq!(status, StatusCode::OK, "the previewed artifact: {body}");
    assert_eq!(body, NESTED);

    let (status, body) = get(&client, &resolve(&nested, "./next/index.html")).await;
    assert_eq!(status, StatusCode::OK, "a click inside the preview: {body}");
    assert_eq!(body, HOP);

    let (status, _) = get(&client, &format!("{gateway}/{slug}/app/{APP_ID}/style.css")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "no pass, no file");

    for path in [
        "/api/v1/credentials",
        "/api/v1/threads/list",
        "/api/v1/data",
    ] {
        let (status, _) = get(&client, &format!("{carrier}{path}")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path} must stay gated");
    }

    let own_font = format!("{gateway}{}", resolve(&base, "own.woff2"));
    let font = cross_origin(&client, reqwest::Method::GET, &own_font, "font").await;
    assert_eq!(font.status(), StatusCode::OK, "the app's own font");
    assert_eq!(
        allow_origin(&font),
        Some("*"),
        "the grant must cross the gateway"
    );
    let read = cross_origin(&client, reqwest::Method::GET, &own_font, "empty").await;
    assert_eq!(allow_origin(&read), None, "a fetch() is never granted");

    let module = format!("{gateway}{}", resolve(&base, "main.js"));
    let script = cross_origin(&client, reqwest::Method::GET, &module, "script").await;
    assert_eq!(script.status(), StatusCode::OK, "the app's own module");
    assert_eq!(
        allow_origin(&script),
        Some("*"),
        "a module behind the gateway"
    );

    let fira = format!("{gateway}/{slug}/api/v1/fonts/fira-code-6.2.woff2");
    let font = cross_origin(&client, reqwest::Method::GET, &fira, "font").await;
    assert_eq!(font.status(), StatusCode::OK, "Fira Code, cookieless");
    assert_eq!(allow_origin(&font), Some("*"), "Fira Code's grant");
    let preflight = cross_origin(&client, reqwest::Method::OPTIONS, &fira, "font").await;
    assert!(preflight.status().is_success(), "{}", preflight.status());
    assert_eq!(allow_origin(&preflight), Some("*"), "the preflight's grant");

    for path in [
        format!("apps/{APP_ID}/index.html"),
        format!("apps/{APP_ID}/style.css"),
        format!("apps/{APP_ID}/own.woff2"),
        format!("apps/{APP_ID}/main.js"),
        format!("artifacts/{APP_ID}/index.html"),
        format!("artifacts/{APP_ID}/next/index.html"),
        format!("artifacts/{APP_ID}/pixel.txt"),
    ] {
        let removed = client
            .delete(format!("{gateway}/{slug}/api/v1/data/{path}"))
            .header("x-lucidos-device-id", "gateway-origin-chain")
            .header(crate::auth::HEADER_LOCAL_TOKEN, &token)
            .send()
            .await
            .expect("remove the disposable chain fixture");
        assert!(removed.status().is_success());
    }
}

async fn cross_origin(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    dest: &str,
) -> reqwest::Response {
    client
        .request(method, url)
        .header("origin", "null")
        .header("sec-fetch-site", "cross-site")
        .header("sec-fetch-dest", dest)
        .header("access-control-request-method", "GET")
        .send()
        .await
        .unwrap_or_else(|e| panic!("{url} failed to send: {e}"))
}

fn allow_origin(res: &reqwest::Response) -> Option<&str> {
    res.headers()
        .get("access-control-allow-origin")
        .and_then(|v| v.to_str().ok())
}

async fn gateway_restore_origin_from_spawned_child(
    engine: &LiveEngine,
    gateway: &str,
    token: &str,
) {
    let base = format!(
        "{}://127.0.0.1:{}",
        if engine.tls { "https" } else { "http" },
        engine.port
    );
    let client = client();
    let device = "gateway-origin-chain";
    let registered = client
        .post(format!("{base}/api/v1/devices/register"))
        .header("content-type", "application/json")
        .body(serde_json::json!({ "device_id": device, "user_agent": "lucidos-e2e/1" }).to_string())
        .send()
        .await
        .expect("register the disposable device");
    assert!(registered.status().is_success());
    let prefs = client
        .get(format!("{base}/api/v1/preferences"))
        .header(crate::auth::HEADER_LOCAL_TOKEN, token)
        .send()
        .await
        .expect("read preferences")
        .text()
        .await
        .expect("preferences body");
    let prefs: serde_json::Value = serde_json::from_str(&prefs).expect("preferences JSON");
    let key = crate::preference_test_keys::key("CODING_AGENT_CLAUDE_PATH");
    let old = prefs["preferences"][key].as_str();
    let fake = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../lucidos-app/e2e/fake-claude-code/claude.mjs")
        .canonicalize()
        .expect("fake agent fixture");
    let changed = client
        .put(format!("{base}/api/v1/preferences?key={key}"))
        .headers(stamped_device_headers(device))
        .header("content-type", "application/json")
        .body(serde_json::json!({ "value": fake.to_str().expect("fixture path") }).to_string())
        .send()
        .await
        .expect("configure the disposable agent");
    assert!(changed.status().is_success());

    let result = async {
        let thread = "bce23d74-18a8-4652-aef0-168258160821";
        let submitted = client.post(format!("{base}/api/v1/chat/stream"))
            .headers(stamped_device_headers(device))
            .header("content-type", "application/json").body(serde_json::json!({
                "message": format!("Probe snapshot gateway restore origin: {gateway}."),
                "thread_id": thread, "new_thread": true, "mode": "human", "use_coding_agent": true,
                "coding_agent": "claude-code",
            }).to_string())
            .send().await.map_err(|_| "submit child")?;
        if !submitted.status().is_success() { return Err("child submission refused"); }
        for _ in 0..120 {
            let snapshot = client.get(format!("{base}/api/v1/threads/{thread}/events"))
                .header(crate::auth::HEADER_LOCAL_TOKEN, token)
                .send().await.map_err(|_| "read child events")?
                .text().await.map_err(|_| "child events body")?;
            let snapshot: serde_json::Value = serde_json::from_str(&snapshot).map_err(|_| "child events JSON")?;
            if let Some(events) = snapshot["events"].as_array() {
                for event in events {
                    if event["event_type"] == "ResponseGenerated" {
                        let content = event["payload"]["text"].as_str().ok_or("child answer missing")?;
                        let proof: serde_json::Value = serde_json::from_str(content).map_err(|_| "child answer JSON")?;
                        return if proof == serde_json::json!({ "verified": true, "workspace_id": "e2e-test", "source_thread_id": thread }) {
                            Ok(())
                        } else {
                            crate::diagnostic_log::probe(format_args!("child proof: {proof}"));
                            Err("gateway did not verify the engine-spawned child")
                        };
                    }
                    if event["event_type"] == "ResponseFailed" { return Err("child failed"); }
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        Err("child proof timed out")
    }.await;
    let restore = match old {
        Some(value) => client
            .put(format!("{base}/api/v1/preferences?key={key}"))
            .header("content-type", "application/json")
            .body(serde_json::json!({ "value": value }).to_string()),
        None => client.delete(format!("{base}/api/v1/preferences?key={key}")),
    }
    .headers(stamped_device_headers(device))
    .send()
    .await
    .expect("restore disposable preference");
    assert!(restore.status().is_success());
    assert_eq!(result, Ok(()));
}
