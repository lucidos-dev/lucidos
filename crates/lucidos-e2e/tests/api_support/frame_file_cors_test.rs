//! An app frame's own files grant a font load across origins, and a module
//! script load only behind the gateway. Nothing else is granted.
//!
//! Unit tests in `api/frame_file_cors.rs` cover the middleware. Only a booted
//! engine shows it is layered on BOTH trees, `/app` and `/data`, which sit
//! outside `/api/v1` and are wired separately. A missing layer is silent in the
//! browser: the app's font falls back and its module never runs.

use crate::support::{base_url, http_client, unique_marker, workspace_path};

fn write_data_file(rel: &str, body: &[u8]) -> std::path::PathBuf {
    let path = workspace_path().join("data").join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("Failed to create parent dirs");
    }
    std::fs::write(&path, body).expect("Failed to write test file");
    path
}

/// How the request reached the engine. The gateway stamps its prefix on every
/// request it forwards, and a page cannot.
#[derive(Clone, Copy, Debug)]
enum Via {
    Gateway,
    Direct,
}

async fn get(url: &str, dest: &str, via: Via) -> reqwest::Response {
    let mut request = http_client()
        .get(url)
        .header("origin", "null")
        .header("sec-fetch-site", "cross-site")
        .header("sec-fetch-dest", dest);
    if let Via::Gateway = via {
        request = request.header("x-forwarded-prefix", "/e2e-test/");
    }
    request
        .send()
        .await
        .unwrap_or_else(|e| panic!("request to {url} failed: {e}"))
}

fn header<'a>(resp: &'a reqwest::Response, name: &str) -> Option<&'a str> {
    resp.headers().get(name).and_then(|v| v.to_str().ok())
}

fn varies_on_destination(resp: &reqwest::Response) -> bool {
    resp.headers()
        .get_all("vary")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|v| v.trim().eq_ignore_ascii_case("sec-fetch-dest"))
}

/// One test, because it writes into the workspace tree and holds its lock for
/// the whole sequence.
#[tokio::test]
async fn an_app_frames_own_files_grant_fonts_and_gated_scripts_only() {
    let _tree = crate::support::workspace_tree_lock().read().await;

    let marker = unique_marker("framecors");
    let app_id = format!("e2e-{marker}");
    let written = [
        write_data_file(
            &format!("apps/{app_id}/manifest.json"),
            format!(r#"{{"id":"{app_id}","name":"CORS probe","description":"e2e"}}"#).as_bytes(),
        ),
        write_data_file(&format!("apps/{app_id}/own.woff2"), b"wOF2 not a real font"),
        write_data_file(&format!("apps/{app_id}/main.js"), b"export const ok = 1;"),
        write_data_file(
            &format!("artifacts/{marker}.woff2"),
            b"wOF2 not a real font",
        ),
        write_data_file(&format!("artifacts/{marker}.mjs"), b"export const ok = 1;"),
    ];

    let app_font = format!("{}/app/{app_id}/own.woff2", base_url());
    let app_module = format!("{}/app/{app_id}/main.js", base_url());
    let data_font = format!("{}/data/artifacts/{marker}.woff2", base_url());
    let data_module = format!("{}/data/artifacts/{marker}.mjs", base_url());

    let granted = [
        (&app_font, "font", Via::Direct),
        (&data_font, "font", Via::Direct),
        (&app_module, "script", Via::Gateway),
        (&data_module, "script", Via::Gateway),
    ];
    for (url, dest, via) in granted {
        let resp = get(url, dest, via).await;
        assert_eq!(resp.status(), 200, "{url}");
        assert_eq!(
            header(&resp, "access-control-allow-origin"),
            Some("*"),
            "{dest} load of {url} via {via:?}"
        );
        assert_eq!(
            header(&resp, "x-content-type-options"),
            Some("nosniff"),
            "a grant must not let a text file run as a script: {url}"
        );
        assert!(varies_on_destination(&resp), "{url}");
    }

    // A module hands its caller its exports. Direct to the engine nothing gates
    // `/data/`, so a script load there must not be granted to any web page.
    // A `fetch()` is `empty`, the read the grant must never cover at all.
    let refused = [
        (&app_module, "script", Via::Direct),
        (&data_module, "script", Via::Direct),
        (&app_font, "empty", Via::Gateway),
        (&data_font, "empty", Via::Direct),
        (&data_module, "empty", Via::Gateway),
    ];
    for (url, dest, via) in refused {
        let resp = get(url, dest, via).await;
        assert_eq!(resp.status(), 200, "{url}");
        assert_eq!(
            header(&resp, "access-control-allow-origin"),
            None,
            "{dest} load of {url} via {via:?}"
        );
        assert!(
            varies_on_destination(&resp),
            "a refusal must vary too, or a cached grant can answer a fetch(): {url}"
        );
    }

    for path in &written {
        let _ = std::fs::remove_file(path);
    }
    let _ = std::fs::remove_dir(written[0].parent().unwrap());
}
