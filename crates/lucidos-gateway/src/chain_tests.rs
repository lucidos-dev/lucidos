//! An app frame's own files, through a real gateway, to a real engine.
//!
//! This is the removal condition of the "Inline your own CSS and JS" entry in
//! `docs/temporary-measures.md`, written as a test: an app that ships a
//! separate `style.css`, opened through a gateway URL, answers 200.

//! # Why it is `#[ignore]`d rather than part of `cargo test`
//!
//! It needs a live engine, and `make test` has none. `./scripts/e2e-api.sh`
//! runs it by name with the e2e session's port in the environment. Same
//! contract as the gated embedder tests: named in one script, never silently
//! skipped, never run against a machine that cannot serve it.

//! # What only this can catch
//!
//! The engine mints and the gateway verifies, and the two never meet in any
//! other test. Both derive their key from the machine-local token, and both
//! reason about the slug. A disagreement about either would break every app in
//! every workspace while every unit test stayed green. That is the shape of the
//! half-landed fix this whole design exists to close (ADR 0238).

//! It also drives the browser's own rule, which is the heart of the design: a
//! nested document's relative link resolves against ITS url, so a path-borne
//! pass survives one hop down with nothing rewritten. The test resolves the
//! link the way a browser would and asks for the result.

use std::path::{Path, PathBuf};

use axum::http::StatusCode;

use crate::registry::Workspace;
use crate::server::{gateway_router, GatewayState};

/// The app this test writes into the e2e workspace and then loads.
const APP_ID: &str = "capability-probe";
const STYLE: &str = ".probe{color:rebeccapurple}\n";
const NESTED: &str = "<!doctype html><title>probe</title><a href=\"./next/index.html\">on</a>";
const HOP: &str = "<!doctype html><title>one hop down</title>";

/// The e2e session's engine, from the environment `scripts/e2e-api.sh` exports.
struct LiveEngine {
    workspace: PathBuf,
    port: u16,
    tls: bool,
}

impl LiveEngine {
    /// Panics rather than skips. The test is `#[ignore]`d, so reaching here at
    /// all means a script asked for it, and a silent pass would be a lie.
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

/// Write the app and the artifacts this test loads.
///
/// The e2e workspace is rebuilt per run and is disposable. So a fixture on its
/// disk is the cheapest way to have real files for a real engine to serve.
fn write_fixture(workspace: &Path) {
    let app = workspace.join("data/apps").join(APP_ID);
    std::fs::create_dir_all(&app).expect("the e2e workspace is writable");
    // A separate stylesheet is the exact thing the temporary measure told app
    // authors to inline. An `<img>` on a root-absolute `/data/` path is the
    // other half, the one the rescope has to thread the pass onto.
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

/// A gateway serving `/<slug>/` in front of the live engine, on a free port.
///
/// The real router, so a request passes the real `auth_api::enforce` and the
/// real `proxy`. Not the gateway BINARY: that one refuses to boot from a
/// coding-agent worktree, deliberately and with no opt-out (ADR 0021). A test
/// process is not the machine-global daemon that guard exists for. Its private
/// registry adopts only the disposable engine, spawns nothing, and dies with the test.
async fn serve_gateway(engine: &LiveEngine, slug: &str, token: &str) -> String {
    let state = GatewayState::for_tests_with(None, token, engine.tls).with_live_engine_sockets();
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

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a free port for the gateway");
    let base = format!("http://{}", listener.local_addr().expect("a bound address"));
    let router = gateway_router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    base
}

/// A client with NO cookie jar, which is the point of every request below one.
///
/// A subresource of an opaque-origin frame carries no device credential. This
/// crate's reqwest has no cookie feature, so the omission is structural rather
/// than a setting somebody could flip.
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        // The engine hop may be TLS with a self-signed dev cert, and nothing in
        // this test goes off the loopback interface.
        .danger_accept_invalid_certs(true)
        .no_proxy()
        .build()
        .expect("a reqwest client")
}

/// The `<base href>` the engine stamped, which is where the pass lives.
fn base_href(html: &str) -> String {
    let at = html
        .find("<base href=\"")
        .unwrap_or_else(|| panic!("the engine stamped no capability base:\n{html}"));
    let rest = &html[at + "<base href=\"".len()..];
    rest[..rest.find('"').expect("a closed attribute")].to_string()
}

/// Resolve `href` the way a browser resolves it against `base`.
///
/// Only the two shapes this test uses: a bare filename, and `./x/y`. A URL
/// crate would do it properly, and doing it by hand keeps the assertion about
/// the PATH rather than about a parser.
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

    gateway_restore_origin_from_spawned_child(&engine, &gateway, &token).await;

    // 1. The frame's own document. This one IS authorized: the host sets the
    //    iframe src from the shell, whose site-for-cookies is the gateway's, so
    //    the browser does send the credential here. A local process stands in
    //    for that, and `sec-fetch-dest` says it is a frame.
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

    // 2. The app's own stylesheet, COOKIELESS, resolved against that base the
    //    way the browser resolves `href="style.css"`. This is the removal
    //    condition in one line.
    let (status, body) = get(
        &client,
        &format!("{gateway}{}", resolve(&base, "style.css")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the app's own stylesheet: {body}");
    assert_eq!(body, STYLE);

    // 3. The root-absolute `/data/` ref the rescope threaded the pass onto.
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

    // 4. A nested document, which is what `lucidos.data.url()` builds for a
    //    preview iframe.
    let nested = format!("{carrier}/data/artifacts/{APP_ID}/index.html");
    let (status, body) = get(&client, &nested).await;
    assert_eq!(status, StatusCode::OK, "the previewed artifact: {body}");
    assert_eq!(body, NESTED);

    // 5. ONE HOP DOWN. The nested document's own relative link, resolved
    //    against its own url, with no query on it and nothing rewritten. This
    //    is the case a query-parameter pass gets wrong.
    let (status, body) = get(&client, &resolve(&nested, "./next/index.html")).await;
    assert_eq!(status, StatusCode::OK, "a click inside the preview: {body}");
    assert_eq!(body, HOP);

    // 6. The same files with NO pass are refused exactly as before.
    let (status, _) = get(&client, &format!("{gateway}/{slug}/app/{APP_ID}/style.css")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "no pass, no file");

    // 7. And the pass reaches no engine route, which is what keeps it from
    //    being a second device credential.
    for path in [
        "/api/v1/credentials",
        "/api/v1/threads/list",
        "/api/v1/data",
    ] {
        let (status, _) = get(&client, &format!("{carrier}{path}")).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path} must stay gated");
    }

    // 8. The app's own font. A font fetch is CORS-mode, and the frame's origin
    //    is `null`, so the engine's grant has to cross the gateway intact. A
    //    `fetch()` of the same file gets none (ADR 0289).
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

    // 9. The app's own module script. The engine grants a script load only to
    //    a request the gateway forwarded, which is what this hop proves.
    let module = format!("{gateway}{}", resolve(&base, "main.js"));
    let script = cross_origin(&client, reqwest::Method::GET, &module, "script").await;
    assert_eq!(script.status(), StatusCode::OK, "the app's own module");
    assert_eq!(
        allow_origin(&script),
        Some("*"),
        "a module behind the gateway"
    );

    // 10. Fira Code, the default UI font every app links. Public at the gate,
    //     and the preflight is answered rather than refused.
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

/// A request shaped the way an opaque-origin frame's is, with no cookie.
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

/// Run the origin probe as an actual engine child. The test only sees its
/// allowlisted answer, never the inherited process-MAC token being forwarded.
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
        .header("x-lucidos-device-id", device)
        .header("content-type", "application/json")
        .body(serde_json::json!({ "value": fake.to_str().expect("fixture path") }).to_string())
        .send()
        .await
        .expect("configure the disposable agent");
    assert!(changed.status().is_success());

    // Keep cleanup outside the assertion path, including when the child fails.
    let result = async {
        let thread = "bce23d74-18a8-4652-aef0-168258160821";
        let submitted = client.post(format!("{base}/api/v1/chat/stream"))
            .header("x-lucidos-device-id", device)
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
                            live_snapshot_approval_matrix(&client, &base, gateway, token, device, thread).await
                        } else { Err("gateway did not verify the engine-spawned child") };
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
    .header("x-lucidos-device-id", device)
    .send()
    .await
    .expect("restore disposable preference");
    assert!(restore.status().is_success());
    assert_eq!(result, Ok(()));
}

// The first turn has established a real coding-agent session. Continue it so
// every card and spend belongs to a live turn, with no inserted fixture events.
async fn live_snapshot_approval_matrix(
    client: &reqwest::Client,
    base: &str,
    gateway: &str,
    token: &str,
    device: &str,
    thread: &str,
) -> Result<(), &'static str> {
    let response = client.post(format!("{base}/api/v1/chat/stream"))
        .header("x-lucidos-device-id", device)
        .header("content-type", "application/json")
        .body(serde_json::json!({
            "message": format!("Probe snapshot gateway restore origin: {gateway}. Spend snapshot approval."),
            "thread_id": thread, "mode": "human", "use_coding_agent": true,
            "coding_agent": "claude-code",
        }).to_string()).send().await.map_err(|_| "submit approval turn")?;
    if !response.status().is_success() {
        return Err("approval turn refused");
    }
    let mut answered = std::collections::HashSet::new();
    for _ in 0..120 {
        let snapshot = client
            .get(format!("{base}/api/v1/threads/{thread}/events"))
            .header(crate::auth::HEADER_LOCAL_TOKEN, token)
            .send()
            .await
            .map_err(|_| "read approval events")?
            .text()
            .await
            .map_err(|_| "read approval body")?;
        let snapshot: serde_json::Value =
            serde_json::from_str(&snapshot).map_err(|_| "approval events JSON")?;
        for event in snapshot["events"]
            .as_array()
            .ok_or("missing approval events")?
        {
            let payload = &event["payload"];
            if event["event_type"] == "UserQuestionAsked" {
                let id = payload["tool_use_id"].as_str().ok_or("card id missing")?;
                if !id.starts_with("restore-approval-") || answered.contains(id) {
                    continue;
                }
                let label = if id.contains("-allow") || id.contains("-lost") {
                    "Allow once"
                } else {
                    "Don't allow"
                };
                let option = payload["options"]
                    .as_array()
                    .ok_or("card options missing")?
                    .iter()
                    .find(|o| o["label"] == label)
                    .ok_or("engine card missing")?;
                if payload["owner_approval"]["target_workspace_id"] != "restore-probe-target"
                    || payload["owner_approval"]["snapshot_handle"]
                        != "snap_00000000000000000000000000"
                {
                    return Err("card dimensions differ");
                }
                let answer = client
                    .post(format!("{base}/api/v1/threads/{thread}/answer-question"))
                    .header("x-lucidos-device-id", device)
                    .header("content-type", "application/json")
                    .body(
                        serde_json::json!({ "tool_use_id": id,
                        "answer": { "kind": "Selected", "option_id": option["id"] } })
                        .to_string(),
                    )
                    .send()
                    .await
                    .map_err(|_| "answer approval card")?;
                if !answer.status().is_success() {
                    return Err("device answer refused");
                }
                answered.insert(id.to_owned());
            }
            if event["event_type"] == "ResponseGenerated" {
                let proof: serde_json::Value =
                    serde_json::from_str(payload["text"].as_str().unwrap_or(""))
                        .map_err(|_| "approval child JSON")?;
                if proof["approval_matrix"] == true {
                    let spent = snapshot["events"]
                        .as_array()
                        .ok_or("missing events")?
                        .iter()
                        .filter(|e| e["event_type"] == "OwnerApprovalSpent")
                        .count();
                    return if answered.len() == 3 && spent == 2 {
                        Ok(())
                    } else {
                        Err("approval event counts differ")
                    };
                }
                if proof["verified"] == false {
                    return Err("live approval matrix failed");
                }
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    Err("live approval matrix timed out")
}
