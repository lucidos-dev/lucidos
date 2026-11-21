//! Side-effect categories granted to triggers.

use serde::{Deserialize, Serialize};

/// The kind of irreversible real-world side-effect a command may perform. Only
/// meaningful for [`RiskLane::IrreversibleDanger`]. The judge tags each
/// irreversible command with a category, and the static fallback derives one
/// for the shapes it recognises. An unattended *trigger* runs the command only
/// when its declared **side-effect grant** contains that category (ADR 0002).
/// A chat turn always asks the user, whatever the category.
///
/// Wire form is snake_case. It rides on the trigger payload's
/// `side_effect_grant` array and in the judge's JSON `category` field.
// Keep the wire documentation stable. RiskLane is defined in the engine.
#[allow(rustdoc::broken_intra_doc_links)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectCategory {
    /// Sending email or messages — `mail`/`mailx`/`sendmail`, `osascript`
    /// driving Mail/Messages, Python `smtplib`.
    Email,
    /// A mutating outbound HTTP request — `curl`/`wget` POST/PUT/DELETE/PATCH or
    /// a data upload, Python `requests`/`httpx`/`session` writes.
    ExternalApi,
    /// A cloud-service mutation via the `gh`, `aws`, or `gcloud` CLIs.
    CloudCli,
    /// Deleting or overwriting files OUTSIDE the workspace — irreversible (no
    /// checkpoint is possible). Tagged by the judge, or by the static
    /// fallback's destruction scan when the judge is off (see
    /// `fallback_classify`).
    OutOfWorkspaceDestruction,
    /// An irreversible side-effect that doesn't fit the named categories — the
    /// judge's catch-all, and the fallback when the judge can't categorise. A
    /// trigger must grant `other` to run such a command.
    Other,
    /// A read of the secrets key directory or the shared Postgres password
    /// file (see `command_reads_secret`). Never grantable, so it has no wire
    /// form: no trigger can name it, and an unattended lane always refuses it.
    #[serde(skip)]
    SecretRead,
}

impl SideEffectCategory {
    /// Every category a trigger's side-effect grant can allow. A new category
    /// stays ungrantable until it is listed here.
    pub const GRANTABLE: [SideEffectCategory; 5] = [
        Self::Email,
        Self::ExternalApi,
        Self::CloudCli,
        Self::OutOfWorkspaceDestruction,
        Self::Other,
    ];

    /// Whether a trigger's side-effect grant can allow this category.
    pub fn is_grantable(&self) -> bool {
        Self::GRANTABLE.contains(self)
    }

    /// Noun phrase for LLM-facing refusal / card text — reads as "may perform
    /// {reason}".
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Email => {
                "an email or messaging side-effect (mail, sendmail, or AppleScript driving Mail/Messages)"
            }
            Self::ExternalApi => {
                "a mutating HTTP request (POST/PUT/DELETE/PATCH or a data upload)"
            }
            Self::CloudCli => "a cloud-service mutation via gh, aws, or gcloud",
            Self::OutOfWorkspaceDestruction => "destruction of files outside the workspace",
            Self::Other => "an irreversible real-world side-effect",
            Self::SecretRead => {
                "a read of the workspace's secret keys or the shared database password"
            }
        }
    }

    /// Short user-facing label for the trigger side-effect-grant UI. Kept in
    /// sync with the frontend `SIDE_EFFECT_CATEGORIES` list by
    /// `components/triggers/__tests__/side-effect-categories-mirror.test.ts`,
    /// which reads these arms and the enum above from this module. The mirror
    /// skips a `#[serde(skip)]` variant, which the UI never offers.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Email => "Send email or messages",
            Self::ExternalApi => "Call external APIs (mutating HTTP)",
            Self::CloudCli => "Cloud CLI mutations (gh / aws / gcloud)",
            Self::OutOfWorkspaceDestruction => "Destroy files outside the workspace",
            Self::Other => "Other irreversible side-effects",
            Self::SecretRead => "Read secret keys",
        }
    }
}
