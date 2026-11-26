use super::*;
use crate::registry::Workspace;
use axum::{routing::get, Router};
use lucidos_installs::{RunningProcess, ScanRoots};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

struct Fixture {
    home: PathBuf,
    _tmp: tempfile::TempDir,
}

type Entry<'a> = (&'a str, &'a str);

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(tmp.path()).unwrap();
        Fixture { home, _tmp: tmp }
    }

    fn roots(&self) -> ScanRoots {
        ScanRoots::under(&self.home)
    }

    fn locate_from(&self, ours: &Path, name: &str) -> Located {
        self.locate_as(ours, lucidos_installs::DEFAULT_DEV_GATEWAY_PORT, name)
    }

    fn locate_as(&self, ours: &Path, own_port: u16, name: &str) -> Located {
        let running = RunningProcess {
            exe: None,
            data_dir: Some(ours.to_path_buf()),
        };
        locate(
            &lucidos_installs::scan(&self.roots(), &running),
            own_port,
            name,
        )
    }

    fn desktop_app(&self, entries: &[Entry]) -> PathBuf {
        std::fs::create_dir_all(self.home.join("Applications/Lucidos.app/Contents")).unwrap();
        let data = self
            .home
            .join("Library/Application Support/com.lucidos.app");
        self.register(&data, entries);
        data
    }

    fn source_checkout(&self, entries: &[Entry]) -> PathBuf {
        let data = self.home.join(".lucidos/private/gateway");
        self.register(&data, entries);
        data
    }

    fn instance(&self, slug: &str, port: u16, entries: &[Entry]) -> PathBuf {
        let data = self.home.join(".lucidos").join(slug);
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("port"), format!("{port}\n")).unwrap();
        self.register(&data, entries);
        data
    }

    fn register(&self, data: &Path, entries: &[Entry]) {
        std::fs::create_dir_all(data).unwrap();
        let registry = Registry {
            version: crate::registry::REGISTRY_VERSION,
            workspaces: entries
                .iter()
                .map(|(id, name)| Workspace {
                    id: (*id).to_string(),
                    name: (*name).to_string(),
                    dir: format!("workspaces/{id}"),
                    port: 5400,
                    database_url: None,
                    cleanup: Default::default(),
                    autostart: true,
                })
                .collect(),
        };
        registry.save(&data.join(REGISTRY_REL)).unwrap();
    }
}

#[test]
fn a_workspace_on_the_other_install_is_found() {
    let fx = Fixture::new();
    fx.desktop_app(&[("work", "work")]);
    let ours = fx.source_checkout(&[("dev", "dev")]);

    let found = fx.locate_from(&ours, "work");

    let Located::One(peer) = found else {
        panic!("expected one hit, got {found:?}");
    };
    assert_eq!(peer.slug, "work");
    assert_eq!(peer.gateway_port, lucidos_installs::DEFAULT_GATEWAY_PORT);
    assert!(peer.install.contains("Lucidos.app"), "{}", peer.install);
}

#[test]
fn our_own_install_is_never_a_peer() {
    let fx = Fixture::new();
    fx.desktop_app(&[("work", "work")]);
    let ours = fx.source_checkout(&[("dev", "dev")]);

    assert_eq!(fx.locate_from(&ours, "dev"), Located::Nowhere);
}

#[test]
fn a_display_name_resolves_to_its_slug() {
    let fx = Fixture::new();
    fx.desktop_app(&[("my-space", "My Space")]);
    let ours = fx.source_checkout(&[]);

    let Located::One(peer) = fx.locate_from(&ours, "My Space") else {
        panic!("display name did not resolve");
    };
    assert_eq!(peer.slug, "my-space");
}

#[test]
fn a_slug_resolves_even_when_no_display_name_matches() {
    let fx = Fixture::new();
    fx.desktop_app(&[("my-space", "Something Else")]);
    let ours = fx.source_checkout(&[]);

    let Located::One(peer) = fx.locate_from(&ours, "my-space") else {
        panic!("slug did not resolve");
    };
    assert_eq!(peer.slug, "my-space");
}

#[test]
fn two_installs_carrying_the_name_are_ambiguous() {
    let fx = Fixture::new();
    fx.desktop_app(&[("shared", "shared")]);
    fx.instance("alt", 5300, &[("shared", "shared")]);
    let ours = fx.source_checkout(&[]);

    let Located::Ambiguous(peers) = fx.locate_from(&ours, "shared") else {
        panic!("expected a refusal naming both");
    };
    assert_eq!(peers.len(), 2);
}

#[test]
fn one_install_found_twice_is_not_ambiguous() {
    let fx = Fixture::new();
    fx.desktop_app(&[("work", "work")]);
    let second_dir = fx.home.join("Applications2");
    std::fs::create_dir_all(second_dir.join("Lucidos.app/Contents")).unwrap();
    let ours = fx.source_checkout(&[]);

    let mut roots = fx.roots();
    roots.application_dirs.push(second_dir);
    let running = RunningProcess {
        exe: None,
        data_dir: Some(ours),
    };
    let inventory = lucidos_installs::scan(&roots, &running);

    assert_eq!(
        inventory
            .installs
            .iter()
            .filter(|i| i.kind == lucidos_installs::InstallKind::DesktopApp)
            .count(),
        2,
        "the fixture must produce the doubled bundle this test is about"
    );
    assert!(matches!(
        locate(
            &inventory,
            lucidos_installs::DEFAULT_DEV_GATEWAY_PORT,
            "work"
        ),
        Located::One(_)
    ));
}

#[test]
fn a_hit_on_a_port_two_other_installs_contend_for_is_dropped() {
    let fx = Fixture::new();
    fx.desktop_app(&[("work", "work")]);
    fx.instance(
        "alt",
        lucidos_installs::DEFAULT_GATEWAY_PORT,
        &[("other", "other")],
    );
    let ours = fx.source_checkout(&[]);

    assert_eq!(fx.locate_from(&ours, "work"), Located::Nowhere);
}

#[test]
fn two_installs_sharing_a_port_and_a_slug_never_pick_a_winner() {
    let fx = Fixture::new();
    fx.desktop_app(&[("shared", "shared")]);
    fx.instance(
        "alt",
        lucidos_installs::DEFAULT_GATEWAY_PORT,
        &[("shared", "shared")],
    );
    let ours = fx.source_checkout(&[]);

    assert_eq!(fx.locate_from(&ours, "shared"), Located::Nowhere);
}

#[test]
fn an_install_contending_for_our_own_port_is_not_a_peer() {
    let fx = Fixture::new();
    fx.desktop_app(&[("work", "work")]);
    let ours = fx.source_checkout(&[]);

    assert!(matches!(
        fx.locate_as(&ours, lucidos_installs::DEFAULT_DEV_GATEWAY_PORT, "work"),
        Located::One(_)
    ));
    assert_eq!(
        fx.locate_as(&ours, lucidos_installs::DEFAULT_GATEWAY_PORT, "work"),
        Located::Nowhere
    );
}

#[test]
fn a_traversal_shaped_name_is_a_miss() {
    let fx = Fixture::new();
    fx.desktop_app(&[("work", "work")]);
    let ours = fx.source_checkout(&[]);

    assert_eq!(
        fx.locate_from(&ours, "../../../../etc/passwd"),
        Located::Nowhere
    );
    assert_eq!(fx.locate_from(&ours, "127.0.0.1:9/evil"), Located::Nowhere);
}

#[test]
fn an_install_with_no_registry_is_skipped() {
    let fx = Fixture::new();
    std::fs::create_dir_all(fx.home.join("Applications/Lucidos.app/Contents")).unwrap();
    std::fs::create_dir_all(fx.home.join("Library/Application Support/com.lucidos.app")).unwrap();
    let ours = fx.source_checkout(&[]);

    assert_eq!(fx.locate_from(&ours, "work"), Located::Nowhere);
}

fn gateway_body() -> Value {
    json!({"status": "ok", "role": "gateway", "release": "0.0.0"})
}

fn stand_in_router(body: Value) -> Router {
    Router::new().route(
        GATEWAY_HEALTH_PATH,
        get(move || {
            let body = body.clone();
            async move { axum::Json(body) }
        }),
    )
}

async fn http_stand_in(body: Value) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, stand_in_router(body)).await.ok();
    });
    port
}

async fn foreign_https_stand_in(body: Value) -> u16 {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let issued = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let config = axum_server::tls_rustls::RustlsConfig::from_pem(
        issued.cert.pem().into_bytes(),
        issued.key_pair.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, config)
            .serve(stand_in_router(body).into_make_service())
            .await
            .ok();
    });
    port
}

#[tokio::test]
async fn a_peer_serving_the_loopback_ca_leaf_answers_as_gateway() {
    let peer = lucidos_local_token::test_support::serve_https(stand_in_router(gateway_body()));
    assert!(answers_as_gateway(&probe_client(), peer.port()).await);
}

#[tokio::test]
async fn a_plaintext_peer_does_not_answer_as_gateway() {
    let port = http_stand_in(gateway_body()).await;
    assert!(!answers_as_gateway(&probe_client(), port).await);
}

#[tokio::test]
async fn a_peer_with_a_foreign_cert_does_not_answer_as_gateway() {
    let port = foreign_https_stand_in(gateway_body()).await;
    assert!(!answers_as_gateway(&probe_client(), port).await);
}

#[tokio::test]
async fn nothing_listening_does_not_answer_as_gateway() {
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = free.local_addr().unwrap().port();
    drop(free);

    assert!(!answers_as_gateway(&probe_client(), port).await);
}

#[tokio::test]
async fn something_else_on_the_port_does_not_answer_as_gateway() {
    let other = lucidos_local_token::test_support::serve_https(stand_in_router(
        json!({"status": "ok", "role": "engine"}),
    ));
    assert!(!answers_as_gateway(&probe_client(), other.port()).await);
}
