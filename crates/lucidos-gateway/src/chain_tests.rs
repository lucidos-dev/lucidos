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
/// process is not the machine-global daemon that guard exists for. It registers
/// nothing, adopts nothing, spawns no engine, and dies with the test.
async fn serve_gateway(engine: &LiveEngine, slug: &str, token: &str) -> String {
    let state = GatewayState::for_tests_with(None, token, engine.tls);
    state.register_for_test(Workspace::gateway_provisioned(
        slug.to_string(),
        slug.to_string(),
        engine.port,
    ));
    state.set_route_for_test(slug, engine.port);

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
}
