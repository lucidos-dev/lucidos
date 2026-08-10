//! `GET /api/v1/workspace-label`: the name the USER gave this workspace.
//!
//! The engine knows only its own directory name (`/api/v1/health`'s
//! `workspace`), and a rename in the picker is a registry write the engine is
//! never told about (ADR 0014: rename edits the registry `name`, nothing moves).
//! A page served BEHIND the gateway resolves the difference itself, by reading
//! the control listing at `/~/api/v1/control/workspaces` and matching on its own
//! slug (`store/actions/workspace-label.ts`).
//!
//! A page served straight off this engine's own port cannot, and not for want
//! of knowing which workspace it is: `inject_workspace_id` stamps the slug into
//! that shell as a meta precisely so a direct-port page can address itself. The
//! obstacle is the listing's LOCATION. It lives on the GATEWAY origin, a
//! different port, and `control_authz` (`lucidos-gateway/src/control.rs`)
//! rejects any browser request whose `Sec-Fetch-Site` is not `same-origin`, so
//! the cross-origin fetch is a deliberate 403. That gate is a CSRF boundary and
//! is not something a display label gets to widen.
//!
//! So the direct-port page asks its own engine, which IS same-origin with it,
//! and the engine asks the co-located gateway over loopback, a call carrying no
//! browser fetch metadata, which `control_authz` allows. Same hop as
//! [`crate::boot_report`] and `api::history::restart_via_gateway`, same posture:
//! scheme via [`crate::net_config::peer_scheme_order`], the self-signed dev cert
//! accepted, no ambient proxy, a short timeout.
//!
//! Two properties this route is responsible for:
//!
//! * **It projects, it does not proxy.** The listing names every workspace on
//!   the machine; only THIS workspace's own name leaves the engine.
//! * **It is best-effort.** No gateway, an unreachable one, a slow one or a
//!   malformed answer all return `{"label": null}`, and the page keeps showing
//!   the engine's own name. There is nothing here worth failing a boot over.
//!
//! Its own route rather than a field on `/api/v1/health`, because the frontend
//! polls health every 5s and the gateway polls it independently, while a
//! workspace is renamed about as often as it is created. One call per app boot.

use axum::{routing::get, Json, Router};
use std::time::Duration;

use super::AppState;

/// Deadline for the whole lookup, spanning BOTH protocol attempts. The caller
/// is on the app's startup path and a label is never worth a wait.
///
/// One outer deadline rather than a per-request `Client::timeout`, which is what
/// [`crate::boot_report`] uses: reqwest applies that one per `send()`, so with a
/// scheme fallback it is a budget the lookup can spend TWICE. A gateway that
/// stalls (rather than refusing) would then hold this handler for 4s while its
/// doc comment promised 2. Here the deadline is the thing the doc names.
const GATEWAY_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/workspace-label", get(workspace_label))
}

/// `{"label": "<registry display name>"}`, or `{"label": null}` when there is no
/// gateway to ask or it could not be reached. Never an error status: see the
/// best-effort note in the module doc.
async fn workspace_label() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "label": resolve_label().await }))
}

/// Who we are, per the gateway that launched us. `None` when none did
/// (`LUCIDOS_GATEWAY_PORT` / `LUCIDOS_WORKSPACE_ID` / `LUCIDOS_API_PORT` unset,
/// which is the `LUCIDOS_NO_GATEWAY` dev mode and a directly-launched engine),
/// and then there is no registry holding another name for us.
///
/// The three are read together because the gateway SETS them together, off one
/// registry row (`lucidos-gateway` `stack.rs`: `LUCIDOS_API_PORT` is `ws.port`,
/// `LUCIDOS_WORKSPACE_ID` is `ws.id`). That pairing is what lets the port check
/// the slug, and the check is not theoretical: an engine started outside the
/// gateway inherits its launcher's whole environment, so a `scripts/e2e-api.sh`
/// run from inside a coding-agent session picked up `LUCIDOS_WORKSPACE_ID=dev`
/// and the `e2e-test` engine answered with the `dev` workspace's label. See
/// [`self`]'s projection property: reporting another workspace's name is the one
/// thing this route must never do.
async fn resolve_label() -> Option<String> {
    let gateway_port = super::base_path::gateway_port()?;
    let id = super::base_path::workspace_id()?;
    let our_port = super::base_path::api_port()?;
    fetch_label(&gateway_port, &id, &our_port).await
}

/// Ask the gateway on `gateway_port` what it calls the workspace `id`, under
/// [`GATEWAY_TIMEOUT`]. `None` when it could not be reached on either protocol,
/// refused, does not list us, or did not answer in time.
///
/// Takes its facts as arguments rather than reading the environment, so the hop
/// itself is testable against a stand-in gateway without a test mutating
/// process-global env that its neighbours are reading concurrently.
async fn fetch_label(gateway_port: &str, id: &str, our_port: &str) -> Option<String> {
    fetch_label_within(GATEWAY_TIMEOUT, gateway_port, id, our_port).await
}

/// [`fetch_label`] with the deadline injected, so a test can prove the bound
/// holds across BOTH protocol attempts without waiting out the real one.
async fn fetch_label_within(
    deadline: Duration,
    gateway_port: &str,
    id: &str,
    our_port: &str,
) -> Option<String> {
    match tokio::time::timeout(deadline, ask_gateway(gateway_port, id, our_port)).await {
        Ok(label) => label,
        Err(_) => {
            crate::log!(
                "[WorkspaceLabel] gateway on :{} did not answer within {:?}; using the engine name",
                gateway_port,
                deadline
            );
            None
        }
    }
}

/// The hop itself, unbounded: [`fetch_label_within`] owns the deadline.
async fn ask_gateway(gateway_port: &str, id: &str, our_port: &str) -> Option<String> {
    // Every give-up path in here says so. The route answers 200 with a null
    // label whatever happens, and the page only warns when the request itself
    // throws, so a line here is the ONLY place a broken hop is visible at all.
    let client = match reqwest::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .build()
    {
        Ok(client) => client,
        Err(e) => {
            crate::log!(
                "[WorkspaceLabel] could not build the gateway client ({}); using the engine name",
                e
            );
            return None;
        }
    };
    // Resolved scheme first, the other protocol second, so a dev/packaged
    // mismatch still connects (the class that broke `restart_via_gateway`).
    for scheme in crate::net_config::peer_scheme_order() {
        let url = format!("{scheme}://127.0.0.1:{gateway_port}/~/api/v1/control/workspaces");
        let Ok(resp) = client.get(&url).send().await else {
            continue; // unreachable on this scheme, so try the other protocol
        };
        if !resp.status().is_success() {
            // The gateway answered and refused; the other scheme won't differ.
            crate::log!(
                "[WorkspaceLabel] gateway control listing returned {}; using the engine name",
                resp.status()
            );
            return None;
        }
        let body = match resp.text().await {
            Ok(body) => body,
            Err(e) => {
                crate::log!(
                    "[WorkspaceLabel] gateway control listing body was unreadable ({}); \
                     using the engine name",
                    e
                );
                return None;
            }
        };
        return label_for(&body, id, our_port);
    }
    // Neither protocol connected.
    crate::log!(
        "[WorkspaceLabel] gateway on :{} unreachable; using the engine name",
        gateway_port
    );
    None
}

/// Pull OUR OWN display name out of a control listing. `None` for a malformed
/// body, an id the listing does not carry (a slug deleted while this engine
/// still runs), or a row that is not us. Pure, so the projection is exhaustively
/// testable: this is the function that must never let another workspace's row
/// through.
///
/// A row is us only when it matches on BOTH the slug and the engine port. The
/// slug alone is not enough, because it reaches us through an inherited
/// environment variable and an engine launched outside the gateway inherits its
/// launcher's: `resolve_label` has the worked case.
///
/// **The port is not a stronger KIND of evidence, it is a second one.** It
/// arrives through the same inherited environment, so on its own it would forge
/// exactly as easily. What makes the pair hold is that the two are set together
/// and rewritten separately: the gateway writes both from one registry row, and
/// every direct launcher rewrites the port for the workspace it is starting
/// (`scripts/lib/workspace.sh` `swap_ports`) while leaving an inherited slug
/// untouched. So a legitimate engine satisfies both halves and a leaked identity
/// splits. A launch that rewrites NEITHER (a bare `cargo run` from an agent
/// session, onto a port its real owner has released) would still match, which is
/// why the underlying leak stays open in `docs/known-gaps.md` rather than being
/// called closed here.
fn label_for(listing: &str, workspace_id: &str, our_port: &str) -> Option<String> {
    let listing = serde_json::from_str::<serde_json::Value>(listing).ok()?;
    let Some(row) = listing
        .get("workspaces")?
        .as_array()?
        .iter()
        .find(|w| w.get("id").and_then(|v| v.as_str()) == Some(workspace_id))
    else {
        // The most diagnostic of the give-up paths, and the shape of the bug
        // this route exists to fix: we reached A gateway, and it has never heard
        // of us. Usually means we are pointed at the wrong one.
        crate::log!(
            "[WorkspaceLabel] gateway does not list '{}'; using the engine name",
            workspace_id
        );
        return None;
    };
    // `port` is a JSON number (`WorkspaceStatus.port` is a `u16`), rendered to
    // text so the env var side needs no parse. Any other shape is a listing we
    // do not understand, and an identity we cannot confirm is not ours.
    let row_port = match row.get("port")? {
        serde_json::Value::Number(n) => n.to_string(),
        _ => return None,
    };
    if row_port != our_port {
        crate::log!(
            "[WorkspaceLabel] '{}' is on :{} but this engine serves :{}; \
             not our row, using the engine name",
            workspace_id,
            row_port,
            our_port
        );
        return None;
    }
    row.get("name")?
        .as_str()
        .map(str::to_string)
        .filter(|name| !name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    fn listing() -> &'static str {
        r#"{"workspaces":[
            {"id":"personal","name":"personal","port":5174,"health":"unhealthy","autostart":false},
            {"id":"dev","name":"development","port":5173,"health":"healthy","autostart":true}
        ]}"#
    }

    #[test]
    fn takes_the_display_name_for_our_own_slug() {
        // The reported case: created as "dev", renamed to "development". The
        // slug is frozen (ADR 0014), so it is the id that identifies us.
        assert_eq!(
            label_for(listing(), "dev", "5173").as_deref(),
            Some("development")
        );
    }

    #[test]
    fn never_answers_with_another_workspaces_name() {
        // The whole security property of this route in one assertion: a listing
        // full of other people's workspaces yields nothing for a slug we don't
        // match, rather than the first row or a merged view.
        assert_eq!(label_for(listing(), "work", "5173"), None);
    }

    #[test]
    fn a_malformed_or_empty_listing_is_no_label() {
        // Anything unparseable degrades to the engine's own name upstream; none
        // of these is an error the user should see.
        for body in ["", "not json", "{}", r#"{"workspaces":"nope"}"#] {
            assert_eq!(label_for(body, "dev", "5173"), None, "body: {body:?}");
        }
    }

    /// Serve `body` at the control-listing path on a loopback port, over plain
    /// HTTP, and hand back the port. Stands in for the gateway.
    ///
    /// Plain HTTP on purpose. `peer_scheme_order()` puts `https` first whenever
    /// this process has TLS certs configured, so a run in that environment has
    /// to fall through to the second protocol to reach this stub at all. That
    /// makes the fallback loop load-bearing for the test rather than incidental,
    /// which is the arm that broke `restart_via_gateway` when someone hardcoded
    /// a scheme.
    async fn stand_in_gateway(body: &'static str, status: StatusCode) -> u16 {
        let app = Router::<()>::new().route(
            "/~/api/v1/control/workspaces",
            get(move || async move { (status, body) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        port
    }

    #[tokio::test]
    async fn reaches_a_real_gateway_over_loopback_and_takes_its_own_name() {
        let port = stand_in_gateway(
            r#"{"workspaces":[
                {"id":"other","name":"Other","port":5174},
                {"id":"dev","name":"development","port":5173}
            ]}"#,
            StatusCode::OK,
        )
        .await;
        assert_eq!(
            fetch_label(&port.to_string(), "dev", "5173")
                .await
                .as_deref(),
            Some("development")
        );
    }

    #[tokio::test]
    async fn a_slug_we_inherited_rather_than_earned_gets_no_label() {
        // The e2e run that caught this: `scripts/e2e-api.sh` launched from a
        // coding-agent session inherited `LUCIDOS_WORKSPACE_ID=dev`, so the
        // `e2e-test` engine asked for `dev` and was handed "development".
        // The port it actually serves on is the half that cannot be inherited
        // wrong, and the registry writes both from one row, so a slug sitting on
        // somebody else's port is not ours.
        let port = stand_in_gateway(
            r#"{"workspaces":[{"id":"dev","name":"development","port":5173}]}"#,
            StatusCode::OK,
        )
        .await;
        assert_eq!(fetch_label(&port.to_string(), "dev", "5341").await, None);
    }

    #[tokio::test]
    async fn nothing_answering_on_the_port_is_no_label() {
        // Both protocols fail, which is the unreachable-gateway path. Bind and
        // immediately drop a listener so the port is one nothing is serving,
        // rather than a guessed number some other test might be using.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        drop(listener);
        assert_eq!(fetch_label(&port.to_string(), "dev", "5173").await, None);
    }

    #[tokio::test]
    async fn a_gateway_that_refuses_is_no_label() {
        let port = stand_in_gateway("nope", StatusCode::FORBIDDEN).await;
        assert_eq!(fetch_label(&port.to_string(), "dev", "5173").await, None);
    }

    #[tokio::test]
    async fn the_deadline_covers_both_protocol_attempts() {
        // A gateway that STALLS rather than refusing is the case a per-request
        // timeout gets wrong: reqwest applies that one per `send()`, so the
        // scheme fallback spends the budget a second time and the lookup runs
        // for twice its documented deadline. Accept the connection and never
        // answer, so both attempts hang, then assert the whole call still
        // returns within one deadline plus slack.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((sock, _)) = listener.accept().await {
                held.push(sock); // hold it open, answer nothing
            }
        });

        // A second, not the ~150ms this wants to cost, because the assertion's
        // whole discriminating power is the gap between one deadline and two:
        // correct lands just over 1x, broken just over 2x, and the boundary is
        // at 2x. A tighter budget puts scheduling jitter on the same scale as
        // the signal, and this repo routinely runs parallel cargo builds beside
        // its tests, so at 150ms a loaded host would fail the CORRECT
        // implementation. One second buys the margin and is still noise in a
        // suite of thousands.
        let deadline = Duration::from_secs(1);
        let started = tokio::time::Instant::now();
        assert_eq!(
            fetch_label_within(deadline, &port.to_string(), "dev", "5173").await,
            None
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed < deadline * 2,
            "one deadline covers both attempts, took {elapsed:?} against a {deadline:?} budget"
        );
    }

    #[test]
    fn a_row_without_a_usable_name_is_no_label() {
        // An empty name would blank every surface that shows it, which is worse
        // than the engine's own name; a missing or non-string one is malformed.
        // Each row carries our port, so it is the NAME each case is failing on.
        let bodies = [
            r#"{"workspaces":[{"id":"dev","name":"","port":5173}]}"#,
            r#"{"workspaces":[{"id":"dev","port":5173}]}"#,
            r#"{"workspaces":[{"id":"dev","name":42,"port":5173}]}"#,
        ];
        for body in bodies {
            assert_eq!(label_for(body, "dev", "5173"), None, "body: {body:?}");
        }
    }

    #[test]
    fn a_row_without_a_usable_port_is_no_label() {
        // The port is half the identity check, so a row that cannot supply one
        // cannot be confirmed as ours. Refuse rather than fall back to matching
        // on the slug alone, which is the half that can be inherited wrong.
        let bodies = [
            r#"{"workspaces":[{"id":"dev","name":"development"}]}"#,
            r#"{"workspaces":[{"id":"dev","name":"development","port":null}]}"#,
        ];
        for body in bodies {
            assert_eq!(label_for(body, "dev", "5173"), None, "body: {body:?}");
        }
    }

    #[test]
    fn a_port_that_is_not_a_number_is_no_label() {
        // `WorkspaceStatus.port` is a `u16`, so nothing can emit another shape.
        // If one ever appears the listing is not one we understand, and an
        // identity we cannot confirm degrades to the engine's own name.
        let body = r#"{"workspaces":[{"id":"dev","name":"development","port":"5173"}]}"#;
        assert_eq!(label_for(body, "dev", "5173"), None);
    }
}
