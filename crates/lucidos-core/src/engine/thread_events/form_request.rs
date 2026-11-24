use serde::{Deserialize, Serialize};

/// How a *form request* was closed. Carried by `FormRequestResolved`.
///
/// A form request stays open until exactly one of these lands. That is what
/// lets a request the user never answered survive a reload or a lost frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormRequestOutcome {
    /// The user did what the form asked: saved the credential, confirmed the
    /// plugin, sent the email, or finished the authorization page.
    Completed,
    /// The user declined: the form's Cancel, a plugin cancel, or a provider
    /// that answered the authorization with an error.
    Canceled,
    /// Something newer took its place: a later request for the same subject in
    /// the same thread, or a new user message in the thread.
    Superseded,
    /// The engine can no longer act on it: a plugin's staged files are gone, or
    /// an authorization's listener timed out or died with the engine.
    Expired,
    /// A value a newer engine wrote. Never emit.
    #[serde(other)]
    Unknown,
}
