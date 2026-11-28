use serde::{Deserialize, Serialize};

#[allow(rustdoc::broken_intra_doc_links)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectCategory {
    Email,
    ExternalApi,
    CloudCli,
    OutOfWorkspaceDestruction,
    Other,
    /// INVARIANT: A read of the secrets key directory or the shared Postgres password
    /// file (see `command_reads_secret`). Never grantable, so it has no wire
    /// form: no trigger can name it, and an unattended lane always refuses it.
    #[serde(skip)]
    SecretRead,
    #[serde(skip)]
    UnprovenWorkspaceDestruction,
}

impl SideEffectCategory {
    pub const GRANTABLE: [SideEffectCategory; 5] = [
        Self::Email,
        Self::ExternalApi,
        Self::CloudCli,
        Self::OutOfWorkspaceDestruction,
        Self::Other,
    ];

    pub fn is_grantable(&self) -> bool {
        Self::GRANTABLE.contains(self)
    }

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
            Self::UnprovenWorkspaceDestruction => {
                "the deletion of a Lucidos workspace or its database, which the command guard \
                 could not rule out"
            }
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Email => "Send email or messages",
            Self::ExternalApi => "Call external APIs (mutating HTTP)",
            Self::CloudCli => "Cloud CLI mutations (gh / aws / gcloud)",
            Self::OutOfWorkspaceDestruction => "Destroy files outside the workspace",
            Self::Other => "Other irreversible side-effects",
            Self::SecretRead => "Read secret keys",
            Self::UnprovenWorkspaceDestruction => "Possibly delete a Lucidos workspace",
        }
    }
}
