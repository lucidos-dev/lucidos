//! Why a workspace's boot attempt failed, and whether the gateway is still
//! trying (ADR 0014, 2026-07-29 + 2026-08-03 addenda).
//!
//! The sibling [`crate::boot_phase`] narrates boot *progress*; this narrates a
//! boot that did not get there. Two producers write one of these:
//!
//!   * **engine-reported** and always [`Terminal`]: a dying engine POSTs the
//!     reason to `/~/api/v1/control/workspaces/:id/boot-failure` on its way out,
//!     canonically a database migrated by a NEWER Lucidos (see the engine's own
//!     `boot_failure.rs`). The engine only reports what it has classified as
//!     unfixable, so the gateway stops respawning immediately.
//!   * **gateway-observed**, either kind: Postgres provisioning failed. The
//!     gateway classifies it via [`crate::postgres::ProvisionErrorKind`], so an
//!     environment condition that can clear (Docker Desktop still starting) is
//!     [`Retrying`] while one that cannot (no `docker` on PATH) is [`Terminal`].
//!
//! The distinction is not cosmetic: it decides both whether the supervisor
//! retries and which splash the user gets. A [`Terminal`] failure renders
//! [`crate::proxy::failed_page`], which deliberately carries no meta-refresh
//! because reloading cannot change the outcome. A [`Retrying`] one is rendered
//! as the label of the ordinary auto-refreshing splash, so the reason is visible
//! without the page claiming a dead end that has not happened yet.
//!
//! [`Terminal`]: BootFailureKind::Terminal
//! [`Retrying`]: BootFailureKind::Retrying

/// Whether the gateway will make another attempt at this workspace.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BootFailureKind {
    /// Another attempt is coming: this was `attempt` of `of` (the restart cap).
    /// The counts are shown so a wait reads as bounded progress rather than a
    /// spinner.
    Retrying { attempt: u32, of: u32 },
    /// No further attempt will be made, either because the failure is
    /// definitionally unfixable or because the budget is spent.
    Terminal,
}

/// One workspace's failed boot attempt: the reason, plus the retry verdict.
///
/// `cause` is stored WITHOUT any retry bookkeeping in it so the same reason can
/// be re-rendered as the attempt count moves and, at give-up, restated without
/// the promise of a retry that is no longer coming ([`BootFailure::gave_up`]).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BootFailure {
    cause: String,
    kind: BootFailureKind,
}

impl BootFailure {
    /// A failure no retry can fix. Both producers reach this: the engine's
    /// reported message, and a provisioning error classified terminal.
    pub fn terminal(cause: impl Into<String>) -> Self {
        Self {
            cause: cause.into(),
            kind: BootFailureKind::Terminal,
        }
    }

    /// A failure the gateway is still working through, on `attempt` of `of`.
    pub fn retrying(cause: impl Into<String>, attempt: u32, of: u32) -> Self {
        Self {
            cause: cause.into(),
            kind: BootFailureKind::Retrying { attempt, of },
        }
    }

    /// The same reason, now that the gateway has stopped trying. Called when the
    /// supervisor marks the workspace unhealthy: without it the splash would keep
    /// auto-refreshing under a message promising an attempt that will never come.
    pub fn gave_up(&self, attempts: u32) -> Self {
        match self.kind {
            BootFailureKind::Terminal => self.clone(),
            BootFailureKind::Retrying { .. } => Self::terminal(format!(
                "{} The workspace did not start after {} attempts.",
                sentence(&self.cause),
                attempts
            )),
        }
    }

    /// Whether the supervisor should stop here. The one input
    /// `respawn_decision`'s short-circuit reads.
    pub fn is_terminal(&self) -> bool {
        self.kind == BootFailureKind::Terminal
    }

    /// The user-facing sentence: the boot splash's label, and the stack's
    /// `last_error` (which is also the picker's health-dot tooltip).
    pub fn message(&self) -> String {
        match self.kind {
            BootFailureKind::Terminal => sentence(&self.cause),
            BootFailureKind::Retrying { attempt, of } => format!(
                "{} Retrying… (attempt {attempt} of {of})",
                sentence(&self.cause)
            ),
        }
    }
}

/// End `s` with terminal punctuation so it can be followed by another sentence.
/// Classified causes are written as sentences already; a raw error string
/// (`"docker port failed: ..."`, the unclassified default) is not, and would
/// otherwise run straight into the retry suffix.
fn sentence(s: &str) -> String {
    let trimmed = s.trim();
    match trimmed.chars().last() {
        None => String::new(),
        Some('.') | Some('!') | Some('?') | Some('…') | Some(':') => trimmed.to_string(),
        Some(_) => format!("{trimmed}."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_terminal_failure_renders_its_cause_verbatim() {
        let f = BootFailure::terminal("Lucidos 0.15.0 cannot open this workspace.");
        assert!(f.is_terminal());
        assert_eq!(f.message(), "Lucidos 0.15.0 cannot open this workspace.");
    }

    #[test]
    fn a_retrying_failure_says_so_and_bounds_the_wait() {
        // The reason is visible AND the page does not read as a dead end: the
        // user can see another attempt is coming, and how many are left.
        let f = BootFailure::retrying("The Docker daemon is not running yet.", 2, 5);
        assert!(!f.is_terminal());
        assert_eq!(
            f.message(),
            "The Docker daemon is not running yet. Retrying… (attempt 2 of 5)"
        );
    }

    #[test]
    fn giving_up_drops_the_promise_of_another_attempt() {
        let f = BootFailure::retrying("The Docker daemon is not running yet.", 5, 5).gave_up(5);
        assert!(f.is_terminal(), "a spent budget stops the supervisor");
        assert_eq!(
            f.message(),
            "The Docker daemon is not running yet. The workspace did not start after 5 attempts."
        );
        assert!(
            !f.message().contains("Retrying"),
            "must not still advertise a retry: {}",
            f.message()
        );
    }

    #[test]
    fn giving_up_on_an_already_terminal_failure_changes_nothing() {
        // The engine's reported message is the specific, actionable text; a
        // give-up restatement would only bury it under attempt bookkeeping.
        let f = BootFailure::terminal("A newer Lucidos migrated this database.");
        assert_eq!(f.gave_up(5), f);
    }

    #[test]
    fn a_raw_error_string_still_reads_as_a_sentence() {
        // The unclassified default is a bare CLI error with no full stop, which
        // would otherwise run into the retry suffix.
        let f = BootFailure::retrying("docker port failed: no such container", 1, 5);
        assert_eq!(
            f.message(),
            "docker port failed: no such container. Retrying… (attempt 1 of 5)"
        );
    }

    #[test]
    fn sentence_leaves_existing_punctuation_alone() {
        assert_eq!(sentence("Already done."), "Already done.");
        assert_eq!(sentence("Really?"), "Really?");
        assert_eq!(sentence("Starting…"), "Starting…");
        assert_eq!(sentence("  padded  "), "padded.");
        assert_eq!(sentence(""), "");
        assert_eq!(sentence("   "), "");
    }
}
