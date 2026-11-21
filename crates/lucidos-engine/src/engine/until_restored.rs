//! A driver that recovery restores holds every message until the restore.
//! The landing lane and the main check both start their driver before their
//! recovery runs. A message handled first would act on an empty state.

/// Holds every message until the restore arrives, then hands the restore over
/// first and what waited behind it in order.
#[derive(Debug)]
pub(crate) struct UntilRestored<T> {
    /// `Some` until the restore arrives, with what came before it.
    held: Option<Vec<T>>,
}

impl<T> Default for UntilRestored<T> {
    fn default() -> Self {
        Self {
            held: Some(Vec::new()),
        }
    }
}

impl<T> UntilRestored<T> {
    /// What to handle now, in order: nothing before the restore, then the
    /// restore and everything held behind it, then each message as it comes.
    pub(crate) fn admit(&mut self, msg: T, is_restore: bool) -> Vec<T> {
        match self.held.take() {
            None => vec![msg],
            Some(held) if is_restore => std::iter::once(msg).chain(held).collect(),
            Some(mut held) => {
                held.push(msg);
                self.held = Some(held);
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_handled_before_the_restore() {
        let mut gate = UntilRestored::default();
        assert!(gate.admit("submit", false).is_empty());
        assert!(gate.admit("thread ended", false).is_empty());
        assert_eq!(
            gate.admit("restored", true),
            vec!["restored", "submit", "thread ended"],
            "the restore first, then what waited, in order"
        );
        assert_eq!(gate.admit("submit again", false), vec!["submit again"]);
    }
}
