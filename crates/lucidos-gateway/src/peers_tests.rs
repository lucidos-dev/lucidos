//! Fixture-tree tests for the peer lookup, and stand-in gateways for the probe.
//!
//! Every lookup case builds a throwaway `$HOME` and scans that, so nothing here
//! reads the machine it runs on. The developer running these has a real bundle
//! and a real dev gateway, and either would otherwise answer for a fixture.
//!
//! The load-bearing pair is `a_peer_serving_http_reports_http` and
//! `a_peer_serving_https_reports_https`. Two shipped gateways disagree on the
//! scheme, so a probe that assumed one composes a dead URL for half the machines
//! this feature exists for.

use super::*;
use crate::registry::Workspace;
use axum::{routing::get, Router};
use lucidos_installs::{RunningProcess, ScanRoots};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

// ── Fixture tree ───────────────────────────────────────────────────────────

/// A throwaway `$HOME`, removed when the test ends.
struct Fixture {
    home: PathBuf,
}

/// One registry entry, as `(slug, display name)`.
type Entry<'a> = (&'a str, &'a str);

impl Fixture {
    fn new(tag: &str) -> Self {
        let home = std::env::temp_dir().join(format!(
            "lucidos-peers-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::remove_dir_all(&home).ok();
        std::fs::create_dir_all(&home).unwrap();
        // macOS puts the temp dir behind a symlink, and an un-canonicalised
        // root makes every `starts_with` on a resolved path miss.
        let home = std::fs::canonicalize(&home).unwrap();
        Fixture { home }
    }

    fn roots(&self) -> ScanRoots {
        ScanRoots::under(&self.home)
    }

    /// Scan the fixture as the source checkout at `ours`, on the dev port.
    fn locate_from(&self, ours: &Path, name: &str) -> Located {
        self.locate_as(ours, lucidos_installs::DEFAULT_DEV_GATEWAY_PORT, name)
    }

    /// Scan the fixture as a process belonging to the install at `ours`, whose
    /// gateway is configured for `own_port`.
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

    /// The packaged bundle, its data dir registered with `entries`. Returns the
    /// data dir, which is what tells one install from another.
    fn desktop_app(&self, entries: &[Entry]) -> PathBuf {
        std::fs::create_dir_all(self.home.join("Applications/Lucidos.app/Contents")).unwrap();
        let data = self
            .home
            .join("Library/Application Support/com.lucidos.app");
        self.register(&data, entries);
        data
    }

    /// The dev source checkout, its data dir registered with `entries`.
    fn source_checkout(&self, entries: &[Entry]) -> PathBuf {
        let data = self.home.join(".lucidos/gateway");
        self.register(&data, entries);
        data
    }

    /// One `install.sh` instance on `port`, registered with `entries`.
    fn instance(&self, slug: &str, port: u16, entries: &[Entry]) -> PathBuf {
        let data = self.home.join(".lucidos").join(slug);
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("port"), format!("{port}\n")).unwrap();
        self.register(&data, entries);
        data
    }

    /// Write a gateway registry holding `entries` under `data`.
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
                    autostart: true,
                })
                .collect(),
        };
        registry.save(&data.join(REGISTRY_REL)).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.home).ok();
    }
}

// ── Locating a workspace ───────────────────────────────────────────────────

#[test]
fn a_workspace_on_the_other_install_is_found() {
    let fx = Fixture::new("other");
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

/// The caller already searched its own gateway, through the live control
/// listing. Answering with ourselves would send the page back here.
#[test]
fn our_own_install_is_never_a_peer() {
    let fx = Fixture::new("self");
    fx.desktop_app(&[("work", "work")]);
    let ours = fx.source_checkout(&[("dev", "dev")]);

    assert_eq!(fx.locate_from(&ours, "dev"), Located::Nowhere);
}

#[test]
fn a_display_name_resolves_to_its_slug() {
    let fx = Fixture::new("display");
    fx.desktop_app(&[("my-space", "My Space")]);
    let ours = fx.source_checkout(&[]);

    let Located::One(peer) = fx.locate_from(&ours, "My Space") else {
        panic!("display name did not resolve");
    };
    assert_eq!(peer.slug, "my-space");
}

/// The second arm: a name nobody displays, which slugifies onto a real slug.
#[test]
fn a_slug_resolves_even_when_no_display_name_matches() {
    let fx = Fixture::new("slug");
    fx.desktop_app(&[("my-space", "Something Else")]);
    let ours = fx.source_checkout(&[]);

    let Located::One(peer) = fx.locate_from(&ours, "my-space") else {
        panic!("slug did not resolve");
    };
    assert_eq!(peer.slug, "my-space");
}

#[test]
fn two_installs_carrying_the_name_are_ambiguous() {
    let fx = Fixture::new("ambiguous");
    fx.desktop_app(&[("shared", "shared")]);
    fx.instance("alt", 5300, &[("shared", "shared")]);
    let ours = fx.source_checkout(&[]);

    let Located::Ambiguous(peers) = fx.locate_from(&ours, "shared") else {
        panic!("expected a refusal naming both");
    };
    assert_eq!(peers.len(), 2);
}

/// One install found twice is not two installs. Two bundles in two application
/// directories share one data dir, so they carry one registry between them.
#[test]
fn one_install_found_twice_is_not_ambiguous() {
    let fx = Fixture::new("twice");
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

/// Two OTHER installs on one port address neither of them. Only one bound it,
/// nothing on disk says which, and the registry that matched belongs to the
/// install that may have lost. Naming it navigates the user to a stranger.
#[test]
fn a_hit_on_a_port_two_other_installs_contend_for_is_dropped() {
    let fx = Fixture::new("contended");
    fx.desktop_app(&[("work", "work")]);
    fx.instance(
        "alt",
        lucidos_installs::DEFAULT_GATEWAY_PORT,
        &[("other", "other")],
    );
    let ours = fx.source_checkout(&[]);

    assert_eq!(fx.locate_from(&ours, "work"), Located::Nowhere);
}

/// Two distinct installs on one port, both carrying the slug, are not one
/// install found twice. Keyed on the port and the slug, the dedupe collapsed
/// them into a single hit and inventory order silently picked the winner.
#[test]
fn two_installs_sharing_a_port_and_a_slug_never_pick_a_winner() {
    let fx = Fixture::new("collapse");
    fx.desktop_app(&[("shared", "shared")]);
    fx.instance(
        "alt",
        lucidos_installs::DEFAULT_GATEWAY_PORT,
        &[("shared", "shared")],
    );
    let ours = fx.source_checkout(&[]);

    assert_eq!(fx.locate_from(&ours, "shared"), Located::Nowhere);
}

/// An install contending for OUR port is skipped, whoever owns it. Only one of
/// the two binds the port (ADR 0189). If that one is us, the peer's URL lands
/// here, on a slug we do not serve.
#[test]
fn an_install_contending_for_our_own_port_is_not_a_peer() {
    let fx = Fixture::new("contention");
    fx.desktop_app(&[("work", "work")]);
    let ours = fx.source_checkout(&[]);

    // The same fixture resolves normally from a gateway on another port.
    assert!(matches!(
        fx.locate_as(&ours, lucidos_installs::DEFAULT_DEV_GATEWAY_PORT, "work"),
        Located::One(_)
    ));
    assert_eq!(
        fx.locate_as(&ours, lucidos_installs::DEFAULT_GATEWAY_PORT, "work"),
        Located::Nowhere
    );
}

/// The name is matched against parsed registry contents, never joined into a
/// path, so a traversal reads as an ordinary miss.
#[test]
fn a_traversal_shaped_name_is_a_miss() {
    let fx = Fixture::new("traversal");
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
    let fx = Fixture::new("bare");
    std::fs::create_dir_all(fx.home.join("Applications/Lucidos.app/Contents")).unwrap();
    std::fs::create_dir_all(fx.home.join("Library/Application Support/com.lucidos.app")).unwrap();
    let ours = fx.source_checkout(&[]);

    assert_eq!(fx.locate_from(&ours, "work"), Located::Nowhere);
}

// ── Probing a peer ─────────────────────────────────────────────────────────

/// A stand-in gateway's health body.
fn gateway_body() -> Value {
    json!({"status": "ok", "role": "gateway", "release": "0.0.0"})
}

/// A server answering `PEER_HEALTH_PATH` with `body`, and nothing else.
fn stand_in_router(body: Value) -> Router {
    Router::new().route(
        PEER_HEALTH_PATH,
        get(move || {
            let body = body.clone();
            async move { axum::Json(body) }
        }),
    )
}

/// Serve `body` over plain http on a free loopback port.
async fn http_stand_in(body: Value) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum::serve(listener, stand_in_router(body)).await.ok();
    });
    port
}

/// Serve `body` over https on a free loopback port, behind a cert minted here.
/// A checked-in cert would expire and fail a test years from now.
async fn https_stand_in(body: Value) -> u16 {
    // Installing twice in one test binary is an error, and every https case
    // asks. The first caller wins and the rest read the same provider.
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
async fn a_peer_serving_http_reports_http() {
    let port = http_stand_in(gateway_body()).await;
    assert_eq!(
        probe_scheme(&probe_client(), port).await,
        Some(SCHEME_HTTP),
        "https is tried first, so http here proves the answer is measured"
    );
}

#[tokio::test]
async fn a_peer_serving_https_reports_https() {
    let port = https_stand_in(gateway_body()).await;
    assert_eq!(
        probe_scheme(&probe_client(), port).await,
        Some(SCHEME_HTTPS)
    );
}

#[tokio::test]
async fn nothing_listening_reports_no_scheme() {
    // Bind and drop, so the port is free and almost certainly still unclaimed.
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = free.local_addr().unwrap().port();
    drop(free);

    assert_eq!(probe_scheme(&probe_client(), port).await, None);
}

/// The port is a well-known number and anything may hold it, so the answer has
/// to say it came from a gateway.
#[tokio::test]
async fn something_else_on_the_port_reports_no_scheme() {
    let port = http_stand_in(json!({"status": "ok", "role": "engine"})).await;
    assert_eq!(probe_scheme(&probe_client(), port).await, None);
}
