use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::error::ApiError;
use super::AppState;
use crate::core::subprocess_origin::HEADER_TARGET_WORKSPACE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TargetCheck {
    Proceed,
    WrongWorkspace,
}

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

pub(crate) fn wrong_workspace_message(asserted: &str, actual: &str) -> String {
    format!(
        "This engine serves the workspace '{actual}', not '{asserted}'. The request was \
         aimed at the wrong engine and nothing was written. Several Lucidos engines run \
         on one machine, each on its own port, so resolve the target from \
         $LUCIDOS_API_BASE_URL (or the workspace's .lucidos/ports file) rather than \
         guessing a port."
    )
}

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
        let asserted = asserted.unwrap_or_default();
        crate::log_legacy!(
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

    #[test]
    fn a_mismatched_assertion_is_refused() {
        assert_eq!(
            check_target_workspace(Some("dev"), "myws"),
            TargetCheck::WrongWorkspace
        );
    }

    #[test]
    fn a_prefix_is_not_a_match() {
        assert_eq!(
            check_target_workspace(Some("dev"), "dev-2"),
            TargetCheck::WrongWorkspace
        );
        assert_eq!(
            check_target_workspace(Some("myws"), "myws-packaged"),
            TargetCheck::WrongWorkspace
        );
    }

    #[test]
    fn the_refusal_names_both_workspaces() {
        let msg = wrong_workspace_message("dev", "myws");
        assert!(msg.contains("myws"), "must name the actual: {msg}");
        assert!(msg.contains("dev"), "must name the asserted: {msg}");
        assert!(
            msg.contains("nothing was written"),
            "must say the write did not happen, so the caller does not go looking for it: {msg}"
        );
    }
}
