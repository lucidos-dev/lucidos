//! Refusal messages and reason slugs for held session claims.

use super::types::ChangeClaim;

/// Why an operation is refused while an apply holds the session's change
/// claim (`ChangeClaim::Apply`).
pub const APPLY_IN_PROGRESS_MESSAGE: &str =
    "An apply is already in progress for this thread. It finishes on its own";

/// Why an operation is refused while an in-session Discard holds the session's
/// change claim (`ChangeClaim::Discard`). It never says "apply": the change is
/// on its way out, not in.
pub const DISCARD_IN_PROGRESS_MESSAGE: &str =
    "A discard is already in progress for this thread. Try again once it has finished";

/// The refusal a change claim gives, naming its holder.
pub fn claim_refusal_message(holder: ChangeClaim) -> &'static str {
    match holder {
        ChangeClaim::Apply => APPLY_IN_PROGRESS_MESSAGE,
        ChangeClaim::Discard => DISCARD_IN_PROGRESS_MESSAGE,
    }
}

/// The machine-readable reason slug for a change claim's refusal.
pub(crate) fn claim_refusal_slug(holder: ChangeClaim) -> &'static str {
    match holder {
        ChangeClaim::Apply => "apply_in_progress",
        ChangeClaim::Discard => "discard_in_progress",
    }
}

/// Why an apply or an in-session Discard is refused while a Stop, Discard or
/// Archive is ending the session.
pub const SESSION_STOPPING_MESSAGE: &str =
    "The coding-agent session is stopping. Try again once it has ended";

/// The machine-readable reason an HTTP handler sends beside a refusal message,
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
