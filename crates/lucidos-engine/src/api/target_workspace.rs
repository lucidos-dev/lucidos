//! Refuse a request that was aimed at a different workspace's engine.
//!
//! Several engines run on one machine (a dev source build, a packaged `.app`,
//! one per workspace behind the gateway), each on its own port, each answering
//! the same `/api/v1` routes. Nothing in a request used to name its intended
//! target: `caller_workspace` says who is CALLING, never who is being CALLED.
//! So a client that resolved the wrong port was served, in full, by whichever
//! engine happened to be listening there.
//!
//! On 2026-08-06 that turned a wrong port into six phantom threads in an
//! unrelated workspace, and the second half is the worse half: because the
//! chat path materializes a thread from a client-supplied id, reading the
//! message back off the same wrong engine FOUND it. A mis-delivery was
//! indistinguishable from a delivery, so the caller's own verification step
//! confirmed the mistake instead of catching it.
//!
//! The fix is one optional header, [`HEADER_TARGET_WORKSPACE`]. State the
//! workspace you mean to reach and you can never be executed by another one;
//! state nothing and behaviour is exactly as before.
//!
//! ## Why optional
//!
//! The browser is same-origin and does not know a workspace name (under the
//! gateway the slug is in the path, and the gateway has already routed on it by
//! the time the engine sees the request). Every existing client predates the
//! header. A mandatory assertion would therefore break every caller to catch a
//! class of mistake that only scripted callers make, and scripted callers are
//! exactly the ones that can opt in: the `lucidos` CLI sends it on every
//! request, and `engine::http::workspace_client` sends it on every
//! cross-workspace POST.
//!
//! ## Why the refusal names the actual workspace
//!
//! A bare 409 tells a mis-aimed caller that something is wrong but not what, so
//! the obvious next move is to retry. The body names the workspace this engine
//! actually serves, which is the one fact that lets the caller re-resolve its
//! target instead. `GET /api/v1/health` already discloses the same name, so
//! this leaks nothing new.

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::actor::HEADER_TARGET_WORKSPACE;
use super::error::ApiError;
use super::AppState;

/// Outcome of comparing an asserted target workspace against this engine's own.
///
/// A two-variant enum rather than a `bool` so the "no assertion" case reads as
/// what it is at the call site (nothing was claimed, so nothing is checked)
/// rather than as a silent `true`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetCheck {
    /// No assertion, or an assertion that names this workspace.
    Proceed,
    /// The assertion names a different workspace. Refuse.
    WrongWorkspace,
}

/// Compare an asserted target workspace against the one this engine serves.
///
/// Trims surrounding whitespace and compares case-insensitively: a workspace
/// name is a directory basename, and both macOS and the gateway's slug
/// resolution treat those case-insensitively, so a stricter comparison here
/// would refuse a caller that would otherwise be routed correctly.
///
/// An assertion that trims to empty is treated as absent. A caller that sent a
/// blank header asserted nothing, and refusing it would only punish a
/// shell-quoting slip with a failure that reads like a mis-target.
///
/// Pure, so the matrix is unit-testable without a router.
pub(crate) fn check_target_workspace(asserted: Option<&str>, actual: &str) -> TargetCheck {
    let Some(asserted) = asserted.map(str::trim).filter(|s| !s.is_empty()) else {
        return TargetCheck::Proceed;
    };
    if asserted.eq_ignore_ascii_case(actual.trim()) {
        TargetCheck::Proceed
    } else {
        TargetCheck::WrongWorkspace
    }
}

/// The refusal a mis-aimed caller gets back. Names both workspaces so the
/// caller can tell a wrong port from a wrong name.
pub(crate) fn wrong_workspace_message(asserted: &str, actual: &str) -> String {
    format!(
        "This engine serves the workspace '{actual}', not '{asserted}'. The request was \
         aimed at the wrong engine and nothing was written. Several Lucidos engines run \
         on one machine, each on its own port, so resolve the target from \
         $LUCIDOS_API_BASE_URL (or the workspace's .lucidos/ports file) rather than \
         guessing a port."
    )
}

/// Middleware over the whole `/api/v1` router.
///
/// Layered at the router rather than per-handler on purpose: a mis-aimed write
/// is a hazard on every mutating endpoint, not just chat, and a per-handler
/// check is one a new endpoint can forget to add.
pub(crate) async fn enforce_target_workspace(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let asserted = request
        .headers()
        .get(HEADER_TARGET_WORKSPACE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let actual = state.engine.workspace_name();
    if check_target_workspace(asserted.as_deref(), &actual) == TargetCheck::WrongWorkspace {
        // `asserted` is Some here: `Proceed` covers every absent / blank case.
        let asserted = asserted.unwrap_or_default();
        crate::log!(
            "[API] Refusing request asserted for workspace '{}' on the engine serving '{}': {} {}",
            asserted.trim(),
            actual,
            request.method(),
            request.uri().path()
        );
        return ApiError::new(
            StatusCode::CONFLICT,
            wrong_workspace_message(asserted.trim(), &actual),
        )
        .into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_assertion_proceeds() {
        assert_eq!(check_target_workspace(None, "dev"), TargetCheck::Proceed);
    }

    #[test]
    fn a_blank_assertion_is_no_assertion() {
        for blank in ["", "   ", "\t"] {
            assert_eq!(
                check_target_workspace(Some(blank), "dev"),
                TargetCheck::Proceed,
                "blank: {blank:?}"
            );
        }
    }

    #[test]
    fn a_matching_assertion_proceeds() {
        assert_eq!(
            check_target_workspace(Some("dev"), "dev"),
            TargetCheck::Proceed
        );
    }

    #[test]
    fn matching_tolerates_case_and_surrounding_whitespace() {
        for asserted in ["DEV", "Dev", " dev ", "\tdev\n"] {
            assert_eq!(
                check_target_workspace(Some(asserted), "dev"),
                TargetCheck::Proceed,
                "asserted: {asserted:?}"
            );
        }
        assert_eq!(
            check_target_workspace(Some("dev"), " dev "),
            TargetCheck::Proceed,
            "the engine's own name is trimmed too"
        );
    }

    /// The incident shape: a request meant for `dev`, delivered to the engine
    /// serving `personal-dmg`.
    #[test]
    fn a_mismatched_assertion_is_refused() {
        assert_eq!(
            check_target_workspace(Some("dev"), "personal-dmg"),
            TargetCheck::WrongWorkspace
        );
    }

    /// A name that merely CONTAINS the other must not pass. Workspace names are
    /// compared whole, never by prefix.
    #[test]
    fn a_prefix_is_not_a_match() {
        assert_eq!(
            check_target_workspace(Some("dev"), "dev-2"),
            TargetCheck::WrongWorkspace
        );
        assert_eq!(
            check_target_workspace(Some("personal"), "personal-dmg"),
            TargetCheck::WrongWorkspace
        );
    }

    #[test]
    fn the_refusal_names_both_workspaces() {
        let msg = wrong_workspace_message("dev", "personal-dmg");
        assert!(msg.contains("personal-dmg"), "must name the actual: {msg}");
        assert!(msg.contains("dev"), "must name the asserted: {msg}");
        assert!(
            msg.contains("nothing was written"),
            "must say the write did not happen, so the caller does not go looking for it: {msg}"
        );
    }
}
