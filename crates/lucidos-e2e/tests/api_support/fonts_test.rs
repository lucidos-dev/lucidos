use crate::support::{base_url, http_client};

#[tokio::test]
async fn fira_code_stylesheet_points_at_a_font_that_is_really_there() {
    let client = http_client();

    let css_url = format!("{}/api/v1/fonts/fira-code.css", base_url());
    let resp = client
        .get(&css_url)
        .send()
        .await
        .expect("Fira Code stylesheet request failed");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/css; charset=utf-8"),
        "a stylesheet served as anything else is ignored by the browser"
    );
    let css = resp.text().await.expect("stylesheet body");
    assert!(css.contains("@font-face"), "got:\n{css}");

    let src = css
        .split("url('")
        .nth(1)
        .and_then(|rest| rest.split('\'').next())
        .expect("the @font-face must carry a url()");
    let font_url = reqwest::Url::parse(&css_url)
        .expect("stylesheet url")
        .join(src)
        .expect("the src must resolve against the stylesheet");

    let resp = client
        .get(font_url.clone())
        .send()
        .await
        .unwrap_or_else(|e| panic!("font request to {font_url} failed: {e}"));
    assert_eq!(resp.status(), 200, "the @font-face src 404s: {font_url}");
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("font/woff2")
    );

    let bytes = resp.bytes().await.expect("font body");
    assert!(
        bytes.len() > 50_000,
        "FiraCode-VF.woff2 should be ~113 KB, got {} bytes",
        bytes.len()
    );
    assert_eq!(&bytes[..4], b"wOF2", "not a woff2 payload");
}

#[tokio::test]
async fn every_vendored_font_in_the_catalog_is_routed() {
    let client = http_client();
    let catalog: serde_json::Value = client
        .get(format!("{}/api/v1/fonts", base_url()))
        .send()
        .await
        .expect("catalog request failed")
        .json()
        .await
        .expect("the catalog is JSON");
    assert_eq!(catalog["follow_theme"], "theme");
    let fonts = catalog["fonts"].as_array().expect("a fonts array");
    let vendored: Vec<&str> = fonts
        .iter()
        .filter(|f| f["source"] == "vendored")
        .filter_map(|f| f["id"].as_str())
        .collect();
    assert!(vendored.contains(&"fira-code") && vendored.contains(&"geist"));

    for id in vendored {
        let css_url = format!("{}/api/v1/fonts/{id}.css", base_url());
        let css = client
            .get(&css_url)
            .send()
            .await
            .expect("stylesheet request failed")
            .text()
            .await
            .expect("stylesheet body");
        for src in css.split("url('").skip(1) {
            let file = src.split('\'').next().expect("a closed url()");
            let font_url = reqwest::Url::parse(&css_url).unwrap().join(file).unwrap();
            let resp = client
                .get(font_url.clone())
                .send()
                .await
                .expect("font request");
            assert_eq!(resp.status(), 200, "{id}: {font_url}");
            let bytes = resp.bytes().await.expect("font body");
            assert_eq!(&bytes[..4], b"wOF2", "{id}: {font_url}");
        }
    }
}

fn allow_origin(resp: &reqwest::Response) -> Option<&str> {
    resp.headers()
        .get("access-control-allow-origin")
        .and_then(|v| v.to_str().ok())
}

#[tokio::test]
async fn an_app_frame_may_read_fira_code_across_its_opaque_origin() {
    let client = http_client();
    for file in ["fira-code.css", "fira-code-6.2.woff2"] {
        let url = format!("{}/api/v1/fonts/{file}", base_url());
        let resp = client
            .get(&url)
            .header("origin", "null")
            .header("sec-fetch-site", "cross-site")
            .header("sec-fetch-mode", "cors")
            .header("sec-fetch-dest", "font")
            .send()
            .await
            .expect("font request failed");
        assert_eq!(resp.status(), 200, "{url}");
        assert_eq!(allow_origin(&resp), Some("*"), "{url}");

        let resp = client
            .request(reqwest::Method::OPTIONS, &url)
            .header("origin", "null")
            .header("access-control-request-method", "GET")
            .header("sec-fetch-site", "cross-site")
            .send()
            .await
            .expect("preflight failed");
        assert!(resp.status().is_success(), "{url}: {}", resp.status());
        assert_eq!(allow_origin(&resp), Some("*"), "{url}");
    }
}

#[tokio::test]
async fn no_other_api_route_offers_a_cross_origin_read() {
    let client = http_client();
    for path in [
        "/api/v1/sdk.js",
        "/api/v1/sdk-iframe.css",
        "/api/v1/fonts",
        "/api/v1/health",
        "/api/v1/threads/list",
    ] {
        let resp = client
            .get(format!("{}{path}", base_url()))
            .header("origin", "null")
            .send()
            .await
            .expect("request failed");
        assert_eq!(allow_origin(&resp), None, "{path}");
    }
}
