#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BootFailureKind {
    Retrying { attempt: u32, of: u32 },
    Terminal,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BootFailure {
    cause: String,
    kind: BootFailureKind,
}

impl BootFailure {
    pub fn terminal(cause: impl Into<String>) -> Self {
        Self {
            cause: cause.into(),
            kind: BootFailureKind::Terminal,
        }
    }

    pub fn retrying(cause: impl Into<String>, attempt: u32, of: u32) -> Self {
        Self {
            cause: cause.into(),
            kind: BootFailureKind::Retrying { attempt, of },
        }
    }

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

    pub fn gave_up_without_cause() -> Self {
        Self::terminal("The engine kept stopping before it was ready.")
    }

    pub fn is_terminal(&self) -> bool {
        self.kind == BootFailureKind::Terminal
    }

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
        let f = BootFailure::terminal("A newer Lucidos migrated this database.");
        assert_eq!(f.gave_up(5), f);
    }

    #[test]
    fn giving_up_with_no_cause_is_still_terminal() {
        let f = BootFailure::gave_up_without_cause();
        assert!(f.is_terminal());
        assert!(!f.message().contains("Retrying"), "{}", f.message());
    }

    #[test]
    fn a_raw_error_string_still_reads_as_a_sentence() {
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
