use super::types::ChangeClaim;

pub const APPLY_IN_PROGRESS_MESSAGE: &str =
    "An apply is already in progress for this thread. It finishes on its own";

pub const DISCARD_IN_PROGRESS_MESSAGE: &str =
    "A discard is already in progress for this thread. Try again once it has finished";

pub fn claim_refusal_message(holder: ChangeClaim) -> &'static str {
    match holder {
        ChangeClaim::Apply => APPLY_IN_PROGRESS_MESSAGE,
        ChangeClaim::Discard => DISCARD_IN_PROGRESS_MESSAGE,
    }
}

pub(crate) fn claim_refusal_slug(holder: ChangeClaim) -> &'static str {
    match holder {
        ChangeClaim::Apply => "apply_in_progress",
        ChangeClaim::Discard => "discard_in_progress",
    }
}

pub const SESSION_STOPPING_MESSAGE: &str =
    "The coding-agent session is stopping. Try again once it has ended";

/// INVARIANT: The machine-readable reason an HTTP handler sends beside a refusal message,
/// matched by identity. `None` for anything that is not a claim refusal. The
/// frontend keys on it: only `apply_in_progress` means an apply is running.
pub fn claim_refusal_reason(message: &str) -> Option<&'static str> {
    match message {
        APPLY_IN_PROGRESS_MESSAGE => Some(claim_refusal_slug(ChangeClaim::Apply)),
        DISCARD_IN_PROGRESS_MESSAGE => Some(claim_refusal_slug(ChangeClaim::Discard)),
        SESSION_STOPPING_MESSAGE => Some("session_stopping"),
        _ => None,
    }
}
